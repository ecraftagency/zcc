// Frame lowering (REARCH.md §8, post-allocation).
//
// Assign a byte offset to every stack object and materialize the prologue and
// epilogue. The prologue is NOT a special printed string: it is ordinary MIR —
// a `Spill` of each callee-saved register the allocator actually used, and a
// matching `Reload` before every return. That is what makes it provable: the
// interpreter executes those instructions like any others, so `⟦mir_p⟧` already
// accounts for register preservation, and there is no "the emitter also prints
// some saves" gap for a bug to live in.
//
// AAPCS64 §6.1.1: exactly the callee-saved registers this function writes are
// preserved — no more (`-fomit-frame-pointer` by construction: x29 is not a
// frame pointer here, every slot is addressed from sp).
use crate::mir::*;

pub fn run(f: &mut MFunc) {
    // x30 (LR) is destroyed by `bl`, so a function that calls anything must
    // preserve it. A leaf function must not: that is one store and one load per
    // call-free function, and sqlite has thousands of them.
    let calls = f
        .blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| matches!(i, MInst::Call { .. })));
    let mut save: Vec<PReg> = f.saved.iter().collect();
    if calls {
        save.push(isa::LR);
    }

    // A `StackAlloc` moves sp inside the body, so the frame stops being
    // sp-addressable: x29 becomes the frame pointer, and it must itself be
    // preserved (AAPCS64 §6.1.1 lists x29 as callee-saved).
    f.dyn_stack = f
        .blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| matches!(i, MInst::StackAlloc { .. })));
    if f.dyn_stack {
        save.push(isa::FP);
    }

    // One slot per preserved register, then the ordinary stack objects.
    let mut cs_slots = Vec::with_capacity(save.len());
    for _ in &save {
        cs_slots.push(f.new_slot(8, 8, SlotKind::Spill));
    }

    // AAPCS64 §6.4: the outgoing stack-argument area starts AT sp when the call
    // executes, so it is pinned to offset 0 — the lowest bytes of the frame.
    // That placement is what lets a dynamic frame keep working: `StackAlloc`
    // moves sp down and the area rides along with it, while every other object
    // stays at its fixed offset from x29.
    f.outgoing = f
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .filter_map(|i| match i {
            MInst::Call { stack_bytes, .. } => Some(*stack_bytes),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    f.outgoing = (f.outgoing + 15) & !15;

    // R4.4 — AN OBJECT NOTHING NAMES OCCUPIES NOTHING. `mem2reg`/`sroa` promote
    // a local into registers but leave its slot behind, so a leaf function whose
    // every local was promoted still carried a frame and paid `sub sp` + `add
    // sp` for it: §13n counted 289 such functions on sqlite, and f1's four
    // promoted locals still bought `sub sp, #32`.
    //
    // COMMUTING SQUARE: ⟦·⟧ reads and writes memory only through the addresses
    // instructions name. A slot no instruction names — no `AddrMode::Slot`, no
    // `Spill`/`Reload`, no `SlotAddr` — is never read and never written, so
    // giving it zero bytes changes no load, no store and no other object's
    // address beyond moving it down, which is a change of address for an object
    // whose address is likewise unobservable. `InArgs` and the outgoing area are
    // the ABI's, not this function's, and are untouched.
    let mut named = vec![false; f.slots.len()];
    for b in &f.blocks {
        for inst in &b.insts {
            let slot = match inst {
                MInst::Load { mem, .. } | MInst::Store { mem, .. } | MInst::Pair { mem, .. } => {
                    match mem {
                        AddrMode::Slot { slot, .. } => Some(*slot),
                        _ => None,
                    }
                }
                MInst::Spill { slot, .. } | MInst::Reload { slot, .. } => Some(*slot),
                MInst::SlotAddr { slot, .. } => Some(*slot),
                _ => None,
            };
            if let Some(k) = slot {
                named[k as usize] = true;
            }
        }
    }
    for (k, s) in f.slots.iter_mut().enumerate() {
        if s.kind == SlotKind::Local && !named[k] {
            s.size = 0;
            s.align = 1;
        }
    }

    // Lay the frame out low-to-high, honoring each object's alignment. The base
    // is sp after the prologue's single adjustment (§8: one frame adjust, by
    // construction — `StackAlloc` excepted, and that is the whole reason for the
    // frame pointer above).
    let mut at: u32 = f.outgoing;
    for s in f.slots.iter_mut() {
        if s.kind == SlotKind::InArgs {
            continue; // fixed below, once the frame's size is known
        }
        let a = s.align.max(1);
        at = (at + a - 1) / a * a;
        s.off = at as i32;
        // A zero-size object occupies nothing: nothing can read or write it, so
        // two of them may share an address (EXT(gcc) empty struct).
        at += s.size;
    }
    // AAPCS64 §6.2.2: sp is 16-byte aligned at every public interface.
    f.frame_size = (at + 15) & !15;
    // The caller's argument area begins exactly where this frame ends — that is
    // the definition of NSAA seen from the other side of the `bl`.
    for s in f.slots.iter_mut() {
        if s.kind == SlotKind::InArgs {
            s.off = f.frame_size as i32;
        }
    }
    f.laid_out = true;

    // `emit` prints the x29 save/restore itself: it is the one pair that cannot
    // be ordinary MIR, because it brackets the instant x29 STOPS being the
    // caller's value and starts being this frame's base. Its slot offset is
    // recorded so both sides name the same address.
    if f.dyn_stack {
        f.fp_slot = *cs_slots.last().unwrap();
    }
    let n_mir_saves = save.len() - f.dyn_stack as usize;

    // Record the saves as (slot, register, width) so `shrink_wrap` relocates
    // exactly these instructions and not a regalloc spill that merely holds a
    // callee-saved colour.
    f.cs_saves = save
        .iter()
        .zip(&cs_slots)
        .take(n_mir_saves)
        .map(|(p, slot)| {
            (
                *slot,
                *p,
                if p.class == Class::Fpr { Width::D } else { Width::W64 },
            )
        })
        .collect();

    let entry = f.entry as usize;
    let mut prologue: Vec<MInst> = Vec::with_capacity(save.len());
    for (p, slot) in save.iter().zip(&cs_slots).take(n_mir_saves) {
        prologue.push(MInst::Spill {
            slot: *slot,
            src: Reg::P(*p),
            w: if p.class == Class::Fpr {
                Width::D
            } else {
                Width::W64
            },
        });
    }
    let mut insts = std::mem::take(&mut f.blocks[entry].insts);
    prologue.append(&mut insts);
    f.blocks[entry].insts = prologue;

    for b in f.blocks.iter_mut() {
        if !matches!(b.term, MTerm::Ret) {
            continue;
        }
        for (p, slot) in save.iter().zip(&cs_slots).take(n_mir_saves) {
            b.insts.push(MInst::Reload {
                slot: *slot,
                dst: Reg::P(*p),
                w: if p.class == Class::Fpr {
                    Width::D
                } else {
                    Width::W64
                },
            });
        }
    }
}

/// R4.4 — ONE EPILOGUE PER SHAPE, NOT ONE PER RETURN PATH.
///
/// `run` gives every `Ret` block its own copy of the callee-saved reloads, and
/// `emit` adds `add sp` and `ret` to each: sqlite paid **3,815 `ret` against
/// gcc's 317** and 3,381 `add sp`. Those tails are IDENTICAL — they name
/// physical registers and fixed slots — so all but one copy of each distinct
/// tail is duplication.
///
/// COMMUTING SQUARE. The return VALUE is already in its ABI register when a
/// `Ret` block is reached (`isel` puts it there), so a shared epilogue needs no
/// parameter and observes nothing about which path reached it. A block that
/// jumps to it instead of running its own tail executes the same reloads, in the
/// same order, and returns the same registers.
///
/// RUNS AFTER `shrink_wrap`, and that ordering is the whole reason this is a
/// separate pass. Shrink-wrapping needs the region below its save point to be a
/// SINK — no successor outside it — and a shared epilogue is exactly such a
/// successor, so merging first silences it (battery
/// `shrink_wrap_moves_saves_off_the_fast_path`). Afterwards the two compose:
/// shrink-wrapping has left some returns with the reloads and some without, and
/// grouping by the EXACT tail keeps those apart.
///
/// WORTH IT ONLY WHEN THE TAIL OUTWEIGHS THE BRANCH THAT REPLACES IT. A
/// redirected path emits `[reloads] [add sp?] ret` and afterwards emits one
/// `b`, so it saves `reloads + (frame ? 1 : 0)` instructions — the `ret` is
/// traded for the branch one-for-one. One reload is therefore already worth it;
/// a leaf with no frame and no saves is not, and keeps its bare `ret` per path.
/// (`layout` may then make the branch a fall-through, which is free.)
pub fn merge_epilogues(f: &mut MFunc) {
    let cs: std::collections::HashSet<SlotId> = f.cs_saves.iter().map(|(s, _, _)| *s).collect();
    if cs.is_empty() && f.frame_size == 0 {
        return;
    }
    // the maximal trailing run of callee-saved reloads — the tail this block
    // would emit before `ret`
    let tail_of = |b: &MBlock| -> Vec<MInst> {
        let mut n = 0;
        for i in b.insts.iter().rev() {
            match i {
                MInst::Reload { slot, .. } if cs.contains(slot) => n += 1,
                _ => break,
            }
        }
        b.insts[b.insts.len() - n..].to_vec()
    };
    let mut groups: Vec<(Vec<MInst>, Vec<usize>)> = Vec::new();
    for b in 0..f.blocks.len() {
        if !matches!(f.blocks[b].term, MTerm::Ret) {
            continue;
        }
        let t = tail_of(&f.blocks[b]);
        match groups.iter_mut().find(|(k, _)| same_tail(k, &t)) {
            Some((_, bs)) => bs.push(b),
            None => groups.push((t, vec![b])),
        }
    }
    for (tail, bs) in groups {
        if bs.len() < 2 || tail.len() + ((f.frame_size > 0) as usize) < 1 {
            continue;
        }
        let ep = f.new_block();
        f.blocks[ep as usize].insts = tail.clone();
        f.blocks[ep as usize].term = MTerm::Ret;
        f.blocks[ep as usize].weight = 1;
        for b in bs {
            let n = f.blocks[b].insts.len() - tail.len();
            f.blocks[b].insts.truncate(n);
            f.blocks[b].term = MTerm::B(MTarget { block: ep, args: Vec::new() });
        }
    }
}

/// Two reload tails are the same when they name the same slots, registers and
/// widths in the same order.
fn same_tail(a: &[MInst], b: &[MInst]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (
                MInst::Reload { slot: s1, dst: d1, w: w1 },
                MInst::Reload { slot: s2, dst: d2, w: w2 },
            ) => s1 == s2 && d1 == d2 && w1 == w2,
            _ => false,
        })
}
