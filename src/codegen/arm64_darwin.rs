// Codegen AArch64 Mach-O (Darwin). Ngữ nghĩa -O0, máy tính biểu thức kiểu chibicc:
// kết quả luôn ở x0; binary op sinh vế phải trước, push xuống stack (16 byte giữ
// alignment), sinh vế trái, pop vế phải vào x1 rồi `x0 = x0 op x1`.
// Local nằm ở [x29 - offset]; sp tại ranh giới statement luôn = x29 - frame.
// Mọi giá trị sống trong thanh ghi 64-bit sign-extended; kiểu chỉ quyết định
// độ rộng load/store (1: ldrsb/strb, 4: ldrsw/str w, 8: ldr/str x) — char
// mặc định signed trên Darwin nên dùng ldrsb. Giá trị kiểu mảng = địa chỉ (decay).
use crate::ast::{Ast, GInit, Node, NodeId, Ty};
use std::fmt::Write;

const EPILOGUE: &str = "\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n";

pub fn emit(ast: &Ast) -> String {
    let mut s = String::from(".section __TEXT,__text\n");
    let mut lbl = 0;
    for f in &ast.funcs {
        _ = write!(
            s,
            ".globl _{0}\n.p2align 2\n_{0}:\n\
             \tstp x29, x30, [sp, #-16]!\n\tmov x29, sp\n\tsub sp, sp, #{1}\n",
            f.name, f.frame
        );
        for (i, &(off, sz)) in f.params.iter().enumerate() {
            _ = match sz {
                1 => writeln!(s, "\tsturb w{i}, [x29, #-{off}]"),
                4 => writeln!(s, "\tstur w{i}, [x29, #-{off}]"),
                _ => writeln!(s, "\tstur x{i}, [x29, #-{off}]"),
            };
        }
        stmt(&mut s, ast, f.body, &mut lbl);
        s += "\tmov x0, #0\n"; // rơi khỏi '}' không return
        s += EPILOGUE;
    }
    for g in &ast.globals {
        let (sz, al) = (ast.tt.size(g.ty), ast.tt.align(g.ty));
        match &g.init {
            GInit::None => {
                _ = writeln!(
                    s,
                    ".globl _{0}\n.zerofill __DATA,__bss,_{0},{1},{2}",
                    g.name,
                    sz,
                    al.trailing_zeros()
                );
            }
            init => {
                _ = writeln!(
                    s,
                    ".section __DATA,__data\n.globl _{0}\n.p2align {1}\n_{0}:",
                    g.name,
                    al.trailing_zeros()
                );
                _ = match init {
                    GInit::Num(v) => match sz {
                        1 => writeln!(s, "\t.byte {v}"),
                        4 => writeln!(s, "\t.long {v}"),
                        _ => writeln!(s, "\t.quad {v}"),
                    },
                    GInit::Str(i) => writeln!(s, "\t.quad l_str{i}"),
                    GInit::None => unreachable!(),
                };
            }
        }
    }
    if !ast.strs.is_empty() {
        s += ".section __TEXT,__cstring\n";
        for (i, bytes) in ast.strs.iter().enumerate() {
            _ = write!(s, "l_str{}:\n\t.asciz \"", i);
            for &b in bytes {
                match b {
                    b'"' | b'\\' => _ = write!(s, "\\{}", b as char),
                    0x20..=0x7e => s.push(b as char),
                    _ => _ = write!(s, "\\{:03o}", b),
                }
            }
            s += "\"\n";
        }
    }
    s
}

fn stmt(s: &mut String, a: &Ast, id: NodeId, lbl: &mut u32) {
    let n = *lbl; // 2 label dự phòng cho nhánh/vòng lặp
    match &a.nodes[id as usize] {
        Node::Ret(e) => {
            expr(s, a, *e);
            *s += EPILOGUE;
        }
        Node::Block(v) => {
            for &c in v {
                stmt(s, a, c, lbl);
            }
        }
        Node::If(c, t, e) => {
            *lbl += 2;
            expr(s, a, *c);
            _ = writeln!(s, "\tcbz x0, L{n}");
            stmt(s, a, *t, lbl);
            _ = writeln!(s, "\tb L{}\nL{n}:", n + 1);
            if let Some(e) = e {
                stmt(s, a, *e, lbl);
            }
            _ = writeln!(s, "L{}:", n + 1);
        }
        Node::While(c, b) => {
            *lbl += 2;
            _ = writeln!(s, "L{n}:");
            expr(s, a, *c);
            _ = writeln!(s, "\tcbz x0, L{}", n + 1);
            stmt(s, a, *b, lbl);
            _ = writeln!(s, "\tb L{n}\nL{}:", n + 1);
        }
        Node::For(i, c, nx, b) => {
            *lbl += 2;
            if let Some(i) = i {
                expr(s, a, *i);
            }
            _ = writeln!(s, "L{n}:");
            if let Some(c) = c {
                expr(s, a, *c);
                _ = writeln!(s, "\tcbz x0, L{}", n + 1);
            }
            stmt(s, a, *b, lbl);
            if let Some(nx) = nx {
                expr(s, a, *nx);
            }
            _ = writeln!(s, "\tb L{n}\nL{}:", n + 1);
        }
        _ => expr(s, a, id), // expression statement, bỏ giá trị
    }
}

