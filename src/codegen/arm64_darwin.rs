// Codegen AArch64 Mach-O (Darwin). Ngữ nghĩa -O0, máy tính biểu thức kiểu chibicc:
// kết quả luôn ở x0; binary op sinh vế phải trước, push xuống stack (16 byte giữ
// alignment), sinh vế trái, pop vế phải vào x1 rồi `x0 = x0 op x1`.
// Local nằm ở [x29 - offset]; sp tại ranh giới statement luôn = x29 - frame.
// Mọi giá trị sống trong thanh ghi 64-bit sign-extended; kiểu chỉ quyết định
// độ rộng load/store (1: ldrsb/strb, 4: ldrsw/str w, 8: ldr/str x) — char
// mặc định signed trên Darwin nên dùng ldrsb. Giá trị kiểu mảng = địa chỉ (decay).
// Label: "L{n}" cấp phát tuần tự; "LC{id}" đích case (id = NodeId của Case);
// "L_{fn}_{tên}" label người dùng (goto).
use crate::ast::{Ast, GInit, Node, NodeId, Ty};
use std::fmt::Write;

const EPILOGUE: &str = "\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n";

struct Cg<'a> {
    s: String,
    a: &'a Ast,
    lbl: u32,
    brks: Vec<u32>,  // đích break của loop/switch đang mở
    conts: Vec<u32>, // đích continue
    fname: String,
}

