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

/// A bit-field access window, decoded once from `Ty::Bitfield`.
#[derive(Clone, Copy)]
struct Bf {
    /// the container's HIR type — the width actually loaded and stored
    ct: Ty,
    unsigned: bool,
    boff: u32,
    w: u32,
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
        self.f.blocks[b as usize].labels.push(name.to_string());
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

    // ── bit-fields (C99 6.7.2.1) ───────────────────────────────────────────
    // A bit-field lvalue is an ordinary lvalue of its CONTAINER type plus a
    // (bit offset, bit width) window: `Ty::Bitfield(container, boff, w)` with
    // the member's byte offset already pointing at the container. Access is the
    // two shift theorems, done at the promoted width so the HIR invariant
    // "no arithmetic at I8/I16" (see `promote`) is never broken:
    //   read  ⟦f⟧ = (x << (N−boff−w)) >>ₛ (N−w)      s = signed ? ashr : lshr
    //   write ⟦f⟧ = (x & ~M) | ((v << boff) & M),  M = ((1<<w)−1) << boff
    // The write is a read-modify-write of the whole container: C99 6.7.2.1p10
    // leaves the other bits of the addressable unit unspecified to touch, and
    // the neighbours' bits must survive — which is exactly what `~M` preserves.
    /// C99 long double on ELF: the object in memory is binary128, so an access
    /// is never a plain load/store (see `IntrinKind::LdLoad`).
    fn is_ld(&self, t: TypeId) -> bool {
        matches!(self.tt().tys[t as usize], ast::Ty::LDouble)
    }

    fn ld_load(&mut self, addr: Operand) -> Operand {
        self.def(Ty::F64, |dst| Inst::Intrinsic {
            dst: Some(dst),
            kind: IntrinKind::LdLoad,
            args: vec![addr],
        })
    }

    fn ld_store(&mut self, addr: Operand, v: Operand) {
        self.push(Inst::Intrinsic {
            dst: None,
            kind: IntrinKind::LdStore,
            args: vec![addr, v],
        });
    }

    fn bf_of(&self, t: TypeId) -> Option<Bf> {
        match self.tt().tys[t as usize] {
            ast::Ty::Bitfield(b, boff, w) => Some(Bf {
                ct: hty(self.tt(), b),
                unsigned: self.tt().is_unsigned(b),
                boff,
                w,
            }),
            _ => None,
        }
    }

    /// The width the shifts run at: a container narrower than `int` is promoted,
    /// which is also the width C99 6.3.1.1 gives the field's value.
    fn bf_wide(bf: &Bf) -> Ty {
        if bf.ct.bits() <= 32 { Ty::I32 } else { Ty::I64 }
    }

    /// Extract the field from a container-width value already in a register.
    fn bf_extract(&mut self, x: Operand, bf: &Bf) -> Operand {
        let cw = Self::bf_wide(bf);
        let n = cw.bits();
        let x = if bf.ct == cw {
            x
        } else {
            self.conv(CvtOp::Zext, bf.ct, cw, x)
        };
        let lsh = n - bf.boff - bf.w;
        let x = if lsh > 0 {
            self.bin(BinOp::Shl, cw, x, Operand::Imm(lsh as i64))
        } else {
            x
        };
        let rsh = n - bf.w;
        let x = if rsh > 0 {
            let op = if bf.unsigned { BinOp::LShr } else { BinOp::AShr };
            self.bin(op, cw, x, Operand::Imm(rsh as i64))
        } else {
            x
        };
        if bf.ct == cw {
            x
        } else {
            self.conv(CvtOp::Trunc, cw, bf.ct, x)
        }
    }

    fn bf_load(&mut self, addr: Operand, bf: Bf, vol: bool) -> Operand {
        let raw = self.load(bf.ct, addr, vol);
        self.bf_extract(raw, &bf)
    }

