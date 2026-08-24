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

    // LAST: relaxation must see the final terminators. Inverting a conditional
    // whose taken target is the next block would otherwise turn a trampoline
    // back into a direct branch to the far target.
    relax_branches(f);
}

/// BRANCH RELAXATION. A64's conditional forms do not all reach as far as the
/// unconditional one: `b.cc`, `cbz` and `cbnz` carry a 19-bit signed offset
/// (±1 MB) and `tbz`/`tbnz` only a 14-bit one (±32 KB), against `b`'s 26 bits
/// (DDI 0487 C6.2.26/C6.2.42/C6.2.375). A function large enough — csmith
/// produces them, and so does any generated dispatch table — puts a target out
/// of reach, and the ASSEMBLER cannot fix it: there is no relaxation for these
/// encodings.
///
/// The fix is a TRAMPOLINE, and it belongs here rather than in `emit` because it
/// changes the block graph: the conditional jumps to a new block placed
/// immediately after it, which jumps the rest of the way unconditionally. The
/// square is the identity — an extra block whose only instruction is a jump
/// changes no value and no order.
fn relax_branches(f: &mut MFunc) {
    // Instruction counts, generously rounded up: a terminator can expand into a
    // few instructions and `MovImm` into a `movz`/`movk` chain. The margin is
    // what makes ONE measurement enough, rather than a fixpoint over distances
    // that the trampolines themselves change.
    let size = |blk: &MBlock| -> usize {
        let body: usize = blk
            .insts
            .iter()
            .map(|i| match i {
                MInst::MovImm { imm, w, .. } => isa::mov_chain(*imm, *w == Width::W64).len(),
                _ => 1,
            })
            .sum();
        body + match blk.term {
            MTerm::Switch { .. } => 8,
            _ => 3,
        }
    };
    // half the true reach, so the instructions relaxation ADDS cannot undo it
    const NEAR: usize = 4096;
    const FAR: usize = 1 << 17;
    for _ in 0..8 {
        let mut at: Vec<usize> = vec![0; f.blocks.len()];
        let mut pos = 0usize;
        for &b in &f.order {
            at[b as usize] = pos;
            pos += size(&f.blocks[b as usize]);
        }
        let mut fixes: Vec<(MBlockId, usize)> = Vec::new();
        for (oi, &b) in f.order.iter().enumerate() {
            let (target, limit) = match &f.blocks[b as usize].term {
                MTerm::Tb { t, .. } => (t.block, NEAR),
                MTerm::Bcc(_, _, t, _) | MTerm::Cbz { t, .. } => (t.block, FAR),
                _ => continue,
            };
            let (x, y) = (at[b as usize], at[target as usize]);
            if x.abs_diff(y) > limit {
                fixes.push((b, oi));
            }
        }
        if fixes.is_empty() {
            return;
        }
        // insert from the back so the earlier order indices stay valid
        for (b, oi) in fixes.into_iter().rev() {
            let far = match &f.blocks[b as usize].term {
                MTerm::Tb { t, .. } => t.clone(),
                MTerm::Bcc(_, _, t, _) | MTerm::Cbz { t, .. } => t.clone(),
                _ => continue,
            };
            let mid = f.new_block();
            f.blocks[mid as usize].weight = f.blocks[b as usize].weight;
            f.blocks[mid as usize].term = MTerm::B(far);
            let near = MTarget { block: mid, args: vec![] };
            match &mut f.blocks[b as usize].term {
                MTerm::Tb { t, .. } => *t = near,
                MTerm::Bcc(_, _, t, _) => *t = near,
                MTerm::Cbz { t, .. } => *t = near,
                _ => {}
            }
            f.order.insert(oi + 1, mid);
        }
    }
}
