// The HIR well-formedness checker (REARCH.md §3.4). This is Law 3 applied at the
// THEORY A6 — HIR's well-formedness; THEORY A8 — proof at the earliest layer
// cheapest possible layer: every property below is decidable on the IR alone, so
// a violation is caught here rather than as a mysterious wrong answer in csmith.
//
// Checked: (1) single assignment, (2) every use dominated by its definition,
// (3) block-argument arity and type against EVERY incoming edge, (4) operand
// types against the opcode, (5) exactly one terminator with in-range targets,
// (6) the entry block takes no block parameters.
use super::*;

pub fn verify(f: &Func) -> Result<(), String> {
    let cfg = dom::cfg(f);
    let dt = dom::domtree(f, &cfg);
    let nv = f.values.len();
    let err = |m: String| Err(format!("hir::verify {}: {}", f.name, m));

    // (1) single assignment + the def index agrees with where it actually sits
    let mut defined = vec![false; nv];
    fn mark(
        f: &Func,
        defined: &mut [bool],
        v: ValueId,
        at: Def,
    ) -> Result<(), String> {
        let i = v as usize;
        if i >= defined.len() {
            return Err(format!("value %{} out of range", v));
        }
        if defined[i] {
            return Err(format!("%{} defined twice", v));
        }
        defined[i] = true;
        if f.values[i].def != at {
            return Err(format!(
                "%{} def record {:?} ≠ actual {:?}",
                v, f.values[i].def, at
            ));
        }
        Ok(())
    }
    // Function parameters are not defined by any instruction: `build` records
    // them as FuncParam and the ABI materializes them in the entry block.
    for (i, vi) in f.values.iter().enumerate() {
        if let Def::FuncParam(_) = vi.def {
            defined[i] = true;
        }
    }
    for (bi, blk) in f.blocks.iter().enumerate() {
        let b = bi as BlockId;
        if !cfg.reachable(b) {
            continue;
        }
        for (k, &p) in blk.params.iter().enumerate() {
            if let Err(e) = mark(f, &mut defined, p, Def::Param(b, k as u32)) {
                return err(e);
            }
        }
        for (i, inst) in blk.insts.iter().enumerate() {
            if let Some(d) = inst.dst() {
                if let Err(e) = mark(f, &mut defined, d, Def::Inst(b, i as u32)) {
                    return err(e);
                }
            }
        }
    }

    // (6) entry takes its values from the ABI, not from an edge
    if !f.blocks[f.entry as usize].params.is_empty() {
        return err("entry block has block parameters".into());
    }

    // (2) dominance, (3) edge arity/type, (4)(5) shape
    let def_site = |v: ValueId| -> Def { f.values[v as usize].def };
    let check_use = |v: ValueId, at_block: BlockId, at_inst: u32| -> Result<(), String> {
        if v as usize >= nv {
            return Err(format!("use of out-of-range %{}", v));
        }
        if !defined[v as usize] {
            return Err(format!("use of undefined %{}", v));
        }
        match def_site(v) {
            Def::FuncParam(_) => Ok(()),
            Def::Param(db, _) => {
                if dt.dominates(db, at_block) {
                    Ok(())
                } else {
                    Err(format!("%{} used in bb{} but defined in bb{}", v, at_block, db))
                }
            }
            Def::Inst(db, di) => {
                if db == at_block {
                    if di < at_inst {
                        Ok(())
                    } else {
                        Err(format!("%{} used before its definition in bb{}", v, db))
                    }
                } else if dt.dominates(db, at_block) {
                    Ok(())
                } else {
                    Err(format!("%{} used in bb{} not dominated by bb{}", v, at_block, db))
                }
            }
        }
    };

    for (bi, blk) in f.blocks.iter().enumerate() {
        let b = bi as BlockId;
        if !cfg.reachable(b) {
            continue;
        }
        for (i, inst) in blk.insts.iter().enumerate() {
            let mut bad = None;
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    if bad.is_none() {
                        if let Err(e) = check_use(v, b, i as u32) {
                            bad = Some(e);
                        }
                    }
                }
            });
            if let Some(e) = bad {
                return err(e);
            }
            if let Err(e) = check_types(f, inst) {
                return err(format!("bb{}[{}]: {} — {:?}", b, i, e, inst));
            }
        }
        // the terminator uses values at the very end of the block
        let n = blk.insts.len() as u32;
        let mut bad = None;
        blk.term.uses(|o| {
            if let Operand::Val(v) = o {
                if bad.is_none() {
                    if let Err(e) = check_use(v, b, n) {
                        bad = Some(e);
                    }
                }
            }
        });
        if let Some(e) = bad {
            return err(e);
        }
        for t in blk.term.targets() {
            if t.block as usize >= f.blocks.len() {
                return err(format!("bb{} jumps to out-of-range bb{}", b, t.block));
            }
            let want = &f.blocks[t.block as usize].params;
            if want.len() != t.args.len() {
                return err(format!(
                    "edge bb{}→bb{}: {} args for {} parameters",
                    b,
                    t.block,
                    t.args.len(),
                    want.len()
                ));
            }
            for (a, &p) in t.args.iter().zip(want) {
                if let Operand::Val(v) = a {
                    if f.ty_of(*v) != f.ty_of(p) {
                        return err(format!(
                            "edge bb{}→bb{}: %{}:{:?} passed to %{}:{:?}",
                            b,
                            t.block,
                            v,
                            f.ty_of(*v),
                            p,
                            f.ty_of(p)
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Operand/result types against the opcode. `Operand::Imm`/`Fimm` take the
/// instruction's type by definition, so only `Val` operands can disagree.
fn check_types(f: &Func, inst: &Inst) -> Result<(), String> {
    let t = |o: &Operand| -> Option<Ty> {
        match o {
            Operand::Val(v) => Some(f.ty_of(*v)),
            _ => None,
        }
    };
    let want = |o: &Operand, ty: Ty, what: &str| -> Result<(), String> {
        match t(o) {
            Some(g) if g != ty => Err(format!("{} is {:?}, expected {:?}", what, g, ty)),
            _ => Ok(()),
        }
    };
    match inst {
        Inst::Bin { dst, op, ty, a, b } => {
            let fp = matches!(
                op,
                BinOp::FAdd | BinOp::FSub | BinOp::FMul | BinOp::FDiv
            );
            if fp != ty.is_float() {
                return Err(format!("{:?} on {:?}", op, ty));
            }
            want(a, *ty, "lhs")?;
            want(b, *ty, "rhs")?;
            if f.ty_of(*dst) != *ty {
                return Err("result type ≠ operand type".into());
            }
        }
        Inst::Un { dst, op, ty, a } => {
            if (*op == UnOp::FNeg) != ty.is_float() {
                return Err(format!("{:?} on {:?}", op, ty));
            }
            want(a, *ty, "operand")?;
            if f.ty_of(*dst) != *ty {
                return Err("result type ≠ operand type".into());
            }
        }
        Inst::Cmp { dst, op, ty, a, b } => {
            if op.is_float() != ty.is_float() {
                return Err(format!("{:?} on {:?}", op, ty));
            }
            want(a, *ty, "lhs")?;
            want(b, *ty, "rhs")?;
            if f.ty_of(*dst) != Ty::I32 {
                return Err("compare result is not I32".into());
            }
        }
        Inst::Cvt {
            dst,
            op,
            from,
            to,
            a,
        } => {
            want(a, *from, "operand")?;
            if f.ty_of(*dst) != *to {
                return Err("result type ≠ target type".into());
            }
            let ok = match op {
                CvtOp::Sext | CvtOp::Zext => {
                    !from.is_float() && !to.is_float() && from.bits() < to.bits()
                }
                CvtOp::Trunc => !from.is_float() && !to.is_float() && from.bits() > to.bits(),
                CvtOp::FpToSi | CvtOp::FpToUi => from.is_float() && !to.is_float(),
                CvtOp::SiToFp | CvtOp::UiToFp => !from.is_float() && to.is_float(),
                CvtOp::FpExt => from.is_float() && to.is_float() && from.bits() < to.bits(),
                CvtOp::FpTrunc => from.is_float() && to.is_float() && from.bits() > to.bits(),
                CvtOp::Bitcast => from.bits() == to.bits(),
            };
            if !ok {
                return Err(format!("{:?} {:?}→{:?}", op, from, to));
            }
        }
        Inst::Load { dst, ty, addr, .. } => {
            want(addr, Ty::I64, "address")?;
            if f.ty_of(*dst) != *ty {
                return Err("load result ≠ load type".into());
            }
        }
        Inst::Store { ty, addr, val, .. } => {
            want(addr, Ty::I64, "address")?;
            want(val, *ty, "stored value")?;
        }
        Inst::SlotAddr { dst, slot, .. } => {
            if *slot as usize >= f.slots.len() {
                return Err(format!("slot {} out of range", slot));
            }
            if f.ty_of(*dst) != Ty::I64 {
                return Err("slot address is not I64".into());
            }
        }
        Inst::SymAddr { dst, .. } => {
            if f.ty_of(*dst) != Ty::I64 {
                return Err("symbol address is not I64".into());
            }
        }
        Inst::Select { dst, ty, c, a, b } => {
            want(c, Ty::I32, "condition")?;
            want(a, *ty, "then")?;
            want(b, *ty, "else")?;
            if f.ty_of(*dst) != *ty {
                return Err("select result ≠ arm type".into());
            }
        }
        Inst::Alloca { dst, size, .. } => {
            want(size, Ty::I64, "size")?;
            if f.ty_of(*dst) != Ty::I64 {
                return Err("alloca result is not I64".into());
            }
        }
        Inst::MemCpy { dst, src, .. } => {
            want(dst, Ty::I64, "dst")?;
            want(src, Ty::I64, "src")?;
        }
        Inst::MemSet { dst, byte, .. } => {
            want(dst, Ty::I64, "dst")?;
            want(byte, Ty::I32, "fill byte")?;
        }
        Inst::Call { .. } | Inst::Intrinsic { .. } => {}
    }
    Ok(())
}
