// HIR → MIR instruction selection (REARCH.md §6, §12 R0.6).
//
// R0 is deliberately NAIVE: one HIR instruction becomes one canonical machine
// sequence, with no munching of operand trees. That is not a shortcut to be
// grown out of by patching — it is the base case of the pattern table. R3.1
// adds munch rows (addressing modes, cmp-branch fusion, madd/msub, bfx, extend
// folding), each as a row in `pattern.rs` with its own `⟦hir-tree⟧ = ⟦mir-seq⟧`
// battery entry, on top of a lowering that is already proven correct.
//
// The one non-obvious invariant, established in `hir::build`: HIR never performs
// arithmetic or comparison at I8/I16 (C99 6.3.1.1 promotes first). Narrow types
// appear only in `load`, `store` and `cvt`, which is exactly where A64 has a
// dedicated form — so no re-extension is ever needed around an ALU op.
use super::abi::{self, Loc};
use super::imm;
use crate::hir::{self, BinOp, CmpOp, CvtOp, Inst, Operand, Term, UnOp, ValueId};
use crate::mir::*;

pub fn lower(m: &hir::Module) -> MModule {
    MModule {
        funcs: m.funcs.iter().map(lower_func).collect(),
    }
}

fn wid(t: hir::Ty) -> Width {
    match t {
        hir::Ty::I8 | hir::Ty::I16 | hir::Ty::I32 => Width::W32,
        hir::Ty::I64 => Width::W64,
        hir::Ty::F32 => Width::S,
        hir::Ty::F64 => Width::D,
    }
}

/// The A64 access form for a load/store of this HIR type. A narrow LOAD is
/// zero-extending, which matches ⟦hir⟧: `Inst::Load` yields the raw bytes, and
/// any sign extension is a separate `Cvt` the C front end asked for.
fn memop(t: hir::Ty) -> MemOp {
    match t {
        hir::Ty::I8 => MemOp::B,
        hir::Ty::I16 => MemOp::H,
        hir::Ty::I32 => MemOp::W,
        hir::Ty::I64 => MemOp::X,
        hir::Ty::F32 => MemOp::S,
        hir::Ty::F64 => MemOp::D,
    }
}

fn cc_of(op: CmpOp) -> CC {
    match op {
        // integer
        CmpOp::Eq => CC::Eq,
        CmpOp::Ne => CC::Ne,
        CmpOp::Slt => CC::Lt,
        CmpOp::Sle => CC::Le,
        CmpOp::Sgt => CC::Gt,
        CmpOp::Sge => CC::Ge,
        CmpOp::Ult => CC::Lo,
        CmpOp::Ule => CC::Ls,
        CmpOp::Ugt => CC::Hi,
        CmpOp::Uge => CC::Hs,
        // floating (DDI 0487 C6.2.65: unordered sets C and V, clears N and Z).
        // Each ORDERED predicate has a single condition that is false when
        // unordered; C's `!=` is the UNORDERED one and is plain `ne`.
        CmpOp::FOeq => CC::Eq,
        CmpOp::FOlt => CC::Mi,
        CmpOp::FOle => CC::Ls,
        CmpOp::FOgt => CC::Gt,
        CmpOp::FOge => CC::Ge,
        CmpOp::FUne => CC::Ne,
        CmpOp::FUno => CC::Vs,
        // ordered-not-equal is the one predicate with no single A64 condition
        // (it is `mi || gt`); C never produces it — `!=` is FUne.
        CmpOp::FOne => unreachable!("ordered != has no single condition code"),
    }
}

struct L<'a> {
    h: &'a hir::Func,
    f: MFunc,
    /// HIR value → the virtual register holding it
    vmap: Vec<Reg>,
    cur: MBlockId,
}

impl<'a> L<'a> {
    fn push(&mut self, i: MInst) {
        self.f.blocks[self.cur as usize].insts.push(i);
    }
    fn tmp(&mut self, w: Width) -> Reg {
        self.f.new_vreg(w)
    }

