// frame_fold (MECHANISM.md §G8, R4.15) — the frame adjust, folded into the save
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
// pair.
//
// The single `sub sp`/`add sp` that brackets an ordinary frame is 3,300
// instructions on sqlite that DDI 0487 C6.2.130's pre/post-indexed `stp`/`ldp`
// fold away for free: `stp x19,x20,[sp,#-N]!` allocates the frame AND saves the
// first pair at the new sp; `ldp x19,x20,[sp],#N` restores the last pair AND
// frees it. §13o forbade doing this in `emit` (that would be `emit` deciding —
// Article B), so `frame` already left the adjust as a real `SpAdj` decision to
// take HERE, after `ldstp` has formed the pairs and `shrink_wrap`/`merge` have
// settled where the saves live.
//
// WHAT IS FOLDABLE, and why each guard:
//   * an ordinary frame — `!dyn_stack` (a dynamic frame's adjust brackets the
//     x29 hand-over and stays printed) and `outgoing == 0` (the pre-index stores
//     at the new sp, i.e. frame offset 0, which the ABI's outgoing area owns when
//     it is non-empty). `frame` places the callee-save slots at offset 0 under
//     exactly this condition, so the offset-0 access below is the first save.
//   * a frame in the writeback's reach — the pair form takes a signed-7 scaled-by-8
//     displacement (N ≤ 512), the single form a signed-9 unscaled one (N ≤ 256).
//   * the offset-0 save present where the writeback needs it — at the HEAD of the
//     prologue (it allocates before any other frame access) and at the TAIL of an
//     epilogue (it frees after the last one). The callee-save reloads name
//     distinct registers and distinct slots, so the offset-0 one may be commuted
//     to the tail with no effect — which is what lets the post-index free last.
//
// COMMUTING SQUARE `⟦before⟧ = ⟦after⟧`. The adjust is a no-op in `⟦·⟧` (the frame
// is established at call entry and every slot addressed absolutely — see
// `MInst::SpAdj`), so removing it changes nothing; the writeback pair addresses
// frame offset 0, which is exactly the address the `Slot { off: 0 }` save it
// replaces named, and `AddrMode::FrameWb` resolves to that same address with the
// sp writeback left as the no-op the adjust also was. Reordering the reloads is
// the identity for the reasons above. Battery: `frame_fold_*` in `tests.rs`.
use crate::mir::*;

/// THEORY A7b  SQUARE frame_fold_folds_the_adjust_into_the_save_pair — DDI 0487 C6.2.130
pub fn run(f: &mut MFunc) {
    if f.dyn_stack || f.frame_size == 0 {
        return;
    }
    let n = f.frame_size as i32;
    let eligible = f.outgoing == 0;

    // ---- prologue: allocate with the first save ----
    let entry = f.entry as usize;
    if !(eligible && fold_prologue(f, entry, n)) {
        f.blocks[entry].insts.insert(0, MInst::SpAdj { delta: -n });
    }

    // ---- epilogue: free with the last restore, on every return ----
    for b in 0..f.blocks.len() {
        if !matches!(f.blocks[b].term, MTerm::Ret) {
            continue;
        }
        if !(eligible && fold_epilogue(f, b, n)) {
            f.blocks[b].insts.push(MInst::SpAdj { delta: n });
        }
    }
}

/// The resolved frame offset a callee-save store/restore names, when the
/// instruction is exactly such a save at width 8 — the only width a callee-saved
/// register is preserved at (`frame` records `W64`/`D`).
fn save_off(f: &MFunc, i: &MInst) -> Option<i32> {
    match i {
        MInst::Pair { w, mem: AddrMode::Slot { slot, off }, .. } if w.bytes() == 8 => {
            Some(f.slots[*slot as usize].off + off)
        }
        MInst::Spill { slot, w, .. } | MInst::Reload { slot, w, .. } if w.bytes() == 8 => {
            Some(f.slots[*slot as usize].off)
        }
        _ => None,
    }
}

/// `MemOp` for a single 8-byte save: an integer or an FP register.
fn wide_op(w: Width) -> Option<MemOp> {
    match w {
        Width::W64 => Some(MemOp::X),
        Width::D => Some(MemOp::D),
        _ => None,
    }
}

