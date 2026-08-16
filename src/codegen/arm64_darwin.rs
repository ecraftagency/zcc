// Codegen AArch64 Mach-O (Darwin). Ngữ nghĩa -O0, máy tính biểu thức kiểu chibicc:
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
// arg VÔ DANH của variadic LUÔN lên stack (đặc sản Apple). Return: x0 / d0.
// Label: "L{n}" tuần tự; "LC{id}" đích case; "L_{fn}_{tên}" label goto.
use crate::ast::{Ast, GInit, Node, NodeId, Ty, TypeId, VOID};
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
}

pub fn emit(ast: &Ast) -> String {
    let mut g = Cg {
        s: String::from(".section __TEXT,__text\n"),
        a: ast,
        lbl: 0,
        brks: Vec::new(),
        conts: Vec::new(),
        fname: String::new(),
        fret: VOID,
        fsret: 0,
    };
    for f in &ast.funcs {
        g.fname = f.name.clone();
        g.fret = f.ret;
        g.fsret = f.sret;
        if !f.is_static {
            _ = writeln!(g.s, ".globl _{}", f.name);
        }
        _ = write!(g.s, ".p2align 2\n_{}:\n\tstp x29, x30, [sp, #-16]!\n\tmov x29, sp\n", f.name);
        if f.frame > 0 {
            g.sp_adjust("sub", f.frame);
        }
        if f.sret != 0 {
            g.lea_local("x9", f.sret);
            g.s += "\tstr x8, [x9]\n";
        }
        // spill param theo ABI: 2 counter gp/fp, tràn thì đọc lại từ vùng stack caller
        let (mut gp, mut fp, mut stk) = (0u32, 0u32, 0u32);
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
                    _ = writeln!(g.s, "\tadd x11, x29, #{}", 16 + 8 * stk);
                    g.lea_local("x9", off);
                    g.imm("x12", sz as i64);
                    let n2 = g.labels(1);
                    _ = writeln!(g.s, "L{n2}:");
                    g.s += "\tldrb w13, [x11], #1\n\tstrb w13, [x9], #1\n\tsubs x12, x12, #1\n";
                    _ = writeln!(g.s, "\tb.ne L{n2}");
                    stk += sz.div_ceil(8);
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
                        _ = writeln!(g.s, "\tldr x11, [x29, #{}]", 16 + 8 * stk);
                        stk += 1;
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
                    _ = writeln!(g.s, "\tldr x8, [x29, #{}]", 16 + 8 * stk);
                    g.store_narrow(0, sz.min(8));
                    if sz > 8 {
                        _ = writeln!(g.s, "\tldr x8, [x29, #{}]", 16 + 8 * (stk + 1));
                        g.store_narrow(8, sz - 8);
                    }
                    stk += need;
                }
                continue;
            }
            let fl = ast.tt.is_float(t);
            if fl && fp < 8 {
                g.lea_local("x9", off);
                if ast.tt.size(t) == 4 {
                    _ = writeln!(g.s, "\tstr s{fp}, [x9]");
                } else {
                    _ = writeln!(g.s, "\tstr d{fp}, [x9]");
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
                // param trên stack của caller: [x29 + 16 + 8*stk], slot 8 byte canonical
                _ = writeln!(g.s, "\tldr x8, [x29, #{}]", 16 + 8 * stk);
                g.lea_local("x9", off);
                if fl && ast.tt.size(t) == 4 {
                    g.s += "\tfmov d7, x8\n\tfcvt s7, d7\n\tstr s7, [x9]\n";
                } else {
                    _ = match ast.tt.size(t) {
                        1 => writeln!(g.s, "\tstrb w8, [x9]"),
                        2 => writeln!(g.s, "\tstrh w8, [x9]"),
                        4 => writeln!(g.s, "\tstr w8, [x9]"),
                        _ => writeln!(g.s, "\tstr x8, [x9]"),
                    };
                }
                stk += 1;
            }
        }
        g.stmt(f.body);
        g.s += "\tmov x0, #0\n";
        g.s += EPILOGUE;
    }
    for gl in &ast.globals {
        if gl.is_extern {
            continue;
        }
        let (sz, al) = (ast.tt.size(gl.ty), ast.tt.align(gl.ty));
        let globl = if gl.is_static { String::new() } else { format!(".globl _{}\n", gl.name) };
        match &gl.init {
            GInit::None if gl.is_static => {
                _ = writeln!(
                    g.s,
                    "{}.zerofill __DATA,__bss,_{},{},{}",
                    globl,
                    gl.name,
                    sz,
                    al.trailing_zeros()
                );
            }
            GInit::None => {
                // tentative definition → common symbol (nhiều TU cùng "int x;" hợp nhất)
                _ = writeln!(g.s, ".comm _{},{},{}", gl.name, sz.max(1), al.trailing_zeros());
            }
            init => {
                _ = writeln!(
                    g.s,
                    ".section __DATA,__data\n{}.p2align {}\n_{}:",
                    globl,
                    al.trailing_zeros(),
                    gl.name
                );
                g.gdata(init, sz);
            }
        }
    }
    if !ast.strs.is_empty() {
        g.s += ".section __TEXT,__cstring\n";
        for (i, bytes) in ast.strs.iter().enumerate() {
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
    g.s
}

impl Cg<'_> {
    // phát data cho một GInit; sz = size vùng phải phủ (List chèn .space vào lỗ hổng)
    fn gdata(&mut self, init: &GInit, sz: u32) {
        _ = match init {
            GInit::Num(v) => match sz {
                1 => writeln!(self.s, "\t.byte {v}"),
                2 => writeln!(self.s, "\t.short {v}"),
                4 => writeln!(self.s, "\t.long {v}"),
                _ => writeln!(self.s, "\t.quad {v}"),
            },
            GInit::Str(i) => writeln!(self.s, "\t.quad l_str{i}"),
            GInit::Addr(n) => writeln!(self.s, "\t.quad _{n}"),
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
    // re-canonicalize x0 theo kiểu (sau op 32-bit / thu hẹp)
    fn ext(&mut self, t: TypeId) {
        if matches!(self.a.tt.tys[t as usize], Ty::Bool) {
            self.s += "\tcmp x0, #0\n\tcset x0, ne\n";
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
            Ty::Bitfield(b, boff, w) => {
                // nạp nguyên đơn vị chứa (unsigned) rồi lắc trái/phải cắt đúng field
                self.s += match self.a.tt.size(b) {
                    1 => "\tldrb w0, [x0]\n",
                    2 => "\tldrh w0, [x0]\n",
                    4 => "\tldr w0, [x0]\n",
                    _ => "\tldr x0, [x0]\n",
                };
                _ = writeln!(self.s, "\tlsl x0, x0, #{}", 64 - boff - w);
                let sh = if self.a.tt.is_unsigned(b) { "lsr" } else { "asr" };
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
    // chuyển kiểu giá trị canonical trong x0: from → to
    fn cast_op(&mut self, from: TypeId, to: TypeId) {
        let tt = &self.a.tt;
        if matches!(tt.tys[to as usize], Ty::Void | Ty::Struct(_) | Ty::Array(..)) {
            return;
        }
        match (tt.is_float(from), tt.is_float(to)) {
            (false, false) => self.ext(to),
            (false, true) => {
                let cvt = if tt.is_unsigned(from) { "ucvtf" } else { "scvtf" };
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
                let cvt = if self.a.tt.is_unsigned(to) { "fcvtzu" } else { "fcvtzs" };
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
                for &c in v {
                    self.stmt(c);
                }
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
                for &(v, cid) in cases {
                    self.imm("x1", v);
                    _ = writeln!(self.s, "\tcmp x0, x1\n\tb.eq LC{cid}");
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
            Node::Goto(name) => _ = writeln!(self.s, "\tb L_{}_{}", self.fname, name),
            Node::Label(name, st) => {
                _ = writeln!(self.s, "L_{}_{}:", self.fname, name);
                self.stmt(*st);
            }
            _ => self.expr(id),
        }
    }
    // x0 = địa chỉ của lvalue
    fn addr(&mut self, id: NodeId) {
        match &self.a.nodes[id as usize] {
            Node::Var(off) => self.lea_local("x0", *off),
            Node::GVar(i) => {
                let gl = &self.a.globals[*i as usize];
                if gl.is_extern {
                    // symbol từ dylib (stdout...): bắt buộc qua GOT
                    _ = writeln!(
                        self.s,
                        "\tadrp x0, _{0}@GOTPAGE\n\tldr x0, [x0, _{0}@GOTPAGEOFF]",
                        gl.name
                    );
                } else {
                    _ = writeln!(
                        self.s,
                        "\tadrp x0, _{0}@PAGE\n\tadd x0, x0, _{0}@PAGEOFF",
                        gl.name
                    );
                }
            }
            Node::Member(b, off) => {
                self.addr(*b);
                if *off > 0 {
                    _ = writeln!(self.s, "\tadd x0, x0, #{off}");
                }
            }
            Node::Deref(e) => self.expr(*e),
            // giá trị của expr kiểu struct = địa chỉ (SRet temp, compound literal...)
            Node::SRet(..) | Node::Comma(..) | Node::Assign(..) | Node::Cond(..) => {
                self.expr(id)
            }
            _ => unreachable!("không phải lvalue"),
        }
    }
    fn expr(&mut self, id: NodeId) {
        let t = self.a.types[id as usize];
        match &self.a.nodes[id as usize] {
            Node::Num(v) => self.imm("x0", *v),
            Node::FNum(v) => self.imm("x0", v.to_bits() as i64),
            Node::Var(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..) => {
                self.addr(id);
                // mảng/struct/hàm: giá trị = địa chỉ, không load
                if !matches!(
                    self.a.tt.tys[t as usize],
                    Ty::Array(..) | Ty::Struct(_) | Ty::Func(_)
                ) {
                    self.load(t);
                }
            }
            Node::Addr(e) => self.addr(*e),
            Node::FunAddr(name) => {
                _ = writeln!(
                    self.s,
                    "\tadrp x0, _{0}@GOTPAGE\n\tldr x0, [x0, _{0}@GOTPAGEOFF]",
                    name
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
                self.addr(l);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(r);
                self.s += "\tldr x1, [sp], #16\n";
                if matches!(self.a.tt.tys[lt as usize], Ty::Struct(_)) {
                    // struct assign: copy từng byte src (x0) → dst (x1)
                    let sz = self.a.tt.size(lt);
                    self.s += "\tmov x4, x1\n";
                    if sz > 0 {
                        let n = self.labels(1);
                        self.imm("x2", sz as i64);
                        _ = writeln!(self.s, "L{n}:");
                        self.s +=
                            "\tldrb w3, [x0], #1\n\tstrb w3, [x1], #1\n\tsubs x2, x2, #1\n";
                        _ = writeln!(self.s, "\tb.ne L{n}");
                    }
                    self.s += "\tmov x0, x4\n"; // giá trị = địa chỉ dst
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
                _ = writeln!(self.s, "\t{} x2, x0, x3", if op == "+" { "add" } else { "sub" });
                self.store(2, lt);
            }
            Node::VaArea(off) => _ = writeln!(self.s, "\tadd x0, x29, #{off}"),
            Node::Str(i) => {
                _ = writeln!(
                    self.s,
                    "\tadrp x0, l_str{0}@PAGE\n\tadd x0, x0, l_str{0}@PAGEOFF",
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
        // phân slot: named float → d0-7, named int → x0-7, còn lại (kể cả mọi
        // arg vô danh variadic) → stack 8-byte theo thứ tự
        #[derive(Clone, Copy)]
        enum Slot {
            G(u32),
            F(u32, bool),      // bool = param float 4 byte (cần fcvt s)
            S(u32),
            St(u32, bool),     // struct → GPR: (reg đầu, chiếm 2 reg)
            StS(u32, u32),     // struct → stack: (slot đầu, size)
            H(u32, u32, bool), // HFA → v-reg: (reg đầu, số member, là double)
        }
        let (mut gp, mut fp, mut stk) = (0u32, 0u32, 0u32);
        let mut plan = Vec::new();
        for (i, &a) in args.iter().enumerate() {
            let t = self.a.types[a as usize];
            if matches!(self.a.tt.tys[t as usize], Ty::Struct(_)) {
                let sz = self.a.tt.size(t);
                let hfa = self.a.tt.hfa(t);
                if let (Some((dbl, n)), true) = (hfa, (i as u32) < nreg as u32) {
                    if fp + n <= 8 {
                        plan.push(Slot::H(fp, n, dbl));
                        fp += n;
                        continue;
                    }
                    fp = 8; // AAPCS C.3
                }
                let need = if sz > 8 { 2 } else { 1 };
                if i < nreg && hfa.is_none() && gp + need <= 8 {
                    plan.push(Slot::St(gp, sz > 8));
                    gp += need;
                } else {
                    plan.push(Slot::StS(stk, sz));
                    stk += sz.div_ceil(8);
                }
                continue;
            }
            let fl = self.a.tt.is_float(t);
            if i < nreg && fl && fp < 8 {
                plan.push(Slot::F(fp, self.a.tt.size(t) == 4));
                fp += 1;
            } else if i < nreg && !fl && gp < 8 {
                plan.push(Slot::G(gp));
                gp += 1;
            } else {
                plan.push(Slot::S(stk));
                stk += 1;
            }
        }
        let pad = (8 * stk + 15) & !15;
        if pad > 0 {
            self.sp_adjust("sub", pad);
        }
        for (&a, &sl) in args.iter().zip(&plan) {
            match sl {
                Slot::S(k) => {
                    self.expr(a);
                    _ = writeln!(self.s, "\tstr x0, [sp, #{}]", 8 * k);
                }
                Slot::StS(k, sz) => {
                    self.expr(a); // x0 = địa chỉ struct
                    let mut o = 0;
                    while o < sz {
                        _ = writeln!(
                            self.s,
                            "\tldr x8, [x0, #{o}]\n\tstr x8, [sp, #{}]",
                            8 * k + o
                        );
                        o += 8;
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
            .filter(|(_, sl)| !matches!(sl, Slot::S(_) | Slot::StS(..)))
            .map(|(&a, &sl)| (a, sl))
            .collect();
        for &(a, _) in &regargs {
            self.expr(a); // struct: x0 = địa chỉ (nạp vào reg lúc pop)
            self.s += "\tstr x0, [sp, #-16]!\n";
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
                Slot::S(_) | Slot::StS(..) => unreachable!(),
            }
        }
        if let Some(off) = sret {
            self.lea_local("x8", off); // đích cho callee ghi struct trả về
        }
        match callee_name {
            Some(n) => _ = writeln!(self.s, "\tbl _{n}"),
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
            Ty::Struct(_) => {} // x0/x1 thô — SRet bên trên hạ xuống temp
            _ => self.ext(rt),
        }
    }
}
