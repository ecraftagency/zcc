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
use crate::ast::{Ast, GInit, SyncOp, Ty, TypeId, VOID};
use crate::ir::{self, Callee, Inst, IrFunc, Op, Place, Term, Tmp, Un, Val};
use std::fmt::Write;

const EPILOGUE: &str = "\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n";

struct Cg<'a> {
    s: String,
    a: &'a Ast,
    lbl: u32,
    fname: String,
    fret: TypeId,
    fsret: u32, // ≠0: slot chứa con trỏ x8 (hàm trả struct >16B)
    // hàm variadic hiện tại (cho VaStart): named đã ăn (gp, fp), byte stack
    // named, frame — save area 192B nằm NGAY DƯỚI frame: [x29-frame-192, x29-frame)
    // = VR 128B rồi GP 64B; gr_top = x29-frame, vr_top = x29-frame-64
    va: (u32, u32, u32, u32),
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
    // IR mode: kích thước vùng temp-slot (tbytes) nằm DƯỚI khung C. VLA-dealloc
    // (reset_sp_base) phải trừ THÊM vùng này, nếu không sp về trên vùng temp và
    // VLA kế `sub sp` ghi đè temp (pr43220). 0 ở AST mode → không đụng.
    ir_tspill: u32,
}


fn emit_params(g: &mut Cg, f: &crate::ast::Func) {
    let ast = g.a;
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
    g.va = (gp.min(8), fp.min(8), boff, f.frame);
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
            // ELF: .rodata trơn cho MỌI string — không có mergeable-dedup theo
            // nội-dung-đến-NUL phải né (khác Darwin __cstring, nơi string chứa NUL
            // nhúng "\0abc" phải tách qua __const kẻo linker merge nhầm).
            g.s += ".section .rodata\n";
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
        let off = self.fframe + if self.fvariadic { 192 } else { 0 } + self.ir_tspill;
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
    // Địa chỉ hàm → x0. Dùng chung bởi AST-walk (Node::FunAddr) và IR (Inst::FunAddr)
    // → asm BYTE-IDENTICAL. hàm static = symbol LOCAL: cấm đi GOT — gas hạ reloc local
    // thành .text+addend, GNU ld tạo GOT entry BỎ addend → con trỏ trỏ nhầm hàm đầu
    // section (musl libc_start_main_stage2 → nhảy vào __syscall3). Local cùng TU →
    // adrp/add trực tiếp.
    fn emit_funaddr(&mut self, name: &str) {
        let sy = sym(name);
        if self.a.funcs.iter().any(|f| f.name == name && f.is_static) {
            _ = writeln!(self.s, "\tadrp x0, {0}\n\tadd x0, x0, :lo12:{0}", sy);
        } else {
            _ = writeln!(self.s, "\tadrp x0, :got:{0}\n\tldr x0, [x0, :got_lo12:{0}]", sy);
        }
    }
    // memset(x0, 0, sz): zero sz byte kể từ địa chỉ đang ở x0. Dùng chung AST-walk
    // (Node::Zero) và IR (Inst::Zero). sz==0 → no-op.
    fn emit_zero(&mut self, sz: u32) {
        if sz == 0 {
            return;
        }
        self.imm("x2", sz as i64);
        let n = self.labels(1);
        _ = writeln!(self.s, "L{n}:");
        self.s += "\tstrb wzr, [x0], #1\n\tsubs x2, x2, #1\n";
        _ = writeln!(self.s, "\tb.ne L{n}");
    }
    // &&label (GNU computed-goto) → x0. Nhãn cục bộ trong hàm hiện tại.
    fn emit_labeladdr(&mut self, name: &str) {
        _ = writeln!(
            self.s,
            "\tadrp x0, lg_{0}.{1}\n\tadd x0, x0, :lo12:lg_{0}.{1}",
            self.fname, name
        );
    }
    // __builtin_*_overflow: x0=a, x1=b, x9=&res. Kết quả bool → x0. Dùng chung
    // AST-walk (Node::Overflow) + IR (Inst::Overflow). ta/tb/rt = kiểu a/b/*rp.
    fn emit_overflow(&mut self, op: u8, ta: TypeId, tb: TypeId, rt: TypeId) {
        let a_sg = !self.a.tt.is_unsigned(ta);
        let b_sg = !self.a.tt.is_unsigned(tb);
        let (r_sg, rw) = (!self.a.tt.is_unsigned(rt), self.a.tt.size(rt));
        crate::ext::overflow_emit(&mut self.s, op, a_sg, b_sg, r_sg, rw);
    }
    // va_start: x0 = &ap. Điền va_list AAPCS từ trạng thái prologue (va=gp,fp,stk,frame).
    // Dùng chung AST-walk (Node::VaStart) + IR (Inst::VaStart).
    fn emit_vastart(&mut self) {
        let (gp, fp, stk, frame) = self.va;
        self.imm("x9", (16 + stk) as i64);
        self.s += "\tadd x9, x29, x9\n\tstr x9, [x0]\n"; // __stack
        self.imm("x9", frame as i64);
        self.s += "\tsub x9, x29, x9\n\tstr x9, [x0, #8]\n"; // __gr_top
        self.s += "\tsub x9, x9, #64\n\tstr x9, [x0, #16]\n"; // __vr_top
        _ = writeln!(self.s, "\tmov x9, #{}\n\tstr w9, [x0, #24]", (gp as i64 - 8) * 8);
        _ = writeln!(self.s, "\tmov x9, #{}\n\tstr w9, [x0, #28]", (fp as i64 - 8) * 16);
    }
    // va_arg(*(&ap)→ ở x0, kiểu t, scratch-local tmp cho HFA gather) → kết quả x0.
    // Dùng chung AST-walk (Node::VaArg) + IR (Inst::VaArg). Chi tiết AAPCS: xem cũ.
    fn emit_vaarg(&mut self, t: TypeId, tmp: u32) {
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
        let ldbl = matches!(self.a.tt.tys[t as usize], Ty::LDouble);
        let stk_step = if st && (sz <= 16 || hfa.is_some()) {
            sz.div_ceil(8) * 8
        } else if ldbl {
            16
        } else {
            8
        };
        // pr92904 AAPCS: composite by-value KHÔNG split reg/stack — consume offs
        // trước, chỉ đi reg khi offs MỚI ≤ 0; vắt qua 0 → rơi nguyên khối xuống stack.
        let blk = st && (sz <= 16 || hfa.is_some());
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
            self.s += "\tadd x10, x10, #15\n\tand x10, x10, #0xfffffffffffffff0\n";
        }
        self.s += "\tadd x11, x10, #";
        _ = writeln!(self.s, "{}\n\tstr x11, [x0]\n\tb L{}", stk_step, l + 1);
        _ = writeln!(self.s, "L{l}:\n\tldr x10, [x0, #{top}]\n\tadd x10, x10, w9, sxtw");
        if !blk {
            _ = writeln!(self.s, "\tadd w9, w9, #{step}\n\tstr w9, [x0, #{offs}]");
        }
        if let Some((dbl, n)) = hfa {
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

}

// EXT(gcc): symbol emit — prefix \x01 (asm-label/label &&) = đã đủ tên; ELF không prefix '_'
fn sym(n: &str) -> String {
    match n.strip_prefix('\x01') {
        Some(raw) => raw.to_string(),
        None => n.to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// ĐƯỜNG IR → asm — đường DUY NHẤT (AST-walk đã xoá, seal-ir-10k). Mô hình
// naive-stack-slot: mỗi temp một slot 8B dưới frame (x29 − (frame+8+i*8)); load
// operand vào x0/x1, tính, str kết quả về slot. Tái dùng method value-contract
// (load/store/cast_op/ext/imm/lea_local). Mọi construct C99 lower thành typed Inst;
// không còn Opaque bridge — không node nào tái-emit subtree AST.
// ═══════════════════════════════════════════════════════════════════════════
// Slot AAPCS cho ir_call_abi (bản độc lập của Slot cục bộ trong self.call — sẽ là
// bản DUY NHẤT khi Stage D xoá AST-walk). G=x-reg, F=v-reg float(4B cần fcvt),
// S=scalar→stack, St=struct→GPR(2 reg?), StS=struct→stack, H=HFA→v-reg, Q=ldouble q.
#[derive(Clone, Copy)]
enum ASlot {
    G(u32),
    F(u32, bool),
    S(u32, u32, bool),
    St(u32, bool),
    StS(u32, u32),
    H(u32, u32, bool),
    Q(u32),
}

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

    // Call ABI-đầy-đủ trên IR: PORT nguyên cấu trúc self.call (push/pop stack, AAPCS
    // C.1–C.11) — chỉ thay `self.expr(arg)` bằng `ld_val(val, "x0")` vì operand đã
    // materialize thành Val (temp x29-relative). Val struct = ĐỊA CHỈ (khớp expr).
    // ret struct: gom v-reg(HFA)/x0:x1(≤16B)/x8-sret(>16B) về local[sret_off], x0=&local.
    fn ir_call_abi(
        &mut self,
        dst: &Option<Tmp>,
        callee: &Callee,
        args: &[(Val, TypeId)],
        ret: TypeId,
        sret_off: u32,
    ) {
        let alup = |o: u32, a: u32| (o + a - 1) & !(a - 1);
        let (mut gp, mut fp, mut off) = (0u32, 0u32, 0u32);
        let mut plan = Vec::with_capacity(args.len());
        for &(_, t) in args {
            if matches!(self.a.tt.tys[t as usize], Ty::Struct(_)) {
                let sz = self.a.tt.size(t);
                let hfa = self.a.tt.hfa(t);
                if let Some((dbl, n)) = hfa {
                    if fp + n <= 8 {
                        plan.push(ASlot::H(fp, n, dbl));
                        fp += n;
                        continue;
                    }
                    fp = 8; // AAPCS C.3
                }
                let need = if sz > 8 { 2 } else { 1 };
                if hfa.is_none() && gp + need <= 8 {
                    plan.push(ASlot::St(gp, sz > 8));
                    gp += need;
                } else {
                    let o = alup(off, 8);
                    plan.push(ASlot::StS(o, sz));
                    off = o + sz.div_ceil(8) * 8;
                    if hfa.is_none() {
                        gp = 8; // C.11 (HFA tràn C.3 KHÔNG khóa NGRN)
                    }
                }
                continue;
            }
            let fl = self.a.tt.is_float(t);
            let szt = self.a.tt.size(t);
            if fl && szt == 16 {
                if fp < 8 {
                    plan.push(ASlot::Q(fp));
                    fp += 1;
                } else {
                    let o = alup(off, 16);
                    plan.push(ASlot::S(o, 16, true));
                    off = o + 16;
                }
            } else if fl && fp < 8 {
                plan.push(ASlot::F(fp, szt == 4));
                fp += 1;
            } else if !fl && gp < 8 {
                plan.push(ASlot::G(gp));
                gp += 1;
            } else {
                let o = alup(off, 8);
                plan.push(ASlot::S(o, szt, fl));
                off = o + 8;
            }
        }
        let pad = (off + 15) & !15;
        if pad > 0 {
            self.sp_adjust("sub", pad);
        }
        for (&(val, _), &sl) in args.iter().zip(&plan) {
            match sl {
                ASlot::S(o, sz, fl) => {
                    self.ld_val(val, "x0");
                    if fl && sz == 16 {
                        _ = writeln!(self.s, "\tfmov d0, x0\n\tbl __extenddftf2\n\tstr q0, [sp, #{o}]");
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
                ASlot::StS(o, sz) => {
                    self.ld_val(val, "x0"); // x0 = địa chỉ struct
                    let mut k = 0;
                    while k < sz {
                        _ = writeln!(self.s, "\tldr x8, [x0, #{k}]\n\tstr x8, [sp, #{}]", o + k);
                        k += 8;
                    }
                }
                _ => {}
            }
        }
        if let Callee::Ptr(p) = callee {
            self.ld_val(*p, "x0");
            self.s += "\tstr x0, [sp, #-16]!\n";
        }
        let regargs: Vec<(Val, ASlot)> = args
            .iter()
            .zip(&plan)
            .filter(|(_, sl)| !matches!(sl, ASlot::S(..) | ASlot::StS(..)))
            .map(|(&(v, _), &sl)| (v, sl))
            .collect();
        for &(val, sl) in &regargs {
            self.ld_val(val, "x0"); // struct: x0 = địa chỉ
            if matches!(sl, ASlot::Q(_)) {
                self.s += "\tfmov d0, x0\n\tbl __extenddftf2\n\tstr q0, [sp, #-16]!\n";
            } else {
                self.s += "\tstr x0, [sp, #-16]!\n";
            }
        }
        for &(_, sl) in regargs.iter().rev() {
            match sl {
                ASlot::G(i) => _ = writeln!(self.s, "\tldr x{i}, [sp], #16"),
                ASlot::F(i, f32_) => {
                    _ = writeln!(self.s, "\tldr x9, [sp], #16\n\tfmov d{i}, x9");
                    if f32_ {
                        _ = writeln!(self.s, "\tfcvt s{i}, d{i}");
                    }
                }
                ASlot::St(i, two) => {
                    _ = writeln!(self.s, "\tldr x9, [sp], #16\n\tldr x{i}, [x9]");
                    if two {
                        _ = writeln!(self.s, "\tldr x{}, [x9, #8]", i + 1);
                    }
                }
                ASlot::H(f0, n, dbl) => {
                    self.s += "\tldr x9, [sp], #16\n";
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tldr d{}, [x9, #{}]", f0 + j, 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tldr s{}, [x9, #{}]", f0 + j, 4 * j);
                        }
                    }
                }
                ASlot::Q(i) => _ = writeln!(self.s, "\tldr q{i}, [sp], #16"),
                ASlot::S(..) | ASlot::StS(..) => unreachable!(),
            }
        }
        // ret struct >16B: callee ghi thẳng qua x8 (đặt SAU pop reg, không bị đè)
        let ret_struct = matches!(self.a.tt.tys[ret as usize], Ty::Struct(_));
        if ret_struct && self.a.tt.size(ret) > 16 && self.a.tt.hfa(ret).is_none() {
            self.lea_local("x8", sret_off);
        }
        match callee {
            Callee::Sym(n) => _ = writeln!(self.s, "\tbl {}", sym(n)),
            Callee::Ptr(_) => self.s += "\tldr x9, [sp], #16\n\tblr x9\n",
        }
        if pad > 0 {
            self.sp_adjust("add", pad);
        }
        // canonical hóa / gom kết quả
        match self.a.tt.tys[ret as usize] {
            Ty::Void => {}
            Ty::Float => self.s += "\tfcvt d0, s0\n\tfmov x0, d0\n",
            Ty::Double => self.s += "\tfmov x0, d0\n",
            Ty::LDouble => self.s += "\tbl __trunctfdf2\n\tfmov x0, d0\n",
            Ty::Struct(_) => {
                let sz = self.a.tt.size(ret);
                if let Some((dbl, n)) = self.a.tt.hfa(ret) {
                    self.lea_local("x9", sret_off);
                    for j in 0..n {
                        if dbl {
                            _ = writeln!(self.s, "\tstr d{j}, [x9, #{}]", 8 * j);
                        } else {
                            _ = writeln!(self.s, "\tstr s{j}, [x9, #{}]", 4 * j);
                        }
                    }
                } else if sz <= 16 {
                    self.lea_local("x9", sret_off);
                    self.s += "\tstr x0, [x9]\n";
                    if sz > 8 {
                        self.s += "\tstr x1, [x9, #8]\n";
                    }
                }
                self.lea_local("x0", sret_off); // giá trị = &local
            }
            _ => self.ext(ret),
        }
        if let Some(d) = dst {
            self.tmp_store(*d, "x0");
        }
    }

    // EXT(gcc) atomics trên IR: PORT thân LL/SC self.expr(Node::Sync), thay arg-eval
    // bằng ld_val (operand đã là Val). x0=ptr, x1=val, x2=val2; vòng dùng x9/x10/x11.
    fn ir_sync(&mut self, dst: &Option<Tmp>, op: SyncOp, operands: &[Val], sz: u32, ret: TypeId) {
        // nạp HẾT operand trước khi vòng chiếm x9 (ld_val dùng x9 làm scratch địa chỉ)
        if let Some(v) = operands.first() {
            self.ld_val(*v, "x0");
        }
        if let Some(v) = operands.get(1) {
            self.ld_val(*v, "x1");
        }
        if let Some(v) = operands.get(2) {
            self.ld_val(*v, "x2");
        }
        let r = if sz == 8 { "x" } else { "w" };
        let unsigned = self.a.tt.is_unsigned(ret);
        let canon = |s: &mut String, res: u32| {
            _ = match (sz, unsigned) {
                (8, _) => writeln!(s, "\tmov x0, x{res}"),
                (_, true) => writeln!(s, "\tmov w0, w{res}"),
                _ => writeln!(s, "\tsxtw x0, w{res}"),
            };
        };
        let n = self.labels(3);
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
                    _ = writeln!(self.s, "\tb L{}\nL{}:\n\tclrex\nL{}:", n + 2, n + 1, n + 2);
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
            SyncOp::Release => _ = writeln!(self.s, "\tstlr {r}zr, [x0]"),
            SyncOp::Barrier => self.s += "\tdmb ish\n",
        }
        if let Some(d) = dst {
            self.tmp_store(*d, "x0");
        }
    }

    // EXT(gcc) inline asm trên IR: PORT thân self.expr(Node::Asm). Operand đã materialize
    // (op.inp = giá trị/địa chỉ; op.wb = địa chỉ writeback) → thay expr/addr bằng ld_val.
    fn ir_asm(&mut self, tpl: &str, ops: &[crate::ir::AsmIrOp]) {
        // gán reg: pin > tied > pool (GP x9.., FP v16.. — caller-saved); mem dùng pool GP
        let (mut gp, mut vp) = (9u32, 16u32);
        let mut regs: Vec<u32> = Vec::with_capacity(ops.len());
        for op in ops {
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
        let sizes: Vec<u32> = ops.iter().map(|o| self.a.tt.size(o.ty)).collect();
        // phase 1: nạp input/mem-addr lên stack (pure output = inp None → bỏ qua)
        let mut pushed: Vec<usize> = Vec::new();
        for (k, op) in ops.iter().enumerate() {
            if let Some(v) = op.inp {
                self.ld_val(v, "x0");
                self.s += "\tstr x0, [sp, #-16]!\n";
                pushed.push(k);
            }
        }
        // phase 2: pop ngược vào reg đích (FP: bit double → demote s nếu size4)
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
        // template substitution: %[xwsd]k → reg, %% → %
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
        // writeback output non-mem (mem tự ghi qua [xN]): giá trị lên stack trước
        let wb: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].wb.is_some()).collect();
        for &k in &wb {
            if ops[k].fp {
                if sizes[k] == 4 {
                    _ = writeln!(self.s, "\tfcvt d{0}, s{0}", regs[k]);
                }
                _ = writeln!(self.s, "\tstr d{}, [sp, #-16]!", regs[k]);
            } else {
                _ = writeln!(self.s, "\tstr x{}, [sp, #-16]!", regs[k]);
            }
        }
        for &k in wb.iter().rev() {
            self.ld_val(ops[k].wb.unwrap(), "x0"); // địa chỉ đích
            self.s += "\tmov x1, x0\n\tldr x2, [sp], #16\n";
            self.store(2, ops[k].ty);
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
            Inst::CallX(dst, callee, args, ret, sret) => {
                self.ir_call_abi(dst, callee, args, *ret, *sret)
            }
            Inst::Sync(dst, op, operands, sz, ret) => self.ir_sync(dst, *op, operands, *sz, *ret),
            Inst::Asm(tpl, ops) => self.ir_asm(tpl, ops),
            Inst::FunAddr(d, name) => {
                self.emit_funaddr(name);
                self.tmp_store(*d, "x0");
            }
            Inst::LabelAddr(d, name) => {
                self.emit_labeladdr(name);
                self.tmp_store(*d, "x0");
            }
            Inst::Zero(a, sz) => {
                self.ld_val(*a, "x0"); // địa chỉ → x0
                self.emit_zero(*sz);
            }
            Inst::VaStart(a) => {
                self.ld_val(*a, "x0"); // &ap → x0
                self.emit_vastart();
            }
            Inst::VaArg(d, a, t, tmp) => {
                self.ld_val(*a, "x0"); // &ap → x0
                self.emit_vaarg(*t, *tmp);
                self.tmp_store(*d, "x0");
            }
            Inst::Overflow(d, op, ta, tb, rt, a, b, rp) => {
                // a→x0, b→x1, rp→x9. tmp_load DÙNG x9 làm scratch địa chỉ → phải nạp
                // rp vào x9 SAU CÙNG (nạp a/b trước sẽ đè x9, nhưng nạp rp cuối thì
                // không gì đè nữa). Sai thứ tự = ghi kết quả sai địa chỉ (pr64006/68381…).
                self.ld_val(*a, "x0");
                self.ld_val(*b, "x1");
                self.ld_val(*rp, "x9");
                self.emit_overflow(*op, *ta, *tb, *rt);
                self.tmp_store(*d, "x0");
            }
            Inst::VaArea(d, off) => {
                _ = writeln!(self.s, "\tadd x0, x29, #{off}");
                self.tmp_store(*d, "x0");
            }
            Inst::GotoPtr(a) => {
                self.ld_val(*a, "x0");
                self.s += "\tbr x0\n";
            }
            Inst::Alloca(d, size) => {
                self.ld_val(*size, "x0"); // số byte
                self.s += "\tadd x0, x0, #15\n\tand x0, x0, #0xfffffffffffffff0\n\tsub sp, sp, x0\n\tmov x0, sp\n";
                self.vla_live += 1; // VLA sống scope hiện tại (dealloc reset_sp_base tại label)
                self.tmp_store(*d, "x0");
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
        self.ir_tspill = tbytes; // reset_sp_base (VLA-dealloc) phải trừ luôn vùng này
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

/// Điểm vào backend — đường DUY NHẤT: lower(AST) → IR → passes → asm. Phủ trọn
/// suite/csmith/musl; AST-walk emit() đã xoá (seal-ir-10k). Backend simulate per-inst.
pub fn emit_ir(ast: &Ast) -> String {
    let mut funcs = ir::lower(ast);
    // Pipeline pass tối ưu (const-fold→copy-prop→CSE→DCE tới fixpoint). Mỗi pass đã
    // chứng bảo-toàn-⟦·⟧ ở IR→IR (opt.rs::tests, equiv commuting-square). Gate ZCC_OPT
    // để A/B trong box trước khi bật mặc định (đo-trước-khi-tuyên; verify chặn IR hỏng).
    if std::env::var_os("ZCC_OPT").is_some() {
        for f in funcs.iter_mut() {
            crate::opt::optimize(&ast.tt, f);
            debug_assert!(ir::verify(f).is_ok(), "opt sinh IR hỏng: {}", f.name);
        }
    }
    let mut g = Cg {
        s: String::from(".cfi_sections .eh_frame\n.text\n"),
        a: ast,
        lbl: 0,
        fname: String::new(),
        fret: VOID,
        fsret: 0,
        va: (0, 0, 0, 0),
        fframe: 0,
        fvariadic: false,
        fhasvla: false,
        vla_live: 0,
        ir_tbase: 0,
        ir_temps: Vec::new(),
        ir_tspill: 0,
    };
    for a in &ast.raw_asm {
        g.s += a;
        g.s += "\n.text\n";
    }
    for (fi, f) in ast.funcs.iter().enumerate() {
        g.fname = f.name.clone();
        g.fret = f.ret;
        g.fsret = f.sret;
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
