// Codegen AArch64 ELF (Linux). Sinh từ arm64_darwin.rs — `diff` hai file chính là
// tài liệu "Mach-O vs ELF khác gì" (giữ cấu trúc song song CÓ CHỦ ĐÍCH, đừng
// refactor lệch). Khác biệt: không prefix `_`, section ELF (.text/.data/.bss/
// .rodata/.tdata/.tbss), reloc :lo12:/:got: thay @PAGE/@GOTPAGE, TLS local-exec
// (mrs tpidr_el0 + :tprel_*) thay descriptor @TLVPPAGE, .weak thay
// .weak_definition, KHÔNG .subsections_via_symbols, variadic vô danh vào
// x0-x7/v0-v7 như named (AAPCS chuẩn — bỏ đặc sản Apple stack-only), stack arg
// scalar slot 8 tròn (bỏ packing natural-align). Ngữ nghĩa -O0, máy tính biểu
// thức kiểu chibicc:
// kết quả luôn ở x0; binary op sinh vế phải trước, push xuống stack (16 byte giữ
// alignment), sinh vế trái, pop vế phải vào x1 rồi `x0 = x0 op x1`.
//
// Hợp đồng giá trị (khớp ast.rs): mọi scalar sống trong x0 dạng "canonical" 64-bit
// — int sign/zero-extend đúng theo kiểu, float/double là BIT PATTERN f64 (float
// nâng lên double ngay khi load, hạ xuống f32 lúc store). Kiểu của node (Ast.types)
// quyết định chọn lệnh: signed/unsigned (sdiv/udiv, lt/lo...), float (fadd, fcmp).
// Sau op 32-bit phải re-canonicalize (sxtw/mov w) để giữ ngữ nghĩa wrap của int.
//
// ABI: args int x0-x7, float v0-v7 (2 counter riêng), quá thì stack 8-byte/slot;
// vô danh variadic đi reg như named (AAPCS chuẩn). Return: x0 / d0.
// Label: "L{n}" tuần tự; "LC{id}" đích case; "lg_{fn}.{tên}" label goto.
use crate::ast::{Ast, GInit, Node, NodeId, SyncOp, Ty, TypeId, VOID};
use crate::ir::{self, Callee, Inst, IrFunc, Op, Place, Term, Tmp, Un, Val};
use std::fmt::Write;

const EPILOGUE: &str = "\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n";

struct Cg<'a> {
    s: String,
    a: &'a Ast,
    lbl: u32,
    brks: Vec<u32>,
    conts: Vec<u32>,
    fname: String,
    fret: TypeId,
    fsret: u32, // ≠0: slot chứa con trỏ x8 (hàm trả struct >16B)
    // hàm variadic hiện tại (cho VaStart): named đã ăn (gp, fp), byte stack
    // named, frame — save area 192B nằm NGAY DƯỚI frame: [x29-frame-192, x29-frame)
    // = VR 128B rồi GP 64B; gr_top = x29-frame, vr_top = x29-frame-64
    va: (u32, u32, u32, u32),
    // EXT(gcc): nested function — fchain = slot static chain (0 nếu top-level),
    // fuid/fparent = danh tính (dùng tính chain của Tramp/NlGoto: cùng cha ⟹ x29,
    // sibling ⟹ chain forward).
    fchain: u32,
    fuid: u32,
    fparent: u32,
    // VLA dealloc (C99 6.8.6.1): base SP = x29 - (frame + variadic?192:0); tại
    // label ở VLA-depth 0 restore SP về base (goto rời scope VLA phải dealloc).
    fframe: u32,
    fvariadic: bool,
    fhasvla: bool,
    vla_live: u32, // số VLA lexical đang sống (scope hiện tại) khi walk
    // đường IR (migrate): base offset vùng temp-slot (= frame; temp i tại
    // x29 − (ir_tbase + 8 + i*8)); ir_temps = bảng kiểu temp hàm hiện tại.
    ir_tbase: u32,
    ir_temps: Vec<TypeId>,
}

pub fn emit(ast: &Ast) -> String {
    let mut g = Cg {
        s: String::from(".cfi_sections .eh_frame\n.text\n"),
        a: ast,
        lbl: 0,
        brks: Vec::new(),
        conts: Vec::new(),
        fname: String::new(),
        fret: VOID,
        fsret: 0,
        va: (0, 0, 0, 0),
        fchain: 0,
        fuid: 0,
        fparent: u32::MAX,
        fframe: 0,
        fvariadic: false,
        fhasvla: false,
        vla_live: 0,
        ir_tbase: 0,
        ir_temps: Vec::new(),
    };
    // EXT(gcc): __asm__("...") cấp toàn cục (musl crt_arch.h _start) — verbatim
    for a in &ast.raw_asm {
        g.s += a;
        g.s += "\n.text\n"; // blob có thể đổi section — trả về .text
    }
    for f in &ast.funcs {
        g.fname = f.name.clone();
        g.fret = f.ret;
        g.fsret = f.sret;
        g.fchain = f.chain;
        g.fuid = f.uid;
        g.fparent = f.parent_uid;
        g.fframe = f.frame;
        g.fvariadic = f.variadic;
        g.fhasvla = f.has_vla;
        g.vla_live = 0;
        if !f.is_static {
            _ = writeln!(g.s, ".globl {}", f.name);
            if f.is_inline || f.is_weak {
                _ = writeln!(g.s, ".weak {}", f.name);
            }
        }
        // .type %function: st_info=STT_FUNC để dladdr/backtrace_symbols nhận diện;
        // .size (cuối hàm) cho st_size ≠ 0 → dladdr match được địa chỉ GIỮA hàm
        // (return-addr), không thì chỉ match đúng byte đầu → tên hàm không resolve.
        _ = writeln!(g.s, ".type {}, %function", f.name);
        // CFI (.eh_frame): frame-pointer-based, CFA = x29 + 16 HẰNG SỐ suốt thân
        // hàm → không cần annotate push tạm/sub sp/alloca. Đủ cho _Unwind/backtrace()
        // tại mọi PC giữa hàm (chỉ sai trong 2-lệnh epilogue window, không ai unwind ở đó).
        _ = write!(
            g.s,
            ".p2align 2\n{}:\n\t.cfi_startproc\n\tstp x29, x30, [sp, #-16]!\n\t.cfi_def_cfa_offset 16\n\t.cfi_offset 29, -16\n\t.cfi_offset 30, -8\n\tmov x29, sp\n\t.cfi_def_cfa_register 29\n",
            f.name
        );
        if f.frame > 0 {
            g.sp_adjust("sub", f.frame);
        }
        emit_params(&mut g, f);
        g.stmt(f.body);
        g.s += "\tmov x0, #0\n";
        g.s += EPILOGUE;
        g.s += "\t.cfi_endproc\n";
        _ = writeln!(g.s, "\t.size {0}, .-{0}", f.name);
    }
    emit_module_tail(&mut g, ast);
    g.s
}

fn emit_params(g: &mut Cg, f: &crate::ast::Func) {
    let ast = g.a;
    // EXT(gcc): nested → lưu static chain (x18, do trampoline/caller nạp) vào
    // slot; Upvar/forward-call/NlGoto đọc lại từ đây (x18 caller-saved, mất
    // sau call). x18 chưa bị đụng ở prologue nên lưu sớm là an toàn.
    if f.parent_uid != u32::MAX {
        g.lea_local("x9", f.chain);
        g.s += "\tstr x18, [x9]\n";
    }
    if f.variadic {
        // register-save area AAPCS: đổ NGUYÊN 8 q-reg + 8 x-reg (kể cả phần
        // named — thừa vô hại, đỡ phân nhánh); phải trước spill (đọc reg gốc)
        g.sp_adjust("sub", 192);
        g.imm("x9", (f.frame + 192) as i64);
        g.s += "\tsub x9, x29, x9\n";
        for i in 0..4u32 {
            _ = writeln!(g.s, "\tstp q{}, q{}, [x9, #{}]", 2 * i, 2 * i + 1, 32 * i);
        }
        for i in 0..4u32 {
            _ = writeln!(
                g.s,
                "\tstp x{}, x{}, [x9, #{}]",
                2 * i,
                2 * i + 1,
                128 + 16 * i
            );
        }
    }
    if f.sret != 0 {
        g.lea_local("x9", f.sret);
        g.s += "\tstr x8, [x9]\n";
    }
    // spill param theo ABI: 2 counter gp/fp, tràn thì đọc lại từ vùng stack
    // caller tại [x29 + 16 + boff]. AAPCS chuẩn: scalar tràn mỗi cái một
    // slot 8 tròn, composite align 8 size tròn 8 (over-alignment aligned(16+)
    // bị BỎ QUA — gcc arm64 verify asm: named x3,x4 / stack [sp,8], pr92904),
    // composite tràn khóa gp=8 (C.11). PHẢI khớp từng byte với call().
    let alup = |o: u32, a: u32| (o + a - 1) & !(a - 1);
    let (mut gp, mut fp, mut boff) = (0u32, 0u32, 0u32);
    for &(off, t) in &f.params {
        // struct by value ≤16B: đến trong 1-2 GPR liên tiếp (hoặc stack)
        if let Some((dbl, n)) = ast.tt.hfa(t) {
            if fp + n <= 8 {
                g.lea_local("x9", off);
                for j in 0..n {
                    if dbl {
                        _ = writeln!(g.s, "\tstr d{}, [x9, #{}]", fp + j, 8 * j);
                    } else {
                        _ = writeln!(g.s, "\tstr s{}, [x9, #{}]", fp + j, 4 * j);
                    }
                }
                fp += n;
            } else {
                fp = 8; // AAPCS C.3: HFA tràn khóa v-reg còn lại
                let sz = ast.tt.size(t);
                let o = alup(boff, 8);
                boff = o + sz.div_ceil(8) * 8;
                _ = writeln!(g.s, "\tadd x11, x29, #{}", 16 + o);
                g.lea_local("x9", off);
                g.imm("x12", sz as i64);
                let n2 = g.labels(1);
                _ = writeln!(g.s, "L{n2}:");
                g.s += "\tldrb w13, [x11], #1\n\tstrb w13, [x9], #1\n\tsubs x12, x12, #1\n";
                _ = writeln!(g.s, "\tb.ne L{n2}");
            }
            continue;
        }
        if matches!(ast.tt.tys[t as usize], Ty::Struct(_)) {
            let sz = ast.tt.size(t);
            if sz > 16 {
                // >16B: đến dưới dạng CON TRỎ (1 GPR / 1 slot) — copy về slot local
                if gp < 8 {
                    _ = writeln!(g.s, "\tmov x11, x{gp}");
                    gp += 1;
                } else {
                    let o = alup(boff, 8); // con trỏ = scalar 8 byte
                    boff = o + 8;
                    _ = writeln!(g.s, "\tldr x11, [x29, #{}]", 16 + o);
                }
                g.lea_local("x9", off);
                g.imm("x12", sz as i64);
                let n = g.labels(1);
                _ = writeln!(g.s, "L{n}:");
                g.s += "\tldrb w13, [x11], #1\n\tstrb w13, [x9], #1\n\tsubs x12, x12, #1\n";
                _ = writeln!(g.s, "\tb.ne L{n}");
                continue;
            }
            let need = if sz > 8 { 2 } else { 1 };
            g.lea_local("x9", off);
            if gp + need <= 8 {
                _ = writeln!(g.s, "\tmov x8, x{gp}");
                g.store_narrow(0, sz.min(8));
                if sz > 8 {
                    _ = writeln!(g.s, "\tmov x8, x{}", gp + 1);
                    g.store_narrow(8, sz - 8);
                }
                gp += need;
            } else {
                let o = alup(boff, 8);
                boff = o + 8 * need;
                gp = 8; // AAPCS C.11: composite tràn stack khóa NGRN
                _ = writeln!(g.s, "\tldr x8, [x29, #{}]", 16 + o);
                g.store_narrow(0, sz.min(8));
                if sz > 8 {
                    _ = writeln!(g.s, "\tldr x8, [x29, #{}]", 16 + o + 8);
                    g.store_narrow(8, sz - 8);
                }
            }
            continue;
        }
        let fl = ast.tt.is_float(t);
        if fl && fp < 8 {
            g.lea_local("x9", off);
            match ast.tt.size(t) {
                4 => _ = writeln!(g.s, "\tstr s{fp}, [x9]"),
                16 => _ = writeln!(g.s, "\tstr q{fp}, [x9]"), // long double: nguyên binary128
                _ => _ = writeln!(g.s, "\tstr d{fp}, [x9]"),
            }
            fp += 1;
        } else if !fl && gp < 8 {
            g.lea_local("x9", off);
            _ = match ast.tt.size(t) {
                1 => writeln!(g.s, "\tstrb w{gp}, [x9]"),
                2 => writeln!(g.s, "\tstrh w{gp}, [x9]"),
                4 => writeln!(g.s, "\tstr w{gp}, [x9]"),
                _ => writeln!(g.s, "\tstr x{gp}, [x9]"),
            };
            gp += 1;
        } else {
            // scalar trên stack caller: slot 8 tròn tại [x29 + 16 + boff]
            // (AAPCS chuẩn); load đúng width — giá trị nằm low bytes slot
            let sz = ast.tt.size(t);
            if sz == 16 {
                // long double tràn: stack arg quad — slot 16, align 16 (AAPCS B/C)
                let o = alup(boff, 16);
                boff = o + 16;
                g.lea_local("x9", off);
                _ = writeln!(g.s, "\tldr q7, [x29, #{}]\n\tstr q7, [x9]", 16 + o);
                continue;
            }
            let o = alup(boff, 8);
            boff = o + 8;
            let src = 16 + o;
            g.lea_local("x9", off);
            if fl && sz == 4 {
                _ = writeln!(g.s, "\tldr s7, [x29, #{src}]\n\tstr s7, [x9]");
            } else {
                _ = match sz {
                    1 => writeln!(g.s, "\tldrb w8, [x29, #{src}]\n\tstrb w8, [x9]"),
                    2 => writeln!(g.s, "\tldrh w8, [x29, #{src}]\n\tstrh w8, [x9]"),
                    4 => writeln!(g.s, "\tldr w8, [x29, #{src}]\n\tstr w8, [x9]"),
                    _ => writeln!(g.s, "\tldr x8, [x29, #{src}]\n\tstr x8, [x9]"),
                };
            }
        }
    }
    g.va = (gp.min(8) as u32, fp.min(8) as u32, boff, f.frame);
}


