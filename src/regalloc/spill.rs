// Spilling: reduce register pressure to ≤ k so that chordal coloring cannot
// fail (REARCH.md §7.2).
//
// STATUS — R0 ships the SOUND base case, not the final algorithm. The R0/R1
// storage model keeps every C local in memory, so the only values competing for
// registers are short-lived expression temporaries and pressure stays far below
// k = 26; this spiller therefore almost never fires, and building the full
// machinery before it can be measured would be building it blind.
//
// What is here: iterative "spill the value with the furthest next use", where a
// spilled value is stored once at its definition and reloaded into a FRESH
// virtual register immediately before each use. That keeps SSA intact with no
// reconstruction (each reload's live range is one instruction) and it does split
// live ranges — the property rc3 could not express at all — but it reloads more
// often than necessary.
//
// What is NOT here, and is REQUIRED before R2.2 (mem2reg creates long-lived
// values and real pressure): Braun & Hack 2009 proper — a per-block working set
// carried across edges with Belady MIN eviction, rematerialization of values
// whose producer is pure (`iconst`, `adrp+add`, an extend), and SSA
// reconstruction (Braun 2013) so one reload can serve several uses. Recorded as
// a residual in REARCH §12 R2.2 rather than left implicit.
use super::live;
use crate::mir::*;
use std::collections::BTreeSet;

/// Bring every program point to pressure ≤ k. Returns the number of values spilled.
pub fn spill(f: &mut MFunc) -> Result<usize, String> {
    let mut spilled = 0;
    loop {
        let cfg = crate::mir::verify::cfg(f);
        let lv = live::compute(f, &cfg);
        let Some((victim, class)) = worst_point(f, &lv, &cfg)? else {
            return Ok(spilled);
        };
        if class == Class::Flags {
            // Flags are never spilled: their producer is pure, so the answer is
            // to rematerialize the compare. Reaching here means isel let two
            // flag values overlap — a Law-2 Side-I defect, not a spill problem.
            return Err(format!(
                "{}: two NZCV values live at once; the compare must be rematerialized",
                f.name
            ));
        }
        spill_value(f, victim);
        spilled += 1;
        if spilled > 10_000 {
            return Err(format!("{}: spilling did not converge", f.name));
        }
    }
}

/// The value to spill next: at the first point whose pressure exceeds k, the
/// live value whose next use is furthest away (Belady's rule, applied globally
/// rather than per working set).
fn worst_point(
    f: &MFunc,
    lv: &live::Liveness,
    cfg: &crate::cfg::Cfg,
) -> Result<Option<(VReg, Class)>, String> {
    // linearize in reverse postorder so "next use" has a meaning
    let mut pos = vec![usize::MAX; f.blocks.len()];
    for (i, &b) in cfg.rpo.iter().enumerate() {
        pos[b as usize] = i;
    }
    for &b in &cfg.rpo {
        let bi = b as usize;
        let blk = &f.blocks[bi];
        let mut live: BTreeSet<usize> = lv.live_in[bi].clone();
        let last = live::last_use(f, lv.sp, lv, bi);
        for (i, inst) in blk.insts.iter().enumerate() {
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    live.insert(lv.sp.idx(*r));
                }
            }
            for class in [Class::Gpr, Class::Fpr, Class::Flags] {
                let members: Vec<usize> = live
                    .iter()
                    .copied()
                    .filter(|&x| class_of(f, lv.sp.reg(x)) == class)
                    .collect();
                if members.len() <= isa::k(class) {
                    continue;
                }
                // never evict something this instruction is about to read or has
                // just defined, and never a physical register
                let mut here = BTreeSet::new();
                for (r, _) in &ops {
                    here.insert(lv.sp.idx(*r));
                }
                let cand = members
                    .iter()
                    .filter(|x| **x < lv.sp.nv && !here.contains(x))
                    .max_by_key(|&&x| next_use(f, cfg, &pos, lv.sp.reg(x), bi, i));
                match cand {
                    Some(&x) => return Ok(Some((x as VReg, class))),
                    // Every live value at this point is pinned by the very
                    // instruction that overflows: the instruction itself needs
                    // more registers than the class has. On A64 no instruction
                    // reads more than four, so this is a Law-2 defect in isel.
                    None => {
                        return Err(format!(
                            "{}: {:?} pressure exceeds k with nothing evictable at bb{}[{}]",
                            f.name, class, bi, i
                        ));
                    }
                }
            }
            // values whose last use is this instruction die here
            let dead: Vec<usize> = live
                .iter()
                .copied()
                .filter(|&x| last[x] == Some(i))
                .collect();
            for x in dead {
                live.remove(&x);
            }
        }
    }
    Ok(None)
}