    /// The register holding an HIR operand, materializing a constant if needed.
    fn reg(&mut self, o: Operand, t: hir::Ty) -> Reg {
        let w = wid(t);
        match o {
            Operand::Val(v) => self.vmap[v as usize],
            // xzr/wzr IS the constant zero: no instruction, no live range.
            Operand::Imm(0) => Reg::P(isa::ZR),
            Operand::Imm(k) => {
                let d = self.tmp(w);
                let i = imm::materialize(d, k, w);
                self.push(i);
                d
            }
            Operand::Fimm(bits) => {
                let d = self.tmp(w);
                if bits == 0 {
                    // `fmov d, xzr` — the zero bit pattern, no literal needed
                    self.push(MInst::FMov {
                        dw: w,
                        sw: if w == Width::S { Width::W32 } else { Width::W64 },
                        dst: d,
                        src: Reg::P(isa::ZR),
                    });
                } else {
                    let gw = if w == Width::S { Width::W32 } else { Width::W64 };
                    let g = self.tmp(gw);
                    let i = imm::materialize(g, bits as i64, gw);
                    self.push(i);
                    self.push(MInst::FMov {
                        dw: w,
                        sw: gw,
                        dst: d,
                        src: g,
                    });
                }
                d
            }
        }
    }

    /// An operand that may ride in an immediate field of `op`.
    fn rhs(&mut self, o: Operand, t: hir::Ty, op: AluOp) -> Rhs {
        if let Operand::Imm(k) = o {
            if let Some(r) = imm::as_rhs(op, k, wid(t)) {
                return r;
            }
        }
        Rhs::Reg(self.reg(o, t))
    }

    fn dst_of(&self, v: ValueId) -> Reg {
        self.vmap[v as usize]
    }

    fn inst(&mut self, i: &Inst) {
        match i {
            Inst::Bin { dst, op, ty, a, b } => self.binop(*dst, *op, *ty, *a, *b),
            Inst::Un { dst, op, ty, a } => {
                let d = self.dst_of(*dst);
                let w = wid(*ty);
                match op {
                    // C99 6.5.3.3: -x is 0 - x, and xzr costs nothing
                    UnOp::Neg => {
                        let x = self.reg(*a, *ty);
                        self.push(MInst::Alu {
                            op: AluOp::Sub,
                            w,
                            dst: d,
                            a: Reg::P(isa::ZR),
                            b: Rhs::Reg(x),
                            flags: None,
                        });
                    }
                    UnOp::Not => {
                        let x = self.reg(*a, *ty);
                        self.push(MInst::Alu {
                            op: AluOp::Orn,
                            w,
                            dst: d,
                            a: Reg::P(isa::ZR),
                            b: Rhs::Reg(x),
                            flags: None,
                        });
                    }
                    UnOp::FNeg => {
                        let x = self.reg(*a, *ty);
                        self.push(MInst::FpUn {
                            op: FpUnOp::Fneg,
                            w,
                            dst: d,
                            src: x,
                            sw: w,
                        });
                    }
                }
            }
            Inst::Cmp { dst, op, ty, a, b } => {
                let d = self.dst_of(*dst);
                let fl = self.compare(*op, *ty, *a, *b);
                self.push(MInst::CSet {
                    w: Width::W32,
                    dst: d,
                    cc: cc_of(*op),
                    flags: fl,
                });
            }
            Inst::Cvt {
                dst,
                op,
                from,
                to,
                a,
            } => self.convert(*dst, *op, *from, *to, *a),
            Inst::Load {
                dst,
                ty,
                addr,
                vol,
                ..
            } => {
                let base = self.reg(*addr, hir::Ty::I64);
                let d = self.dst_of(*dst);
                self.push(MInst::Load {
                    op: memop(*ty),
                    dst: d,
                    mem: AddrMode::BaseImm { base, off: 0 },
                    vol: *vol,
                });
            }
            Inst::Store {
                ty,
                addr,
                val,
                vol,
                ..
            } => {
                let base = self.reg(*addr, hir::Ty::I64);
                let v = self.reg(*val, *ty);
                self.push(MInst::Store {
                    op: memop(*ty),
                    src: v,
                    mem: AddrMode::BaseImm { base, off: 0 },
                    vol: *vol,
                });
            }
            Inst::SlotAddr { dst, slot, off } => {
                let d = self.dst_of(*dst);
                self.push(MInst::SlotAddr {
                    dst: d,
                    slot: *slot,
                    off: *off as i32,
                });
            }
            Inst::SymAddr { dst, sym } => {
                let d = self.dst_of(*dst);
                let page = self.tmp(Width::W64);
                self.push(MInst::Adrp {
                    dst: page,
                    sym: sym.clone(),
                    got: false,
                });
                self.push(MInst::AddLo12 {
                    dst: d,
                    base: page,
                    sym: sym.clone(),
                    got: false,
                });
            }
            Inst::Select { dst, ty, c, a, b } => {
                let d = self.dst_of(*dst);
                let fl = self.test(*c);
                let (x, y) = (self.reg(*a, *ty), self.reg(*b, *ty));
                self.push(MInst::CSel {
                    op: CSelOp::Csel,
                    w: wid(*ty),
                    dst: d,
                    a: x,
                    b: y,
                    cc: CC::Ne,
                    flags: fl,
                });
            }
            Inst::Call {
                dst,
                sig,
                callee,
                args,
            } => self.call(*dst, sig, callee, args),
            Inst::MemCpy { dst, src, len } => {
                let (d, s) = (self.reg(*dst, hir::Ty::I64), self.reg(*src, hir::Ty::I64));
                let n = self.reg(Operand::Imm(*len as i64), hir::Ty::I64);
                self.libcall("memcpy", &[d, s, n]);
            }
            Inst::MemSet { dst, byte, len } => {
                let d = self.reg(*dst, hir::Ty::I64);
                let b = self.reg(*byte, hir::Ty::I32);
                let n = self.reg(Operand::Imm(*len as i64), hir::Ty::I64);
                self.libcall("memset", &[d, b, n]);
            }
            Inst::Alloca { .. } => todo!("R1.1: VLA / alloca"),
            Inst::Intrinsic { .. } => todo!("R1.3: EXT intrinsics"),
        }
    }