// x0 = địa chỉ của lvalue
fn addr(s: &mut String, a: &Ast, id: NodeId) {
    match &a.nodes[id as usize] {
        Node::Var(off) => _ = writeln!(s, "\tsub x0, x29, #{off}"),
        Node::GVar(i) => {
            let name = &a.globals[*i as usize].name;
            _ = writeln!(s, "\tadrp x0, _{0}@PAGE\n\tadd x0, x0, _{0}@PAGEOFF", name);
        }
        Node::Member(b, off) => {
            addr(s, a, *b);
            if *off > 0 {
                _ = writeln!(s, "\tadd x0, x0, #{off}");
            }
        }
        Node::Deref(e) => expr(s, a, *e),
        _ => unreachable!("không phải lvalue"),
    }
}

fn load(s: &mut String, sz: u32) {
    *s += match sz {
        1 => "\tldrsb x0, [x0]\n",
        4 => "\tldrsw x0, [x0]\n",
        _ => "\tldr x0, [x0]\n",
    };
}

fn expr(s: &mut String, a: &Ast, id: NodeId) {
    let t = a.types[id as usize];
    match &a.nodes[id as usize] {
        Node::Num(v) => {
            // mov chỉ nhận imm16; số lớn ghép bằng movk từng khúc 16 bit
            let u = *v as u64;
            _ = writeln!(s, "\tmov x0, #{}", u & 0xffff);
            for sh in [16, 32, 48] {
                if (u >> sh) & 0xffff != 0 {
                    _ = writeln!(s, "\tmovk x0, #{}, lsl #{sh}", (u >> sh) & 0xffff);
                }
            }
        }
        Node::Var(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..) => {
            addr(s, a, id);
            // mảng: giá trị = địa chỉ (decay); struct: chỉ dùng qua member/&, không load
            if !matches!(a.tt.tys[t as usize], Ty::Array(..) | Ty::Struct(_)) {
                load(s, a.tt.size(t));
            }
        }
        Node::Addr(e) => addr(s, a, *e),
        Node::Assign(l, r) => {
            addr(s, a, *l);
            *s += "\tstr x0, [sp, #-16]!\n";
            expr(s, a, *r);
            *s += "\tldr x1, [sp], #16\n";
            *s += match a.tt.size(a.types[*l as usize]) {
                1 => "\tstrb w0, [x1]\n",
                4 => "\tstr w0, [x1]\n",
                _ => "\tstr x0, [x1]\n",
            };
        }
        Node::Neg(e) => {
            expr(s, a, *e);
            *s += "\tneg x0, x0\n";
        }
        Node::Str(i) => {
            _ = writeln!(s, "\tadrp x0, l_str{0}@PAGE\n\tadd x0, x0, l_str{0}@PAGEOFF", i);
        }
        Node::Call(name, args, nreg) => {
            // Arg vô danh của hàm variadic đi LÊN STACK, mỗi arg 8 byte (đặc sản
            // Apple, ngược Linux ARM64); arg đặt tên vẫn x0..x7.
            let nreg = *nreg as usize;
            let pad = (8 * (args.len() - nreg) + 15) & !15;
            if pad > 0 {
                _ = writeln!(s, "\tsub sp, sp, #{pad}");
            }
            for (i, &arg) in args[nreg..].iter().enumerate() {
                expr(s, a, arg);
                _ = writeln!(s, "\tstr x0, [sp, #{}]", 8 * i);
            }
            for &arg in &args[..nreg] {
                expr(s, a, arg);
                *s += "\tstr x0, [sp, #-16]!\n";
            }
            for i in (0..nreg).rev() {
                _ = writeln!(s, "\tldr x{i}, [sp], #16");
            }
            _ = writeln!(s, "\tbl _{name}");
            if pad > 0 {
                _ = writeln!(s, "\tadd sp, sp, #{pad}");
            }
        }
        Node::Bin(op, l, r) => {
            let (op, l, r) = (*op, *l, *r);
            expr(s, a, r);
            *s += "\tstr x0, [sp, #-16]!\n";
            expr(s, a, l);
            *s += "\tldr x1, [sp], #16\n";
            *s += match op {
                "+" => "\tadd x0, x0, x1\n",
                "-" => "\tsub x0, x0, x1\n",
                "*" => "\tmul x0, x0, x1\n",
                "/" => "\tsdiv x0, x0, x1\n",
                "%" => "\tsdiv x2, x0, x1\n\tmsub x0, x2, x1, x0\n",
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