    /// `lv = v` on a bit-field. Returns the value of the assignment expression,
    /// which C99 6.5.16p3 defines as the value STORED — i.e. after truncation to
    /// `w` bits, so `(v.a = 5)` on `int a:3` is −3, not 5.
    fn bf_store(&mut self, addr: Operand, bf: Bf, v: Operand, vol: bool) -> Operand {
        let cw = Self::bf_wide(&bf);
        let field: i64 = if bf.w >= 64 { -1 } else { (1i64 << bf.w) - 1 };
        let mask = trunc(cw, field << bf.boff);
        let raw = self.load(bf.ct, addr, vol);
        let old = if bf.ct == cw {
            raw
        } else {
            self.conv(CvtOp::Zext, bf.ct, cw, raw)
        };
        let vv = if bf.ct == cw {
            v
        } else {
            self.conv(CvtOp::Zext, bf.ct, cw, v)
        };
        let ins = self.bin(BinOp::And, cw, vv, Operand::Imm(trunc(cw, field)));
        let ins = if bf.boff > 0 {
            self.bin(BinOp::Shl, cw, ins, Operand::Imm(bf.boff as i64))
        } else {
            ins
        };
        let kept = self.bin(BinOp::And, cw, old, Operand::Imm(trunc(cw, !mask)));
        let new = self.bin(BinOp::Or, cw, kept, ins);
        let st = if bf.ct == cw {
            new
        } else {
            self.conv(CvtOp::Trunc, cw, bf.ct, new)
        };
        self.store(bf.ct, addr, st, vol);
        // the stored field, read back without a second memory access
        self.bf_extract(st, &bf)
    }

    // ── EXT(gcc) __sync_* (ARM DDI 0487 B2.9) ──────────────────────────────
    // Every read-modify-write is the same theorem: take the exclusive monitor
    // with `ldaxr`, compute, release it with `stlxr`, and retry while the store
    // reports failure. The loop is written HERE, in ordinary HIR control flow,
    // rather than hidden inside one machine instruction — which is what makes
    // it visible to the verifier and executable by ⟦hir⟧.
    fn sync(&mut self, op: ast::SyncOp, args: &[NodeId], size: u32) -> Operand {
        use ast::SyncOp::*;
        let t = if size == 8 { Ty::I64 } else { Ty::I32 };
        if matches!(op, Barrier) {
            self.push(Inst::Intrinsic {
                dst: None,
                kind: IntrinKind::Dmb,
                args: vec![],
            });
            return Operand::Imm(0);
        }
        let p = self.expr(args[0]);
        if matches!(op, Release) {
            self.push(Inst::Intrinsic {
                dst: None,
                kind: IntrinKind::Stlr(t),
                args: vec![p, Operand::Imm(0)],
            });
            return Operand::Imm(0);
        }
        // `__sync_*_compare_and_swap(ptr, expected, desired)`: the SECOND
        // argument is the value compared against, the third the one stored.
        let a1 = self.expr(args[1]);
        let (cmpv, v) = if matches!(op, ValCas | BoolCas) {
            (Some(a1), self.expr(args[2]))
        } else {
            (None, a1)
        };
        let (head, body, join) = (self.f.new_block(), self.f.new_block(), self.f.new_block());
        // the loop carries nothing; the result leaves through `join`'s parameter
        let jp = self.f.new_value(t, Def::Param(join, 0));
        self.f.blocks[join as usize].params.push(jp);
        // a compare-and-swap also reports whether it fired
        let okp = self.f.new_value(Ty::I32, Def::Param(join, 1));
        self.f.blocks[join as usize].params.push(okp);
        self.goto(head);

        let old = self.def(t, |dst| Inst::Intrinsic {
            dst: Some(dst),
            kind: IntrinKind::LdAxr(t),
            args: vec![p],
        });
        match cmpv {
            // §B.2.9: a failed comparison abandons the monitor and reports 0
            Some(want) => {
                let eq = self.def(Ty::I32, |dst| Inst::Cmp {
                    dst,
                    op: CmpOp::Eq,
                    ty: t,
                    a: old,
                    b: want,
                });
                self.seal(Term::Br(
                    eq,
                    Target { block: body, args: vec![] },
                    Target {
                        block: join,
                        args: vec![old, Operand::Imm(0)],
                    },
                ));
            }
            None => self.seal(Term::Jmp(Target { block: body, args: vec![] })),
        }
        self.cur = body;
        let new = match op {
            FetchAdd | AddFetch => self.bin(BinOp::Add, t, old, v),
            FetchSub | SubFetch => self.bin(BinOp::Sub, t, old, v),
            FetchAnd => self.bin(BinOp::And, t, old, v),
            FetchOr => self.bin(BinOp::Or, t, old, v),
            FetchXor => self.bin(BinOp::Xor, t, old, v),
            TestSet | ValCas | BoolCas => v,
            Release | Barrier => unreachable!(),
        };
        let st = self.def(Ty::I32, |dst| Inst::Intrinsic {
            dst: Some(dst),
            kind: IntrinKind::StlXr(t),
            args: vec![p, new],
        });
        let result = match op {
            AddFetch | SubFetch => new,
            _ => old,
        };
        self.seal(Term::Br(
            st,
            Target { block: head, args: vec![] },
            Target {
                block: join,
                args: vec![result, Operand::Imm(1)],
            },
        ));
        self.cur = join;
        // `__sync_*` are FULL barriers, which acquire/release alone are not
        self.push(Inst::Intrinsic {
            dst: None,
            kind: IntrinKind::Dmb,
            args: vec![],
        });
        if matches!(op, BoolCas) {
            Operand::Val(okp)
        } else {
            Operand::Val(jp)
        }
    }

