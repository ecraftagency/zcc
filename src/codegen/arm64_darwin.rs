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
    };
    for f in &ast.funcs {
        g.fname = f.name.clone();
        g.fret = f.ret;
        if !f.is_static {
            _ = writeln!(g.s, ".globl _{}", f.name);
        }
        _ = write!(g.s, ".p2align 2\n_{}:\n\tstp x29, x30, [sp, #-16]!\n\tmov x29, sp\n", f.name);
        if f.frame > 0 {
            g.sp_adjust("sub", f.frame);
        }
        // spill param theo ABI: 2 counter gp/fp, tràn thì đọc lại từ vùng stack caller
        let (mut gp, mut fp, mut stk) = (0u32, 0u32, 0u32);
        for &(off, t) in &f.params {
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
            GInit::None => {
                _ = writeln!(
                    g.s,
                    "{}.zerofill __DATA,__bss,_{},{},{}",
                    globl,
                    gl.name,
                    sz,
                    al.trailing_zeros()
                );
            }
            init => {
                _ = writeln!(
                    g.s,
                    ".section __DATA,__data\n{}.p2align {}\n_{}:",
                    globl,
                    al.trailing_zeros(),
                    gl.name
                );
                _ = match init {
                    GInit::Num(v) => match sz {
                        1 => writeln!(g.s, "\t.byte {v}"),
                        2 => writeln!(g.s, "\t.short {v}"),
                        4 => writeln!(g.s, "\t.long {v}"),
                        _ => writeln!(g.s, "\t.quad {v}"),
                    },
                    GInit::Str(i) => writeln!(g.s, "\t.quad l_str{i}"),
                    GInit::Addr(n) => writeln!(g.s, "\t.quad _{n}"),
                    GInit::None => unreachable!(),
                };
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
            Ty::Float => {
                _ = writeln!(self.s, "\tfmov d7, x{reg}\n\tfcvt s7, d7\n\tstr s7, [x1]");
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
                self.addr(l);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(r);
                self.s += "\tldr x1, [sp], #16\n";
                self.store(0, self.a.types[l as usize]);
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
            Node::Str(i) => {
                _ = writeln!(
                    self.s,
                    "\tadrp x0, l_str{0}@PAGE\n\tadd x0, x0, l_str{0}@PAGEOFF",
                    i
                );
            }
            Node::Call(..) | Node::CallPtr(..) => self.call(id),
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
    fn call(&mut self, id: NodeId) {
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
            F(u32, bool), // bool = param float 4 byte (cần fcvt s)
            S(u32),
        }
        let (mut gp, mut fp, mut stk) = (0u32, 0u32, 0u32);
        let mut plan = Vec::new();
        for (i, &a) in args.iter().enumerate() {
            let t = self.a.types[a as usize];
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
            if let Slot::S(k) = sl {
                self.expr(a);
                _ = writeln!(self.s, "\tstr x0, [sp, #{}]", 8 * k);
            }
        }
        if let Some(e) = callee_expr {
            self.expr(e);
            self.s += "\tstr x0, [sp, #-16]!\n";
        }
        let regargs: Vec<(NodeId, Slot)> = args
            .iter()
            .zip(&plan)
            .filter(|(_, sl)| !matches!(sl, Slot::S(_)))
            .map(|(&a, &sl)| (a, sl))
            .collect();
        for &(a, _) in &regargs {
            self.expr(a);
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
                Slot::S(_) => unreachable!(),
            }
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
            _ => self.ext(rt),
        }
    }
}
