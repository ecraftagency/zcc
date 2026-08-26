// Block layout (REARCH.md §8, post-allocation).
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
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
//
// It opens by THREADING empty blocks, which is the one thing here that does
// change the edge set. An empty block is not an accident: critical edges are
// split before allocation precisely so SSA destruction has somewhere to put a
// parallel copy, and when coalescing succeeds in giving both sides the same
// register there is no copy left to put. What remains is a block containing
// nothing but a branch to a branch. Removing it is not cosmetic — it is the
// difference between one branch per loop iteration and two, and it is the second
// half of the reason loop rotation was measured worthless (§13e).
use crate::cfg::{Cfg, LoopForest, DomTree};
use crate::mir::*;

/// THEORY A6b  SQUARE layout_preserves_every_edge — order changes, edges do not
pub fn run(f: &mut MFunc) {
    thread_empty_blocks(f);
    duplicate_latch(f);
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
                // R4.15: the frame adjust is one `sub`/`add` when the immediate
                // fits, else a `movz/movk` chain into the scratch then the op.
                MInst::SpAdj { delta } => {
                    let n = (*delta).unsigned_abs() as i64;
                    if isa::add_imm(n).is_some() {
                        1
                    } else {
                        isa::mov_chain(n, true).len() + 1
                    }
                }
                _ => 1,
            })
            .sum();
        body + match blk.term {
            MTerm::Switch { .. } => 8,
            _ => 3,
        }
    };
    /// THEORY II-5 — the conditional-branch imm19 reach (DDI 0487 C6.2.25)
    // half the true reach, so the instructions relaxation ADDS cannot undo it
    const NEAR: usize = 4096;
    /// THEORY II-5 — the unconditional-branch imm26 reach
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