    /// `cmp`/`fcmp`, returning the flag value it defines.
    fn compare(&mut self, op: CmpOp, ty: hir::Ty, a: Operand, b: Operand) -> Reg {
        let fl = self.f.new_flags();
        if ty.is_float() {
            let x = self.reg(a, ty);
            let zero = matches!(b, Operand::Fimm(0));
            let y = if zero { x } else { self.reg(b, ty) };
            self.push(MInst::FpCmp {
                w: wid(ty),
                a: x,
                b: y,
                zero,
                flags: fl,
            });
        } else {
            let _ = op;
            let x = self.reg(a, ty);
            let y = self.rhs(b, ty, AluOp::Sub);
            self.push(MInst::Cmp {
                kind: CmpKind::Cmp,
                w: wid(ty),
                a: x,
                b: y,
                flags: fl,
            });
        }
        fl
    }

    /// `cmp c, #0` for a value used as a truth value.
    fn test(&mut self, c: Operand) -> Reg {
        let fl = self.f.new_flags();
        let x = self.reg(c, hir::Ty::I32);
        self.push(MInst::Cmp {
            kind: CmpKind::Cmp,
            w: Width::W32,
            a: x,
            b: Rhs::Imm(0),
            flags: fl,
        });
        fl
    }

    fn binop(&mut self, dst: ValueId, op: BinOp, ty: hir::Ty, a: Operand, b: Operand) {
        let d = self.dst_of(dst);
        let w = wid(ty);
        if let Some(fop) = match op {
            BinOp::FAdd => Some(FpOp::Fadd),
            BinOp::FSub => Some(FpOp::Fsub),
            BinOp::FMul => Some(FpOp::Fmul),
            BinOp::FDiv => Some(FpOp::Fdiv),
            _ => None,
        } {
            let (x, y) = (self.reg(a, ty), self.reg(b, ty));
            self.push(MInst::FpAlu {
                op: fop,
                w,
                dst: d,
                a: x,
                b: y,
            });
            return;
        }
        // C99 6.5.5: a % b is a - (a / b) * b, which A64 spells sdiv + msub —
        // there is no remainder instruction.
        if matches!(op, BinOp::SRem | BinOp::URem) {
            let (x, y) = (self.reg(a, ty), self.reg(b, ty));
            let q = self.tmp(w);
            self.push(MInst::Alu {
                op: if op == BinOp::SRem {
                    AluOp::SDiv
                } else {
                    AluOp::UDiv
                },
                w,
                dst: q,
                a: x,
                b: Rhs::Reg(y),
                flags: None,
            });
            self.push(MInst::Alu3 {
                op: Alu3Op::Msub,
                w,
                dst: d,
                a: q,
                b: y,
                c: x,
            });
            return;
        }
        let aop = match op {
            BinOp::Add => AluOp::Add,
            BinOp::Sub => AluOp::Sub,
            BinOp::Mul => AluOp::Mul,
            BinOp::SDiv => AluOp::SDiv,
            BinOp::UDiv => AluOp::UDiv,
            BinOp::And => AluOp::And,
            BinOp::Or => AluOp::Orr,
            BinOp::Xor => AluOp::Eor,
            BinOp::Shl => AluOp::Lsl,
            BinOp::LShr => AluOp::Lsr,
            BinOp::AShr => AluOp::Asr,
            _ => unreachable!(),
        };
        let x = self.reg(a, ty);
        let y = self.rhs(b, ty, aop);
        self.push(MInst::Alu {
            op: aop,
            w,
            dst: d,
            a: x,
            b: y,
            flags: None,
        });
    }

