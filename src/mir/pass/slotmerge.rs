// slotmerge — TWO VALUES NEVER LIVE AT ONCE SHARE ONE STACK SLOT.
// THEORY A7b — optimization: this pass ships its commuting square
//
// The spiller gives one slot per SSA WEB, and a web is one C variable's chain of
// SSA names. That is exactly right for deciding WHERE a value lives, and it says
// nothing about whether two DIFFERENT variables need two different addresses.
// They do not, when their live ranges do not overlap — the classic example being
// a `switch` whose arms each spill a local of their own, since at most one arm
// runs per dispatch.
//
// WHAT IT COSTS TO SKIP, measured (2026-08-27). sqlite's `sqlite3VdbeExec`
// dispatches 196 opcodes and holds **199 distinct frame slots where gcc holds
// 43** — a 4.6× frame for the function that carries ~47% of sqlite's remaining
// runtime gap (`MEASURED M16`). Nearly every one of those arms declares locals
// that cannot be live at the same time as any other arm's.
//
// THE ALGORITHM is interference-graph merging, the same shape as register
// colouring one layer down: a slot is LIVE from a `Spill` that writes it to the
// last `Reload` that reads it, two slots INTERFERE when both are live at one
// program point, and any two that do not interfere and agree on size and
// alignment may share. Greedy first-fit in slot-id order — the graph is small
// (one node per spilled web) and an optimal colouring buys nothing a frame
// pointer would notice.
//
// COMMUTING SQUARE. Merging renames slot `b` to slot `a` throughout. Memory
// differs from the unmerged program only at addresses inside `a`, and every read
// of `a` is a `Reload` whose value was written by the `Spill` that dominates it
// — which is `regalloc::verify`'s standing obligation ("every reload preceded by
// a spill of its slot ON EVERY PATH"), asserted on every compile. Non-
// interference is precisely the statement that no such write-read pair of `a`
// is separated by a write of `b`, so every load yields the byte the unmerged
// program would have yielded and `⟦mir⟧` is unchanged.
//
// WHAT IS REFUSED, each because the pass cannot see far enough to be sure:
//   * a slot whose ADDRESS is taken (`AddrMode::Slot`) — its bytes may be read
//     through a pointer this analysis does not follow. Only allocator slots,
//     which are reached exclusively by `Spill`/`Reload`, are candidates.
//   * anything but `SlotKind::Spill`. A C local's lifetime is the language's to
//     decide, the outgoing-argument area is pinned by the ABI, and an incoming
//     argument lives in the caller's frame.
//   * slots of different size or alignment. Merging those needs the union's
//     layout, which is the frame's business, not this pass's.
use crate::mir::*;
use std::collections::BTreeSet;

