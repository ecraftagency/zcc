// shrink_wrap (REARCH.md §8, gcc `-fshrink-wrap`) — save the callee-saved
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
// registers only on the path that uses them.
//
// frame.rs saves every used callee-saved register (and LR) at the entry and
// restores it before every return, so a function with a cheap early exit —
// `if (err) return -1;` before the real work — pays that save/restore on the
// fast path too. This pass moves the SAVES to the nearest block that dominates
// every use and drops the RESTORES from the returns that path never reaches.
//
// SOUND SUBSET, and the four things it checks (each is a miscompile if skipped):
// let S be the blocks that need preservation — a Call (which clobbers LR) or a
// reference to a saved register OUTSIDE the save/restore instructions themselves
// — and D their nearest common dominator; let R = { b : D dom b }.
//   1. D != entry, or there is nothing to move.
//   2. D executes at most ONCE: no predecessor of D is in R. Otherwise D heads a
//      loop, its re-execution would re-save the loop's live value over the
//      caller's, and the epilogue would restore garbage.
//   3. R is a SINK REGION: no block in R has a successor outside R, so R is
//      entered only through D and left only by a `ret`. A region that merges
//      back into shared code needs edge-split restores and is left alone.
//   4. (Falls out of 1+3.) No block before D uses a saved register — such a
//      block would be in S and so dominated by D — so the fast path clobbers
//      nothing, and every use is dominated by the save.
// Then the saves move to D's head, the returns in R keep their restores, and the
// returns the fast path reaches (outside R) have theirs removed.
//
// The sp adjustment and the x29 frame-pointer save stay at the entry (emit
// prints them from `frame_size`); this moves only the callee-saved register
// traffic, the part that is pure loss on the fast path. A dynamic frame is
// skipped entirely.
use crate::cfg::DomTree;
use crate::mir::*;
use std::collections::HashSet;

/// THEORY A6b  SQUARE shrink_wrap_moves_saves_off_the_fast_path — the sink region
pub fn run(f: &mut MFunc) {
    if f.dyn_stack || f.cs_saves.is_empty() {
        return;
    }
    let entry = f.entry;
    let saved: Vec<PReg> = f.cs_saves.iter().map(|(_, p, _)| *p).collect();
    let cs_slots: HashSet<SlotId> = f.cs_saves.iter().map(|(s, _, _)| *s).collect();

    // a pure callee-saved save/restore — invisible to "needs preservation"
    let is_csr_mem = |i: &MInst| match i {
        MInst::Spill { slot, .. } | MInst::Reload { slot, .. } => cs_slots.contains(slot),
        _ => false,
    };
    let needs = |b: &MBlock| -> bool {
        b.insts.iter().any(|i| {
            if is_csr_mem(i) {
                return false;
            }
            if matches!(i, MInst::Call { .. }) {
                return true;
            }
            let mut hit = false;
            i.visit(&mut |r, _| {
                if let Reg::P(p) = r {
                    if saved.contains(&p) {
                        hit = true;
                    }
                }
            });
            hit
        })
    };

    let cfg = crate::mir::verify::cfg(f);
    let dt = DomTree::new(&cfg, entry);

    let need: Vec<u32> = cfg
        .rpo
        .iter()
        .copied()
        .filter(|&b| needs(&f.blocks[b as usize]))
        .collect();
    if need.is_empty() {
        return;
    }
    // D = nearest common dominator of every block that needs preservation
    let mut d = need[0];
    for &b in &need {
        while !dt.dominates(d, b) {
            d = dt.idom[d as usize];
        }
    }
    if d == entry {
        return; // (1) nothing gained
    }
    // (2) D must not be re-entered by a back edge — else the save runs per loop
    // iteration and overwrites the caller's register with a loop value.
    if cfg.preds[d as usize].iter().any(|&p| dt.dominates(d, p)) {
        return;
    }
    // (3) R must be a sink region: no R block leaves R except by `ret`.
    for &b in &cfg.rpo {
        if dt.dominates(d, b) {
            for s in f.blocks[b as usize].term.succs() {
                if !dt.dominates(d, s) {
                    return;
                }
            }
        }
    }

    // FIRE. The entry's leading `cs_saves.len()` instructions are exactly the
    // prologue spills frame.rs prepended (nothing runs between the two passes),
    // and by (1)+(3) the entry uses no saved register, so removing them is safe.
    let n = f.cs_saves.len();
    debug_assert!(
        f.blocks[entry as usize].insts[..n]
            .iter()
            .all(|i| is_csr_mem(i)),
        "shrink_wrap: entry prologue is not the recorded csr saves"
    );
    f.blocks[entry as usize].insts.drain(0..n);

    // returns the fast path reaches (outside R) never saved — drop their restores
    for &b in &cfg.rpo {
        if matches!(f.blocks[b as usize].term, MTerm::Ret) && !dt.dominates(d, b) {
            f.blocks[b as usize].insts.retain(|i| !is_csr_mem(i));
        }
    }

    // saves move to D's head
    let mut spills: Vec<MInst> = f
        .cs_saves
        .iter()
        .map(|(slot, p, w)| MInst::Spill {
            slot: *slot,
            src: Reg::P(*p),
            w: *w,
        })
        .collect();
    let rest = std::mem::take(&mut f.blocks[d as usize].insts);
    spills.extend(rest);
    f.blocks[d as usize].insts = spills;
}