    fn convert(&mut self, dst: ValueId, op: CvtOp, from: hir::Ty, to: hir::Ty, a: Operand) {
        let d = self.dst_of(dst);
        let (fw, tw) = (wid(from), wid(to));
        let x = self.reg(a, from);
        match op {
            CvtOp::Sext | CvtOp::Zext => {
                let e = match (from, op) {
                    (hir::Ty::I8, CvtOp::Sext) => Some(ExtOp::Sxtb),
                    (hir::Ty::I16, CvtOp::Sext) => Some(ExtOp::Sxth),
                    (hir::Ty::I32, CvtOp::Sext) => Some(ExtOp::Sxtw),
                    (hir::Ty::I8, _) => Some(ExtOp::Uxtb),
                    (hir::Ty::I16, _) => Some(ExtOp::Uxth),
                    // zero-extending w→x is free: a `w`-form move already clears
                    // the upper half (DDI 0487 B1.2.1)
                    (hir::Ty::I32, _) => None,
                    _ => unreachable!(),
                };
                match e {
                    Some(op) => self.push(MInst::Ext {
                        op,
                        w: tw,
                        dst: d,
                        src: x,
                    }),
                    None => self.push(MInst::Copy {
                        w: Width::W32,
                        dst: d,
                        src: x,
                    }),
                }
            }
            // Narrowing keeps the low bits where they are; every consumer of a
            // narrow value reads only those bits (`strb`, `uxtb`, a promoted
            // compare), so no instruction is required beyond the width change.
            CvtOp::Trunc => self.push(MInst::Copy {
                w: tw,
                dst: d,
                src: x,
            }),
            CvtOp::SiToFp | CvtOp::UiToFp => {
                // scvtf/ucvtf read a whole w or x register, so a narrow source
                // must be extended first — its upper bits are not defined by C.
                let signed = op == CvtOp::SiToFp;
                let src = match from {
                    hir::Ty::I8 | hir::Ty::I16 => {
                        let e = match (from, signed) {
                            (hir::Ty::I8, true) => ExtOp::Sxtb,
                            (hir::Ty::I8, false) => ExtOp::Uxtb,
                            (_, true) => ExtOp::Sxth,
                            (_, false) => ExtOp::Uxth,
                        };
                        let t = self.tmp(Width::W32);
                        self.push(MInst::Ext {
                            op: e,
                            w: Width::W32,
                            dst: t,
                            src: x,
                        });
                        t
                    }
                    _ => x,
                };
                self.push(MInst::FpCvt {
                    op: if signed { CvtOp2::Scvtf } else { CvtOp2::Ucvtf },
                    dw: tw,
                    sw: if fw == Width::W64 {
                        Width::W64
                    } else {
                        Width::W32
                    },
                    dst: d,
                    src,
                });
            }
            CvtOp::FpToSi | CvtOp::FpToUi => self.push(MInst::FpCvt {
                op: if op == CvtOp::FpToSi {
                    CvtOp2::Fcvtzs
                } else {
                    CvtOp2::Fcvtzu
                },
                dw: if tw == Width::W64 {
                    Width::W64
                } else {
                    Width::W32
                },
                sw: fw,
                dst: d,
                src: x,
            }),
            CvtOp::FpExt | CvtOp::FpTrunc => self.push(MInst::FpUn {
                op: FpUnOp::Fcvt,
                w: tw,
                dst: d,
                src: x,
                sw: fw,
            }),
            CvtOp::Bitcast => self.push(MInst::FMov {
                dw: tw,
                sw: fw,
                dst: d,
                src: x,
            }),
        }
    }

    fn libcall(&mut self, name: &str, args: &[Reg]) {
        let pairs: Vec<(Reg, Reg, Width)> = args
            .iter()
            .enumerate()
            .map(|(i, r)| (Reg::P(PReg::gpr(i as u8)), *r, Width::W64))
            .collect();
        self.push(MInst::ParallelCopy(pairs));
        let uses = (0..args.len())
            .map(|i| (Reg::P(PReg::gpr(i as u8)), PReg::gpr(i as u8)))
            .collect();
        self.push(MInst::Call {
            callee: CallTarget::Direct(name.to_string()),
            uses,
            defs: vec![],
            clobbers: isa::caller_saved(),
            stack_bytes: 0,
            tail: false,
        });
    }