/// THEORY A7b  SQUARE slots_that_never_overlap_share_one — non-interference IS the licence
///
/// Run before `frame::run`: a slot must still be a NUMBER, not an offset.
pub fn run(f: &mut MFunc) -> bool {
    if f.laid_out {
        return false;
    }
    let n = f.slots.len();
    if n < 2 {
        return false;
    }
    // (1) candidates: allocator slots whose address is never taken
    let mut escaped = vec![false; n];
    for b in f.blocks.iter() {
        for inst in b.insts.iter() {
            if let MInst::Load { mem, .. } | MInst::Store { mem, .. } = inst {
                if let AddrMode::Slot { slot, .. } = mem {
                    escaped[*slot as usize] = true;
                }
            }
            if let MInst::SlotAddr { slot, .. } = inst {
                escaped[*slot as usize] = true;
            }
        }
    }
    let cand: Vec<SlotId> = (0..n as SlotId)
        .filter(|&s| f.slots[s as usize].kind == SlotKind::Spill && !escaped[s as usize])
        .collect();
    if cand.len() < 2 {
        return false;
    }
    let mut idx = vec![usize::MAX; n];
    for (i, &s) in cand.iter().enumerate() {
        idx[s as usize] = i;
    }
    let m = cand.len();

    // (2) backward liveness over SLOTS. A `Reload` reads, a `Spill` writes; the
    //     dataflow is the ordinary one, with the slot in place of the value.
    let cfg = crate::mir::verify::cfg(f);
    let words = m.div_ceil(64);
    let mut live_out: Vec<Vec<u64>> = vec![vec![0; words]; f.blocks.len()];
    let get = |v: &[u64], i: usize| v[i / 64] >> (i % 64) & 1 == 1;
    let set = |v: &mut [u64], i: usize| v[i / 64] |= 1 << (i % 64);
    let clr = |v: &mut [u64], i: usize| v[i / 64] &= !(1 << (i % 64));
    let mut changed = true;
    while changed {
        changed = false;
        for &b in cfg.rpo.iter().rev() {
            let bi = b as usize;
            let mut cur = vec![0u64; words];
            for &s in cfg.succs[bi].iter() {
                let si = s as usize;
                for w in 0..words {
                    cur[w] |= live_in_of(f, &live_out, &idx, si, words)[w];
                }
            }
            if cur != live_out[bi] {
                live_out[bi] = cur;
                changed = true;
            }
        }
    }

    // (3) interference: walk each block backward from its live-out set; when a
    //     slot becomes live it interferes with everything already live.
    let mut inter: Vec<Vec<u64>> = vec![vec![0; words]; m];
    for bi in 0..f.blocks.len() {
        if !cfg.reachable(bi as MBlockId) {
            continue;
        }
        let mut live = live_out[bi].clone();
        // Everything live at the block's EXIT is simultaneously live there, and
        // a slot can cross a whole block without being named in it — so the
        // exit set is a clique in its own right, not merely a starting point.
        for i in 0..m {
            if !get(&live, i) {
                continue;
            }
            for j in (i + 1)..m {
                if get(&live, j) {
                    set(&mut inter[i], j);
                    set(&mut inter[j], i);
                }
            }
        }
        for inst in f.blocks[bi].insts.iter().rev() {
            match inst {
                // A WRITE is where interference is decided, not a read. Walking
                // backward, everything still live at a `Spill` is a slot whose
                // value must survive this write, so it cannot share an address
                // with the slot being written.
                //
                // The first cut marked interference at the `Reload` instead, and
                // it was a MISCOMPILE: two slots both live across a stretch with
                // neither reloaded inside it were never compared, so they merged
                // and overwrote each other. The 42-program suite and 185 unit
                // tests all passed; sqlite's output diverged. A corpus
                // corroborates, it does not discover.
                MInst::Spill { slot, .. } if idx[*slot as usize] != usize::MAX => {
                    let i = idx[*slot as usize];
                    for j in 0..m {
                        if j != i && get(&live, j) {
                            set(&mut inter[i], j);
                            set(&mut inter[j], i);
                        }
                    }
                    clr(&mut live, i);
                }
                MInst::Reload { slot, .. } if idx[*slot as usize] != usize::MAX => {
                    set(&mut live, idx[*slot as usize]);
                }
                _ => {}
            }
        }
    }

    // (4) greedy first-fit in slot-id order — deterministic, and the graph is
    //     one node per spilled web.
    let mut groups: Vec<(SlotId, Vec<u64>, BTreeSet<SlotId>)> = Vec::new();
    let mut moved = 0usize;
    for &s in cand.iter() {
        let i = idx[s as usize];
        let (sz, al) = (f.slots[s as usize].size, f.slots[s as usize].align);
        let mut placed = false;
        for (rep, mask, members) in groups.iter_mut() {
            if f.slots[*rep as usize].size != sz || f.slots[*rep as usize].align != al {
                continue;
            }
            if get(mask, i) {
                continue; // interferes with a member
            }
            for w in 0..words {
                mask[w] |= inter[i][w];
            }
            members.insert(s);
            if s != *rep {
                moved += 1;
            }
            placed = true;
            break;
        }
        if !placed {
            let mut mask = inter[i].clone();
            let _ = &mut mask;
            groups.push((s, mask, BTreeSet::from([s])));
        }
    }
    if moved == 0 {
        return false;
    }

    // (5) rename, and give the emptied slots no bytes. `frame::run` already
    //     treats a zero-size object as occupying nothing, so the frame shrinks
    //     without renumbering anything.
    let mut rep_of = vec![None; n];
    for (rep, _, members) in groups.iter() {
        for &mem in members.iter() {
            rep_of[mem as usize] = Some(*rep);
        }
    }
    for b in f.blocks.iter_mut() {
        for inst in b.insts.iter_mut() {
            match inst {
                MInst::Spill { slot, .. } | MInst::Reload { slot, .. } => {
                    if let Some(r) = rep_of[*slot as usize] {
                        *slot = r;
                    }
                }
                _ => {}
            }
        }
    }
    for (rep, _, members) in groups.iter() {
        for &mem in members.iter() {
            if mem != *rep {
                f.slots[mem as usize].size = 0;
            }
        }
    }
    true
}

/// `live_in(b)` recomputed from `live_out(b)`: the ordinary transfer function,
/// kept as a helper so the fixpoint above reads as the dataflow it is.
fn live_in_of(
    f: &MFunc,
    live_out: &[Vec<u64>],
    idx: &[usize],
    bi: usize,
    words: usize,
) -> Vec<u64> {
    let mut live = live_out[bi].clone();
    for inst in f.blocks[bi].insts.iter().rev() {
        match inst {
            MInst::Spill { slot, .. } if idx[*slot as usize] != usize::MAX => {
                let i = idx[*slot as usize];
                live[i / 64] &= !(1 << (i % 64));
            }
            MInst::Reload { slot, .. } if idx[*slot as usize] != usize::MAX => {
                let i = idx[*slot as usize];
                live[i / 64] |= 1 << (i % 64);
            }
            _ => {}
        }
    }
    let _ = words;
    live
}
