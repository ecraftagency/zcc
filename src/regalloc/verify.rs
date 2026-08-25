// The allocator's own obligations (REARCH.md §7.6). Structural properties that
// THEORY A7 — the allocator's obligations; THEORY A8 — certify at the middle
// are decidable on the physical MIR alone; the semantic obligation
// `⟦mir_v⟧ = ⟦mir_p⟧` is discharged by the battery, which runs both.
//
//   (a) no virtual register survives, and every ABI-fixed operand is satisfied
//       — `mir::verify` in its physical mode;
//   (b) every `Reload` is dominated by a `Spill` of the same slot, so no reload
//       can read an undefined slot;
//   (c) no `ParallelCopy` remains — sequentialization is complete.
use crate::cfg::DomTree;
use crate::mir::*;
use std::collections::BTreeSet;

pub fn verify(f: &MFunc) -> Result<(), String> {
    crate::mir::verify::verify(f)?;
    let cfg = crate::mir::verify::cfg(f);
    let dt = DomTree::new(&cfg, f.entry);
    let err = |m: String| Err(format!("regalloc::verify {}: {}", f.name, m));

    // (b) slot dataflow: walk the dominator tree carrying the set of slots
    // already stored on every path to this point.
    let mut stored_at: Vec<BTreeSet<SlotId>> = vec![BTreeSet::new(); f.blocks.len()];
    for &b in &dt.preorder {
        let bi = b as usize;
        let mut have = if b == f.entry {
            BTreeSet::new()
        } else {
            stored_at[dt.idom[bi] as usize].clone()
        };
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            match inst {
                MInst::Spill { slot, .. } => {
                    have.insert(*slot);
                }
                MInst::Reload { slot, .. } if !have.contains(slot) => {
                    return err(format!("bb{}[{}]: reload of unstored slot {}", b, i, slot));
                }
                MInst::ParallelCopy(_) => {
                    return err(format!("bb{}[{}]: parallel copy not sequentialized", b, i));
                }
                _ => {}
            }
        }
        stored_at[bi] = have;
    }
    Ok(())
}