/// Distance to the next use, in linearized instruction positions; `usize::MAX`
/// when the value is never used again.
fn next_use(
    f: &MFunc,
    cfg: &crate::cfg::Cfg,
    pos: &[usize],
    r: Reg,
    from_block: usize,
    from_inst: usize,
) -> usize {
    let mut best = usize::MAX;
    for &b in &cfg.rpo {
        let bi = b as usize;
        if pos[bi] < pos[from_block] {
            continue;
        }
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            if bi == from_block && i <= from_inst {
                continue;
            }
            let mut hit = false;
            inst.visit(&mut |q, c| {
                if q == r && matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    hit = true;
                }
            });
            if hit {
                return (pos[bi] - pos[from_block]) * 1000 + i;
            }
        }
        let mut hit = false;
        f.blocks[bi].term.visit(&mut |q, _| {
            if q == r {
                hit = true;
            }
        });
        if hit {
            best = best.min((pos[bi] - pos[from_block]) * 1000 + 999);
        }
    }
    best
}

fn class_of(f: &MFunc, r: Reg) -> Class {
    match r {
        Reg::V(v) => f.vregs[v as usize].class,
        Reg::P(p) => p.class,
    }
}

/// Store `v` once at its definition; reload it into a fresh register before each
/// use. The fresh registers are what makes this a live-range SPLIT rather than
/// rc3's one-home-per-value.
fn spill_value(f: &mut MFunc, v: VReg) {
    let w = f.vregs[v as usize].width;
    let slot = f.new_slot(w.bytes().max(8), 8, SlotKind::Spill);
    let target = Reg::V(v);
    for b in 0..f.blocks.len() {
        let mut out: Vec<MInst> = Vec::with_capacity(f.blocks[b].insts.len() + 2);
        // a block parameter is defined at the block's entry
        if f.blocks[b].params.contains(&target) {
            out.push(MInst::Spill {
                slot,
                src: target,
                w,
            });
        }
        let insts = std::mem::take(&mut f.blocks[b].insts);
        for mut inst in insts {
            let mut uses_it = false;
            inst.visit(&mut |r, c| {
                if r == target && matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    uses_it = true;
                }
            });
            if uses_it {
                let d = f.new_vreg(w);
                out.push(MInst::Reload { slot, dst: d, w });
                inst.visit_mut(&mut |r, c| {
                    if *r == target && matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                        *r = d;
                    }
                });
            }
            let mut defines_it = false;
            inst.visit(&mut |r, c| {
                if r == target && matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    defines_it = true;
                }
            });
            out.push(inst);
            if defines_it {
                out.push(MInst::Spill {
                    slot,
                    src: target,
                    w,
                });
            }
        }
        // the terminator's operands (including edge arguments)
        let mut term = f.blocks[b].term.clone();
        let mut uses_it = false;
        term.visit(&mut |r, _| {
            if r == target {
                uses_it = true;
            }
        });
        if uses_it {
            let d = f.new_vreg(w);
            out.push(MInst::Reload { slot, dst: d, w });
            term.visit_mut(&mut |r, _| {
                if *r == target {
                    *r = d;
                }
            });
            f.blocks[b].term = term;
        }
        f.blocks[b].insts = out;
    }
}