/// Rewrite one save instruction to carry the frame writeback `delta`.
fn to_writeback(i: &MInst, delta: i32) -> Option<MInst> {
    match i {
        MInst::Pair { w, load, a, b, mem: AddrMode::Slot { slot, off } } if *off == 0 => {
            Some(MInst::Pair {
                w: *w,
                load: *load,
                a: *a,
                b: *b,
                mem: AddrMode::FrameWb { slot: *slot, delta },
            })
        }
        MInst::Spill { slot, src, w } => Some(MInst::Store {
            op: wide_op(*w)?,
            src: *src,
            mem: AddrMode::FrameWb { slot: *slot, delta },
            vol: false,
        }),
        MInst::Reload { slot, dst, w } => Some(MInst::Load {
            op: wide_op(*w)?,
            dst: *dst,
            mem: AddrMode::FrameWb { slot: *slot, delta },
            vol: false,
        }),
        _ => None,
    }
}

/// The prologue's first instruction is the offset-0 save exactly when the entry
/// block still leads with it (`shrink_wrap` did not carry the saves away). The
/// pair reaches N ≤ 512, the single N ≤ 256.
fn fold_prologue(f: &mut MFunc, entry: usize, n: i32) -> bool {
    let first = match f.blocks[entry].insts.first() {
        Some(i) => i,
        None => return false,
    };
    if save_off(f, first) != Some(0) || !reach_ok(first, n) {
        return false;
    }
    match to_writeback(first, -n) {
        Some(w) => {
            f.blocks[entry].insts[0] = w;
            true
        }
        None => false,
    }
}

/// The epilogue frees with its LAST access, so the offset-0 restore is commuted
/// to the tail (an identity — the reloads name distinct registers and slots) and
/// made a post-index. Only the trailing run of callee-save reloads is searched, so
/// a fast-path return `shrink_wrap` stripped is left to take a plain `SpAdj`.
fn fold_epilogue(f: &mut MFunc, b: usize, n: i32) -> bool {
    // the maximal trailing run of callee-save reloads
    let insts = &f.blocks[b].insts;
    let mut start = insts.len();
    while start > 0 && is_cs_reload(f, &f.blocks[b].insts[start - 1]) {
        start -= 1;
    }
    let run = start..f.blocks[b].insts.len();
    if run.is_empty() {
        return false;
    }
    let at = match run
        .clone()
        .find(|&k| save_off(f, &f.blocks[b].insts[k]) == Some(0))
    {
        Some(k) => k,
        None => return false,
    };
    if !reach_ok(&f.blocks[b].insts[at], n) {
        return false;
    }
    let w = match to_writeback(&f.blocks[b].insts[at], n) {
        Some(w) => w,
        None => return false,
    };
    f.blocks[b].insts.remove(at);
    f.blocks[b].insts.push(w);
    true
}

/// A pure callee-save reload — a `Reload`, or a `Pair` load addressing a slot
/// (the paired form `ldstp` produced from two such reloads).
fn is_cs_reload(_f: &MFunc, i: &MInst) -> bool {
    matches!(
        i,
        MInst::Reload { .. } | MInst::Pair { load: true, mem: AddrMode::Slot { .. }, .. }
    )
}

/// The writeback immediate must encode BOTH as the prologue's negative pre-index
/// and the epilogue's positive post-index. The signed range is symmetric-minus-one
/// (`[-256,255]` unscaled for a single, `[-64,63]×8` for a pair), and the POSITIVE
/// end is the binding one: a single post-index `ldr x30,[sp],#N` needs N ≤ 255, a
/// pair post-index `ldp …,[sp],#N` needs N/8 ≤ 63, i.e. N ≤ 504. Taking the tighter
/// (positive) bound for both directions keeps the two sites in lockstep and was the
/// `ldr x30,[sp],#256` assembler reject (torture 980205/stdarg-1). N is 16-aligned.
fn reach_ok(i: &MInst, n: i32) -> bool {
    match i {
        MInst::Pair { .. } => n <= 504,
        _ => n <= 255,
    }
}