    // ── EXT(gcc) __builtin_{add,sub,mul}_overflow ──────────────────────────
    // ℤ semantics: compute at infinite precision, store the truncation, report
    // whether the exact value was representable. For a result narrower than 64
    // bits the 64-bit computation IS the infinite-precision one, so the test is
    // simply "does the truncation read back unchanged"; at 64 bits the carry /
    // sign rules of DDI 0487 C6.2 and the high half of the product take over.
    fn overflow(&mut self, op: u8, a: NodeId, b: NodeId, r: NodeId) -> Operand {
        let rt = self
            .tt()
            .pointee(self.ty(r))
            .expect("__builtin_*_overflow: third argument is a pointer");
        let n = self.tt().size(rt) * 8;
        let dst_uns = self.tt().is_unsigned(rt);
        let dt = hty(self.tt(), rt);
        // Widen each operand into the 64-bit domain under ITS OWN signedness.
        let mut wide = |b: &mut Self, e: NodeId| -> (Operand, u32, bool) {
            let t = b.ty(e);
            let ht = hty(b.tt(), t);
            let u = b.tt().is_unsigned(t);
            let v = b.expr(e);
            let v = if ht == Ty::I64 {
                v
            } else {
                b.conv(if u { CvtOp::Zext } else { CvtOp::Sext }, ht, Ty::I64, v)
            };
            (v, ht.bits(), u)
        };
        let (y, yb, yu) = wide(self, b);
        let (x, xb, xu) = wide(self, a);
        let dst = self.expr(r);
        let bop = match op {
            0 => BinOp::Add,
            1 => BinOp::Sub,
            _ => BinOp::Mul,
        };
        let s = self.bin(bop, Ty::I64, x, y);
        // Is the 64-bit computation ITSELF the infinite-precision one? For `+`
        // and `-` two 32-bit inputs can never leave the 64-bit range, and for
        // `*` the full product of two 32-bit inputs is exactly 64 bits wide.
        let exact = xb <= 32 && yb <= 32;
        // Otherwise the domain's own signedness decides how the 64-bit step
        // overflows (DDI 0487 C6.2's carry/overflow rules, and the high half of
        // the product for `*`).
        let dom_uns = (xb == 64 && xu) || (yb == 64 && yu);
        let mut flags: Vec<Operand> = Vec::new();
        if !exact {
            flags.push(match (op, dom_uns) {
                (0, true) => self.def(Ty::I32, |d| Inst::Cmp {
                    dst: d,
                    op: CmpOp::Ult,
                    ty: Ty::I64,
                    a: s,
                    b: x,
                }),
                (1, true) => self.def(Ty::I32, |d| Inst::Cmp {
                    dst: d,
                    op: CmpOp::Ult,
                    ty: Ty::I64,
                    a: x,
                    b: y,
                }),
                (_, true) => {
                    let hi = self.bin(BinOp::UMulHi, Ty::I64, x, y);
                    self.def(Ty::I32, |d| Inst::Cmp {
                        dst: d,
                        op: CmpOp::Ne,
                        ty: Ty::I64,
                        a: hi,
                        b: Operand::Imm(0),
                    })
                }
                (2, false) => {
                    let hi = self.bin(BinOp::SMulHi, Ty::I64, x, y);
                    let want = self.bin(BinOp::AShr, Ty::I64, s, Operand::Imm(63));
                    self.def(Ty::I32, |d| Inst::Cmp {
                        dst: d,
                        op: CmpOp::Ne,
                        ty: Ty::I64,
                        a: hi,
                        b: want,
                    })
                }
                // signed +/-: the classic sign-agreement test
                (o, false) => {
                    let m = if o == 0 {
                        let u = self.bin(BinOp::Xor, Ty::I64, x, s);
                        let v = self.bin(BinOp::Xor, Ty::I64, y, s);
                        self.bin(BinOp::And, Ty::I64, u, v)
                    } else {
                        let u = self.bin(BinOp::Xor, Ty::I64, x, y);
                        let v = self.bin(BinOp::Xor, Ty::I64, x, s);
                        self.bin(BinOp::And, Ty::I64, u, v)
                    };
                    self.def(Ty::I32, |d| Inst::Cmp {
                        dst: d,
                        op: CmpOp::Slt,
                        ty: Ty::I64,
                        a: m,
                        b: Operand::Imm(0),
                    })
                }
            });
        }
        // …and, independently, is the exact value representable in the
        // DESTINATION type? Below 64 bits that is the round trip; at 64 bits it
        // is only the sign, and only when the two domains disagree.
        if n < 64 {
            let narrow = self.conv(CvtOp::Trunc, Ty::I64, dt, s);
            let back = self.conv(
                if dst_uns { CvtOp::Zext } else { CvtOp::Sext },
                dt,
                Ty::I64,
                narrow,
            );
            flags.push(self.def(Ty::I32, |d| Inst::Cmp {
                dst: d,
                op: CmpOp::Ne,
                ty: Ty::I64,
                a: s,
                b: back,
            }));
        } else if dst_uns != dom_uns {
            // unsigned destination, signed value: negative is unrepresentable —
            // and the mirror case is the same bit test
            flags.push(self.def(Ty::I32, |d| Inst::Cmp {
                dst: d,
                op: CmpOp::Slt,
                ty: Ty::I64,
                a: s,
                b: Operand::Imm(0),
            }));
        }
        let narrow = if dt == Ty::I64 {
            s
        } else {
            self.conv(CvtOp::Trunc, Ty::I64, dt, s)
        };
        self.store(dt, dst, narrow, false);
        match flags.len() {
            0 => Operand::Imm(0),
            1 => flags[0],
            _ => {
                let mut acc = flags[0];
                for f in &flags[1..] {
                    acc = self.bin(BinOp::Or, Ty::I32, acc, *f);
                }
                acc
            }
        }
    }

