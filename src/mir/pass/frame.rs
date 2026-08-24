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

    // One slot per preserved register, then the ordinary stack objects.
    let mut cs_slots = Vec::with_capacity(save.len());
    for _ in &save {
        cs_slots.push(f.new_slot(8, 8, SlotKind::Spill));
    }

    // Lay the frame out low-to-high, honoring each object's alignment. The base
    // is sp after the prologue's single adjustment (§8: one frame adjust, by
    // construction — there is nowhere else that moves sp).
    let mut at: u32 = 0;
    for s in f.slots.iter_mut() {
        let a = s.align.max(1);
        at = (at + a - 1) / a * a;
        s.off = at as i32;
        at += s.size.max(1);
    }
    // AAPCS64 §6.2.2: sp is 16-byte aligned at every public interface.
    f.frame_size = (at + 15) & !15;
    f.laid_out = true;

    let entry = f.entry as usize;
    let mut prologue: Vec<MInst> = Vec::with_capacity(save.len());
    for (p, slot) in save.iter().zip(&cs_slots) {
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
        for (p, slot) in save.iter().zip(&cs_slots) {
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
