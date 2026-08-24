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
        at += s.size.max(1);
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