// Đuôi module (globals/TLS/strings/weak/aliases/nested-stack) — dùng CHUNG
// cho emit() (AST) và emit_ir() (IR). Chỉ đọc ast + phát vào g.s.
fn emit_module_tail(g: &mut Cg, ast: &Ast) {
    for gl in &ast.globals {
        if gl.is_extern {
            // EXT(gcc): extern weak (musl _DYNAMIC) — ref phải là weak undef
            if gl.is_weak {
                _ = writeln!(g.s, ".weak {}", gl.name);
            }
            continue;
        }
        let (sz, al) = (ast.tt.size(gl.ty), ast.tt.data_align(gl.ty));
        let globl = if gl.is_static {
            String::new()
        } else if gl.is_weak {
            format!(".weak {}\n", gl.name) // EXT(gcc): .weak bao hàm global
        } else {
            format!(".globl {}\n", gl.name)
        };
        if gl.is_tls {
            // TLS ELF: symbol chính LÀ label trong .tdata/.tbss ("awT" = TLS),
            // không descriptor — access qua tpidr_el0 + :tprel (xem addr())
            match &gl.init {
                GInit::None => {
                    _ = writeln!(
                        g.s,
                        ".section .tbss,\"awT\",@nobits\n{}.p2align {}\n{}:\n\t.space {}",
                        globl,
                        al.trailing_zeros(),
                        gl.name,
                        sz.max(1)
                    );
                }
                init => {
                    _ = writeln!(
                        g.s,
                        ".section .tdata,\"awT\",@progbits\n{}.p2align {}\n{}:",
                        globl,
                        al.trailing_zeros(),
                        gl.name
                    );
                    g.gdata(init, sz);
                }
            }
            continue;
        }
        match &gl.init {
            GInit::None if gl.is_static => {
                // GNU .comm: alignment tính BYTE (Darwin tính log2)
                _ = writeln!(g.s, ".local {0}\n.comm {0},{1},{2}", gl.name, sz.max(1), al);
            }
            GInit::None => {
                // tentative definition → common symbol (nhiều TU cùng "int x;" hợp nhất)
                _ = writeln!(g.s, ".comm {},{},{}", gl.name, sz.max(1), al);
            }
            init => {
                _ = writeln!(
                    g.s,
                    ".data\n{}.p2align {}\n{}:",
                    globl,
                    al.trailing_zeros(),
                    gl.name
                );
                g.gdata(init, sz);
            }
        }
    }
    if !ast.strs.is_empty() {
        for (i, bytes) in ast.strs.iter().enumerate() {
            // __cstring bị linker dedup theo nội-dung-đến-NUL — string chứa
            // NUL nhúng ("\0abc") phải qua __const kẻo bị merge nhầm
            // ELF: .rodata trơn cho mọi string (không có mergeable-dedup phải né)
            g.s += if bytes.contains(&0) {
                ".section .rodata\n"
            } else {
                ".section .rodata\n"
            };
            _ = write!(g.s, "l_str{}:\n\t.asciz \"", i);
            for &b in bytes {
                match b {
                    b'"' | b'\\' => _ = write!(g.s, "\\{}", b as char),
                    0x20..=0x7e => g.s.push(b as char),
                    _ => _ = write!(g.s, "\\{:03o}", b),
                }
            }
            g.s += "\"\n";
        }
    }
    // EXT(gcc): weak prototype — undef ref hạ về weak (link không đòi symbol)
    for w in &ast.weak_decls {
        _ = writeln!(g.s, ".weak {}", w);
    }
    // EXT(gcc): __attribute__((alias)) — musl weak_alias: symbol mới = symbol cũ
    for (new, old, weak) in &ast.aliases {
        let vis = if *weak { ".weak" } else { ".globl" };
        _ = writeln!(g.s, "{} {}\n.set {}, {}", vis, new, new, old);
    }
    // EXT(gcc): nested function → trampoline sinh runtime trên stack cần stack
    // THỰC THI. Note "x" đẩy PT_GNU_STACK mang cờ X (ld gộp: một object đòi exec
    // stack là đủ). CHỈ phát khi TU thật có nested — giữ NX cho mọi chương trình khác.
    if ast.funcs.iter().any(|f| f.parent_uid != u32::MAX) {
        g.s += ".section .note.GNU-stack,\"x\",@progbits\n";
    }
}

