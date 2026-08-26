// The allocator's own obligations (REARCH.md §7.6). Structural properties that
// THEORY A7 — the allocator's obligations; THEORY A8 — certify at the middle
// are decidable on the physical MIR alone; the semantic obligation
// `⟦mir_v⟧ = ⟦mir_p⟧` is discharged by the battery, which runs both.
//
//   (a) no virtual register survives, and every ABI-fixed operand is satisfied
//       — `mir::verify` in its physical mode;
//   (b) every `Reload` reads a slot some `Spill` has written ON EVERY PATH that
//       reaches it, so no reload can read an undefined slot;
//   (c) no `ParallelCopy` remains — sequentialization is complete.
use crate::mir::*;
use std::collections::BTreeSet;

pub fn verify(f: &MFunc) -> Result<(), String> {
    crate::mir::verify::verify(f)?;
    let cfg = crate::mir::verify::cfg(f);
    let err = |m: String| Err(format!("regalloc::verify {}: {}", f.name, m));

    // (b) SLOT DATAFLOW — "stored on every path to here", computed as the MUST
    // analysis it is: `in[b]` is the INTERSECTION over `b`'s predecessors of what
    // each of them leaves stored, and `out[b]` is that plus the slots `b` itself
    // stores. Initialized to the whole slot universe away from the entry and
    // iterated down to the greatest fixed point, which is what makes a loop's
    // back edge assume nothing it cannot prove.
    //
    // IT USED TO INHERIT FROM THE IMMEDIATE DOMINATOR INSTEAD, and that was a
    // sound approximation of the same property (a slot stored on every path to
    // `idom(b)` is stored on every path to `b`) but an incomplete one — it can
    // only see a store that DOMINATES the reload. The obligation is not
    // dominance; dominance was the cheap way to establish it. `evict_params` is
    // the shape that separates the two: a spilled block parameter stops existing
    // and EVERY INCOMING EDGE stores the value it would have passed, so the slot
    // is written on every path into the block and on none of the blocks that
    // dominate it. Measured on an eight-arm fixture whose parameters the spiller
    // evicts, the dominator form reported `reload of unstored slot 26` on a
    // function the interpreter agrees is correct — a false alarm, not a defect,
    // and the kind that trains a reader to distrust the checker. The must
    // analysis answers the question the obligation actually asks.
    let ns = f.slots.len();
    let nb = f.blocks.len();
    let stores: Vec<BTreeSet<SlotId>> = f
        .blocks
        .iter()
        .map(|b| {
            b.insts
                .iter()
                .filter_map(|i| match i {
                    MInst::Spill { slot, .. } => Some(*slot),
                    _ => None,
                })
                .collect()
        })
        .collect();
    let all: BTreeSet<SlotId> = (0..ns as SlotId).collect();
    let mut out: Vec<BTreeSet<SlotId>> = (0..nb)
        .map(|bi| {
            if bi == f.entry as usize {
                stores[bi].clone()
            } else {
                all.clone()
            }
        })
        .collect();
    let mut inn: Vec<BTreeSet<SlotId>> = vec![BTreeSet::new(); nb];
    // Monotone descent on a finite lattice: each round can only remove slots, so
    // it stops in at most |blocks| × |slots| steps.
    loop {
        let mut changed = false;
        for &b in &cfg.rpo {
            let bi = b as usize;
            let have: BTreeSet<SlotId> = if bi == f.entry as usize || cfg.preds[bi].is_empty() {
                BTreeSet::new()
            } else {
                let mut it = cfg.preds[bi].iter();
                let mut acc = out[*it.next().unwrap() as usize].clone();
                for &p in it {
                    acc = acc.intersection(&out[p as usize]).copied().collect();
                }
                acc
            };
            let o: BTreeSet<SlotId> = have.union(&stores[bi]).copied().collect();
            if o != out[bi] {
                out[bi] = o;
                changed = true;
            }
            inn[bi] = have;
        }
        if !changed {
            break;
        }
    }
    for bi in 0..nb {
        let mut have = inn[bi].clone();
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            match inst {
                MInst::Spill { slot, .. } => {
                    have.insert(*slot);
                }
                MInst::Reload { slot, .. } if !have.contains(slot) => {
                    return err(format!("bb{}[{}]: reload of unstored slot {}", bi, i, slot));
                }
                MInst::ParallelCopy(_) => {
                    return err(format!("bb{}[{}]: parallel copy not sequentialized", bi, i));
                }
                _ => {}
            }
        }
    }
    Ok(())
}
