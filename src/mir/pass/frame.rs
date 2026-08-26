// Frame lowering (REARCH.md §8, post-allocation).
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
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

/// THEORY A6b  SQUARE callee_saved_preservation_is_realized_by_the_prologue — the ABI promise, in instructions
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
    //
    // SPILLS FIRST, AND THAT ORDER IS WORTH AN INSTRUCTION EACH (§13o). `ldp`
    // and `stp` take a SCALED SIGNED 7-BIT displacement (DDI 0487 C6.2.130), so
    // a paired 64-bit access reaches only 504 bytes from the base — one eighth
    // of what a single access reaches. Slots were laid out in creation order,
    // which put the C locals first and the callee-saved saves and allocator
    // spills ABOVE them, so in any function with a kilobyte of locals the
    // prologue, the epilogue and every spill run sat out of the paired form's
    // range. Measured on sqlite: of 2,598 adjacent-or-near frame accesses that
    // could pair, **1,903 were refused for the offset alone**.
    //
    // The objects that are accessed in PAIRED RUNS therefore go where the paired
    // form can reach: the outgoing-argument area keeps offset 0 (the ABI pins
    // it), then every `Spill` — callee-saved saves and allocator spills — then
    // the locals, whose accesses are ordinary singles with a 32× larger reach.
    // Nothing else changes: an offset is an offset, and `emit` resolves each
    // slot through the same path either way.
    let mut at: u32 = f.outgoing;
    let place_one = |s: &mut StackSlot, at: &mut u32| {
        let a = s.align.max(1);
        *at = (*at + a - 1) / a * a;
        s.off = *at as i32;
        // A zero-size object occupies nothing: nothing can read or write it,
        // so two of them may share an address (EXT(gcc) empty struct).
        *at += s.size;
    };
    // R4.15 — THE CALLEE-SAVE PAIR AT OFFSET 0, SO `frame_fold` CAN FOLD THE
    // ADJUST INTO IT. A pre-index `stp x19,x20,[sp,#-N]!` stores at the NEW sp,
    // i.e. frame offset 0, so the pair it folds must live there. When the fold can
    // possibly fire — an ordinary (sp-addressed) frame whose bottom is free of the
    // ABI's outgoing area — the callee-save slots go first; otherwise the layout
    // is unchanged. The saves stay a contiguous run either way, so R4.8's pairing
    // is untouched; only their base offset moves, and `emit` resolves it the same.
    let cs_first = !f.dyn_stack && f.outgoing == 0;
    if cs_first {
        for &sl in &cs_slots {
            place_one(&mut f.slots[sl as usize], &mut at);
        }
    }
    let placed: std::collections::HashSet<SlotId> = if cs_first {
        cs_slots.iter().copied().collect()
    } else {
        std::collections::HashSet::new()
    };
    let mut place = |slots: &mut Vec<StackSlot>, at: &mut u32, kind: SlotKind| {
        for (k, s) in slots.iter_mut().enumerate() {
            if s.kind != kind || placed.contains(&(k as SlotId)) {
                continue;
            }
            place_one(s, at);
        }
    };
    place(&mut f.slots, &mut at, SlotKind::Spill);
    place(&mut f.slots, &mut at, SlotKind::Local);
    place(&mut f.slots, &mut at, SlotKind::OutArgs);
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

/// Drop a spill whose slot is never reloaded.
///
/// THE MEASUREMENT. sqlite holds **1,042 frame slots that are STORED AND NEVER
/// LOADED — 1,090 dead stores, 0.63% of the whole program** and 5.4% of its size
/// gap against gcc -O1. They exist because the spiller places a store at the
/// value's DEFINITION whether or not any path later reloads it (REARCH §13o):
/// the value stays in its register, the slot is written for nothing, and in
/// `sqlite3VdbeExec` alone 102 slots are written exactly once and read never.
///
/// SQUARE. Removing a store to a location nothing ever reads cannot change what
/// the program computes — the slot's contents are unobservable. The fence is
/// what makes "nothing ever reads it" true: the slot must be touched ONLY by
/// `Spill` and `Reload`, never by a `Load`/`Store` naming it as an address,
/// because a local whose address escaped can be read through a pointer this pass
/// cannot see. A slot with even one reload keeps all of its spills — this pass
/// does no path reasoning, only whole-function counting.
pub fn drop_dead_spills(f: &mut MFunc) -> usize {
    use std::collections::HashSet;
    let mut reloaded: HashSet<SlotId> = HashSet::new();
    let mut addressed: HashSet<SlotId> = HashSet::new();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                MInst::Reload { slot, .. } => {
                    reloaded.insert(*slot);
                }
                MInst::Load { mem, .. } | MInst::Store { mem, .. } => {
                    if let AddrMode::Slot { slot, .. } = mem {
                        addressed.insert(*slot);
                    }
                }
                MInst::Pair { mem, .. } => {
                    if let AddrMode::Slot { slot, .. } = mem {
                        addressed.insert(*slot);
                    }
                }
                _ => {}
            }
        }
    }
    let mut n = 0usize;
    for b in f.blocks.iter_mut() {
        b.insts.retain(|i| match i {
            MInst::Spill { slot, .. } => {
                let dead = !reloaded.contains(slot) && !addressed.contains(slot);
                if dead {
                    n += 1;
                }
                !dead
            }
            _ => true,
        });
    }
    n
}