    // ── EXT(gcc) inline asm ────────────────────────────────────────────────
    // The template is opaque; only its OPERANDS are the compiler's business.
    // Each is evaluated to a value (or an address, for `"m"`), and isel pins it
    // to a register from a reserved pool — so the whole feature costs one MIR
    // instruction and no allocator special case.
    fn asm(&mut self, tmpl: String, ops: &[ast::AsmOp]) {
        let mut vals = Vec::with_capacity(ops.len());
        let mut descs = Vec::with_capacity(ops.len());
        for o in ops {
            let t = self.ty(o.e);
            let ht = hty(self.tt(), t);
            if o.mem {
                let a = self.addr(o.e);
                vals.push(a);
            } else if o.out {
                let a = self.addr(o.e);
                vals.push(a);
                if o.rw {
                    let v = self.load(ht, a, false);
                    vals.push(v);
                }
            } else {
                let v = self.expr(o.e);
                vals.push(v);
            }
            descs.push(AsmOperand {
                out: o.out,
                rw: o.rw,
                mem: o.mem,
                fp: o.fp,
                tied: o.tied,
                pin: o.pin,
                ty: ht,
            });
        }
        self.push(Inst::Intrinsic {
            dst: None,
            kind: IntrinKind::Asm { tmpl, ops: descs },
            args: vals,
        });
    }