    fn call(
        &mut self,
        dst: Option<ValueId>,
        sig: &hir::Sig,
        callee: &hir::Callee,
        args: &[Operand],
    ) {
        let asn = abi::classify(sig);
        // Hack 2007 §4: a constrained instruction is preceded by ONE parallel
        // copy that puts every operand where the ABI wants it. The allocator
        // then sees no fixed constraint at all — the argument registers are
        // ordinary physical registers with very short live ranges, and a cycle
        // among them (f(b, a) where a is in x1 and b in x0) is resolved by the
        // same windmill sequentialization every block edge uses.
        let mut pairs = Vec::with_capacity(args.len());
        for ((o, p), loc) in args.iter().zip(&sig.params).zip(&asn.args) {
            let t = match p {
                hir::PTy::S(t) => *t,
                _ => todo!("R1.2: composite argument"),
            };
            let r = self.reg(*o, t);
            match loc {
                Loc::Reg(p, w) => pairs.push((Reg::P(*p), r, *w)),
                Loc::Stack(..) => todo!("R1.2: stack arguments"),
            }
        }
        let target = match callee {
            hir::Callee::Direct(n) => CallTarget::Direct(n.clone()),
            hir::Callee::Indirect(o) => CallTarget::Indirect(self.reg(*o, hir::Ty::I64)),
        };
        if !pairs.is_empty() {
            self.push(MInst::ParallelCopy(pairs));
        }
        let uses = asn
            .args
            .iter()
            .filter_map(|l| match l {
                Loc::Reg(p, _) => Some((Reg::P(*p), *p)),
                Loc::Stack(..) => None,
            })
            .collect();
        let defs = match asn.ret {
            Some(Loc::Reg(p, _)) if dst.is_some() => vec![(Reg::P(p), p)],
            _ => vec![],
        };
        self.push(MInst::Call {
            callee: target,
            uses,
            defs,
            clobbers: isa::caller_saved(),
            stack_bytes: asn.stack_bytes,
            tail: false,
        });
        // and one copy out of the result register
        if let (Some(v), Some(Loc::Reg(p, w))) = (dst, asn.ret) {
            let d = self.dst_of(v);
            if p.class == Class::Fpr {
                self.push(MInst::FMov {
                    dw: w,
                    sw: w,
                    dst: d,
                    src: Reg::P(p),
                });
            } else {
                self.push(MInst::Copy { w, dst: d, src: Reg::P(p) });
            }
        }
    }

    fn target(&mut self, t: &hir::Target) -> MTarget {
        // An edge argument takes the type of the block PARAMETER it feeds, so a
        // constant is materialized at the parameter's width.
        let ptys: Vec<hir::Ty> = self.h.blocks[t.block as usize]
            .params
            .iter()
            .map(|p| self.h.ty_of(*p))
            .collect();
        let args = t
            .args
            .iter()
            .zip(&ptys)
            .map(|(a, ty)| self.reg(*a, *ty))
            .collect();
        MTarget {
            block: t.block,
            args,
        }
    }

