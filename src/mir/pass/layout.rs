// Block layout (REARCH.md §8, post-allocation).
//
// Choose the order the blocks are printed in, so that as many branches as
// possible become fall-through. Reverse postorder already keeps a loop body
// contiguous and puts a loop's exit after it; the one extra rule is to INVERT a
// conditional whose taken-target is the next block, so the untaken path falls
// through instead of costing an unconditional branch.
//
// This pass has no semantic obligation beyond preserving the CFG: it reorders
// and inverts, never adds or removes an edge. Its square is the identity on
// ⟦·⟧, which the interpreter confirms because the interpreter follows edges,
// not order.
use crate::cfg::{Cfg, LoopForest, DomTree};
use crate::mir::*;

pub fn run(f: &mut MFunc) {
    let cfg = crate::mir::verify::cfg(f);
    let dt = DomTree::new(&cfg, f.entry);
    let lf = LoopForest::new(&cfg, &dt);
    // reverse postorder, but visiting the deeper-nested successor first so a
    // loop body stays contiguous
    let mut order = cfg.rpo.clone();
    order.sort_by_key(|&b| (cfg.rpo_num[b as usize], std::cmp::Reverse(lf.depth[b as usize])));
    order.sort_by_key(|&b| cfg.rpo_num[b as usize]);
    f.order = order;

    // fall-through: invert a conditional whose TAKEN target is the next block
    for i in 0..f.order.len() {
        let b = f.order[i] as usize;
        let next = f.order.get(i + 1).copied();
        let term = f.blocks[b].term.clone();
        let new = match term {
            MTerm::Bcc(cc, fl, t, e) if Some(t.block) == next => {
                MTerm::Bcc(cc.invert(), fl, e, t)
            }
            MTerm::Cbz { w, reg, zero, t, f: e } if Some(t.block) == next => MTerm::Cbz {
                w,
                reg,
                zero: !zero,
                t: e,
                f: t,
            },
            MTerm::Tb { w, reg, bit, set, t, f: e } if Some(t.block) == next => MTerm::Tb {
                w,
                reg,
                bit,
                set: !set,
                t: e,
                f: t,
            },
            other => other,
        };
        f.blocks[b].term = new;
    }
}