pub fn emit(ast: &Ast) -> String {
    let mut g = Cg {
        s: String::from(".section __TEXT,__text\n"),
        a: ast,
        lbl: 0,
        brks: Vec::new(),
        conts: Vec::new(),
        fname: String::new(),
    };
    for f in &ast.funcs {
        g.fname = f.name.clone();
        _ = write!(
            g.s,
            ".globl _{0}\n.p2align 2\n_{0}:\n\
             \tstp x29, x30, [sp, #-16]!\n\tmov x29, sp\n\tsub sp, sp, #{1}\n",
            f.name, f.frame
        );
        for (i, &(off, sz)) in f.params.iter().enumerate() {
            _ = match sz {
                1 => writeln!(g.s, "\tsturb w{i}, [x29, #-{off}]"),
                4 => writeln!(g.s, "\tstur w{i}, [x29, #-{off}]"),
                _ => writeln!(g.s, "\tstur x{i}, [x29, #-{off}]"),
            };
        }
        g.stmt(f.body);
        g.s += "\tmov x0, #0\n"; // rơi khỏi '}' không return
        g.s += EPILOGUE;
    }
    for gl in &ast.globals {
        let (sz, al) = (ast.tt.size(gl.ty), ast.tt.align(gl.ty));
        match &gl.init {
            GInit::None => {
                _ = writeln!(
                    g.s,
                    ".globl _{0}\n.zerofill __DATA,__bss,_{0},{1},{2}",
                    gl.name,
                    sz,
                    al.trailing_zeros()
                );
            }
            init => {
                _ = writeln!(
                    g.s,
                    ".section __DATA,__data\n.globl _{0}\n.p2align {1}\n_{0}:",
                    gl.name,
                    al.trailing_zeros()
                );
                _ = match init {
                    GInit::Num(v) => match sz {
                        1 => writeln!(g.s, "\t.byte {v}"),
                        4 => writeln!(g.s, "\t.long {v}"),
                        _ => writeln!(g.s, "\t.quad {v}"),
                    },
                    GInit::Str(i) => writeln!(g.s, "\t.quad l_str{i}"),
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
    // cấp k label liên tiếp, trả số đầu
    fn labels(&mut self, k: u32) -> u32 {
        let n = self.lbl;
        self.lbl += k;
        n
    }
    // nạp hằng vào thanh ghi: mov imm16 + movk từng khúc 16 bit
    fn imm(&mut self, reg: &str, v: i64) {
        let u = v as u64;
        _ = writeln!(self.s, "\tmov {reg}, #{}", u & 0xffff);
        for sh in [16, 32, 48] {
            if (u >> sh) & 0xffff != 0 {
                _ = writeln!(self.s, "\tmovk {reg}, #{}, lsl #{sh}", (u >> sh) & 0xffff);
            }
        }
    }
    // store x{reg} → [x1] theo độ rộng
    fn store(&mut self, reg: u32, sz: u32) {
        _ = match sz {
            1 => writeln!(self.s, "\tstrb w{reg}, [x1]"),
            4 => writeln!(self.s, "\tstr w{reg}, [x1]"),
            _ => writeln!(self.s, "\tstr x{reg}, [x1]"),
        };
    }
    fn stmt(&mut self, id: NodeId) {
        match &self.a.nodes[id as usize] {
            Node::Ret(e) => {
                self.expr(*e);
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
                let n = self.labels(2); // n: top+continue, n+1: break
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
                let n = self.labels(3); // n: top, n+1: break, n+2: continue (trước increment)
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
                let n = self.labels(3); // n: top, n+1: break, n+2: continue (trước cond)
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
                let n = self.labels(1); // n: break/end
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
            _ => self.expr(id), // expression statement, bỏ giá trị
        }
    }
    // x0 = địa chỉ của lvalue
    fn addr(&mut self, id: NodeId) {
        match &self.a.nodes[id as usize] {
            Node::Var(off) => _ = writeln!(self.s, "\tsub x0, x29, #{off}"),
            Node::GVar(i) => {
                let name = &self.a.globals[*i as usize].name;
                _ = writeln!(self.s, "\tadrp x0, _{0}@PAGE\n\tadd x0, x0, _{0}@PAGEOFF", name);
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
    fn load(&mut self, sz: u32) {
        self.s += match sz {
            1 => "\tldrsb x0, [x0]\n",
            4 => "\tldrsw x0, [x0]\n",
            _ => "\tldr x0, [x0]\n",
        };
    }
    fn expr(&mut self, id: NodeId) {
        let t = self.a.types[id as usize];
        match &self.a.nodes[id as usize] {
            Node::Num(v) => self.imm("x0", *v),
            Node::Var(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..) => {
                self.addr(id);
                // mảng: giá trị = địa chỉ (decay); struct: chỉ dùng qua member/&, không load
                if !matches!(self.a.tt.tys[t as usize], Ty::Array(..) | Ty::Struct(_)) {
                    self.load(self.a.tt.size(t));
                }
            }
            Node::Addr(e) => self.addr(*e),
            Node::Assign(l, r) => {
                let (l, r) = (*l, *r);
                self.addr(l);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(r);
                self.s += "\tldr x1, [sp], #16\n";
                self.store(0, self.a.tt.size(self.a.types[l as usize]));
            }
            Node::Neg(e) => {
                self.expr(*e);
                self.s += "\tneg x0, x0\n";
            }
            Node::Cond(c, t, e) => {
                let (c, t, e) = (*c, *t, *e);
                let n = self.labels(2);
                self.expr(c);
                _ = writeln!(self.s, "\tcbz x0, L{n}");
                self.expr(t);
                _ = writeln!(self.s, "\tb L{}\nL{n}:", n + 1);
                self.expr(e);
                _ = writeln!(self.s, "L{}:", n + 1);
            }
            Node::Comma(l, r) => {
                self.expr(*l);
                self.expr(*r);
            }
            // x++/x--: trả giá trị CŨ trong x0, ô nhớ nhận cũ ± delta
            Node::Post(op, l, delta) => {
                let (op, l, delta) = (*op, *l, *delta);
                let sz = self.a.tt.size(self.a.types[l as usize]);
                self.addr(l);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.load(sz);
                self.s += "\tldr x1, [sp], #16\n";
                self.imm("x3", delta);
                _ = writeln!(
                    self.s,
                    "\t{} x2, x0, x3",
                    if op == "+" { "add" } else { "sub" }
                );
                self.store(2, sz);
            }
            Node::Str(i) => {
                _ = writeln!(
                    self.s,
                    "\tadrp x0, l_str{0}@PAGE\n\tadd x0, x0, l_str{0}@PAGEOFF",
                    i
                );
            }
            Node::Call(name, args, nreg) => {
                // Arg vô danh của hàm variadic đi LÊN STACK, mỗi arg 8 byte (đặc sản
                // Apple, ngược Linux ARM64); arg đặt tên vẫn x0..x7.
                let nreg = *nreg as usize;
                let pad = (8 * (args.len() - nreg) + 15) & !15;
                if pad > 0 {
                    _ = writeln!(self.s, "\tsub sp, sp, #{pad}");
                }
                for (i, &arg) in args[nreg..].iter().enumerate() {
                    self.expr(arg);
                    _ = writeln!(self.s, "\tstr x0, [sp, #{}]", 8 * i);
                }
                for &arg in &args[..nreg] {
                    self.expr(arg);
                    self.s += "\tstr x0, [sp, #-16]!\n";
                }
                for i in (0..nreg).rev() {
                    _ = writeln!(self.s, "\tldr x{i}, [sp], #16");
                }
                _ = writeln!(self.s, "\tbl _{name}");
                if pad > 0 {
                    _ = writeln!(self.s, "\tadd sp, sp, #{pad}");
                }
            }
            Node::Bin(op, l, r) => {
                let (op, l, r) = (*op, *l, *r);
                self.expr(r);
                self.s += "\tstr x0, [sp, #-16]!\n";
                self.expr(l);
                self.s += "\tldr x1, [sp], #16\n";
                self.s += match op {
                    "+" => "\tadd x0, x0, x1\n",
                    "-" => "\tsub x0, x0, x1\n",
                    "*" => "\tmul x0, x0, x1\n",
                    "/" => "\tsdiv x0, x0, x1\n",
                    "%" => "\tsdiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
                    "&" => "\tand x0, x0, x1\n",
                    "|" => "\torr x0, x0, x1\n",
                    "^" => "\teor x0, x0, x1\n",
                    "<<" => "\tlsl x0, x0, x1\n",
                    ">>" => "\tasr x0, x0, x1\n",
                    "==" => "\tcmp x0, x1\n\tcset x0, eq\n",
                    "!=" => "\tcmp x0, x1\n\tcset x0, ne\n",
                    "<" => "\tcmp x0, x1\n\tcset x0, lt\n",
                    "<=" => "\tcmp x0, x1\n\tcset x0, le\n",
                    ">" => "\tcmp x0, x1\n\tcset x0, gt\n",
                    ">=" => "\tcmp x0, x1\n\tcset x0, ge\n",
                    _ => unreachable!(),
                };
            }
            _ => unreachable!("statement node không lọt vào expr"),
        }
    }
}
