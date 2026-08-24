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
    phase("  spill", || spill::spill(f))?;
    let cfg = phase("  cfg", || crate::mir::verify::cfg(f));
    let lv = phase("  liveness", || live::compute(f, &cfg));
    let dt = phase("  domtree", || DomTree::new(&cfg, f.entry));
    let col = phase("  colour", || color::color(f, &lv, &dt))?;
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
