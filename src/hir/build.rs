// AST → HIR lowering (REARCH.md §12 R0.3).
//
// The frontend hands the backend an arena of `Node`s whose every convergence
// point already carries an explicit `Cast` (parser.rs header): types in the AST
// are exact C types, so lowering never re-derives a conversion — it reads one.
//
// R0/R1 storage model. The parser assigns every C local a frame offset and
// reports only `Var(off)`, not the variable's identity: two locals in disjoint
// scopes may legitimately share an offset. Reconstructing identity from offsets
// is a memory-disambiguation problem, not a lowering one — so this file emits
// ONE stack slot for the parser's whole frame and leaves every local in memory,
// addressed by `SlotAddr`. Locals become SSA values in R2.2 (`pass/sroa.rs`),
// where SROA splits the frame slot on constant offsets and Braun's construction
// promotes the pieces, each under its own commuting square. The consequence is
// deliberate: R0/R1 measure the ALLOCATOR on expression temporaries only.
//
// zcc frame convention (unchanged from rc3, THEORY II-3): a local at parser
// offset `off` lives at `x29 − off`, and the frame occupies `[x29 − frame, x29)`.
// Slot 0 therefore starts at `x29 − frame`, so the local's address inside it is
// `frame − off`.
use super::*;
use crate::ast::{self, Ast, Node, NodeId, TypeId};
use std::collections::HashMap;

pub fn build(ast: &Ast) -> Module {
    Module {
        funcs: ast.funcs.iter().map(|f| build_func(ast, f)).collect(),
    }
}

/// The C type of a value, as HIR sees it. Aggregates and functions never travel
/// as values — an expression of aggregate type evaluates to its ADDRESS (the
/// parser's convention, parser.rs:3034), which is `I64`.
fn hty(tt: &ast::TyTab, t: TypeId) -> Ty {
    match tt.tys[t as usize] {
        ast::Ty::Char | ast::Ty::UChar | ast::Ty::Bool => Ty::I8,
        ast::Ty::Short | ast::Ty::UShort => Ty::I16,
        ast::Ty::Int | ast::Ty::UInt => Ty::I32,
        ast::Ty::Long
        | ast::Ty::ULong
        | ast::Ty::Ptr(_)
        | ast::Ty::Array(..)
        | ast::Ty::Struct(_)
        | ast::Ty::Func(_)
        | ast::Ty::Void => Ty::I64,
        ast::Ty::Float => Ty::F32,
        ast::Ty::Double => Ty::F64,
        // C99 long double is binary128 in MEMORY (psABI) but zcc computes it at
        // double (THEORY II-2, float.h LDBL_MANT_DIG 53); R1.1 lowers the
        // load/store/ABI boundaries through __extenddftf2/__trunctfdf2.
        ast::Ty::LDouble => Ty::F64,
        ast::Ty::Bitfield(b, ..) => hty(tt, b),
    }
}

/// Does an expression of this type evaluate to a VALUE (scalar) rather than to
/// an address (aggregate / function)?
fn scalar(tt: &ast::TyTab, t: TypeId) -> bool {
    matches!(
        tt.tys[t as usize],
        ast::Ty::Char
            | ast::Ty::UChar
            | ast::Ty::Short
            | ast::Ty::UShort
            | ast::Ty::Int
            | ast::Ty::UInt
            | ast::Ty::Long
            | ast::Ty::ULong
            | ast::Ty::Bool
            | ast::Ty::Ptr(_)
            | ast::Ty::Float
            | ast::Ty::Double
            | ast::Ty::LDouble
            | ast::Ty::Bitfield(..)
    )
}

fn param_ty(tt: &ast::TyTab, t: TypeId) -> PTy {
    match tt.tys[t as usize] {
        ast::Ty::LDouble => PTy::LDouble,
        ast::Ty::Struct(_) | ast::Ty::Array(..) => PTy::Agg {
            size: tt.size(t),
            align: tt.align(t),
            hfa: tt.hfa(t),
        },
        _ => PTy::S(hty(tt, t)),
    }
}

struct Loop {
    brk: BlockId,
    cont: BlockId,
}

