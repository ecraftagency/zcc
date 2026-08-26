// The MIR well-formedness checker (REARCH.md §5.4). One function, two
// THEORY A6b — MIR's well-formedness; THEORY A8 — certify at the middle
// obligations, selected by `MFunc.physical`:
//
//   VIRTUAL  — SSA: every virtual register defined once, every use dominated by
//              its definition, block-argument arity and register class matching
//              on every edge, and register class agreeing with the instruction
//              form (a GPR never appears where the encoding needs a v-register).
//   PHYSICAL — no virtual register survives, and every fixed constraint the ABI
//              placed on a call is satisfied by the assigned physical register.
//
// The allocator's own obligations — interference, clobber safety, slot dataflow —
// are proven in `regalloc/verify.rs` (§7.6), which needs liveness this file does
// not compute.
use super::*;
use crate::cfg::{Cfg, DomTree};

pub fn cfg(f: &MFunc) -> Cfg {
    Cfg::build(f.blocks.len(), f.entry, |b| f.blocks[b as usize].term.succs())
}

pub fn verify(f: &MFunc) -> Result<(), String> {
    let c = cfg(f);
    let dt = DomTree::new(&c, f.entry);
    let err = |m: String| Err(format!("mir::verify {}: {}", f.name, m));

    if f.physical {
        return verify_physical(f, &c);
    }

    // (1) single assignment, and where each definition sits
    let n = f.vregs.len();
    let mut def_at: Vec<Option<(MBlockId, u32)>> = vec![None; n];
    for (bi, blk) in f.blocks.iter().enumerate() {
        let b = bi as MBlockId;
        if !c.reachable(b) {
            continue;
        }
        for &p in &blk.params {
            if let Reg::V(v) = p {
                if def_at[v as usize].is_some() {
                    return err(format!("v{} defined twice", v));
                }
                def_at[v as usize] = Some((b, 0));
            }
        }
        for (i, inst) in blk.insts.iter().enumerate() {
            let mut dup = None;
            inst.visit(&mut |r, k| {
                if let (Reg::V(v), Constraint::Def | Constraint::DefFixed(_)) = (r, k) {
                    if def_at[v as usize].is_some() {
                        dup = Some(v);
                    }
                    def_at[v as usize] = Some((b, i as u32 + 1));
                }
            });
            if let Some(v) = dup {
                return err(format!("v{} defined twice", v));
            }
        }
    }

    // (2) uses dominated by definitions; (3) class agreement per instruction form
    for (bi, blk) in f.blocks.iter().enumerate() {
        let b = bi as MBlockId;
        if !c.reachable(b) {
            continue;
        }
        for (i, inst) in blk.insts.iter().enumerate() {
            let mut bad = None;
            inst.visit(&mut |r, k| {
                if let (Reg::V(v), Constraint::Use | Constraint::UseFixed(_)) = (r, k) {
                    if bad.is_none() {
                        bad = check_use(f, &dt, &def_at, v, b, i as u32 + 1).err();
                    }
                }
            });
            if let Some(e) = bad {
                return err(format!("bb{}[{}]: {}", b, i, e));
            }
            if let Err(e) = check_classes(f, inst) {
                return err(format!("bb{}[{}]: {}", b, i, e));
            }
            if let Err(e) = check_mem(f, inst) {
                return err(format!("bb{}[{}]: {}", b, i, e));
            }
        }
        let at = blk.insts.len() as u32 + 1;
        let mut bad = None;
        blk.term.visit(&mut |r, _| {
            if let Reg::V(v) = r {
                if bad.is_none() {
                    bad = check_use(f, &dt, &def_at, v, b, at).err();
                }
            }
        });
        if let Some(e) = bad {
            return err(format!("bb{} terminator: {}", b, e));
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
            for (a, p) in t.args.iter().zip(want) {
                // Only virtual registers carry a width; a physical register (xzr
                // as a constant-zero argument) takes the parameter's form.
                if a.preg().is_some() || p.preg().is_some() {
                    continue;
                }
                if width_of(f, *a) != width_of(f, *p) {
                    return err(format!(
                        "edge bb{}→bb{}: {:?} of width {:?} passed to {:?} of width {:?}",
                        b,
                        t.block,
                        a,
                        width_of(f, *a),
                        p,
                        width_of(f, *p)
                    ));
                }
            }
        }
    }
    Ok(())
}

fn check_use(
    f: &MFunc,
    dt: &DomTree,
    def_at: &[Option<(MBlockId, u32)>],
    v: VReg,
    b: MBlockId,
    at: u32,
) -> Result<(), String> {
    let _ = f;
    match def_at[v as usize] {
        None => Err(format!("use of undefined v{}", v)),
        Some((db, di)) => {
            if db == b {
                if di <= at {
                    Ok(())
                } else {
                    Err(format!("v{} used before its definition", v))
                }
            } else if dt.dominates(db, b) {
                Ok(())
            } else {
                Err(format!("v{} used in bb{} not dominated by bb{}", v, b, db))
            }
        }
    }
}