/// Redirect every edge that lands on a block containing nothing but `b LABEL`
/// straight to that label.
///
/// COMMUTING SQUARE: an empty block executes no instruction, so a trace through
/// it and the trace that skips it visit the same states in the same order. The
/// block is left in place and simply becomes unreachable; `f.order` is rebuilt
/// from the reachable set immediately afterwards, so it is never printed.
///
/// Three blocks are never threaded THROUGH, each for a reason about identity
/// rather than about cost: the entry (the ABI materializes parameters there), a
/// block carrying a C `goto` label or named by a computed goto (`BrReg` lists
/// the address-taken set, and an address must keep pointing at something), and a
/// block with parameters — after SSA destruction there should be none, and one
/// that survives is carrying an edge value that this walk would drop.
/// TAIL DUPLICATION of a loop latch — one taken branch per iteration.
///
/// THE MEASUREMENT. d1_switch's switch arms each end `b .Lwork_3`, and the block
/// they jump to is the whole loop tail: bump the counter, test it, branch back.
/// So every iteration pays TWO taken branches to reach the top — the arm's jump
/// to the tail, and the tail's jump to the header. Copying the tail into each
/// arm removes the first:
///
///     gcc -O1   10 ms     zcc before   12 ms     zcc with this   10 ms
///
/// Hand-validated in zcc's own `.s` before a line of this was written (the §13q
/// method), three passes, output identical at 8000006000000. d1 was the last
/// program above 1.1x and it had already refused FIVE other hypotheses —
/// including gcc's own dispatch shape transcribed verbatim, which measured
/// SLOWER. The time model is why this one was reachable: it reported recurrence
/// 1 for this loop, which ruled out every latency explanation and left the
/// branch count as the only thing it could be.
///
/// WHAT IS COPIED, and it is a SHAPE rather than a size. Only a block that is
/// exactly a loop tail — its terminator is the loop's back edge, and it holds
/// nothing but the instructions that feed that terminator — is duplicated, and
/// only into predecessors that reach it by an unconditional branch. There is no
/// threshold to tune and none is invented: `provenance.sh` rejected an earlier
/// constant in `inline.rs` this same session for exactly that reason.
///
/// SQUARE. Duplication preserves meaning trivially — each predecessor now
/// executes the same instructions in the same order, reaching the same
/// successors — provided the copy is EXACT and every edge out of the original is
/// reproduced. Running after `regalloc`, the block is already physical, so there
/// are no names to rewrite; that is why this sits in `layout` and not above it.
fn duplicate_latch(f: &mut MFunc) {
    let n = f.blocks.len();
    // THE BACK-EDGE FENCE. The first cut tested only "conditional terminator,
    // two or more unconditional predecessors", which describes ANY join — and
    // duplicating a join that reloads a spilled value moves the reload above its
    // store on one path. `regalloc::verify` caught it immediately: "reload of
    // unstored slot 31". A loop TAIL is a join whose own terminator branches
    // BACK to a block that dominates it, and that is what has to be tested.
    let cfg = crate::mir::verify::cfg(f);
    let dt = DomTree::new(&cfg, f.entry);
    let mut preds: Vec<Vec<MBlockId>> = vec![Vec::new(); n];
    for (i, b) in f.blocks.iter().enumerate() {
        for t in b.term.succs() {
            preds[t as usize].push(i as MBlockId);
        }
    }
    // Candidates: a block whose terminator branches BACK to a block that
    // dominates it — the loop tail — reached by at least two unconditional
    // branches. One predecessor is not a duplication, it is a merge.
    let mut plan: Vec<(MBlockId, Vec<MBlockId>)> = Vec::new();
    for (i, b) in f.blocks.iter().enumerate() {
        if !b.labels.is_empty() || !b.params.is_empty() {
            continue;
        }
        // The tail's own terminator must be the loop's conditional BACK EDGE:
        // one of its successors dominates it.
        if !matches!(b.term, MTerm::Bcc(..) | MTerm::Cbz { .. } | MTerm::Tb { .. }) {
            continue;
        }
        if !b.term.succs().iter().any(|&s| dt.dominates(s, i as MBlockId)) {
            continue;
        }
        let jumpers: Vec<MBlockId> = preds[i]
            .iter()
            .copied()
            .filter(|&p| {
                p != i as MBlockId
                    && matches!(&f.blocks[p as usize].term, MTerm::B(t)
                        if t.block == i as MBlockId && t.args.is_empty())
            })
            .collect();
        // THREE, not two, and the number is not a tuning knob — it is the
        // difference between a SWITCH and an if-else. An ordinary two-armed
        // join reaches its loop tail from both arms, and duplicating there
        // fired on nearly every loop in the suite: INSN 1.0240 -> 1.3668, 32 of
        // 35 programs above 1.1x, sqlite +3,906. That is 33% of size for 2% of
        // time, the same trade R4.14 refused at 16-for-7. A tail reached by
        // three or more unconditional branches is a multi-way dispatch, which is
        // where the second branch per iteration actually repeats.
        if jumpers.len() < 3 {
            continue;
        }
        plan.push((i as MBlockId, jumpers));
    }
    for (tail, jumpers) in plan {
        let body = f.blocks[tail as usize].insts.clone();
        let term = f.blocks[tail as usize].term.clone();
        for p in jumpers {
            f.blocks[p as usize].insts.extend(body.iter().cloned());
            f.blocks[p as usize].term = term.clone();
        }
    }
}

fn thread_empty_blocks(f: &mut MFunc) {
    let n = f.blocks.len();
    let mut pinned = vec![false; n];
    pinned[f.entry as usize] = true;
    for b in &f.blocks {
        if let MTerm::BrReg(_, bs) = &b.term {
            for &t in bs {
                pinned[t as usize] = true;
            }
        }
    }
    // Where each block forwards to, resolved through chains of empty blocks.
    let mut to: Vec<Option<MBlockId>> = vec![None; n];
    for (i, b) in f.blocks.iter().enumerate() {
        if pinned[i] || !b.labels.is_empty() || !b.params.is_empty() || !b.insts.is_empty() {
            continue;
        }
        if let MTerm::B(t) = &b.term {
            if t.block != i as MBlockId && t.args.is_empty() {
                to[i] = Some(t.block);
            }
        }
    }
    let resolve = |mut x: MBlockId, to: &Vec<Option<MBlockId>>| -> MBlockId {
        // A cycle of empty blocks is an infinite loop with no body; the bound
        // stops the walk rather than the compiler.
        for _ in 0..n {
            match to[x as usize] {
                Some(y) if y != x => x = y,
                _ => break,
            }
        }
        x
    };
    if to.iter().all(|x| x.is_none()) {
        return;
    }
    let snapshot = to.clone();
    for b in f.blocks.iter_mut() {
        for t in b.term.targets_mut() {
            t.block = resolve(t.block, &snapshot);
        }
    }
}