impl Cg<'_> {
    // phát data cho một GInit; sz = size vùng phải phủ (List chèn .space vào lỗ hổng)
    fn gdata(&mut self, init: &GInit, sz: u32) {
        _ = match init {
            GInit::Num(v) => match sz {
                1 => writeln!(self.s, "\t.byte {}", *v as u8),
                2 => writeln!(self.s, "\t.short {}", *v as u16),
                4 => writeln!(self.s, "\t.long {}", *v as u32),
                _ => writeln!(self.s, "\t.quad {v}"),
            },
            GInit::Str(i) => writeln!(self.s, "\t.quad l_str{i}"),
            GInit::StrOff(i, k) => writeln!(self.s, "\t.quad l_str{i} + {k}"),
            // prefix \x01 = symbol nội bộ đã đủ tên (label &&); ELF không prefix
            GInit::Addr(n, k) => {
                let sym = match n.strip_prefix('\x01') {
                    Some(raw) => raw.to_string(),
                    None => n.to_string(),
                };
                if *k == 0 {
                    writeln!(self.s, "\t.quad {sym}")
                } else {
                    writeln!(self.s, "\t.quad {sym} + {k}")
                }
            }
            GInit::Diff(a, b) => match sz {
                4 => writeln!(self.s, "\t.long {a} - {b}"),
                _ => writeln!(self.s, "\t.quad {a} - {b}"),
            },
            GInit::Bytes(b) => {
                let list: Vec<String> = b.iter().map(|x| x.to_string()).collect();
                writeln!(self.s, "\t.byte {}", list.join(","))
            }
            GInit::List(items) => {
                let mut pos = 0u32;
                for (off, isz, it) in items {
                    if *off > pos {
                        _ = writeln!(self.s, "\t.space {}", off - pos);
                    }
                    self.gdata(it, *isz);
                    pos = off + isz;
                }
                if pos < sz {
                    _ = writeln!(self.s, "\t.space {}", sz - pos);
                }
                Ok(())
            }
            GInit::None => unreachable!(),
        };
    }
    // ghi `sz` byte (≤8) thấp của x8 vào [x9, #off..] — chính xác từng mảnh,
    // không đè slot bên cạnh (x8 bị dịch nát, x9 giữ nguyên)
    fn store_narrow(&mut self, mut off: u32, mut sz: u32) {
        while sz > 0 {
            if sz >= 8 {
                _ = writeln!(self.s, "\tstr x8, [x9, #{off}]");
                off += 8;
                sz -= 8;
            } else if sz >= 4 {
                _ = writeln!(self.s, "\tstr w8, [x9, #{off}]\n\tlsr x8, x8, #32");
                off += 4;
                sz -= 4;
            } else if sz >= 2 {
                _ = writeln!(self.s, "\tstrh w8, [x9, #{off}]\n\tlsr x8, x8, #16");
                off += 2;
                sz -= 2;
            } else {
                _ = writeln!(self.s, "\tstrb w8, [x9, #{off}]");
                off += 1;
                sz -= 1;
            }
        }
    }
    fn labels(&mut self, k: u32) -> u32 {
        let n = self.lbl;
        self.lbl += k;
        n
    }
    fn imm(&mut self, reg: &str, v: i64) {
        let u = v as u64;
        _ = writeln!(self.s, "\tmov {reg}, #{}", u & 0xffff);
        for sh in [16, 32, 48] {
            if (u >> sh) & 0xffff != 0 {
                _ = writeln!(self.s, "\tmovk {reg}, #{}, lsl #{sh}", (u >> sh) & 0xffff);
            }
        }
    }
    // reg = x29 - off (off có thể vượt imm12)
    fn lea_local(&mut self, reg: &str, off: u32) {
        if off <= 4095 {
            _ = writeln!(self.s, "\tsub {reg}, x29, #{off}");
        } else {
            self.imm("x10", off as i64);
            _ = writeln!(self.s, "\tsub {reg}, x29, x10");
        }
    }
    // EXT(gcc): địa chỉ upvar = static_chain - off. Chain (x29 hàm bao) đọc từ
    // slot fchain (do prologue lưu). reg ≠ x10 (imm path xài x10).
    fn lea_chain(&mut self, reg: &str, off: u32) {
        self.lea_local(reg, self.fchain);
        _ = writeln!(self.s, "\tldr {reg}, [{reg}]");
        if off <= 4095 {
            _ = writeln!(self.s, "\tsub {reg}, {reg}, #{off}");
        } else {
            self.imm("x10", off as i64);
            _ = writeln!(self.s, "\tsub {reg}, {reg}, x10");
        }
    }
    // EXT(gcc): parent_uid của nested func theo symbol (u32::MAX nếu không thấy)
    fn parent_of(&self, sym: &str) -> u32 {
        self.a
            .funcs
            .iter()
            .find(|f| f.name == sym)
            .map_or(u32::MAX, |f| f.parent_uid)
    }
    fn sp_adjust(&mut self, op: &str, n: u32) {
        if n <= 4095 {
            _ = writeln!(self.s, "\t{op} sp, sp, #{n}");
        } else {
            self.imm("x10", n as i64);
            _ = writeln!(self.s, "\t{op} sp, sp, x10");
        }
    }
    // C99 6.8.6.1: SP về base cố định của frame = x29 - (frame + reg-save variadic).
    // Dùng khi tới label depth-0 trong hàm có VLA: mọi VLA đã cấp bằng `sub sp`
    // (địa chỉ động) phải được thu hồi trước khi thân label chạy tiếp, nếu không
    // goto-lùi trong vòng lặp làm SP trôi xuống mãi → tràn stack.
    fn reset_sp_base(&mut self) {
        let off = self.fframe + if self.fvariadic { 192 } else { 0 };
        self.lea_local("x9", off);
        _ = writeln!(self.s, "\tmov sp, x9");
    }
    // re-canonicalize x0 theo kiểu (sau op 32-bit / thu hẹp)
    fn ext(&mut self, t: TypeId) {
        if matches!(self.a.tt.tys[t as usize], Ty::Bool) {
            self.s += "\tcmp x0, #0\n\tcset x0, ne\n";
            return;
        }
        // Bitfield: cắt về w bit theo dấu của base — giá trị của (l.m = v)
        // là v SAU truncate (921016-1)
        if let Ty::Bitfield(b, _, w) = self.a.tt.tys[t as usize] {
            let sh = 64 - w;
            let op = if self.a.tt.is_unsigned(b) {
                "lsr"
            } else {
                "asr"
            };
            _ = writeln!(self.s, "\tlsl x0, x0, #{sh}\n\t{op} x0, x0, #{sh}");
            return;
        }
        let u = self.a.tt.is_unsigned(t);
        self.s += match (self.a.tt.size(t), u) {
            (1, false) => "\tsxtb x0, w0\n",
            (1, true) => "\tuxtb w0, w0\n", // ghi w → upper 32 tự zero
            (2, false) => "\tsxth x0, w0\n",
            (2, true) => "\tuxth w0, w0\n",
            (4, false) => "\tsxtw x0, w0\n",
            (4, true) => "\tmov w0, w0\n",
            _ => return,
        };
    }
    fn load(&mut self, t: TypeId) {
        match self.a.tt.tys[t as usize] {
            Ty::Float => self.s += "\tldr s0, [x0]\n\tfcvt d0, s0\n\tfmov x0, d0\n",
            // long double: memory binary128 → hạ về f64 canonical (libgcc round đúng)
            Ty::LDouble => self.s += "\tldr q0, [x0]\n\tbl __trunctfdf2\n\tfmov x0, d0\n",
            Ty::Bitfield(b, boff, w) => {
                // nạp nguyên đơn vị chứa (unsigned) rồi lắc trái/phải cắt đúng field
                self.s += match self.a.tt.size(b) {
                    1 => "\tldrb w0, [x0]\n",
                    2 => "\tldrh w0, [x0]\n",
                    4 => "\tldr w0, [x0]\n",
                    _ => "\tldr x0, [x0]\n",
                };
                _ = writeln!(self.s, "\tlsl x0, x0, #{}", 64 - boff - w);
                let sh = if self.a.tt.is_unsigned(b) {
                    "lsr"
                } else {
                    "asr"
                };
                _ = writeln!(self.s, "\t{sh} x0, x0, #{}", 64 - w);
            }
            _ => {
                let u = self.a.tt.is_unsigned(t);
                self.s += match (self.a.tt.size(t), u) {
                    (1, false) => "\tldrsb x0, [x0]\n",
                    (1, true) => "\tldrb w0, [x0]\n",
                    (2, false) => "\tldrsh x0, [x0]\n",
                    (2, true) => "\tldrh w0, [x0]\n",
                    (4, false) => "\tldrsw x0, [x0]\n",
                    (4, true) => "\tldr w0, [x0]\n",
                    _ => "\tldr x0, [x0]\n",
                };
            }
        }
    }
    // store x{reg} → [x1] theo kiểu
    fn store(&mut self, reg: u32, t: TypeId) {
        match self.a.tt.tys[t as usize] {
            Ty::Bool => {
                _ = writeln!(
                    self.s,
                    "\tcmp x{reg}, #0\n\tcset x{reg}, ne\n\tstrb w{reg}, [x1]"
                );
            }
            Ty::Float => {
                _ = writeln!(self.s, "\tfmov d7, x{reg}\n\tfcvt s7, d7\n\tstr s7, [x1]");
            }
            Ty::LDouble => {
                // bl clobber x1 (caller-saved) — che địa chỉ qua stack
                _ = writeln!(
                    self.s,
                    "\tstr x1, [sp, #-16]!\n\tfmov d0, x{reg}\n\tbl __extenddftf2\n\tldr x1, [sp], #16\n\tstr q0, [x1]"
                );
            }
            Ty::Bitfield(b, boff, w) => {
                // read-modify-write đơn vị chứa
                let usz = self.a.tt.size(b);
                self.s += match usz {
                    1 => "\tldrb w3, [x1]\n",
                    2 => "\tldrh w3, [x1]\n",
                    4 => "\tldr w3, [x1]\n",
                    _ => "\tldr x3, [x1]\n",
                };
                let mask = ((!0u64 >> (64 - w)) << boff) as i64;
                self.imm("x4", mask);
                self.s += "\tbic x3, x3, x4\n";
                _ = writeln!(self.s, "\tlsl x5, x{reg}, #{boff}");
                self.s += "\tand x5, x5, x4\n\torr x3, x3, x5\n";
                self.s += match usz {
                    1 => "\tstrb w3, [x1]\n",
                    2 => "\tstrh w3, [x1]\n",
                    4 => "\tstr w3, [x1]\n",
                    _ => "\tstr x3, [x1]\n",
                };
            }
            _ => {
                _ = match self.a.tt.size(t) {
                    1 => writeln!(self.s, "\tstrb w{reg}, [x1]"),
                    2 => writeln!(self.s, "\tstrh w{reg}, [x1]"),
                    4 => writeln!(self.s, "\tstr w{reg}, [x1]"),
                    _ => writeln!(self.s, "\tstr x{reg}, [x1]"),
                };
            }
        }
    }
    // copy `sz` byte: src (x0) → dst (x1), xuôi. Để lại địa chỉ dst ở x0 (rvalue
    // của gán struct = địa chỉ đích). Dùng chung: AST-path + IR Inst::Memcpy.
    fn blk_copy(&mut self, sz: u32) {
        self.s += "\tmov x4, x1\n";
        if sz > 0 {
            let n = self.labels(1);
            self.imm("x2", sz as i64);
            _ = writeln!(self.s, "L{n}:");
            self.s += "\tldrb w3, [x0], #1\n\tstrb w3, [x1], #1\n\tsubs x2, x2, #1\n";
            _ = writeln!(self.s, "\tb.ne L{n}");
        }
        self.s += "\tmov x0, x4\n"; // giá trị = địa chỉ dst
    }
    // chuyển kiểu giá trị canonical trong x0: from → to
    fn cast_op(&mut self, from: TypeId, to: TypeId) {
        let tt = &self.a.tt;
        if matches!(
            tt.tys[to as usize],
            Ty::Void | Ty::Struct(_) | Ty::Array(..)
        ) {
            return;
        }
        match (tt.is_float(from), tt.is_float(to)) {
            (false, false) => self.ext(to),
            (false, true) => {
                let cvt = if tt.is_unsigned(from) {
                    "ucvtf"
                } else {
                    "scvtf"
                };
                _ = writeln!(self.s, "\t{cvt} d0, x0");
                if tt.size(to) == 4 {
                    self.s += "\tfcvt s0, d0\n\tfcvt d0, s0\n";
                }
                self.s += "\tfmov x0, d0\n";
            }
            (true, false) => {
                if matches!(tt.tys[to as usize], Ty::Bool) {
                    self.s += "\tfmov d0, x0\n\tfcmp d0, #0.0\n\tcset x0, ne\n";
                    return;
                }
                self.s += "\tfmov d0, x0\n";
                let cvt = if self.a.tt.is_unsigned(to) {
                    "fcvtzu"
                } else {
                    "fcvtzs"
                };
                if self.a.tt.size(to) == 8 {
                    _ = writeln!(self.s, "\t{cvt} x0, d0");
                } else {
                    _ = writeln!(self.s, "\t{cvt} w0, d0");
                    self.ext(to);
                }
            }
            (true, true) => {
                if tt.size(to) == 4 {
                    self.s += "\tfmov d0, x0\n\tfcvt s0, d0\n\tfcvt d0, s0\n\tfmov x0, d0\n";
                }
            }
        }
    }
    fn stmt(&mut self, id: NodeId) {
        match &self.a.nodes[id as usize] {
            Node::Ret(e) => {
                if let Some(e) = e {
                    self.expr(*e);
                    match self.a.tt.tys[self.fret as usize] {
                        Ty::Double => self.s += "\tfmov d0, x0\n",
                        Ty::LDouble => self.s += "\tfmov d0, x0\n\tbl __extenddftf2\n", // ra q0

                        Ty::Float => self.s += "\tfmov d0, x0\n\tfcvt s0, d0\n",
                        Ty::Struct(_) => {
                            let sz = self.a.tt.size(self.fret);
                            if let Some((dbl, n)) = self.a.tt.hfa(self.fret) {
                                // HFA về bằng v0-v3
                                self.s += "\tmov x9, x0\n";
                                for j in 0..n {
                                    if dbl {
                                        _ = writeln!(self.s, "\tldr d{j}, [x9, #{}]", 8 * j);
                                    } else {
                                        _ = writeln!(self.s, "\tldr s{j}, [x9, #{}]", 4 * j);
                                    }
                                }
                            } else if sz > 16 {
                                // >16B: copy struct (địa chỉ trong x0) về đích x8 đã giấu
                                let fs = self.fsret;
                                self.lea_local("x9", fs);
                                self.s += "\tldr x1, [x9]\n";
                                self.imm("x2", sz as i64);
                                let n = self.labels(1);
                                _ = writeln!(self.s, "L{n}:");
                                self.s +=
                                    "\tldrb w3, [x0], #1\n\tstrb w3, [x1], #1\n\tsubs x2, x2, #1\n";
                                _ = writeln!(self.s, "\tb.ne L{n}");
                            } else {
                                // ≤16B: nạp x0 (và x1) từ địa chỉ struct
                                self.s += "\tmov x9, x0\n\tldr x0, [x9]\n";
                                if sz > 8 {
                                    self.s += "\tldr x1, [x9, #8]\n";
                                }
                            }
                        }
                        _ => {}
                    }
                }
                self.s += EPILOGUE;
            }
            Node::Block(v) => {
                // vla_live = số VLA lexical sống; VLA của block này chết khi thoát
                let save = self.vla_live;
                for &c in v {
                    self.stmt(c);
                }
                self.vla_live = save;
            }
            Node::If(c, t, e) => {
                let n = self.labels(2);
                self.expr(*c);
                _ = writeln!(self.s, "\tcbz x0, L{n}");
                self.stmt(*t);
                _ = writeln!(self.s, "\tb L{}\nL{n}:", n + 1);
                if let Some(e) = e {
                    self.stmt(*e);
                }
                _ = writeln!(self.s, "L{}:", n + 1);
            }
            Node::While(c, b) => {
                let n = self.labels(2);
                _ = writeln!(self.s, "L{n}:");
                self.expr(*c);
                _ = writeln!(self.s, "\tcbz x0, L{}", n + 1);
                self.brks.push(n + 1);
                self.conts.push(n);
                self.stmt(*b);
                self.brks.pop();
                self.conts.pop();
                _ = writeln!(self.s, "\tb L{n}\nL{}:", n + 1);
            }
            Node::For(i, c, nx, b) => {
                let n = self.labels(3);
                if let Some(i) = i {
                    self.expr(*i);
                }
                _ = writeln!(self.s, "L{n}:");
                if let Some(c) = c {
                    self.expr(*c);
                    _ = writeln!(self.s, "\tcbz x0, L{}", n + 1);
                }
                self.brks.push(n + 1);
                self.conts.push(n + 2);
                self.stmt(*b);
                self.brks.pop();
                self.conts.pop();
                _ = writeln!(self.s, "L{}:", n + 2);
                if let Some(nx) = nx {
                    self.expr(*nx);
                }
                _ = writeln!(self.s, "\tb L{n}\nL{}:", n + 1);
            }
            Node::Do(b, c) => {
                let n = self.labels(3);
                _ = writeln!(self.s, "L{n}:");
                self.brks.push(n + 1);
                self.conts.push(n + 2);
                self.stmt(*b);
                self.brks.pop();
                self.conts.pop();
                _ = writeln!(self.s, "L{}:", n + 2);
                self.expr(*c);
                _ = writeln!(self.s, "\tcbnz x0, L{n}\nL{}:", n + 1);
            }
            Node::Switch(c, b, cases, def) => {
                let n = self.labels(1);
                self.expr(*c);
                // (lo,hi): val∈[lo,hi] ⟺ (unsigned)(val-lo) ≤ (hi-lo). Phủ single
                // (hi-lo=0 ⟺ b.ls là ==) lẫn EXT(gcc) case-range, không cần dấu.
                for &(lo, hi, cid) in cases {
                    self.imm("x1", lo);
                    self.s += "\tsub x2, x0, x1\n";
                    self.imm("x1", hi.wrapping_sub(lo));
                    _ = writeln!(self.s, "\tcmp x2, x1\n\tb.ls LC{cid}");
                }
                match def {
                    Some(d) => _ = writeln!(self.s, "\tb LC{d}"),
                    None => _ = writeln!(self.s, "\tb L{n}"),
                }
                self.brks.push(n);
                self.stmt(*b);
                self.brks.pop();
                _ = writeln!(self.s, "L{n}:");
            }
            Node::Case(st) => {
                _ = writeln!(self.s, "LC{id}:");
                self.stmt(*st);
            }
            Node::Break => _ = writeln!(self.s, "\tb L{}", self.brks.last().unwrap()),
            Node::Continue => _ = writeln!(self.s, "\tb L{}", self.conts.last().unwrap()),
            Node::Goto(name) => _ = writeln!(self.s, "\tb lg_{}.{}", self.fname, name),
            // EXT(gcc): non-local goto — khôi phục (x29, sp) hàm bao qua static
            // chain rồi br tới label của nó. chain (slot fchain) = x29 hàm bao
            // trực tiếp = chủ label (depth-1). sp hàm bao = x29 - frame (không VLA).
            Node::NlGoto(owner, label) => {
                let (owner, label) = (*owner, label.clone());
                let (pname, pframe) = self
                    .a
                    .funcs
                    .iter()
                    .find(|f| f.uid == owner)
                    .map(|f| (f.name.clone(), f.frame))
                    .unwrap();
                self.lea_local("x9", self.fchain);
                self.s += "\tldr x9, [x9]\n"; // x9 = parent x29
                if pframe <= 4095 {
                    _ = writeln!(self.s, "\tsub sp, x9, #{pframe}");
                } else {
                    self.imm("x10", pframe as i64);
                    self.s += "\tsub sp, x9, x10\n";
                }
                self.s += "\tmov x29, x9\n";
                _ = writeln!(self.s, "\tb lg_{pname}.{label}");
            }
            Node::GotoPtr(e) => {
                self.expr(*e);
                self.s += "\tbr x0\n";
            }
            Node::Label(name, st) => {
                _ = writeln!(self.s, "lg_{}.{}:", self.fname, name);
                // C99 6.8.6.1: label ở VLA-depth 0 ⟹ SP phải = base mọi lối vào;
                // goto-lùi từ trong scope VLA phải dealloc (không thì loop tràn
                // stack — 20040811/pr43220/vla-dealloc-1). Cấm goto NHẢY VÀO scope
                // VLA nên label đích luôn depth ≤ hiện tại → restore base an toàn.
                if self.fhasvla && self.vla_live == 0 {
                    self.reset_sp_base();
                }
                self.stmt(*st);
            }
            _ => self.expr(id),
        }
    }
    // x0 = địa chỉ của lvalue
    fn addr(&mut self, id: NodeId) {
        match &self.a.nodes[id as usize] {
            Node::Var(off) => self.lea_local("x0", *off),
            Node::Upvar(off) => self.lea_chain("x0", *off), // EXT(gcc): biến hàm bao
            Node::GVar(i) => {
                let gl = &self.a.globals[*i as usize];
                if gl.is_tls {
                    // local-exec: đọc thẳng thread pointer — hợp lệ trong
                    // executable; TLS trong .so cần model khác = NỢ ai đòi thì trả
                    _ = writeln!(
                        self.s,
                        "\tmrs x0, tpidr_el0\n\tadd x0, x0, #:tprel_hi12:{0}, lsl #12\n\tadd x0, x0, #:tprel_lo12_nc:{0}",
                        gl.name
                    );
                } else if gl.is_extern || (self.a.pic && !gl.is_static) {
                    // extern (stdout...) bắt buộc GOT; -fPIC: global TỰ ĐỊNH
                    // NGHĨA non-static cũng preemptible trong .so → GOT nốt.
                    // static (kể cả static trong hàm: intfmts.6 của hiredis)
                    // CẤM đi GOT — cùng bẫy với FunAddr bên dưới: gas hạ reloc
                    // local thành section+addend, GNU ld tạo GOT entry BỎ
                    // addend → con trỏ trỏ đầu section (redis-cli "Out of
                    // memory" theo layout). Local binding → adrp/:lo12: thẳng
                    // hợp lệ cả trong .so.
                    _ = writeln!(
                        self.s,
                        "\tadrp x0, :got:{0}\n\tldr x0, [x0, :got_lo12:{0}]",
                        gl.name
                    );
                } else {
                    _ = writeln!(self.s, "\tadrp x0, {0}\n\tadd x0, x0, :lo12:{0}", gl.name);
                }
            }
            Node::Member(b, off) => {
                self.addr(*b);
                if *off > 4095 {
                    self.imm("x9", *off as i64);
                    self.s += "\tadd x0, x0, x9\n";
                } else if *off > 0 {
                    _ = writeln!(self.s, "\tadd x0, x0, #{off}");
                }
            }
            Node::Deref(e) => self.expr(*e),
            // giá trị của expr kiểu struct = địa chỉ (SRet temp, compound literal...)
            Node::SRet(..)
            | Node::Comma(..)
            | Node::Assign(..)
            | Node::Cond(..)
            | Node::Block(_)
            | Node::Str(_) => self.expr(id),
            _ => unreachable!("không phải lvalue"),
        }
    }
    fn expr(&mut self, id: NodeId) {
        let t = self.a.types[id as usize];
        match &self.a.nodes[id as usize] {
            Node::Num(v) => self.imm("x0", *v),
            Node::FNum(v) => self.imm("x0", v.to_bits() as i64),
            Node::Var(_) | Node::Upvar(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..) => {
                self.addr(id);
                // mảng/struct/hàm: giá trị = địa chỉ, không load
                if !matches!(
                    self.a.tt.tys[t as usize],
                    Ty::Array(..) | Ty::Struct(_) | Ty::Func(_)
                ) {
                    self.load(t);
                }
            }
            // EXT(gcc): tham chiếu nested function → dựng trampoline 40B runtime
            // trên frame (slot), patch (fn_addr, static_chain) rồi __clear_cache;
            // giá trị = địa chỉ trampoline. Template 6 lệnh:
            //   bti c; ldr x17,.+20; ldr x18,.+24; br x17; dsb sy; isb; .xword fn; .xword chain
            Node::Tramp(sym, slot) => {
                let (sym, slot) = (sym.clone(), *slot);
                self.lea_local("x9", slot); // x9 = base trampoline (giữ tới __clear_cache)
                self.imm("x10", 0x580000B1_D503245Fu64 as i64); // [bti | ldr x17,.+20]
                self.s += "\tstr x10, [x9]\n";
                self.imm("x10", 0xD61F0220_580000D2u64 as i64); // [ldr x18,.+24 | br x17]
                self.s += "\tstr x10, [x9, #8]\n";
                self.imm("x10", 0xD5033FDF_D5033F9Fu64 as i64); // [dsb sy | isb]
                self.s += "\tstr x10, [x9, #16]\n";
                // fn addr (nested = symbol LOCAL → adrp/add trực tiếp)
                _ = writeln!(self.s, "\tadrp x10, {0}\n\tadd x10, x10, :lo12:{0}", sym);
                self.s += "\tstr x10, [x9, #24]\n";
                // static chain: cùng cha (current LÀ hàm bao) ⟹ x29; sibling ⟹
                // forward chain của mình (đọc slot)
                if self.parent_of(&sym) == self.fuid {
                    self.s += "\tmov x10, x29\n";
                } else {
                    self.lea_local("x10", self.fchain);
                    self.s += "\tldr x10, [x10]\n";
                }
                self.s += "\tstr x10, [x9, #32]\n";
                // đồng bộ I-cache 40B (libgcc, đã link -lgcc); x9 mất sau bl → lea lại
                self.s += "\tmov x0, x9\n\tadd x1, x9, #40\n\tbl __clear_cache\n";
                self.lea_local("x0", slot);
            }
            Node::Addr(e) => self.addr(*e),
            Node::FunAddr(name) => {
                let sy = sym(name);
                // hàm static = symbol LOCAL: cấm đi GOT — gas hạ reloc local
                // thành .text+addend, GNU ld tạo GOT entry BỎ addend → con trỏ
                // trỏ nhầm hàm đầu section (musl libc_start_main_stage2 → nhảy
                // vào __syscall3). Local luôn cùng TU → adrp/add trực tiếp.
                if self.a.funcs.iter().any(|f| f.name == *name && f.is_static) {
                    _ = writeln!(self.s, "\tadrp x0, {0}\n\tadd x0, x0, :lo12:{0}", sy);
                } else {
                    _ = writeln!(
                        self.s,
                        "\tadrp x0, :got:{0}\n\tldr x0, [x0, :got_lo12:{0}]",
                        sy
                    );
                }
            }
            Node::Alloca(e) => {
                self.expr(*e);
                self.s += "\tadd x0, x0, #15\n\tand x0, x0, #0xfffffffffffffff0\n\tsub sp, sp, x0\n\tmov x0, sp\n";
                self.vla_live += 1; // VLA sống trong scope hiện tại (dealloc tại label base-level)
            }
            Node::LabelAddr(name) => {
                _ = writeln!(
                    self.s,
                    "\tadrp x0, lg_{0}.{1}\n\tadd x0, x0, :lo12:lg_{0}.{1}",
                    self.fname, name
                );
            }
            Node::Cast(e) => {
                let from = self.a.types[*e as usize];
                self.expr(*e);
                self.cast_op(from, t);
            }
            Node::Assign(l, r) => {
                let (l, r) = (*l, *r);
                let lt = self.a.types[l as usize];
                // RHS chứa alloca: sub sp làm lệch mọi push đang treo — phải
                // eval alloca TRƯỚC rồi mới lấy địa chỉ lvalue
                let mut rr = r;
                while let Node::Cast(i) = self.a.nodes[rr as usize] {
                    rr = i;
                }
                // RHS là setjmp-family (returns-twice): điểm return-lần-2 (qua
                // longjmp) tái-thực-thi vùng SAU `bl`, nên KHÔNG được pop dest-addr
                // từ slot đẩy TRƯỚC call — slot đó đã bị các push xen giữa ghi đè →
                // str vào địa chỉ rác. Phải RECOMPUTE addr SAU call (như Alloca).
                let rt2 = matches!(&self.a.nodes[rr as usize],
                    Node::Call(n, ..) if matches!(n.as_str(),
                        "setjmp" | "_setjmp" | "sigsetjmp" | "__sigsetjmp" | "__setjmp"));
                if (matches!(self.a.nodes[rr as usize], Node::Alloca(_)) || rt2)
                    && !matches!(self.a.tt.tys[lt as usize], Ty::Struct(_))
                {
                    self.expr(r);
                    self.s += "\tstr x0, [sp, #-16]!\n";
                    self.addr(l);
                    self.s += "\tmov x1, x0\n\tldr x0, [sp], #16\n";
                    self.store(0, lt);
                    return;
                }
                self.addr(l);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(r);
                self.s += "\tldr x1, [sp], #16\n";
                if matches!(self.a.tt.tys[lt as usize], Ty::Struct(_)) {
                    self.blk_copy(self.a.tt.size(lt)); // src x0 → dst x1, rvalue = dst
                } else {
                    self.store(0, lt);
                }
            }
            Node::Neg(e) => {
                self.expr(*e);
                if self.a.tt.is_float(t) {
                    self.s += "\tfmov d0, x0\n\tfneg d0, d0\n\tfmov x0, d0\n";
                } else {
                    self.s += "\tneg x0, x0\n";
                    self.ext(t);
                }
            }
            Node::Cond(c, tb, eb) => {
                let (c, tb, eb) = (*c, *tb, *eb);
                let n = self.labels(2);
                self.expr(c);
                if tb == c {
                    // elvis "a ?: b": a đã ở x0
                    _ = writeln!(self.s, "\tcbnz x0, L{}", n + 1);
                    self.expr(eb);
                    _ = writeln!(self.s, "L{}:", n + 1);
                    return;
                }
                _ = writeln!(self.s, "\tcbz x0, L{n}");
                self.expr(tb);
                _ = writeln!(self.s, "\tb L{}\nL{n}:", n + 1);
                self.expr(eb);
                _ = writeln!(self.s, "L{}:", n + 1);
            }
            Node::Comma(l, r) => {
                self.expr(*l);
                self.expr(*r);
            }
            Node::Post(op, l, delta) => {
                let (op, l, delta) = (*op, *l, *delta);
                let lt = self.a.types[l as usize];
                self.addr(l);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.load(lt);
                self.s += "\tldr x1, [sp], #16\n";
                self.imm("x3", delta);
                _ = writeln!(
                    self.s,
                    "\t{} x2, x0, x3",
                    if op == "+" { "add" } else { "sub" }
                );
                self.store(2, lt);
            }
            Node::VaArea(off) => _ = writeln!(self.s, "\tadd x0, x29, #{off}"),
            Node::VaStart(ap) => {
                let ap = *ap;
                // điền va_list AAPCS từ trạng thái prologue (va = gp,fp,stk,frame)
                let (gp, fp, stk, frame) = self.va;
                self.addr(ap); // x0 = &ap
                self.imm("x9", (16 + stk) as i64);
                self.s += "\tadd x9, x29, x9\n\tstr x9, [x0]\n"; // __stack
                self.imm("x9", frame as i64);
                self.s += "\tsub x9, x29, x9\n\tstr x9, [x0, #8]\n"; // __gr_top
                self.s += "\tsub x9, x9, #64\n\tstr x9, [x0, #16]\n"; // __vr_top
                _ = writeln!(
                    self.s,
                    "\tmov x9, #{}\n\tstr w9, [x0, #24]",
                    (gp as i64 - 8) * 8
                );
                _ = writeln!(
                    self.s,
                    "\tmov x9, #{}\n\tstr w9, [x0, #28]",
                    (fp as i64 - 8) * 16
                );
            }
            Node::VaArg(ap, t, tmp) => {
                let (ap, t, tmp) = (*ap, *t, *tmp);
                // chọn vùng: offs âm → còn reg trong save area, ≥0 → stack caller.
                // Scalar khớp chính xác AAPCS; HFA đi VR (C.3) — save area để mỗi
                // member 1 q-slot 16B nên phải gather về scratch liên tục;
                // composite thường ≤16 đi block GP, >16 gián tiếp qua con trỏ
                // (HFA >16 KHÔNG gián tiếp — C.3 giữ by-value).
                let st = matches!(self.a.tt.tys[t as usize], Ty::Struct(_));
                let sz = self.a.tt.size(t);
                let fl = self.a.tt.is_float(t);
                let hfa = if st { self.a.tt.hfa(t) } else { None };
                let (offs, top, step) = if fl {
                    (28, 16, 16)
                } else if let Some((_, n)) = hfa {
                    (28, 16, n * 16)
                } else if st && sz <= 16 {
                    (24, 8, sz.div_ceil(8) * 8)
                } else {
                    (24, 8, 8)
                };
                // stack caller: mọi scalar slot 8 (kể cả double — 16 chỉ là bước
                // VR save area), composite by-value theo size tròn 8, indirect = 1
                // slot con trỏ; trên stack HFA nằm LIỀN như composite thường
                let ldbl = matches!(self.a.tt.tys[t as usize], Ty::LDouble);
                let stk_step = if st && (sz <= 16 || hfa.is_some()) {
                    sz.div_ceil(8) * 8
                } else if ldbl {
                    16
                } else {
                    8
                };
                // pr92904, luật runtime AAPCS: composite by-value KHÔNG BAO GIỜ
                // split reg/stack — consume offs trước (ghi lại luôn), chỉ đi reg
                // khi offs MỚI ≤ 0; vắt qua 0 (vd gr_offs=-8, cần 16B) → rơi
                // nguyên khối xuống stack, offs dương khóa mọi va_arg sau (khớp
                // caller khóa NGRN/NSRN C.11/C.3). Over-alignment (aligned(16+))
                // của composite bị BỎ QUA — gcc arm64 không round NGRN/stack
                // (verify asm: named x3,x4 / anon x4,x5 / stack [sp,8]).
                let blk = st && (sz <= 16 || hfa.is_some());
                self.addr(ap); // x0 = &ap
                let l = self.labels(2);
                _ = writeln!(self.s, "\tldr w9, [x0, #{offs}]");
                if blk {
                    _ = writeln!(self.s, "\tadd w10, w9, #{step}\n\tstr w10, [x0, #{offs}]");
                    _ = writeln!(self.s, "\tcmp w10, #0\n\tb.le L{l}");
                } else {
                    _ = writeln!(self.s, "\ttbnz w9, #31, L{l}");
                }
                self.s += "\tldr x10, [x0]\n";
                if ldbl {
                    // AAPCS va_arg quad: __stack round lên 16 trước khi đọc
                    self.s += "\tadd x10, x10, #15\n\tand x10, x10, #0xfffffffffffffff0\n";
                }
                self.s += "\tadd x11, x10, #";
                _ = writeln!(self.s, "{}\n\tstr x11, [x0]\n\tb L{}", stk_step, l + 1);
                _ = writeln!(
                    self.s,
                    "L{l}:\n\tldr x10, [x0, #{top}]\n\tadd x10, x10, w9, sxtw"
                );
                if !blk {
                    _ = writeln!(self.s, "\tadd w9, w9, #{step}\n\tstr w9, [x0, #{offs}]");
                }
                if let Some((dbl, n)) = hfa {
                    // gather: member j ở [x10 + 16j] → scratch + j*esz
                    self.lea_local("x11", tmp);
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tldr x12, [x10, #{}]", 16 * j);
                            _ = writeln!(self.s, "\tstr x12, [x11, #{}]", 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tldr w12, [x10, #{}]", 16 * j);
                            _ = writeln!(self.s, "\tstr w12, [x11, #{}]", 4 * j);
                        }
                    }
                    self.s += "\tmov x10, x11\n";
                }
                _ = writeln!(self.s, "L{}:\n\tmov x0, x10", l + 1);
                if st {
                    if sz > 16 && hfa.is_none() {
                        self.s += "\tldr x0, [x0]\n"; // >16B: slot chứa CON TRỎ
                    } // struct: giá trị = địa chỉ (quy ước struct-expr của zcc)
                } else {
                    self.load(t);
                }
            }
            // EXT(gcc): inline asm subset — operand %k gán cứng x{9+k} (an toàn
            // vì -O0 không giữ giá trị sống trong thanh ghi qua statement);
            // %xk/%wk ép độ rộng, %k trần theo size kiểu operand (8→x, khác→w)
            Node::Asm(tpl, ops) => {
                let (tpl, ops) = (tpl.clone(), ops.clone());
                // gán reg: pin > tied > pool (GP x9.., FP v16.. — đều caller-
                // saved); mem dùng pool GP giữ ĐỊA CHỈ
                let (mut gp, mut vp) = (9u32, 16u32);
                let mut regs: Vec<u32> = Vec::with_capacity(ops.len());
                for op in &ops {
                    let r = if let Some(p) = op.pin {
                        p as u32
                    } else if let Some(t) = op.tied {
                        regs[t as usize]
                    } else if op.fp {
                        vp += 1;
                        vp - 1
                    } else {
                        gp += 1;
                        gp - 1
                    };
                    regs.push(r);
                }
                // eval: mem → địa chỉ, out thuần → bỏ, còn lại → giá trị; tất cả
                // lên stack trước vì expr()/addr() phá scratch
                let mut pushed: Vec<usize> = Vec::new();
                for (k, op) in ops.iter().enumerate() {
                    if op.mem {
                        self.addr(op.e);
                    } else if op.out && !op.rw {
                        continue;
                    } else {
                        self.expr(op.e);
                    }
                    self.s += "\tstr x0, [sp, #-16]!\n";
                    pushed.push(k);
                }
                let sizes: Vec<u32> = ops
                    .iter()
                    .map(|o| self.a.tt.size(self.a.types[o.e as usize]))
                    .collect();
                // pop ngược vào reg đích; FP nhận bit pattern qua ldr d —
                // convention: float sống trong x-reg dạng bit DOUBLE (load()
                // fcvt lên) nên float phải demote về s-lane cho template
                for &k in pushed.iter().rev() {
                    if ops[k].fp {
                        _ = writeln!(self.s, "\tldr d{}, [sp], #16", regs[k]);
                        if sizes[k] == 4 {
                            _ = writeln!(self.s, "\tfcvt s{0}, d{0}", regs[k]);
                        }
                    } else {
                        _ = writeln!(self.s, "\tldr x{}, [sp], #16", regs[k]);
                    }
                }
                let mut sub = String::new();
                let cs: Vec<char> = tpl.chars().collect();
                let mut i = 0;
                while i < cs.len() {
                    if cs[i] == '%' && i + 1 < cs.len() {
                        let (mut j, mut m) = (i + 1, ' ');
                        match cs[j] {
                            'x' | 'w' | 's' | 'd' => {
                                m = cs[j];
                                j += 1;
                            }
                            '%' => {
                                sub.push('%');
                                i = j + 1;
                                continue;
                            }
                            _ => {}
                        }
                        if let Some(d) = cs.get(j).and_then(|c| c.to_digit(10)) {
                            let d = d as usize;
                            let (r, op) = (regs[d], &ops[d]);
                            if op.mem {
                                _ = write!(sub, "[x{r}]");
                            } else if op.fp || m == 's' || m == 'd' {
                                let sgl = m == 's' || (m == ' ' && sizes[d] == 4);
                                _ = write!(sub, "{}{}", if sgl { 's' } else { 'd' }, r);
                            } else {
                                let w = m == 'w' || (m == ' ' && sizes[d] < 8);
                                _ = write!(sub, "{}{}", if w { 'w' } else { 'x' }, r);
                            }
                            i = j + 1;
                            continue;
                        }
                    }
                    sub.push(cs[i]);
                    i += 1;
                }
                if !sub.is_empty() {
                    _ = writeln!(self.s, "\t{}", sub.replace('\n', "\n\t"));
                }
                // writeback output (mem tự ghi qua địa chỉ): giá trị lên stack
                // trước vì addr() phá scratch
                let wb: Vec<usize> = (0..ops.len())
                    .filter(|&k| ops[k].out && !ops[k].mem)
                    .collect();
                for &k in &wb {
                    if ops[k].fp {
                        if sizes[k] == 4 {
                            // float ra từ s-lane → promote về bit double (convention)
                            _ = writeln!(self.s, "\tfcvt d{0}, s{0}", regs[k]);
                        }
                        _ = writeln!(self.s, "\tstr d{}, [sp, #-16]!", regs[k]);
                    } else {
                        _ = writeln!(self.s, "\tstr x{}, [sp, #-16]!", regs[k]);
                    }
                }
                for &k in wb.iter().rev() {
                    self.addr(ops[k].e);
                    self.s += "\tmov x1, x0\n\tldr x2, [sp], #16\n";
                    self.store(2, self.a.types[ops[k].e as usize]);
                }
            }
            // EXT(gcc): atomics __sync_* (M12) — vòng LL/SC ldaxr/stlxr; cặp
            // acquire+release mỗi lần = đủ mạnh cho seq_cst mà GCC hứa ở __sync.
            // Quy ước reg: x0=ptr, {r}1/{r}2=value, {r}9=giá trị cũ, {r}10=mới,
            // w11=cờ store-exclusive (0 = thành công).
            Node::Sync(op, args, sz) => {
                let (op, args, sz) = (*op, args.clone(), *sz);
                for (k, &a) in args.iter().enumerate() {
                    self.expr(a);
                    if k + 1 < args.len() {
                        self.s += "\tstr x0, [sp, #-16]!\n";
                    }
                }
                // arg cuối đang ở x0 → dời lên vị trí, pop ngược phần còn lại
                match args.len() {
                    3 => self.s += "\tmov x2, x0\n\tldr x1, [sp], #16\n\tldr x0, [sp], #16\n",
                    2 => self.s += "\tmov x1, x0\n\tldr x0, [sp], #16\n",
                    _ => {}
                }
                let r = if sz == 8 { "x" } else { "w" };
                let unsigned = self.a.tt.is_unsigned(t);
                // kết quả về x0 theo hợp đồng canonical 64-bit (sign/zero-extend đúng kiểu)
                let canon = |s: &mut String, res: u32| {
                    _ = match (sz, unsigned) {
                        (8, _) => writeln!(s, "\tmov x0, x{res}"),
                        (_, true) => writeln!(s, "\tmov w0, w{res}"),
                        _ => writeln!(s, "\tsxtw x0, w{res}"),
                    };
                };
                let n = self.labels(3); // loop, fail (clrex), join
                match op {
                    SyncOp::FetchAdd
                    | SyncOp::AddFetch
                    | SyncOp::FetchSub
                    | SyncOp::SubFetch
                    | SyncOp::FetchAnd
                    | SyncOp::FetchOr
                    | SyncOp::FetchXor => {
                        let ins = match op {
                            SyncOp::FetchAdd | SyncOp::AddFetch => "add",
                            SyncOp::FetchSub | SyncOp::SubFetch => "sub",
                            SyncOp::FetchAnd => "and",
                            SyncOp::FetchOr => "orr",
                            _ => "eor",
                        };
                        _ = writeln!(
                            self.s,
                            "L{n}:\n\tldaxr {r}9, [x0]\n\t{ins} {r}10, {r}9, {r}1\n\tstlxr w11, {r}10, [x0]\n\tcbnz w11, L{n}"
                        );
                        let old = !matches!(op, SyncOp::AddFetch | SyncOp::SubFetch);
                        canon(&mut self.s, if old { 9 } else { 10 });
                    }
                    SyncOp::ValCas | SyncOp::BoolCas => {
                        // fail phải clrex để nhả exclusive monitor
                        _ = writeln!(
                            self.s,
                            "L{n}:\n\tldaxr {r}9, [x0]\n\tcmp {r}9, {r}1\n\tb.ne L{}\n\tstlxr w11, {r}2, [x0]\n\tcbnz w11, L{n}",
                            n + 1
                        );
                        if matches!(op, SyncOp::BoolCas) {
                            _ = writeln!(
                                self.s,
                                "\tmov x0, #1\n\tb L{}\nL{}:\n\tclrex\n\tmov x0, #0\nL{}:",
                                n + 2,
                                n + 1,
                                n + 2
                            );
                        } else {
                            _ = writeln!(
                                self.s,
                                "\tb L{}\nL{}:\n\tclrex\nL{}:",
                                n + 2,
                                n + 1,
                                n + 2
                            );
                        }
                        if matches!(op, SyncOp::ValCas) {
                            canon(&mut self.s, 9);
                        }
                    }
                    SyncOp::TestSet => {
                        _ = writeln!(
                            self.s,
                            "L{n}:\n\tldaxr {r}9, [x0]\n\tstlxr w11, {r}1, [x0]\n\tcbnz w11, L{n}"
                        );
                        canon(&mut self.s, 9);
                    }
                    SyncOp::Release => {
                        _ = writeln!(self.s, "\tstlr {r}zr, [x0]");
                    }
                    SyncOp::Barrier => self.s += "\tdmb ish\n",
                }
            }
            // EXT(gcc): overflow builtin — eval 3 arg (x0=al, x1=bl, x9=&res), gọi ext
            Node::Overflow(op, a, b, rp) => {
                let (op, a, b, rp) = (*op, *a, *b, *rp);
                self.expr(a);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(b);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(rp);
                self.s += "\tmov x9, x0\n\tldr x1, [sp], #16\n\tldr x0, [sp], #16\n";
                let a_sg = !self.a.tt.is_unsigned(self.a.types[a as usize]);
                let b_sg = !self.a.tt.is_unsigned(self.a.types[b as usize]);
                let rt = self.a.tt.pointee(self.a.types[rp as usize]).unwrap();
                let (r_sg, rw) = (!self.a.tt.is_unsigned(rt), self.a.tt.size(rt));
                crate::ext::overflow_emit(&mut self.s, op, a_sg, b_sg, r_sg, rw);
            }
            Node::Str(i) => {
                _ = writeln!(
                    self.s,
                    "\tadrp x0, l_str{0}\n\tadd x0, x0, :lo12:l_str{0}",
                    i
                );
            }
            Node::Call(..) | Node::CallPtr(..) => self.call(id, None),
            Node::Block(v) => {
                // statement expression: x0 sau stmt cuối là giá trị
                for &c in &v.clone() {
                    self.stmt(c);
                }
            }
            Node::SRet(call, off, sz) => {
                let (call, off, sz) = (*call, *off, *sz);
                if let Some((dbl, n)) = self.a.tt.hfa(t) {
                    self.expr(call); // kết quả trong v0..v3
                    self.lea_local("x9", off);
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tstr d{j}, [x9, #{}]", 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tstr s{j}, [x9, #{}]", 4 * j);
                        }
                    }
                    self.s += "\tmov x0, x9\n";
                } else if sz > 16 {
                    // callee tự ghi vào temp qua x8; giá trị = địa chỉ temp
                    self.call(call, Some(off));
                    self.lea_local("x0", off);
                } else {
                    self.expr(call); // struct về trong x0 (và x1 nếu >8B)
                    self.lea_local("x9", off);
                    self.s += "\tstr x0, [x9]\n";
                    if sz > 8 {
                        self.s += "\tstr x1, [x9, #8]\n";
                    }
                    self.s += "\tmov x0, x9\n";
                }
            }
            Node::Zero(l, sz) => {
                let (l, sz) = (*l, *sz);
                self.addr(l);
                if sz == 0 {
                    return;
                }
                self.imm("x2", sz as i64);
                let n = self.labels(1);
                _ = writeln!(self.s, "L{n}:");
                self.s += "\tstrb wzr, [x0], #1\n\tsubs x2, x2, #1\n";
                _ = writeln!(self.s, "\tb.ne L{n}");
            }
            Node::Bin(op, l, r) => {
                let (op, l, r) = (*op, *l, *r);
                let ct = self.a.types[l as usize]; // kiểu chung sau conversion của parser
                self.expr(r);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(l);
                self.s += "\tldr x1, [sp], #16\n";
                if self.a.tt.is_float(ct) {
                    self.s += "\tfmov d0, x0\n\tfmov d1, x1\n";
                    match op {
                        "+" => self.s += "\tfadd d0, d0, d1\n\tfmov x0, d0\n",
                        "-" => self.s += "\tfsub d0, d0, d1\n\tfmov x0, d0\n",
                        "*" => self.s += "\tfmul d0, d0, d1\n\tfmov x0, d0\n",
                        "/" => self.s += "\tfdiv d0, d0, d1\n\tfmov x0, d0\n",
                        _ => {
                            let cond = match op {
                                "==" => "eq",
                                "!=" => "ne",
                                "<" => "mi",
                                "<=" => "ls",
                                ">" => "gt",
                                ">=" => "ge",
                                _ => unreachable!(),
                            };
                            _ = writeln!(self.s, "\tfcmp d0, d1\n\tcset x0, {cond}");
                        }
                    }
                    return;
                }
                let u = self.a.tt.is_unsigned(ct);
                match op {
                    "+" => self.s += "\tadd x0, x0, x1\n",
                    "-" => self.s += "\tsub x0, x0, x1\n",
                    "*" => self.s += "\tmul x0, x0, x1\n",
                    "/" if u => self.s += "\tudiv x0, x0, x1\n",
                    "/" => self.s += "\tsdiv x0, x0, x1\n",
                    "%" if u => self.s += "\tudiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
                    "%" => self.s += "\tsdiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
                    "&" => self.s += "\tand x0, x0, x1\n",
                    "|" => self.s += "\torr x0, x0, x1\n",
                    "^" => self.s += "\teor x0, x0, x1\n",
                    "<<" => self.s += "\tlsl x0, x0, x1\n",
                    ">>" if u => self.s += "\tlsr x0, x0, x1\n",
                    ">>" => self.s += "\tasr x0, x0, x1\n",
                    _ => {
                        let cond = match (op, u) {
                            ("==", _) => "eq",
                            ("!=", _) => "ne",
                            ("<", true) => "lo",
                            ("<", false) => "lt",
                            ("<=", true) => "ls",
                            ("<=", false) => "le",
                            (">", true) => "hi",
                            (">", false) => "gt",
                            (">=", true) => "hs",
                            (">=", false) => "ge",
                            _ => unreachable!(),
                        };
                        _ = writeln!(self.s, "\tcmp x0, x1\n\tcset x0, {cond}");
                        return; // kết quả 0/1, khỏi ext
                    }
                }
                // op số học 32-bit: wrap về đúng ngữ nghĩa int/uint (con trỏ/mảng thì không)
                if self.a.tt.is_integer(ct) && self.a.tt.size(ct) == 4 {
                    self.ext(ct);
                }
            }
            _ => unreachable!("statement node không lọt vào expr"),
        }
    }
    fn call(&mut self, id: NodeId, sret: Option<u32>) {
        let (callee_name, callee_expr, args, nreg) = match &self.a.nodes[id as usize] {
            Node::Call(n, a, r) => (Some(n.clone()), None, a.clone(), *r as usize),
            Node::CallPtr(e, a, r) => (None, Some(*e), a.clone(), *r as usize),
            _ => unreachable!(),
        };
        // phân slot AAPCS CHUẨN: variadic vô danh đi reg NHƯ NAMED (khác Apple),
        // float → d0-7, int → x0-7; tràn thì stack mỗi scalar một slot 8 tròn
        // (khác Apple pack natural-align); composite align 8 (over-alignment bị
        // BỎ QUA — gcc verify, xem comment spill) size tròn 8; composite tràn
        // stack khóa NGRN=8 (C.11), HFA tràn (C.3) không.
        let _ = nreg; // AAPCS không phân biệt named/vô danh khi phát call
        #[derive(Clone, Copy)]
        enum Slot {
            G(u32),
            F(u32, bool),      // bool = param float 4 byte (cần fcvt s)
            S(u32, u32, bool), // scalar → stack slot 8: (offset, size, float)
            St(u32, bool),     // struct → GPR: (reg đầu, chiếm 2 reg)
            StS(u32, u32),     // struct → stack: (offset, size)
            H(u32, u32, bool), // HFA → v-reg: (reg đầu, số member, là double)
            Q(u32),            // long double → q-reg nguyên binary128
        }
        let alup = |o: u32, a: u32| (o + a - 1) & !(a - 1);
        let (mut gp, mut fp, mut off) = (0u32, 0u32, 0u32);
        let mut plan = Vec::new();
        for &a in args.iter() {
            let t = self.a.types[a as usize];
            if matches!(self.a.tt.tys[t as usize], Ty::Struct(_)) {
                let sz = self.a.tt.size(t);
                let hfa = self.a.tt.hfa(t);
                if let Some((dbl, n)) = hfa {
                    if fp + n <= 8 {
                        plan.push(Slot::H(fp, n, dbl));
                        fp += n;
                        continue;
                    }
                    fp = 8; // AAPCS C.3
                }
                let need = if sz > 8 { 2 } else { 1 };
                if hfa.is_none() && gp + need <= 8 {
                    plan.push(Slot::St(gp, sz > 8));
                    gp += need;
                } else {
                    let o = alup(off, 8);
                    plan.push(Slot::StS(o, sz));
                    off = o + sz.div_ceil(8) * 8;
                    if hfa.is_none() {
                        gp = 8; // AAPCS C.11 — chỉ composite thường; HFA tràn (C.3) KHÔNG khóa NGRN
                    }
                }
                continue;
            }
            let fl = self.a.tt.is_float(t);
            let szt = self.a.tt.size(t);
            if fl && szt == 16 {
                // long double: q-reg (NSRN như float); tràn thì stack slot 16/16
                if fp < 8 {
                    plan.push(Slot::Q(fp));
                    fp += 1;
                } else {
                    let o = alup(off, 16);
                    plan.push(Slot::S(o, 16, true));
                    off = o + 16;
                }
            } else if fl && fp < 8 {
                plan.push(Slot::F(fp, szt == 4));
                fp += 1;
            } else if !fl && gp < 8 {
                plan.push(Slot::G(gp));
                gp += 1;
            } else {
                let o = alup(off, 8);
                plan.push(Slot::S(o, szt, fl));
                off = o + 8;
            }
        }
        let pad = (off + 15) & !15;
        if pad > 0 {
            self.sp_adjust("sub", pad);
        }
        for (&a, &sl) in args.iter().zip(&plan) {
            match sl {
                Slot::S(o, sz, fl) => {
                    self.expr(a);
                    if fl && sz == 16 {
                        _ = writeln!(
                            self.s,
                            "\tfmov d0, x0\n\tbl __extenddftf2\n\tstr q0, [sp, #{o}]"
                        );
                    } else if fl && sz == 4 {
                        _ = writeln!(self.s, "\tfmov d7, x0\n\tfcvt s7, d7\n\tstr s7, [sp, #{o}]");
                    } else {
                        _ = match sz {
                            1 => writeln!(self.s, "\tstrb w0, [sp, #{o}]"),
                            2 => writeln!(self.s, "\tstrh w0, [sp, #{o}]"),
                            4 => writeln!(self.s, "\tstr w0, [sp, #{o}]"),
                            _ => writeln!(self.s, "\tstr x0, [sp, #{o}]"),
                        };
                    }
                }
                Slot::StS(o, sz) => {
                    self.expr(a); // x0 = địa chỉ struct
                    let mut k = 0;
                    while k < sz {
                        _ = writeln!(self.s, "\tldr x8, [x0, #{k}]\n\tstr x8, [sp, #{}]", o + k);
                        k += 8;
                    }
                }
                _ => {}
            }
        }
        if let Some(e) = callee_expr {
            self.expr(e);
            self.s += "\tstr x0, [sp, #-16]!\n";
        }
        let regargs: Vec<(NodeId, Slot)> = args
            .iter()
            .zip(&plan)
            .filter(|(_, sl)| !matches!(sl, Slot::S(..) | Slot::StS(..)))
            .map(|(&a, &sl)| (a, sl))
            .collect();
        for &(a, sl) in &regargs {
            self.expr(a); // struct: x0 = địa chỉ (nạp vào reg lúc pop)
            if matches!(sl, Slot::Q(_)) {
                // nới NGAY (bl ở đây an toàn — chưa reg nào được nạp), push nguyên q
                self.s += "\tfmov d0, x0\n\tbl __extenddftf2\n\tstr q0, [sp, #-16]!\n";
            } else {
                self.s += "\tstr x0, [sp, #-16]!\n";
            }
        }
        for &(_, sl) in regargs.iter().rev() {
            match sl {
                Slot::G(i) => _ = writeln!(self.s, "\tldr x{i}, [sp], #16"),
                Slot::F(i, f32_) => {
                    _ = writeln!(self.s, "\tldr x9, [sp], #16\n\tfmov d{i}, x9");
                    if f32_ {
                        _ = writeln!(self.s, "\tfcvt s{i}, d{i}");
                    }
                }
                Slot::St(i, two) => {
                    _ = writeln!(self.s, "\tldr x9, [sp], #16\n\tldr x{i}, [x9]");
                    if two {
                        _ = writeln!(self.s, "\tldr x{}, [x9, #8]", i + 1);
                    }
                }
                Slot::H(f0, n, dbl) => {
                    self.s += "\tldr x9, [sp], #16\n";
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tldr d{}, [x9, #{}]", f0 + j, 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tldr s{}, [x9, #{}]", f0 + j, 4 * j);
                        }
                    }
                }
                Slot::Q(i) => _ = writeln!(self.s, "\tldr q{i}, [sp], #16"),
                Slot::S(..) | Slot::StS(..) => unreachable!(),
            }
        }
        if let Some(off) = sret {
            self.lea_local("x8", off); // đích cho callee ghi struct trả về
        }
        match callee_name {
            Some(n) => _ = writeln!(self.s, "\tbl {}", sym(&n)),
            None => self.s += "\tldr x9, [sp], #16\n\tblr x9\n",
        }
        if pad > 0 {
            self.sp_adjust("add", pad);
        }
        // canonical hóa giá trị trả về
        let rt = self.a.types[id as usize];
        match self.a.tt.tys[rt as usize] {
            Ty::Void => {}
            Ty::Float => self.s += "\tfcvt d0, s0\n\tfmov x0, d0\n",
            Ty::Double => self.s += "\tfmov x0, d0\n",
            Ty::LDouble => self.s += "\tbl __trunctfdf2\n\tfmov x0, d0\n", // q0 → f64 canonical
            Ty::Struct(_) => {} // x0/x1 thô — SRet bên trên hạ xuống temp
            _ => self.ext(rt),
        }
    }
}