struct B<'a> {
    ast: &'a Ast,
    f: Func,
    cur: BlockId,
    /// slot 0 = the parser's frame; `frame` is its size
    frame: u32,
    loops: Vec<Loop>,
    /// `goto` label → block, created on first mention (forward or backward)
    labels: HashMap<String, BlockId>,
    /// blocks whose address is taken (EXT computed goto) — the `goto *e` edge set
    addr_taken: Vec<BlockId>,
    /// switch: the block a `case` inside the body belongs to
    cases: HashMap<NodeId, BlockId>,
}

impl<'a> B<'a> {
    fn tt(&self) -> &ast::TyTab {
        &self.ast.tt
    }
    fn ty(&self, n: NodeId) -> TypeId {
        self.ast.types[n as usize]
    }
    fn node(&self, n: NodeId) -> &'a Node {
        &self.ast.nodes[n as usize]
    }
    fn hty(&self, n: NodeId) -> Ty {
        hty(self.tt(), self.ty(n))
    }

    fn push(&mut self, inst: Inst) {
        self.f.blocks[self.cur as usize].insts.push(inst);
    }
    /// Allocate the destination of an instruction and append it.
    fn def(&mut self, ty: Ty, mk: impl FnOnce(ValueId) -> Inst) -> Operand {
        let i = self.f.blocks[self.cur as usize].insts.len() as u32;
        let v = self.f.new_value(ty, Def::Inst(self.cur, i));
        let inst = mk(v);
        self.push(inst);
        Operand::Val(v)
    }
    fn seal(&mut self, term: Term) {
        self.f.blocks[self.cur as usize].term = term;
    }
    /// Start a fresh block. Code emitted after an unconditional transfer lands in
    /// an unreachable block, which `Cfg` simply never visits.
    fn goto(&mut self, b: BlockId) {
        self.seal(Term::Jmp(Target {
            block: b,
            args: vec![],
        }));
        self.cur = b;
    }
    fn label(&mut self, name: &str) -> BlockId {
        if let Some(&b) = self.labels.get(name) {
            return b;
        }
        let b = self.f.new_block();
        self.labels.insert(name.to_string(), b);
        b
    }

    fn bin(&mut self, op: BinOp, ty: Ty, a: Operand, b: Operand) -> Operand {
        self.def(ty, |dst| Inst::Bin { dst, op, ty, a, b })
    }
    fn addk(&mut self, a: Operand, k: i64) -> Operand {
        if k == 0 {
            return a;
        }
        self.bin(BinOp::Add, Ty::I64, a, Operand::Imm(k))
    }
    fn load(&mut self, ty: Ty, addr: Operand, vol: bool) -> Operand {
        self.def(ty, |dst| Inst::Load {
            dst,
            ty,
            addr,
            aclass: ACLASS_ANY,
            vol,
        })
    }
    fn store(&mut self, ty: Ty, addr: Operand, val: Operand, vol: bool) {
        self.push(Inst::Store {
            ty,
            addr,
            val,
            aclass: ACLASS_ANY,
            vol,
        });
    }

    // ── addresses ──────────────────────────────────────────────────────────
    fn addr(&mut self, n: NodeId) -> Operand {
        match self.node(n) {
            Node::Var(off) => {
                let off = (self.frame - *off) as i64;
                self.def(Ty::I64, |dst| Inst::SlotAddr { dst, slot: 0, off })
            }
            Node::GVar(i) => {
                let sym = Sym::Global(*i);
                self.def(Ty::I64, |dst| Inst::SymAddr { dst, sym })
            }
            Node::Str(i) => {
                let sym = Sym::Str(*i);
                self.def(Ty::I64, |dst| Inst::SymAddr { dst, sym })
            }
            Node::FunAddr(name) => {
                let sym = Sym::Func(name.clone());
                self.def(Ty::I64, |dst| Inst::SymAddr { dst, sym })
            }
            Node::Member(base, off) => {
                let (base, off) = (*base, *off as i64);
                let a = self.addr(base);
                self.addk(a, off)
            }
            Node::Deref(e) => self.expr(*e),
            Node::Comma(a, b) => {
                let (a, b) = (*a, *b);
                self.expr_discard(a);
                self.addr(b)
            }
            // An lvalue may also be the result of ?: or an assignment; both reduce
            // to "evaluate, the value IS the address" for aggregates.
            _ => self.expr(n),
        }
    }

    // ── expressions ────────────────────────────────────────────────────────
    fn expr_discard(&mut self, n: NodeId) {
        self.expr(n);
    }

    fn expr(&mut self, n: NodeId) -> Operand {
        let nty = self.ty(n);
        match self.node(n) {
            Node::Num(k) => {
                let t = self.hty(n);
                if t.is_float() {
                    // an integer constant node typed float only appears after a
                    // parser-folded cast; carry the bit pattern
                    Operand::Fimm(fbits(t, *k as f64))
                } else {
                    Operand::Imm(trunc(t, *k))
                }
            }
            Node::FNum(x) => {
                let t = self.hty(n);
                Operand::Fimm(fbits(t, *x))
            }
            // An aggregate or function expression evaluates to its address.
            Node::Var(_) | Node::GVar(_) | Node::Member(..) | Node::Deref(_) => {
                let a = self.addr(n);
                if scalar(self.tt(), nty) {
                    let t = self.hty(n);
                    let vol = self.tt().is_volatile(nty);
                    self.load(t, a, vol)
                } else {
                    a
                }
            }
            Node::Str(_) | Node::FunAddr(_) => self.addr(n),
            Node::Addr(e) => {
                let e = *e;
                self.addr(e)
            }
            Node::Assign(l, r) => {
                let (l, r) = (*l, *r);
                self.assign(l, r)
            }
            Node::Cast(e) => {
                let e = *e;
                self.cast(e, nty)
            }
            Node::Neg(e) => {
                let e = *e;
                let t = self.hty(n);
                let a = self.expr(e);
                let op = if t.is_float() { UnOp::FNeg } else { UnOp::Neg };
                self.def(t, |dst| Inst::Un { dst, op, ty: t, a })
            }
            Node::Bin(op, l, r) => {
                let (op, l, r) = (*op, *l, *r);
                self.binary(op, l, r, nty)
            }
            Node::Comma(a, b) => {
                let (a, b) = (*a, *b);
                self.expr_discard(a);
                self.expr(b)
            }
            Node::Cond(c, a, b) => {
                let (c, a, b) = (*c, *a, *b);
                self.conditional(c, a, b, nty)
            }
            Node::Post(op, lv, delta) => {
                let (op, lv, delta) = (*op, *lv, *delta);
                self.postfix(op, lv, delta)
            }
            Node::Call(name, args, nreg) => {
                let (name, args, nreg) = (name.clone(), args.clone(), *nreg);
                self.call(Callee::Direct(name), &args, nreg, nty)
            }
            Node::CallPtr(fp, args, nreg) => {
                let (fp, args, nreg) = (*fp, args.clone(), *nreg);
                let p = self.expr(fp);
                self.call(Callee::Indirect(p), &args, nreg, nty)
            }
            Node::LabelAddr(name) => {
                let b = self.label(&name.clone());
                if !self.addr_taken.contains(&b) {
                    self.addr_taken.push(b);
                }
                let sym = Sym::Label(b);
                self.def(Ty::I64, |dst| Inst::SymAddr { dst, sym })
            }
            Node::Zero(lv, size) => {
                let (lv, size) = (*lv, *size as u64);
                let a = self.addr(lv);
                self.push(Inst::MemSet {
                    dst: a,
                    byte: Operand::Imm(0),
                    len: size,
                });
                a
            }
            // Statements appearing in expression position (EXT statement-expr, and
            // the statement forms the parser nests inside a `for` init/step).
            Node::Block(_)
            | Node::Ret(_)
            | Node::If(..)
            | Node::While(..)
            | Node::For(..)
            | Node::Do(..)
            | Node::Switch(..)
            | Node::Case(_)
            | Node::Break
            | Node::Continue
            | Node::Goto(_)
            | Node::GotoPtr(_)
            | Node::Label(..) => {
                self.stmt(n);
                Operand::Imm(0)
            }
            Node::SRet(..) => todo!("R1.1: struct return ≤16B"),
            Node::Alloca(_) => todo!("R1.1: VLA / alloca"),
            Node::VaArea(_) | Node::VaStart(_) | Node::VaArg(..) => todo!("R1.2: varargs"),
            Node::Sync(..) | Node::Overflow(..) | Node::Asm(..) => todo!("R1.3: EXT builtins"),
        }
    }

    fn assign(&mut self, l: NodeId, r: NodeId) -> Operand {
        let lty = self.ty(l);
        if !scalar(self.tt(), lty) {
            let size = self.tt().size64(lty);
            let d = self.addr(l);
            let s = self.expr(r);
            self.push(Inst::MemCpy {
                dst: d,
                src: s,
                len: size,
            });
            return d;
        }
        if let ast::Ty::Bitfield(..) = self.tt().tys[lty as usize] {
            todo!("R1.1: bitfield store");
        }
        let t = hty(self.tt(), lty);
        let vol = self.tt().is_volatile(lty);
        let a = self.addr(l);
        let v = self.expr(r);
        self.store(t, a, v, vol);
        v
    }

    fn cast(&mut self, e: NodeId, to: TypeId) -> Operand {
        let from = self.ty(e);
        let a = self.expr(e);
        let (ft, tt_) = (hty(self.tt(), from), hty(self.tt(), to));
        if !scalar(self.tt(), from) || !scalar(self.tt(), to) {
            return a; // aggregate ↔ pointer: an address is an address
        }
        // C99 6.3.1.2: (_Bool)x is x != 0, not a truncation.
        if matches!(self.tt().tys[to as usize], ast::Ty::Bool) {
            let (a, ft) = self.promote(a, ft, src_unsigned_of(self.tt(), from));
            let z = if ft.is_float() {
                Operand::Fimm(0)
            } else {
                Operand::Imm(0)
            };
            let op = if ft.is_float() { CmpOp::FUne } else { CmpOp::Ne };
            let c = self.def(Ty::I32, |dst| Inst::Cmp {
                dst,
                op,
                ty: ft,
                a,
                b: z,
            });
            return self.conv(CvtOp::Trunc, Ty::I32, Ty::I8, c);
        }
        if ft == tt_ {
            return a;
        }
        let src_unsigned = self.tt().is_unsigned(from);
        let op = match (ft.is_float(), tt_.is_float()) {
            (false, false) => {
                if ft.bits() < tt_.bits() {
                    if src_unsigned { CvtOp::Zext } else { CvtOp::Sext }
                } else {
                    CvtOp::Trunc
                }
            }
            (false, true) => {
                if src_unsigned { CvtOp::UiToFp } else { CvtOp::SiToFp }
            }
            (true, false) => {
                if self.tt().is_unsigned(to) { CvtOp::FpToUi } else { CvtOp::FpToSi }
            }
            (true, true) => {
                if ft.bits() < tt_.bits() { CvtOp::FpExt } else { CvtOp::FpTrunc }
            }
        };
        self.conv(op, ft, tt_, a)
    }

    /// HIR invariant (relied on by isel and by `⟦hir⟧ = ⟦mir⟧`): `Cmp` and `Bin`
    /// never appear at I8/I16. C99 6.3.1.1 promotes every narrow operand to int
    /// before any operation, so this only makes the invariant explicit — but it
    /// is what lets isel compare in a `w` register with no re-extension.
    fn promote(&mut self, a: Operand, t: Ty, unsigned: bool) -> (Operand, Ty) {
        if matches!(t, Ty::I8 | Ty::I16) {
            let op = if unsigned { CvtOp::Zext } else { CvtOp::Sext };
            (self.conv(op, t, Ty::I32, a), Ty::I32)
        } else {
            (a, t)
        }
    }

    fn conv(&mut self, op: CvtOp, from: Ty, to: Ty, a: Operand) -> Operand {
        self.def(to, |dst| Inst::Cvt {
            dst,
            op,
            from,
            to,
            a,
        })
    }

    fn binary(&mut self, op: &'static str, l: NodeId, r: NodeId, nty: TypeId) -> Operand {
        let opnd_ty = self.ty(l);
        let ot = hty(self.tt(), opnd_ty);
        let uns = self.tt().is_unsigned(opnd_ty);
        let fp = ot.is_float();
        if let Some(cmp) = cmp_op(op, fp, uns) {
            let a = self.expr(l);
            let b = self.expr(r);
            let (a, pt) = self.promote(a, ot, uns);
            let (b, ot) = self.promote(b, ot, uns);
            debug_assert_eq!(pt, ot);
            let c = self.def(Ty::I32, |dst| Inst::Cmp {
                dst,
                op: cmp,
                ty: ot,
                a,
                b,
            });
            // the parser types a relational expression `int`
            return self.fit(c, Ty::I32, hty(self.tt(), nty));
        }
        let t = hty(self.tt(), nty);
        let a = self.expr(l);
        let b = self.expr(r);
        let bop = match (op, fp) {
            ("+", false) => BinOp::Add,
            ("-", false) => BinOp::Sub,
            ("*", false) => BinOp::Mul,
            ("/", false) => {
                if self.tt().is_unsigned(nty) { BinOp::UDiv } else { BinOp::SDiv }
            }
            ("%", false) => {
                if self.tt().is_unsigned(nty) { BinOp::URem } else { BinOp::SRem }
            }
            ("+", true) => BinOp::FAdd,
            ("-", true) => BinOp::FSub,
            ("*", true) => BinOp::FMul,
            ("/", true) => BinOp::FDiv,
            ("&", _) => BinOp::And,
            ("|", _) => BinOp::Or,
            ("^", _) => BinOp::Xor,
            ("<<", _) => BinOp::Shl,
            (">>", _) => {
                if self.tt().is_unsigned(nty) { BinOp::LShr } else { BinOp::AShr }
            }
            _ => panic!("hir::build: binary operator {:?}", op),
        };
        // C99 6.5.7: the shift count is converted to the left operand's type.
        let b = if matches!(bop, BinOp::Shl | BinOp::LShr | BinOp::AShr) {
            let rt = hty(self.tt(), self.ty(r));
            self.fit(b, rt, t)
        } else {
            b
        };
        self.bin(bop, t, a, b)
    }

    /// Widen/narrow an already-computed integer value to `to` (both integer).
    fn fit(&mut self, v: Operand, from: Ty, to: Ty) -> Operand {
        if from == to {
            return v;
        }
        let op = if from.bits() < to.bits() {
            CvtOp::Zext // every producer here yields 0/1 or an unsigned count
        } else {
            CvtOp::Trunc
        };
        self.conv(op, from, to, v)
    }

    fn conditional(&mut self, c: NodeId, a: NodeId, b: NodeId, nty: TypeId) -> Operand {
        let (tb, eb, jb) = (self.f.new_block(), self.f.new_block(), self.f.new_block());
        let cond = self.cond_value(c);
        self.seal(Term::Br(
            cond,
            Target { block: tb, args: vec![] },
            Target { block: eb, args: vec![] },
        ));
        let void = matches!(self.tt().tys[nty as usize], ast::Ty::Void);
        let ty = hty(self.tt(), nty);
        if !void {
            let p = self.f.new_value(ty, Def::Param(jb, 0));
            self.f.blocks[jb as usize].params.push(p);
        }
        self.cur = tb;
        let av = self.expr(a);
        let args = if void { vec![] } else { vec![av] };
        self.seal(Term::Jmp(Target { block: jb, args }));
        self.cur = eb;
        let bv = self.expr(b);
        let args = if void { vec![] } else { vec![bv] };
        self.seal(Term::Jmp(Target { block: jb, args }));
        self.cur = jb;
        if void {
            Operand::Imm(0)
        } else {
            Operand::Val(self.f.blocks[jb as usize].params[0])
        }
    }

    /// An expression used as a truth value: HIR's `Br` tests an I32 ≠ 0, so a
    /// wider or floating operand must first be compared.
    fn cond_value(&mut self, n: NodeId) -> Operand {
        let t = self.hty(n);
        let uns = self.tt().is_unsigned(self.ty(n));
        let v = self.expr(n);
        if t == Ty::I32 {
            return v;
        }
        // ≠0 is preserved by any widening, so the promotion is free of choice
        let (v, t) = self.promote(v, t, uns);
        if t == Ty::I32 {
            return v;
        }
        let (op, zero) = if t.is_float() {
            (CmpOp::FUne, Operand::Fimm(0))
        } else {
            (CmpOp::Ne, Operand::Imm(0))
        };
        self.def(Ty::I32, |dst| Inst::Cmp {
            dst,
            op,
            ty: t,
            a: v,
            b: zero,
        })
    }

    fn postfix(&mut self, op: &'static str, lv: NodeId, delta: i64) -> Operand {
        let lty = self.ty(lv);
        let t = hty(self.tt(), lty);
        let uns = self.tt().is_unsigned(lty);
        let vol = self.tt().is_volatile(lty);
        let a = self.addr(lv);
        let old = self.load(t, a, vol);
        // C99 6.5.2.4: x++ is x = x + 1 with the usual promotion, then the
        // conversion back — which keeps the HIR "no narrow arithmetic" invariant.
        let (v, at) = self.promote(old, t, uns);
        let one = if at.is_float() {
            Operand::Fimm(fbits(at, delta as f64))
        } else {
            Operand::Imm(delta)
        };
        let bop = match (op, at.is_float()) {
            ("+", false) => BinOp::Add,
            ("-", false) => BinOp::Sub,
            ("+", true) => BinOp::FAdd,
            ("-", true) => BinOp::FSub,
            _ => panic!("hir::build: postfix {:?}", op),
        };
        let new = self.bin(bop, at, v, one);
        let new = if at == t {
            new
        } else {
            self.conv(CvtOp::Trunc, at, t, new)
        };
        self.store(t, a, new, vol);
        old
    }

    fn call(&mut self, callee: Callee, args: &[NodeId], nreg: u32, nty: TypeId) -> Operand {
        let mut sig = Sig {
            params: Vec::with_capacity(args.len()),
            ret: None,
            nfix: nreg,
            variadic: (nreg as usize) < args.len(),
        };
        let mut ops = Vec::with_capacity(args.len());
        for &a in args {
            let at = self.ty(a);
            sig.params.push(param_ty(self.tt(), at));
            ops.push(self.expr(a));
        }
        let void = matches!(self.tt().tys[nty as usize], ast::Ty::Void);
        if !void {
            sig.ret = Some(param_ty(self.tt(), nty));
        }
        if void {
            self.push(Inst::Call {
                dst: None,
                sig,
                callee,
                args: ops,
            });
            Operand::Imm(0)
        } else {
            let t = hty(self.tt(), nty);
            self.def(t, |dst| Inst::Call {
                dst: Some(dst),
                sig,
                callee,
                args: ops,
            })
        }
    }

    // ── statements ─────────────────────────────────────────────────────────
    fn stmt(&mut self, n: NodeId) {
        match self.node(n) {
            Node::Block(items) => {
                for &s in items.clone().iter() {
                    self.stmt(s);
                }
            }
            Node::Ret(e) => {
                let v = e.map(|e| self.expr(e));
                self.seal(Term::Ret(v));
                self.cur = self.f.new_block();
            }
            Node::If(c, t, e) => {
                let (c, t, e) = (*c, *t, *e);
                let (tb, jb) = (self.f.new_block(), self.f.new_block());
                let eb = if e.is_some() { self.f.new_block() } else { jb };
                let cond = self.cond_value(c);
                self.seal(Term::Br(
                    cond,
                    Target { block: tb, args: vec![] },
                    Target { block: eb, args: vec![] },
                ));
                self.cur = tb;
                self.stmt(t);
                self.goto(jb);
                if let Some(e) = e {
                    self.cur = eb;
                    self.stmt(e);
                    self.goto(jb);
                }
                self.cur = jb;
            }
            Node::While(c, body) => {
                let (c, body) = (*c, *body);
                let (hb, bb, xb) = (self.f.new_block(), self.f.new_block(), self.f.new_block());
                self.goto(hb);
                let cond = self.cond_value(c);
                self.seal(Term::Br(
                    cond,
                    Target { block: bb, args: vec![] },
                    Target { block: xb, args: vec![] },
                ));
                self.cur = bb;
                self.loops.push(Loop { brk: xb, cont: hb });
                self.stmt(body);
                self.loops.pop();
                self.goto(hb);
                self.cur = xb;
            }
            Node::Do(body, c) => {
                let (body, c) = (*body, *c);
                let (bb, cb, xb) = (self.f.new_block(), self.f.new_block(), self.f.new_block());
                self.goto(bb);
                self.loops.push(Loop { brk: xb, cont: cb });
                self.stmt(body);
                self.loops.pop();
                self.goto(cb);
                let cond = self.cond_value(c);
                self.seal(Term::Br(
                    cond,
                    Target { block: bb, args: vec![] },
                    Target { block: xb, args: vec![] },
                ));
                self.cur = xb;
            }
            Node::For(init, cond, step, body) => {
                let (init, cond, step, body) = (*init, *cond, *step, *body);
                if let Some(i) = init {
                    self.stmt_or_expr(i);
                }
                let (hb, bb, sb, xb) = (
                    self.f.new_block(),
                    self.f.new_block(),
                    self.f.new_block(),
                    self.f.new_block(),
                );
                self.goto(hb);
                match cond {
                    Some(c) => {
                        let v = self.cond_value(c);
                        self.seal(Term::Br(
                            v,
                            Target { block: bb, args: vec![] },
                            Target { block: xb, args: vec![] },
                        ));
                    }
                    None => self.seal(Term::Jmp(Target { block: bb, args: vec![] })),
                }
                self.cur = bb;
                self.loops.push(Loop { brk: xb, cont: sb });
                self.stmt(body);
                self.loops.pop();
                self.goto(sb);
                if let Some(s) = step {
                    self.stmt_or_expr(s);
                }
                self.goto(hb);
                self.cur = xb;
            }
            Node::Break => {
                let b = self.loops.last().expect("break outside loop").brk;
                self.goto(b);
                self.cur = self.f.new_block();
            }
            Node::Continue => {
                let b = self.loops.last().expect("continue outside loop").cont;
                self.goto(b);
                self.cur = self.f.new_block();
            }
            Node::Goto(name) => {
                let b = self.label(&name.clone());
                self.goto(b);
                self.cur = self.f.new_block();
            }
            Node::GotoPtr(e) => {
                let e = *e;
                let v = self.expr(e);
                let set = self.addr_taken.clone();
                self.seal(Term::GotoPtr(v, set));
                self.cur = self.f.new_block();
            }
            Node::Label(name, s) => {
                let (name, s) = (name.clone(), *s);
                let b = self.label(&name);
                self.goto(b);
                self.stmt(s);
            }
            Node::Switch(c, body, cases, default) => {
                let (c, body, cases, default) = (*c, *body, cases.clone(), *default);
                self.switch(c, body, &cases, default);
            }
            Node::Case(s) => {
                let s = *s;
                let b = self.cases[&n];
                self.goto(b);
                self.stmt(s);
            }
            _ => self.expr_discard(n),
        }
    }

    /// A `for` init/step is an expression in the grammar but may be a declaration
    /// (which the parser emits as a Block of assignments).
    fn stmt_or_expr(&mut self, n: NodeId) {
        self.stmt(n);
    }

    fn switch(
        &mut self,
        c: NodeId,
        body: NodeId,
        cases: &[(i64, i64, NodeId)],
        default: Option<NodeId>,
    ) {
        let cty = self.ty(c);
        let t = hty(self.tt(), cty);
        let v = self.expr(c);
        let xb = self.f.new_block();
        // One block per `case` label; `case lo...hi` (EXT gcc) expands to the
        // range's values, which is why the parser keeps (lo, hi) rather than one key.
        let mut arms: Vec<(i64, Target)> = Vec::new();
        for &(lo, hi, id) in cases {
            let b = match self.cases.get(&id) {
                Some(&b) => b,
                None => {
                    let b = self.f.new_block();
                    self.cases.insert(id, b);
                    b
                }
            };
            let mut k = lo;
            loop {
                arms.push((
                    k,
                    Target {
                        block: b,
                        args: vec![],
                    },
                ));
                if k == hi {
                    break;
                }
                k += 1;
            }
        }
        let dflt = match default {
            Some(id) => {
                let b = match self.cases.get(&id) {
                    Some(&b) => b,
                    None => {
                        let b = self.f.new_block();
                        self.cases.insert(id, b);
                        b
                    }
                };
                b
            }
            None => xb,
        };
        self.seal(Term::Switch(
            v,
            t,
            arms,
            Target {
                block: dflt,
                args: vec![],
            },
        ));
        self.cur = self.f.new_block(); // statements before the first case are dead
        self.loops.push(Loop {
            brk: xb,
            cont: self
                .loops
                .last()
                .map(|l| l.cont)
                .unwrap_or(xb),
        });
        self.stmt(body);
        self.loops.pop();
        self.goto(xb);
        self.cur = xb;
    }
}