    // ── va_arg (AAPCS64 §B.6, the `va_list` walk) ──────────────────────────
    // `va_list` is the five-field struct the psABI defines (parser.rs seeds it):
    //   0 __stack · 8 __gr_top · 16 __vr_top · 24 __gr_offs · 28 __vr_offs
    // The two `*_offs` are NEGATIVE byte offsets from the corresponding `*_top`,
    // counting UP to zero as registers are consumed; zero or above means the
    // save area is exhausted and the argument is on the caller's stack. All of
    // that is a software convention over the layout `isel` builds, so the walk
    // is expressible in target-independent HIR — the only thing HIR does not
    // know is where the areas ARE, which is what `va_start` (an isel intrinsic)
    // records.
    fn va_arg(&mut self, ap: NodeId, ty: TypeId, tmp: u32) -> Operand {
        let apa = self.addr(ap);
        let pt = param_ty(self.tt(), ty);
        let size = self.tt().size(ty);
        // Over-alignment is ignored for argument passing (see `isel/abi.rs`): a
        // composite walks the stack at 8, a scalar at its own width. Honouring
        // an `aligned(32)` attribute here would round an ABSOLUTE address the
        // caller never rounded (torture pr92904).
        let align = if scalar(self.tt(), ty) {
            self.tt().align(ty).min(16)
        } else {
            8
        };
        // (save area is the VR one, bytes consumed there, HFA shape, indirect)
        let (vr, need, hfa, indirect) = match &pt {
            PTy::S(t) if t.is_float() => (true, 16, None, false),
            PTy::S(_) => (false, 8, None, false),
            PTy::LDouble => (true, 16, None, false),
            PTy::Agg { hfa: Some((dbl, n)), .. } => (true, 16 * *n, Some((*dbl, *n)), false),
            // §6.8.2 B.4: over 16 bytes the slot holds a POINTER to the object
            PTy::Agg { size, .. } if *size > 16 => (false, 8, None, true),
            PTy::Agg { size, .. } => (false, 8 * size.div_ceil(8).max(1), None, false),
        };
        let (offs_at, top_at) = if vr { (28i64, 16i64) } else { (24i64, 8i64) };
        let (regb, stkb, joinb) = (self.f.new_block(), self.f.new_block(), self.f.new_block());
        let jp = self.f.new_value(Ty::I64, Def::Param(joinb, 0));
        self.f.blocks[joinb as usize].params.push(jp);

        let oa = self.addk(apa, offs_at);
        let o = self.load(Ty::I32, oa, false);
        let newo = self.bin(BinOp::Add, Ty::I32, o, Operand::Imm(need as i64));
        let fits = self.def(Ty::I32, |dst| Inst::Cmp {
            dst,
            op: CmpOp::Sle,
            ty: Ty::I32,
            a: newo,
            b: Operand::Imm(0),
        });
        self.seal(Term::Br(
            fits,
            Target { block: regb, args: vec![] },
            Target { block: stkb, args: vec![] },
        ));

        // ── the register path ──────────────────────────────────────────────
        self.cur = regb;
        let oa = self.addk(apa, offs_at);
        self.store(Ty::I32, oa, newo, false);
        let ta = self.addk(apa, top_at);
        let top = self.load(Ty::I64, ta, false);
        let ox = self.conv(CvtOp::Sext, Ty::I32, Ty::I64, o);
        let base = self.bin(BinOp::Add, Ty::I64, top, ox);
        // §5.9.5: an HFA is SCATTERED one element per 16-byte register slot, so
        // the register path must gather it into the contiguous scratch object
        // the parser reserved.
        let rv = match hfa {
            Some((dbl, n)) if n > 1 || size != if dbl { 8 } else { 4 } => {
                let esz = if dbl { 8 } else { 4 };
                let et = if dbl { Ty::F64 } else { Ty::F32 };
                let d = {
                    let off = (self.frame - tmp) as i64;
                    self.def(Ty::I64, |dst| Inst::SlotAddr { dst, slot: 0, off })
                };
                for i in 0..n {
                    let src = self.addk(base, (16 * i) as i64);
                    let v = self.load(et, src, false);
                    let dst = self.addk(d, (esz * i) as i64);
                    self.store(et, dst, v, false);
                }
                d
            }
            _ => base,
        };
        self.seal(Term::Jmp(Target {
            block: joinb,
            args: vec![rv],
        }));

        // ── the stack path ─────────────────────────────────────────────────
        self.cur = stkb;
        // once one argument has overflowed, every later one is on the stack too
        let oa = self.addk(apa, offs_at);
        self.store(Ty::I32, oa, Operand::Imm(0), false);
        let st = self.load(Ty::I64, apa, false);
        let st = if align > 8 {
            // C.13: the NSAA is rounded to the type's natural alignment
            let a = self.bin(BinOp::Add, Ty::I64, st, Operand::Imm(align as i64 - 1));
            self.bin(BinOp::And, Ty::I64, a, Operand::Imm(!(align as i64 - 1)))
        } else {
            st
        };
        // C.16: the slot is at least 8 bytes, and composites round up to 8
        let bump = if indirect { 8 } else { (size.max(1)).div_ceil(8) * 8 };
        let next = self.bin(BinOp::Add, Ty::I64, st, Operand::Imm(bump as i64));
        self.store(Ty::I64, apa, next, false);
        self.seal(Term::Jmp(Target {
            block: joinb,
            args: vec![st],
        }));

        self.cur = joinb;
        let a = Operand::Val(jp);
        let a = if indirect {
            self.load(Ty::I64, a, false)
        } else {
            a
        };
        if self.is_ld(ty) {
            self.ld_load(a)
        } else if scalar(self.tt(), ty) {
            let t = hty(self.tt(), ty);
            self.load(t, a, false)
        } else {
            a
        }
    }