fn width_of(f: &MFunc, r: Reg) -> Width {
    match r {
        Reg::V(v) => f.vregs[v as usize].width,
        // A physical register carries no width of its own; the instruction form
        // decides. Comparing edge arguments only makes sense in the SSA phase.
        Reg::P(_) => Width::W64,
    }
}

/// The register class an operand must have, read off the instruction form. This
/// is where "a GPR fed to `fadd`" is caught — a Law-2 Side-I defect in isel,
/// found here instead of in a wrong answer three layers down.
fn check_classes(f: &MFunc, inst: &MInst) -> Result<(), String> {
    let want = |r: Reg, c: Class, what: &str| -> Result<(), String> {
        if f.class_of(r) == c {
            Ok(())
        } else {
            Err(format!("{} is {:?}, expected {:?}", what, f.class_of(r), c))
        }
    };
    match inst {
        MInst::Alu { dst, a, b, flags, w, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*a, Class::Gpr, "lhs")?;
            if let Rhs::Reg(r) | Rhs::Shifted(r, ..) | Rhs::Extended(r, ..) = b {
                want(*r, Class::Gpr, "rhs")?;
            }
            if let Some(fl) = flags {
                want(*fl, Class::Flags, "flags")?;
            }
            if !matches!(w, Width::W32 | Width::W64) {
                return Err(format!("integer ALU at width {:?}", w));
            }
        }
        MInst::Bfx { w, dst, src, lsb, width, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*src, Class::Gpr, "src")?;
            let bits = if *w == Width::W32 { 32 } else { 64 };
            if *width == 0 || *lsb as u32 + *width as u32 > bits {
                return Err(format!("bfx #{}:#{} outside a {}-bit register", lsb, width, bits));
            }
        }
        MInst::Pair { w, a, b, .. } => {
            let c = if w.class() == Class::Fpr { Class::Fpr } else { Class::Gpr };
            want(*a, c, "first")?;
            want(*b, c, "second")?;
            // DDI 0487 C6.2.130: `ldp` with two identical destinations is
            // CONSTRAINED UNPREDICTABLE.
            if *a == *b {
                return Err("a pair may not name one register twice".into());
            }
        }
        MInst::StackAlloc { dst, size } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*size, Class::Gpr, "size")?;
        }
        MInst::LdAxr { dst, addr, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*addr, Class::Gpr, "addr")?;
        }
        MInst::StlXr {
            status, src, addr, ..
        } => {
            want(*status, Class::Gpr, "status")?;
            want(*src, Class::Gpr, "src")?;
            want(*addr, Class::Gpr, "addr")?;
        }
        MInst::Stlr { src, addr, .. } => {
            want(*src, Class::Gpr, "src")?;
            want(*addr, Class::Gpr, "addr")?;
        }
        MInst::Mrs { dst } | MInst::SpAddr { dst, .. } => want(*dst, Class::Gpr, "dst")?,
        MInst::AddTprel { dst, base, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*base, Class::Gpr, "base")?;
        }
        // opaque by construction: the template's registers are already physical
        MInst::Dmb | MInst::Asm { .. } => {}
        // the frame adjust names no register (sp is implicit)
        MInst::SpAdj { .. } => {}
        MInst::Alu3 { dst, a, b, c, .. } => {
            for (r, n) in [(dst, "dst"), (a, "a"), (b, "b"), (c, "c")] {
                want(*r, Class::Gpr, n)?;
            }
        }
        MInst::Cmp { a, b, flags, .. } => {
            want(*a, Class::Gpr, "lhs")?;
            if let Rhs::Reg(r) | Rhs::Shifted(r, ..) | Rhs::Extended(r, ..) = b {
                want(*r, Class::Gpr, "rhs")?;
            }
            want(*flags, Class::Flags, "flags")?;
        }
        MInst::MovImm { dst, .. } | MInst::Adrp { dst, .. } | MInst::SlotAddr { dst, .. } => {
            want(*dst, Class::Gpr, "dst")?
        }
        MInst::AddLo12 { dst, base, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*base, Class::Gpr, "base")?;
        }
        MInst::Ext { dst, src, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*src, Class::Gpr, "src")?;
        }
        MInst::Load { op, dst, mem, .. } => {
            want(*dst, op.class(), "dst")?;
            check_addr(f, mem)?;
        }
        MInst::Store { op, src, mem, .. } => {
            want(*src, op.class(), "src")?;
            check_addr(f, mem)?;
        }
        MInst::CSel { dst, a, b, flags, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*a, Class::Gpr, "a")?;
            want(*b, Class::Gpr, "b")?;
            want(*flags, Class::Flags, "flags")?;
        }
        MInst::CSet { dst, flags, .. } => {
            want(*dst, Class::Gpr, "dst")?;
            want(*flags, Class::Flags, "flags")?;
        }
        MInst::FpAlu { dst, a, b, w, .. } => {
            want(*dst, Class::Fpr, "dst")?;
            want(*a, Class::Fpr, "a")?;
            want(*b, Class::Fpr, "b")?;
            if !matches!(w, Width::S | Width::D) {
                return Err(format!("FP ALU at width {:?}", w));
            }
        }
        MInst::FpUn { dst, src, .. } => {
            want(*dst, Class::Fpr, "dst")?;
            want(*src, Class::Fpr, "src")?;
        }
        MInst::FpCmp { a, b, zero, flags, .. } => {
            want(*a, Class::Fpr, "a")?;
            if !*zero {
                want(*b, Class::Fpr, "b")?;
            }
            want(*flags, Class::Flags, "flags")?;
        }
        MInst::FpCvt { op, dst, src, .. } => {
            let (dc, sc) = match op {
                CvtOp::Scvtf | CvtOp::Ucvtf => (Class::Fpr, Class::Gpr),
                CvtOp::Fcvtzs | CvtOp::Fcvtzu => (Class::Gpr, Class::Fpr),
            };
            want(*dst, dc, "dst")?;
            want(*src, sc, "src")?;
        }
        // `fmov` is the one instruction that legitimately crosses the files.
        MInst::FMov { .. } => {}
        MInst::Copy { dst, src, .. } => {
            if f.class_of(*dst) != f.class_of(*src) {
                return Err("copy between register classes".into());
            }
        }
        MInst::ParallelCopy(pairs) => {
            for (d, s, _) in pairs {
                if f.class_of(*d) != f.class_of(*s) {
                    return Err("parallel copy between register classes".into());
                }
            }
        }
        MInst::Spill { src, slot, .. } => {
            if *slot as usize >= f.slots.len() {
                return Err(format!("spill to out-of-range slot {}", slot));
            }
            if f.class_of(*src) == Class::Flags {
                return Err("flags are not spillable: rematerialize the compare".into());
            }
        }
        MInst::Reload { slot, .. } => {
            if *slot as usize >= f.slots.len() {
                return Err(format!("reload from out-of-range slot {}", slot));
            }
        }
        MInst::Call { .. } => {}
    }
    Ok(())
}