fn src_unsigned_of(tt: &ast::TyTab, t: TypeId) -> bool {
    tt.is_unsigned(t)
}

fn cmp_op(op: &str, fp: bool, uns: bool) -> Option<CmpOp> {
    Some(match (op, fp, uns) {
        ("==", false, _) => CmpOp::Eq,
        ("!=", false, _) => CmpOp::Ne,
        ("<", false, false) => CmpOp::Slt,
        ("<=", false, false) => CmpOp::Sle,
        (">", false, false) => CmpOp::Sgt,
        (">=", false, false) => CmpOp::Sge,
        ("<", false, true) => CmpOp::Ult,
        ("<=", false, true) => CmpOp::Ule,
        (">", false, true) => CmpOp::Ugt,
        (">=", false, true) => CmpOp::Uge,
        ("==", true, _) => CmpOp::FOeq,
        ("!=", true, _) => CmpOp::FUne,
        ("<", true, _) => CmpOp::FOlt,
        ("<=", true, _) => CmpOp::FOle,
        (">", true, _) => CmpOp::FOgt,
        (">=", true, _) => CmpOp::FOge,
        _ => return None,
    })
}

/// The IEEE bit pattern of `x` at HIR type `t` — the representation `Operand::Fimm`
/// carries (never an `f64` field: a constant must compare bitwise, NaN included).
fn fbits(t: Ty, x: f64) -> u64 {
    match t {
        Ty::F32 => (x as f32).to_bits() as u64,
        _ => x.to_bits(),
    }
}