    fn terminator(&mut self, t: &Term) {
        let term = match t {
            Term::Jmp(x) => {
                let x = self.target(x);
                MTerm::B(x)
            }
            // The condition is an I32 that C says is "nonzero = true": `cbnz`
            // tests it directly, with no compare instruction at all.
            Term::Br(c, x, y) => {
                let r = self.reg(*c, hir::Ty::I32);
                let (x, y) = (self.target(x), self.target(y));
                MTerm::Cbz {
                    w: Width::W32,
                    reg: r,
                    zero: false,
                    t: x,
                    f: y,
                }
            }
            Term::Ret(v) => {
                if let Some(o) = v {
                    let ty = match self.h.sig.ret.as_ref() {
                        Some(hir::PTy::S(t)) => *t,
                        Some(_) => todo!("R1.2: composite return"),
                        None => hir::Ty::I64,
                    };
                    let r = self.reg(*o, ty);
                    if ty.is_float() {
                        self.push(MInst::FMov {
                            dw: wid(ty),
                            sw: wid(ty),
                            dst: Reg::P(PReg::fpr(0)),
                            src: r,
                        });
                    } else {
                        self.push(MInst::Copy {
                            w: wid(ty),
                            dst: Reg::P(PReg::gpr(0)),
                            src: r,
                        });
                    }
                }
                MTerm::Ret
            }
            Term::Unreachable => MTerm::Unreachable,
            Term::GotoPtr(o, bs) => {
                let r = self.reg(*o, hir::Ty::I64);
                MTerm::BrReg(r, bs.clone())
            }
            // R0 lowers a switch to a compare chain; R3.3 adds the jump table
            // (the density threshold is a dated policy constant there).
            Term::Switch(c, ty, arms, d) => {
                let x = self.reg(*c, *ty);
                let dflt = self.target(d);
                let mut next = dflt;
                let arms: Vec<(i64, MTarget)> = arms
                    .iter()
                    .map(|(k, t)| (*k, self.target(t)))
                    .collect();
                // build the chain backwards so each test falls through to the next
                for (k, t) in arms.into_iter().rev() {
                    let test = self.f.new_block();
                    let fl = self.f.new_flags();
                    let w = wid(*ty);
                    let rhs = match imm::as_rhs(AluOp::Sub, k, w) {
                        Some(r) => r,
                        None => {
                            let g = self.tmp(w);
                            let seq = imm::materialize(g, k, w);
                            self.f.blocks[test as usize].insts.push(seq);
                            Rhs::Reg(g)
                        }
                    };
                    self.f.blocks[test as usize].insts.push(MInst::Cmp {
                        kind: CmpKind::Cmp,
                        w,
                        a: x,
                        b: rhs,
                        flags: fl,
                    });
                    self.f.blocks[test as usize].term = MTerm::Bcc(CC::Eq, fl, t, next);
                    next = MTarget {
                        block: test,
                        args: vec![],
                    };
                }
                MTerm::B(next)
            }
        };
        self.f.blocks[self.cur as usize].term = term;
    }
}

/// `mir::CvtOp` (integer↔float) under a distinct name: `hir::CvtOp` is a
/// different, larger set and both are in scope here.
use crate::mir::CvtOp as CvtOp2;

fn lower_func(h: &hir::Func) -> MFunc {
    let mut f = MFunc {
        name: h.name.clone(),
        blocks: Vec::new(),
        vregs: Vec::new(),
        slots: Vec::new(),
        entry: h.entry,
        is_static: h.is_static,
        is_weak: h.is_weak,
        frame_size: 0,
        saved: RegSet::default(),
        physical: false,
    };
    for s in &h.slots {
        f.new_slot(s.size, s.align, SlotKind::Local);
    }
    for _ in 0..h.blocks.len() {
        f.new_block();
    }
    // one virtual register per HIR value: HIR is already SSA, so this mapping is
    // a bijection and needs no renaming
    let mut vmap = Vec::with_capacity(h.values.len());
    for v in &h.values {
        vmap.push(f.new_vreg(wid(v.ty)));
    }
    for (bi, b) in h.blocks.iter().enumerate() {
        f.blocks[bi].weight = b.weight;
        f.blocks[bi].params = b.params.iter().map(|p| vmap[*p as usize]).collect();
    }
    let mut l = L {
        h,
        f,
        vmap,
        cur: h.entry,
    };
    // AAPCS64 entry: each parameter arrives in the register the ABI names, and
    // the entry block copies it into its virtual register. The copies are what
    // the allocator later coalesces away when the value happens to stay put.
    let asn = abi::classify(&h.sig);
    // The incoming arguments arrive together, so they leave together: ONE
    // parallel copy, for the same reason as at a call site.
    let mut pairs = Vec::new();
    for (i, vi) in h.values.iter().enumerate() {
        if let hir::Def::FuncParam(k) = vi.def {
            let d = l.vmap[i];
            match asn.args.get(k as usize) {
                Some(Loc::Reg(p, w)) => pairs.push((d, Reg::P(*p), *w)),
                Some(Loc::Stack(..)) => todo!("R1.2: incoming stack arguments"),
                None => {}
            }
        }
    }
    let mut prologue: Vec<MInst> = if pairs.is_empty() {
        Vec::new()
    } else {
        vec![MInst::ParallelCopy(pairs)]
    };
    for (bi, b) in h.blocks.iter().enumerate() {
        l.cur = bi as MBlockId;
        if bi == h.entry as usize {
            let p = std::mem::take(&mut prologue);
            l.f.blocks[bi].insts.extend(p);
        }
        for i in &b.insts {
            l.inst(i);
        }
        l.terminator(&b.term);
    }
    l.f
}