/// Every memory operand an instruction carries.
fn check_mem(f: &MFunc, i: &MInst) -> Result<(), String> {
    match i {
        MInst::Load { mem, .. } | MInst::Store { mem, .. } | MInst::Pair { mem, .. } => {
            check_addr(f, mem)
        }
        _ => Ok(()),
    }
}

fn check_addr(f: &MFunc, m: &AddrMode) -> Result<(), String> {
    let gpr = |r: Reg| -> Result<(), String> {
        // DDI 0487 C1.2.5: in the Rn field of a load/store, register 31 decodes
        // as SP, NOT as ZR. An address that folded to a literal zero must be
        // materialized; riding it for free in the zero register — legal for a
        // data operand — silently assembles as an SP-relative access or is
        // rejected outright (`strb wzr, [xzr]`, torture 930719-1).
        if r == Reg::P(crate::mir::isa::ZR) {
            return Err("zero register used as a memory base (Rn=31 means SP)".into());
        }
        if f.class_of(r) == Class::Gpr {
            Ok(())
        } else {
            Err("address operand is not a GPR".into())
        }
    };
    match m {
        AddrMode::BaseImm { base, .. } | AddrMode::SymLo12 { base, .. } => gpr(*base),
        AddrMode::BaseReg { base, idx, .. } => gpr(*base).and(gpr(*idx)),
        AddrMode::PreIdx { base, wb, .. } | AddrMode::PostIdx { base, wb, .. } => {
            gpr(*base).and(gpr(*wb))
        }
        AddrMode::Slot { slot, .. } | AddrMode::FrameWb { slot, .. } => {
            if *slot as usize >= f.slots.len() {
                Err(format!("out-of-range slot {}", slot))
            } else {
                Ok(())
            }
        }
        // AAPCS64 §6.4: the outgoing area is sp-relative by definition — it has
        // no register operand to check.
        AddrMode::SpArg { .. } => Ok(()),
    }
}

fn verify_physical(f: &MFunc, c: &Cfg) -> Result<(), String> {
    let err = |m: String| Err(format!("mir::verify {} (physical): {}", f.name, m));
    for (bi, blk) in f.blocks.iter().enumerate() {
        let b = bi as MBlockId;
        if !c.reachable(b) {
            continue;
        }
        if !blk.params.is_empty() {
            return err(format!("bb{} still has block parameters", b));
        }
        for (i, inst) in blk.insts.iter().enumerate() {
            let mut bad = None;
            inst.visit(&mut |r, k| {
                if bad.is_some() {
                    return;
                }
                match (r, k) {
                    (Reg::V(v), _) => bad = Some(format!("v{} survived allocation", v)),
                    (Reg::P(p), Constraint::UseFixed(q) | Constraint::DefFixed(q)) if p != q => {
                        bad = Some(format!("{:?} where the ABI requires {:?}", p, q))
                    }
                    _ => {}
                }
            });
            if let Some(e) = bad {
                return err(format!("bb{}[{}]: {}", b, i, e));
            }
        }
        let mut bad = None;
        blk.term.visit(&mut |r, _| {
            if let Reg::V(v) = r {
                bad = Some(format!("v{} survived allocation", v));
            }
        });
        if let Some(e) = bad {
            return err(format!("bb{} terminator: {}", b, e));
        }
    }
    Ok(())
}