// EXT(gcc): symbol emit — prefix \x01 (asm-label/label &&) = đã đủ tên; ELF không prefix '_'
fn sym(n: &str) -> String {
    match n.strip_prefix('\x01') {
        Some(raw) => raw.to_string(),
        None => n.to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// ĐƯỜNG IR → asm (migrate, song song đường AST ở trên; bật bằng --ir). Mô hình
// naive-stack-slot: mỗi temp một slot 8B dưới frame (x29 − (frame+8+i*8)); load
// operand vào x0/x1, tính, str kết quả về slot. Tái dùng method value-contract
// (load/store/cast_op/ext/imm/lea_local). Đuôi exotic: Opaque bridge re-emit
// subtree AST cũ. Đường này sẽ THAY đường AST khi phủ hết suite (rồi xoá AST-walk).
// ═══════════════════════════════════════════════════════════════════════════
impl<'a> Cg<'a> {
    fn ir_toff(&self, i: Tmp) -> u32 {
        self.ir_tbase + 8 + i * 8
    }
    fn tmp_load(&mut self, i: Tmp, reg: &str) {
        self.lea_local("x9", self.ir_toff(i));
        _ = writeln!(self.s, "\tldr {reg}, [x9]");
    }
    fn tmp_store(&mut self, i: Tmp, reg: &str) {
        self.lea_local("x9", self.ir_toff(i));
        _ = writeln!(self.s, "\tstr {reg}, [x9]");
    }
    fn ld_val(&mut self, v: Val, reg: &str) {
        match v {
            Val::Tmp(t) => self.tmp_load(t, reg),
            Val::Imm(x) => self.imm(reg, x),
            Val::FImm(b) => self.imm(reg, b as i64), // bit pattern f64 trong GPR
        }
    }
    fn ir_label(&self, b: u32) -> String {
        format!(".Lir_{}_{}", self.fname, b)
    }
    fn val_is_float(&self, v: Val) -> bool {
        match v {
            Val::FImm(_) => true,
            Val::Imm(_) => false,
            Val::Tmp(t) => self.a.tt.is_float(self.ir_temps[t as usize]),
        }
    }
    // x0 = &global (+ off). Mirror addr() nhánh GVar: TLS local-exec / GOT (extern
    // hoặc -fPIC non-static) / adrp+:lo12: (local). Flag tra từ ast.globals theo tên.
    fn lea_global_x0(&mut self, name: &str, off: i64) {
        let (is_tls, is_got) = {
            let gl = self.a.globals.iter().find(|g| g.name.as_str() == name);
            (
                gl.is_some_and(|g| g.is_tls),
                gl.is_some_and(|g| g.is_extern || (self.a.pic && !g.is_static)),
            )
        };
        if is_tls {
            _ = writeln!(self.s, "\tmrs x0, tpidr_el0\n\tadd x0, x0, #:tprel_hi12:{name}, lsl #12\n\tadd x0, x0, #:tprel_lo12_nc:{name}");
        } else if is_got {
            _ = writeln!(self.s, "\tadrp x0, :got:{name}\n\tldr x0, [x0, :got_lo12:{name}]");
        } else {
            _ = writeln!(self.s, "\tadrp x0, {name}\n\tadd x0, x0, :lo12:{name}");
        }
        if off > 4095 {
            self.imm("x9", off);
            self.s += "\tadd x0, x0, x9\n";
        } else if off > 0 {
            _ = writeln!(self.s, "\tadd x0, x0, #{off}");
        }
    }

    // x0 = lhs, x1 = rhs → x0 = lhs ⟨op⟩ rhs, canonical theo ct. Bản sao ngữ nghĩa
    // của Node::Bin (dùng chung khi xoá đường AST); Op enum thay punct.
    fn ir_bin(&mut self, op: Op, ct: TypeId) {
        if self.a.tt.is_float(ct) {
            self.s += "\tfmov d0, x0\n\tfmov d1, x1\n";
            match op {
                Op::Add => self.s += "\tfadd d0, d0, d1\n\tfmov x0, d0\n",
                Op::Sub => self.s += "\tfsub d0, d0, d1\n\tfmov x0, d0\n",
                Op::Mul => self.s += "\tfmul d0, d0, d1\n\tfmov x0, d0\n",
                Op::Div => self.s += "\tfdiv d0, d0, d1\n\tfmov x0, d0\n",
                _ => {
                    let cond = match op {
                        Op::Eq => "eq", Op::Ne => "ne", Op::Lt => "mi",
                        Op::Le => "ls", Op::Gt => "gt", Op::Ge => "ge",
                        _ => unreachable!(),
                    };
                    _ = writeln!(self.s, "\tfcmp d0, d1\n\tcset x0, {cond}");
                }
            }
            return;
        }
        let u = self.a.tt.is_unsigned(ct);
        match op {
            Op::Add => self.s += "\tadd x0, x0, x1\n",
            Op::Sub => self.s += "\tsub x0, x0, x1\n",
            Op::Mul => self.s += "\tmul x0, x0, x1\n",
            Op::Div if u => self.s += "\tudiv x0, x0, x1\n",
            Op::Div => self.s += "\tsdiv x0, x0, x1\n",
            Op::Rem if u => self.s += "\tudiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
            Op::Rem => self.s += "\tsdiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
            Op::And => self.s += "\tand x0, x0, x1\n",
            Op::Or => self.s += "\torr x0, x0, x1\n",
            Op::Xor => self.s += "\teor x0, x0, x1\n",
            Op::Shl => self.s += "\tlsl x0, x0, x1\n",
            Op::Shr if u => self.s += "\tlsr x0, x0, x1\n",
            Op::Shr => self.s += "\tasr x0, x0, x1\n",
            _ => {
                let cond = match (op, u) {
                    (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
                    (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
                    (Op::Le, true) => "ls", (Op::Le, false) => "le",
                    (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
                    (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
                    _ => unreachable!(),
                };
                _ = writeln!(self.s, "\tcmp x0, x1\n\tcset x0, {cond}");
                return; // 0/1, khỏi ext
            }
        }
        if self.a.tt.is_integer(ct) && self.a.tt.size(ct) == 4 {
            self.ext(ct);
        }
    }

    // canonical hóa giá trị trả (x0) theo self.fret rồi đặt vào reg ABI (bản sao
    // Node::Ret; dùng self.fret/self.fsret do emit_ir set).
    fn ir_ret_conv(&mut self) {
        match self.a.tt.tys[self.fret as usize] {
            Ty::Double => self.s += "\tfmov d0, x0\n",
            Ty::LDouble => self.s += "\tfmov d0, x0\n\tbl __extenddftf2\n",
            Ty::Float => self.s += "\tfmov d0, x0\n\tfcvt s0, d0\n",
            Ty::Struct(_) => {
                let sz = self.a.tt.size(self.fret);
                if let Some((dbl, n)) = self.a.tt.hfa(self.fret) {
                    self.s += "\tmov x9, x0\n";
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tldr d{j}, [x9, #{}]", 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tldr s{j}, [x9, #{}]", 4 * j);
                        }
                    }
                } else if sz > 16 {
                    let fs = self.fsret;
                    self.lea_local("x9", fs);
                    self.s += "\tldr x1, [x9]\n";
                    self.imm("x2", sz as i64);
                    let n = self.labels(1);
                    _ = writeln!(self.s, "L{n}:");
                    self.s += "\tldrb w3, [x0], #1\n\tstrb w3, [x1], #1\n\tsubs x2, x2, #1\n";
                    _ = writeln!(self.s, "\tb.ne L{n}");
                } else {
                    self.s += "\tmov x9, x0\n\tldr x0, [x9]\n";
                    if sz > 8 {
                        self.s += "\tldr x1, [x9, #8]\n";
                    }
                }
            }
            _ => {}
        }
    }

    fn ir_call(&mut self, dst: &Option<Tmp>, callee: &Callee, args: &[Val]) {
        let (mut gp, mut fp) = (0u32, 0u32);
        for &a in args {
            if self.val_is_float(a) && fp < 8 {
                self.ld_val(a, "x9");
                _ = writeln!(self.s, "\tfmov d{fp}, x9");
                fp += 1;
            } else if !self.val_is_float(a) && gp < 8 {
                let r = format!("x{gp}");
                self.ld_val(a, &r);
                gp += 1;
            }
            // TODO(expand): arg tràn stack (>8) + struct/HFA by value
        }
        match callee {
            Callee::Sym(name) => _ = writeln!(self.s, "\tbl {}", sym(name)),
            Callee::Ptr(p) => {
                self.ld_val(*p, "x9");
                self.s += "\tblr x9\n";
            }
        }
        if let Some(d) = dst {
            let rt = self.ir_temps[*d as usize];
            // canonical hóa return (khớp AST call): int → ext theo width (callee
            // extern trả w0 bit-cao rác), float → f64 canonical.
            match self.a.tt.tys[rt as usize] {
                Ty::Float => self.s += "\tfcvt d0, s0\n\tfmov x0, d0\n",
                Ty::Double => self.s += "\tfmov x0, d0\n",
                Ty::LDouble => self.s += "\tbl __trunctfdf2\n\tfmov x0, d0\n",
                Ty::Void | Ty::Struct(_) => {}
                _ => self.ext(rt),
            }
            self.tmp_store(*d, "x0");
        }
    }

    fn emit_inst(&mut self, i: &Inst) {
        match i {
            Inst::Copy(d, _ty, a) => {
                self.ld_val(*a, "x0");
                self.tmp_store(*d, "x0");
            }
            Inst::Bin(d, op, ty, a, b) => {
                self.ld_val(*a, "x0");
                self.ld_val(*b, "x1");
                self.ir_bin(*op, *ty);
                self.tmp_store(*d, "x0");
            }
            Inst::Un(d, u, ty, a) => {
                self.ld_val(*a, "x0");
                match u {
                    Un::Neg if self.a.tt.is_float(*ty) => {
                        self.s += "\tfmov d0, x0\n\tfneg d0, d0\n\tfmov x0, d0\n"
                    }
                    Un::Neg => {
                        self.s += "\tneg x0, x0\n";
                        self.ext(*ty);
                    }
                    Un::BNot => {
                        self.s += "\tmvn x0, x0\n";
                        self.ext(*ty);
                    }
                }
                self.tmp_store(*d, "x0");
            }
            Inst::Load(d, ty, a) => {
                self.ld_val(*a, "x0");
                self.load(*ty);
                self.tmp_store(*d, "x0");
            }
            Inst::Store(ty, a, v) => {
                self.ld_val(*v, "x0");
                self.ld_val(*a, "x1");
                self.store(0, *ty);
            }
            Inst::Memcpy(d, s, sz) => {
                self.ld_val(*s, "x0"); // src
                self.ld_val(*d, "x1"); // dst
                self.blk_copy(*sz);
            }
            Inst::Lea(d, p) => {
                match p {
                    Place::Local(off) => self.lea_local("x0", *off),
                    Place::Upvar(off) => self.lea_chain("x0", *off), // EXT(gcc): nested
                    Place::Global(name, off) => self.lea_global_x0(name, *off),
                    Place::Str(i) => _ = writeln!(
                        self.s,
                        "\tadrp x0, l_str{0}\n\tadd x0, x0, :lo12:l_str{0}",
                        i
                    ),
                }
                self.tmp_store(*d, "x0");
            }
            Inst::Cast(d, from, to, a) => {
                self.ld_val(*a, "x0");
                self.cast_op(*from, *to);
                self.tmp_store(*d, "x0");
            }
            Inst::Call(dst, callee, args, _nfix) => self.ir_call(dst, callee, args),
            Inst::Opaque(dst, node) => {
                // BRIDGE tạm: re-emit subtree AST cũ (kết quả x0), nạp vào temp.
                match dst {
                    Some(d) => {
                        self.expr(*node);
                        self.tmp_store(*d, "x0");
                    }
                    None => self.stmt(*node),
                }
            }
        }
    }

    fn emit_term(&mut self, t: &Term) {
        match t {
            Term::Jmp(b) => _ = writeln!(self.s, "\tb {}", self.ir_label(*b)),
            Term::Br(c, tb, eb) => {
                self.ld_val(*c, "x0");
                let (lt, le) = (self.ir_label(*tb), self.ir_label(*eb));
                _ = writeln!(self.s, "\tcbnz x0, {lt}\n\tb {le}");
            }
            Term::Ret(v) => {
                match v {
                    Some(v) => {
                        self.ld_val(*v, "x0");
                        self.ir_ret_conv();
                    }
                    None => self.s += "\tmov x0, #0\n",
                }
                self.s += EPILOGUE;
            }
            Term::Switch(v, cases, def) => {
                self.ld_val(*v, "x0");
                for (k, b) in cases {
                    self.imm("x1", *k);
                    _ = writeln!(self.s, "\tcmp x0, x1\n\tb.eq {}", self.ir_label(*b));
                }
                _ = writeln!(self.s, "\tb {}", self.ir_label(*def));
            }
            // rơi khỏi hàm không được: chốt bằng default (giống blanket đường AST)
            Term::Unreachable => self.s += "\tmov x0, #0\n\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n",
        }
    }

    fn emit_ir_body(&mut self, irf: &IrFunc) {
        // Temp IR sống DƯỚI khung C (và dưới vùng variadic-save 192 nếu có, do
        // emit_params đã sub trước). Param đã nằm sẵn trong slot khung (emit_params
        // theo ABI) → body đọc qua Var(off)→Load, KHÔNG cần param-temp.
        self.ir_tbase = irf.frame + if self.fvariadic { 192 } else { 0 };
        self.ir_temps = irf.temps.clone();
        let tbytes = (irf.temps.len() as u32 * 8).next_multiple_of(16);
        if tbytes > 0 {
            self.sp_adjust("sub", tbytes);
        }
        for (bi, blk) in irf.blocks.iter().enumerate() {
            _ = writeln!(self.s, "{}:", self.ir_label(bi as u32));
            // EXT(gcc): nhãn C tại block này → phát `lg_fname.name:` cho computed-goto
            // (&&label / goto *). Có nhãn ⟹ đích goto: C99 6.8.6.1 đòi SP=base mọi
            // lối vào (goto-lùi từ trong scope VLA phải dealloc). goto CẤM nhảy VÀO
            // scope VLA nên đích luôn ở depth ≤ hiện tại → reset base an toàn.
            let is_label = irf.labels.iter().any(|(_, b)| *b == bi as u32);
            for (name, _) in irf.labels.iter().filter(|(_, b)| *b == bi as u32) {
                _ = writeln!(self.s, "lg_{}.{}:", self.fname, name);
            }
            if is_label && self.fhasvla {
                self.reset_sp_base();
            }
            for inst in &blk.insts {
                self.emit_inst(inst);
            }
            self.emit_term(&blk.term);
        }
    }
}

/// Điểm vào đường IR: lower(AST) → asm. Bật bằng --ir (driver). Chưa phủ hết
/// suite — CORE (scalar/control/mem/call) chạy; đuôi exotic + global/string đang
/// mở rộng. Khi xanh hết suite: thay emit(), xoá đường AST, đo trần ≤10k.
pub fn emit_ir(ast: &Ast) -> String {
    let funcs = ir::lower(ast);
    let mut g = Cg {
        s: String::from(".cfi_sections .eh_frame\n.text\n"),
        a: ast,
        lbl: 0,
        brks: Vec::new(),
        conts: Vec::new(),
        fname: String::new(),
        fret: VOID,
        fsret: 0,
        va: (0, 0, 0, 0),
        fchain: 0,
        fuid: 0,
        fparent: u32::MAX,
        fframe: 0,
        fvariadic: false,
        fhasvla: false,
        vla_live: 0,
        ir_tbase: 0,
        ir_temps: Vec::new(),
    };
    for a in &ast.raw_asm {
        g.s += a;
        g.s += "\n.text\n";
    }
    for (fi, f) in ast.funcs.iter().enumerate() {
        g.fname = f.name.clone();
        g.fret = f.ret;
        g.fsret = f.sret;
        g.fchain = f.chain;
        g.fuid = f.uid;
        g.fparent = f.parent_uid;
        g.fframe = f.frame;
        g.fvariadic = f.variadic;
        g.fhasvla = f.has_vla;
        g.vla_live = 0;
        if !f.is_static {
            _ = writeln!(g.s, ".globl {}", f.name);
            if f.is_inline || f.is_weak {
                _ = writeln!(g.s, ".weak {}", f.name);
            }
        }
        _ = writeln!(g.s, ".type {}, %function", f.name);
        _ = write!(
            g.s,
            ".p2align 2\n{}:\n\t.cfi_startproc\n\tstp x29, x30, [sp, #-16]!\n\t.cfi_def_cfa_offset 16\n\t.cfi_offset 29, -16\n\t.cfi_offset 30, -8\n\tmov x29, sp\n\t.cfi_def_cfa_register 29\n",
            f.name
        );
        if f.frame > 0 {
            g.sp_adjust("sub", f.frame);
        }
        // Prologue param-ABI CHUNG với emit() (nested-chain/variadic-save/sret/
        // spill scalar+struct+HFA) → param nằm sẵn trong slot khung cho IR body.
        emit_params(&mut g, f);
        g.emit_ir_body(&funcs[fi]);
        g.s += "\t.cfi_endproc\n";
        _ = writeln!(g.s, "\t.size {0}, .-{0}", f.name);
    }
    emit_module_tail(&mut g, ast);
    g.s
}
