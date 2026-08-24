// ldst_pair (REARCH.md §8) — two accesses to consecutive addresses become one.
//
// A64 has `ldp`/`stp` (DDI 0487 C6.2.130), and the code that uses them most is
// not user code: the prologue saves the callee-saved set, the epilogue restores
// it, and the spiller writes runs of adjacent slots. Every such pair is two
// instructions the machine could have done in one.
//
// Runs LAST, after `frame` and `legalize`, for two reasons: only then does a
// stack object have a NUMBER (so "consecutive" is decidable), and only then is
// every address a `BaseImm` whose displacement is final.
//
// COMMUTING SQUARE. Two accesses fuse only when they are ADJACENT — nothing at
// all between them — so no instruction can observe the intermediate state, and
// memory ends the same either way. Three further conditions are checked rather
// than assumed: the pair's displacement must fit the signed-7 SCALED field the
// paired form uses (a range a full one-register access does not have); an `ldp`
// may not name one destination twice (C6.2.130 makes that UNPREDICTABLE); and a
// load must not overwrite the base register it is still addressing through.
use crate::mir::*;

pub fn run(f: &mut MFunc) {
    let offs: Vec<i32> = f.slots.iter().map(|s| s.off).collect();
    for b in f.blocks.iter_mut() {
        let insts = std::mem::take(&mut b.insts);
        let mut out: Vec<MInst> = Vec::with_capacity(insts.len());
        let mut i = 0;
        while i < insts.len() {
            if i + 1 < insts.len() {
                if let Some(p) = fuse(&offs, &insts[i], &insts[i + 1]) {
                    out.push(p);
                    i += 2;
                    continue;
                }
            }
            out.push(insts[i].clone());
            i += 1;
        }
        b.insts = out;
    }
}

/// The pair form's element width for an access, when it has one. `ldp`/`stp`
/// exist for 32- and 64-bit integers and for S/D/Q floats — but NOT for the
/// byte and halfword forms, and not for a sign-extending load other than
/// `ldpsw`, which this does not build.
fn pair_width(op: MemOp) -> Option<Width> {
    match op {
        MemOp::W => Some(Width::W32),
        MemOp::X => Some(Width::W64),
        MemOp::S => Some(Width::S),
        MemOp::D => Some(Width::D),
        MemOp::Q => Some(Width::Q),
        _ => None,
    }
}

/// DDI 0487 C6.2.130: the paired forms take a SCALED signed 7-bit offset, so the
/// displacement must be a multiple of the element size and lie in ±64 elements.
fn pair_off_ok(off: i32, size: u32) -> bool {
    off % size as i32 == 0 && (-64..=63).contains(&(off / size as i32))
}

/// Which register a slot is addressed from is decided once per function
/// (`frame`), so two `Slot` operands are consecutive exactly when their resolved
/// offsets are. `Base::Slot(s)` keeps the slot id so the pair can be rebuilt as
/// a `Slot` operand and let `emit` resolve it as it does every other one.
#[derive(PartialEq, Clone, Copy)]
enum Base {
    Reg(Reg),
    Slot(SlotId),
}

fn fuse(offs: &[i32], x: &MInst, y: &MInst) -> Option<MInst> {
    // (load?, element width, register, base, resolved offset, volatile)
    let part = |i: &MInst| -> Option<(bool, Width, Reg, Base, i32, bool)> {
        match i {
            MInst::Load { op, dst, mem: AddrMode::BaseImm { base, off }, vol } => {
                Some((true, pair_width(*op)?, *dst, Base::Reg(*base), *off, *vol))
            }
            MInst::Store { op, src, mem: AddrMode::BaseImm { base, off }, vol } => {
                Some((false, pair_width(*op)?, *src, Base::Reg(*base), *off, *vol))
            }
            MInst::Load { op, dst, mem: AddrMode::Slot { slot, off }, vol } => Some((
                true,
                pair_width(*op)?,
                *dst,
                Base::Slot(*slot),
                offs[*slot as usize] + off,
                *vol,
            )),
            MInst::Store { op, src, mem: AddrMode::Slot { slot, off }, vol } => Some((
                false,
                pair_width(*op)?,
                *src,
                Base::Slot(*slot),
                offs[*slot as usize] + off,
                *vol,
            )),
            // the spiller's own pseudos: the same access, already scheduled next
            // to its neighbour by the frame layout
            MInst::Spill { slot, src, w } => {
                Some((false, *w, *src, Base::Slot(*slot), offs[*slot as usize], false))
            }
            MInst::Reload { slot, dst, w } => {
                Some((true, *w, *dst, Base::Slot(*slot), offs[*slot as usize], false))
            }
            _ => None,
        }
    };
    let (l1, o1, r1, b1, f1, v1) = part(x)?;
    let (l2, o2, r2, b2, f2, v2) = part(y)?;
    // C99 6.7.3: a volatile access is performed exactly as written.
    // Two `Slot` operands share a base by construction; a register base has to
    // be the same register.
    let same_base = match (b1, b2) {
        (Base::Reg(p), Base::Reg(q)) => p == q,
        (Base::Slot(_), Base::Slot(_)) => true,
        _ => false,
    };
    if v1 || v2 || l1 != l2 || o1 != o2 || !same_base {
        return None;
    }
    let w = o1;
    let size = w.bytes() as i32;
    // whichever comes first in memory is the pair's first register
    let (first, second, low) = if f2 == f1 + size {
        (r1, r2, x)
    } else if f1 == f2 + size {
        (r2, r1, y)
    } else {
        return None;
    };
    let mem = match part(low)?.3 {
        Base::Reg(base) => {
            let off = part(low)?.4;
            if !pair_off_ok(off, w.bytes()) {
                return None;
            }
            AddrMode::BaseImm { base, off }
        }
        Base::Slot(slot) => {
            if !pair_off_ok(part(low)?.4, w.bytes()) {
                return None;
            }
            let off = match low {
                MInst::Load { mem: AddrMode::Slot { off, .. }, .. }
                | MInst::Store { mem: AddrMode::Slot { off, .. }, .. } => *off,
                _ => 0,
            };
            AddrMode::Slot { slot, off }
        }
    };
    if l1 {
        // a load may not repeat a destination, nor clobber the base it reads
        if first == second {
            return None;
        }
        if let Base::Reg(base) = b1 {
            if first == base || second == base {
                return None;
            }
        }
    }
    Some(MInst::Pair { w, load: l1, a: first, b: second, mem })
}
