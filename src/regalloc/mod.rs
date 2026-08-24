// Register allocation ON SSA — the core of the re-architecture (REARCH.md §7).
//
// The order is the whole point, and it is the exact inverse of rc3's:
//
//   split critical edges → SPILL to pressure ≤ k → COLOR (chordal, optimal)
//                        → destruct SSA → sequentialize copies
//
// rc3 ran `to_ssa ▸ passes ▸ out_of_ssa ▸ abi_alloc`: allocation AFTER SSA was
// destroyed, on a graph that is not chordal, where coloring is NP-hard and the
// heuristic gives every value one home for its whole life — so live-range
// splitting was structurally impossible and sqlite carried 27,403 frame-slot
// memory operations. Allocating while the program is still in SSA makes the
// interference graph chordal (Hack 2007), so the greedy walk in dominance
// preorder is optimal and cannot fail once the spiller has done its job.
pub mod color;
pub mod destruct;
pub mod live;
pub mod spill;
#[cfg(test)]
mod tests;
pub mod verify;

use crate::cfg::DomTree;
use crate::mir::*;

pub fn allocate(f: &mut MFunc) -> Result<(), String> {
    use crate::compile::phase;
    prune_unreachable(f);
    destruct::split_critical_edges(f);
    let (col, lv) = spill_and_color(f)?;
    phase("  colour-check", || color::check(f, &lv, &col))?;
    phase("  destruct", || {
        destruct::apply_colors(f, &col.color)?;
        destruct::destruct(f);
        destruct::sequentialize(f);
        Ok::<(), String>(())
    })?;
    // The prologue must preserve exactly the callee-saved registers this
    // function actually writes — no more (AAPCS64 §6.1.1). `frame` reads this.
    let mut saved = RegSet::default();
    for p in col.used.iter() {
        if isa::is_callee_saved(p) {
            saved.add(p);
        }
    }
    f.saved = saved;
    f.physical = true;
    Ok(())
}

/// Spill, then colour — and if the colouring still runs out of registers, force
/// the value it failed on into memory and start over.
///
/// THE THEOREM STILL HOLDS, and this loop is not a retreat from it. Hack 2007
/// guarantees that a chordal greedy cannot get stuck once pressure ≤ k, and the
/// spiller's post-condition (`spill::check_pressure`) establishes exactly that.
/// What the theorem does not cover is the ABI's asymmetry: AAPCS64 §6.1.1 splits
/// the register file into caller- and callee-saved, a value live across a call
/// may only use the second half, and a value that is NOT live across a call may
/// still end up occupying it. That makes the allowed sets NESTED rather than
/// equal, and greedy in dominance order — the order chordality requires — is not
/// free to colour the most-constrained value first. The honest answer is
/// Chaitin's: when the colouring fails, spill and try again. Each round makes one
/// more value memory-resident, so it terminates, and the common case still
/// colours on the first pass.
fn spill_and_color(f: &mut MFunc) -> Result<(color::Coloring, live::Liveness), String> {
    use crate::compile::phase;
    let mut forced: std::collections::BTreeSet<VReg> = std::collections::BTreeSet::new();
    let mut cross_cap = usize::MAX;
    let snapshot = f.clone();
    // one round per value that can be forced, plus the first attempt
    let bound = f.vregs.len() + 1;
    for round in 0..bound {
        let _ = round;
        phase("  spill", || spill::spill_with(f, &forced, cross_cap))?;
        let cfg = phase("  cfg", || crate::mir::verify::cfg(f));
        let lv = phase("  liveness", || live::compute(f, &cfg));
        let dt = phase("  domtree", || DomTree::new(&cfg, f.entry));
        match phase("  colour", || color::color(f, &lv, &dt)) {
            Ok(col) => return Ok((col, lv)),
            Err(color::ColorErr::NoColour(v, holders, m)) => {
                let nholders = holders.len();
                let forcible = holders
                    .iter()
                    .filter(|&&x| (x as usize) < snapshot.vregs.len())
                    .count();
                // A value the spiller INVENTED (a reload copy) cannot itself be
                // forced — it does not exist in the snapshot we restart from — so
                // the relief comes from one of the values HOLDING a register of
                // that class instead. Either way one more value becomes
                // memory-resident, which is what makes the loop terminate.
                let pick = std::iter::once(v)
                    .chain(holders)
                    .find(|&x| (x as usize) < snapshot.vregs.len() && !forced.contains(&x));
                match pick {
                    Some(x) => {
                        forced.insert(x);
                        *f = snapshot.clone();
                        continue;
                    }
                    // Nothing left to force: every holder is either already
                    // memory-resident or a reload copy the spiller invented. The
                    // demand for callee-saved registers is what has to come down,
                    // so lower its ceiling and start over.
                    None if cross_cap > 0 => {
                        let cur = if cross_cap == usize::MAX {
                            isa::callee_saved_mask(Class::Gpr).count_ones() as usize
                        } else {
                            cross_cap
                        };
                        cross_cap = cur.saturating_sub(1);
                        forced.clear();
                        *f = snapshot.clone();
                        continue;
                    }
                    None => {
                        return Err(format!(
                            "{} (after {} forced-spill rounds, {} values forced, {} holders \
                             of which {} are pre-existing values)",
                            m,
                            round,
                            forced.len(),
                            nholders,
                            forcible
                        ));
                    }
                }
            }
            Err(color::ColorErr::Other(m)) => return Err(m),
        }
    }
    Err(format!("{}: register allocation did not converge", f.name))
}

/// Empty every block the entry cannot reach. `hir::build` leaves such blocks
/// behind by construction (statements after a `return`, a label no `goto`
/// mentions), and their instructions define values the colourer — which walks
/// the dominator tree, i.e. exactly the reachable blocks — never sees.
fn prune_unreachable(f: &mut MFunc) {
    let cfg = crate::mir::verify::cfg(f);
    for b in 0..f.blocks.len() {
        if !cfg.reachable(b as MBlockId) {
            f.blocks[b].params.clear();
            f.blocks[b].insts.clear();
            f.blocks[b].term = MTerm::Unreachable;
        }
    }
}

pub fn allocate_module(m: &mut MModule) -> Result<(), String> {
    for f in m.funcs.iter_mut() {
        allocate(f)?;
    }
    Ok(())
}