    // ── addresses ──────────────────────────────────────────────────────────
    fn addr(&mut self, n: NodeId) -> Operand {
        match self.node(n) {
            Node::Var(off) => {
                let off = (self.frame - *off) as i64;
                self.def(Ty::I64, |dst| Inst::SlotAddr { dst, slot: 0, off })
            }
            Node::GVar(i) => {
                // EXT(gcc) `__thread`: a thread-local object is not at a link-time
                // address at all — it is at an offset from the thread pointer.
                let sym = if self.ast.globals[*i as usize].is_tls {
                    Sym::Tls(*i)
                } else {
                    Sym::Global(*i)
                };
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
                if let Some(bf) = self.bf_of(nty) {
                    let vol = self.tt().is_volatile(nty);
                    return self.bf_load(a, bf, vol);
                }
                if self.is_ld(nty) {
                    return self.ld_load(a);
                }
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
                self.call(Callee::Direct(name), &args, nreg, nty, None)
            }
            Node::CallPtr(fp, args, nreg) => {
                let (fp, args, nreg) = (*fp, args.clone(), *nreg);
                let p = self.expr(fp);
                self.call(Callee::Indirect(p), &args, nreg, nty, None)
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
            // EXT(gcc) statement expression: every statement but the last runs
            // for effect, and the last one IS the value (parser.rs types the
            // node after it).
            Node::Block(items) => {
                let items = items.clone();
                match items.split_last() {
                    Some((&last, init)) => {
                        for &s in init {
                            self.stmt(s);
                        }
                        self.expr(last)
                    }
                    None => Operand::Imm(0),
                }
            }
            Node::Ret(_)
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
            // A call returning a composite: the parser has already reserved the
            // destination temporary, so lowering only has to name it. AAPCS64
            // §6.9 (registers vs the x8 indirection) is isel's job, not HIR's.
            Node::SRet(c, off, _) => {
                let (c, off) = (*c, *off);
                let o = (self.frame - off) as i64;
                let a = self.def(Ty::I64, |dst| Inst::SlotAddr { dst, slot: 0, off: o });
                let cty = self.ty(c);
                match self.node(c) {
                    Node::Call(name, args, nreg) => {
                        let (name, args, nreg) = (name.clone(), args.clone(), *nreg);
                        self.call(Callee::Direct(name), &args, nreg, cty, Some(a));
                    }
                    Node::CallPtr(fp, args, nreg) => {
                        let (fp, args, nreg) = (*fp, args.clone(), *nreg);
                        let p = self.expr(fp);
                        self.call(Callee::Indirect(p), &args, nreg, cty, Some(a));
                    }
                    _ => panic!("hir::build: SRet does not wrap a call"),
                }
                a
            }
            // C99 6.7.5.2 / EXT(gcc) alloca: `sub sp` by the rounded byte count;
            // the value is the new stack pointer. `Effect::Call` keeps it from
            // being moved, duplicated or removed by any pass.
            Node::Alloca(e) => {
                let e = *e;
                let n = self.expr(e);
                self.def(Ty::I64, |dst| Inst::Alloca {
                    dst,
                    size: n,
                    // AAPCS64 §6.2.2: sp stays 16-byte aligned.
                    align: 16,
                })
            }
            // EXT(gcc) `__va_area__`: the first UNNAMED stack argument. `va_off`
            // counts from the caller's frame link (x29+16 in the classic frame),
            // so the offset into the argument area proper is `va_off − 16`.
            Node::VaArea(off) => {
                let off = *off as i64 - 16;
                self.def(Ty::I64, |dst| Inst::Intrinsic {
                    dst: Some(dst),
                    kind: IntrinKind::VaArea,
                    args: vec![Operand::Imm(off)],
                })
            }
            Node::VaStart(ap) => {
                let ap = *ap;
                let a = self.addr(ap);
                self.push(Inst::Intrinsic {
                    dst: None,
                    kind: IntrinKind::VaStart,
                    args: vec![a],
                });
                Operand::Imm(0)
            }
            Node::VaArg(ap, ty, tmp) => {
                let (ap, ty, tmp) = (*ap, *ty, *tmp);
                self.va_arg(ap, ty, tmp)
            }
            Node::Sync(op, args, size) => {
                let (op, args, size) = (*op, args.clone(), *size);
                self.sync(op, &args, size)
            }
            Node::Overflow(op, a, b, r) => {
                let (op, a, b, r) = (*op, *a, *b, *r);
                self.overflow(op, a, b, r)
            }
            Node::Asm(tmpl, ops) => {
                let (tmpl, ops) = (tmpl.clone(), ops.clone());
                self.asm(tmpl, &ops);
                Operand::Imm(0)
            }
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
        let t = hty(self.tt(), lty);
        let vol = self.tt().is_volatile(lty);
        let a = self.addr(l);
        let v = self.expr(r);
        if let Some(bf) = self.bf_of(lty) {
            return self.bf_store(a, bf, v, vol);
        }
        if self.is_ld(lty) {
            self.ld_store(a, v);
            return v;
        }
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
        // C99 6.5p3 leaves operand order unspecified; the RIGHT operand is
        // evaluated first, matching the referee. `x[i] |= foo()` needs foo() to
        // run before x[i] is read, and choosing the other order turns every such
        // program into differential noise (torture pr58943).
        if let Some(cmp) = cmp_op(op, fp, uns) {
            let b = self.expr(r);
            let a = self.expr(l);
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
        let b = self.expr(r);
        let a = self.expr(l);
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
        let bf = self.bf_of(lty);
        let ld = self.is_ld(lty);
        let old = match bf {
            Some(bf) => self.bf_load(a, bf, vol),
            None if ld => self.ld_load(a),
            None => self.load(t, a, vol),
        };
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
        match bf {
            // the increment wraps inside the field (`int a:3`, 3+1 → −4)
            Some(bf) => {
                self.bf_store(a, bf, new, vol);
            }
            None if ld => self.ld_store(a, new),
            None => self.store(t, a, new, vol),
        }
        old
    }

    fn call(
        &mut self,
        callee: Callee,
        args: &[NodeId],
        nreg: u32,
        nty: TypeId,
        sret: Option<Operand>,
    ) -> Operand {
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
        if void || sret.is_some() {
            self.push(Inst::Call {
                dst: None,
                sig,
                callee,
                args: ops,
                sret,
            });
            sret.unwrap_or(Operand::Imm(0))
        } else {
            let t = hty(self.tt(), nty);
            self.def(t, |dst| Inst::Call {
                dst: Some(dst),
                sig,
                callee,
                args: ops,
                sret: None,
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
        let uns = self.tt().is_unsigned(cty);
        let v = self.expr(c);
        // C99 6.8.4.2p5: the controlling expression undergoes the integer
        // promotions, and each case constant is converted to the promoted type.
        // Without it a `switch (signed char)` compares the RAW BYTE against a
        // negative label and never matches (torture pr48809).
        let (v, t) = self.promote(v, hty(self.tt(), cty), uns);
        let xb = self.f.new_block();
        // One block per `case` label; `case lo...hi` (EXT gcc) expands to the
        // range's values, which is why the parser keeps (lo, hi) rather than one key.
        let mut arms: Vec<(i64, Target)> = Vec::new();
        let mut ranges: Vec<(i64, i64, BlockId)> = Vec::new();
        for &(lo, hi, id) in cases {
            let b = match self.cases.get(&id) {
                Some(&b) => b,
                None => {
                    let b = self.f.new_block();
                    self.cases.insert(id, b);
                    b
                }
            };
            if lo == hi {
                arms.push((
                    trunc(t, lo),
                    Target {
                        block: b,
                        args: vec![],
                    },
                ));
            } else {
                // EXT(gcc) `case lo ... hi`: a RANGE, tested as one unsigned
                // comparison `(v − lo) ≤ᵤ (hi − lo)` rather than enumerated.
                // Enumeration is not an optimization question — `case 1e18 ...
                // 1e19` has 9·10^18 values and simply cannot be listed
                // (torture pr34154).
                ranges.push((lo, hi, b));
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
        // C99 6.8.4.2p3 forbids two labels with the same value, so the ranges
        // and the single values are disjoint and may be tested in either order.
        for (lo, hi, b) in ranges {
            let next = self.f.new_block();
            let d = self.bin(BinOp::Sub, t, v, Operand::Imm(trunc(t, lo)));
            let span = trunc(t, hi.wrapping_sub(lo));
            let c = self.def(Ty::I32, |dst| Inst::Cmp {
                dst,
                op: CmpOp::Ule,
                ty: t,
                a: d,
                b: Operand::Imm(span),
            });
            self.seal(Term::Br(
                c,
                Target { block: b, args: vec![] },
                Target { block: next, args: vec![] },
            ));
            self.cur = next;
        }
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
        // Slot 0 is the parser's frame block. It always EXISTS — `Node::Var(off)`
        // names it unconditionally, and a frame of zero bytes is a real case
        // (EXT(gcc) empty struct: every local has size 0) — but a zero-size slot
        // occupies nothing, so a leaf still gets no `sub sp`.
        slots: vec![Slot {
            size: af.frame,
            // AAPCS64 §6.2.2: the frame block is 16-byte aligned.
            align: 16,
        }],
        entry: 0,
        is_static: af.is_static,
        is_weak: af.is_weak || af.is_inline,
        has_vla: af.has_vla,
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
        let o = (af.frame - off) as i64;
        if !scalar(&ast.tt, t) || matches!(ast.tt.tys[t as usize], ast::Ty::LDouble) {
            // AAPCS64 §6.8.2: a composite parameter is delivered either in
            // registers, on the stack, or (over 16 bytes) as a pointer to the
            // caller's copy. All three become one thing at this level — the
            // ADDRESS of the incoming object, which isel materializes — and the
            // parameter's own object is the frame slot the body reads.
            let v = b.f.new_value(Ty::I64, Def::FuncParam(k as u32));
            let a = b.def(Ty::I64, |dst| Inst::SlotAddr { dst, slot: 0, off: o });
            b.push(Inst::MemCpy {
                dst: a,
                src: Operand::Val(v),
                len: ast.tt.size64(t),
            });
            continue;
        }
        let ht = hty(&ast.tt, t);
        let v = b.f.new_value(ht, Def::FuncParam(k as u32));
        let a = b.def(Ty::I64, |dst| Inst::SlotAddr { dst, slot: 0, off: o });
        let vol = ast.tt.is_volatile(t);
        b.store(ht, a, Operand::Val(v), vol);
    }
    b.stmt(af.body);
    // C99 6.9.1p12: falling off the end of a non-void function is undefined, but
    // main returns 0. Either way the block needs a terminator.
    if matches!(b.f.blocks[b.cur as usize].term, Term::Unreachable) {
        let r = if af.name == "main" {
            Some(Operand::Imm(0))
        } else if matches!(b.f.sig.ret, Some(PTy::Agg { .. }) | Some(PTy::LDouble)) {
            // C99 6.9.1p12: falling off a composite-returning function is
            // undefined. Returning NO value leaves the destination untouched,
            // which is the least destructive realization of ⊥.
            None
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
    // EXT(gcc) computed goto: the edge set of `goto *e` is every LABELLED block
    // of the function, not only the labels whose address `&&l` was taken in an
    // expression — a static initializer (`static void *j[] = {&&x, &&y};`) takes
    // addresses the AST never shows as `LabelAddr`. Narrowing it there left the
    // targets unreachable, so they were pruned and their symbols never emitted.
    // The set is settled here, after every forward label has been created.
    let all: Vec<BlockId> = {
        let mut v: Vec<BlockId> = b.labels.values().copied().collect();
        v.sort_unstable();
        v
    };
    for blk in b.f.blocks.iter_mut() {
        if let Term::GotoPtr(_, set) = &mut blk.term {
            *set = all.clone();
        }
    }
    f = b.f;
    // The label map may have created blocks a `goto` never reached — they keep
    // their `Unreachable` terminator and `Cfg` never visits them.
    f
}