/// Truncate an integer constant to its HIR type, sign-extended back to i64 — the
/// canonical form every `Operand::Imm` is in.
fn trunc(t: Ty, k: i64) -> i64 {
    match t {
        Ty::I8 => k as i8 as i64,
        Ty::I16 => k as i16 as i64,
        Ty::I32 => k as i32 as i64,
        _ => k,
    }
}

fn build_func(ast: &Ast, af: &ast::Func) -> Func {
    let sig = Sig {
        params: af
            .params
            .iter()
            .map(|&(_, t)| param_ty(&ast.tt, t))
            .collect(),
        ret: if matches!(ast.tt.tys[af.ret as usize], ast::Ty::Void) {
            None
        } else {
            Some(param_ty(&ast.tt, af.ret))
        },
        nfix: af.params.len() as u32,
        variadic: af.variadic,
    };
    let mut f = Func {
        name: af.name.clone(),
        sig,
        blocks: Vec::new(),
        values: Vec::new(),
        // Slot 0 is the parser's frame block. A function whose parser frame is
        // empty gets NO stack object at all — and therefore no `sub sp` — which
        // is the common case for a leaf.
        slots: if af.frame == 0 {
            Vec::new()
        } else {
            vec![Slot {
                size: af.frame,
                // AAPCS64 §6.2.2: the frame block is 16-byte aligned.
                align: 16,
            }]
        },
        entry: 0,
        is_static: af.is_static,
        is_weak: af.is_weak || af.is_inline,
    };
    let entry = f.new_block();
    let mut b = B {
        ast,
        f,
        cur: entry,
        frame: af.frame,
        loops: Vec::new(),
        labels: HashMap::new(),
        addr_taken: Vec::new(),
        cases: HashMap::new(),
    };
    // R0/R1: a parameter arrives as a value (isel materializes it from its
    // AAPCS64 register or stack slot) and the entry block writes it into the
    // parser-assigned frame slot the body reads. R2.2 promotes both in one step,
    // at which point these stores disappear rather than being special-cased.
    for (k, &(off, t)) in af.params.iter().enumerate() {
        if !scalar(&ast.tt, t) {
            todo!("R1.2: aggregate parameter passed by value");
        }
        let ht = hty(&ast.tt, t);
        let v = b.f.new_value(ht, Def::FuncParam(k as u32));
        let off = (af.frame - off) as i64;
        let a = b.def(Ty::I64, |dst| Inst::SlotAddr { dst, slot: 0, off });
        let vol = ast.tt.is_volatile(t);
        b.store(ht, a, Operand::Val(v), vol);
    }
    b.stmt(af.body);
    // C99 6.9.1p12: falling off the end of a non-void function is undefined, but
    // main returns 0. Either way the block needs a terminator.
    if matches!(b.f.blocks[b.cur as usize].term, Term::Unreachable) {
        let r = if af.name == "main" {
            Some(Operand::Imm(0))
        } else if b.f.sig.ret.is_some() {
            Some(match b.f.sig.ret.as_ref().unwrap() {
                PTy::S(t) if t.is_float() => Operand::Fimm(0),
                _ => Operand::Imm(0),
            })
        } else {
            None
        };
        b.seal(Term::Ret(r));
    }
    f = b.f;
    // The label map may have created blocks a `goto` never reached — they keep
    // their `Unreachable` terminator and `Cfg` never visits them.
    f
}
