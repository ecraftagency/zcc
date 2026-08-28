// ldst_pair (MECHANISM.md §G8) — two accesses to consecutive addresses become one.
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
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

/// MEASURED M5 — the pairing window saturates within ten instructions
/// How far ahead a partner is looked for. §13o measured the distribution on
/// sqlite: of the frame accesses that could pair, 433 sit ADJACENT and 1,374
/// more sit two to ten instructions away — the second access is simply not next
/// to the first, because nothing ever scheduled them together. Ten covers the
/// measured tail; beyond it the count is flat.
const WINDOW: usize = 10;

/// THEORY A6b  SQUARE a_pair_replaces_two_adjacent_accesses — nothing observes the intermediate state
pub fn run(f: &mut MFunc) {
    let offs: Vec<i32> = f.slots.iter().map(|s| s.off).collect();
    for b in f.blocks.iter_mut() {
        let insts = std::mem::take(&mut b.insts);
        let mut taken = vec![false; insts.len()];
        let mut out: Vec<MInst> = Vec::with_capacity(insts.len());
        for i in 0..insts.len() {
            if taken[i] {
                continue;
            }
            let mut made = None;
            let hi = (i + WINDOW).min(insts.len() - 1);
            for j in (i + 1)..=hi {
                if taken[j] {
                    continue;
                }
                if let Some(p) = fuse(&offs, &insts[i], &insts[j]) {
                    if hoistable(&offs, &insts[(i + 1)..j], &insts[j]) {
                        made = Some((j, p));
                        break;
                    }
                }
            }
            match made {
                Some((j, p)) => {
                    taken[j] = true;
                    out.push(p);
                    PAIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                None => {
                    residual(&offs, &insts, &taken, i);
                    out.push(insts[i].clone());
                }
            }
        }
        b.insts = out;
    }
}

/// LAW 4 — the residual of this theorem, measured rather than assumed.
///
/// An access left unpaired is one of two things: a FUNDAMENTAL limit (no
/// partner exists, or the paired form cannot encode the displacement) or a
/// CONVENIENCE truncation (a partner exists and something about how this pass
/// looks refused it). The row is exhausted only when the second set is empty,
/// so it is counted rather than argued about. `ZCC_LDSTP=1` prints the split.
/// MEASURED M15 — the instrument, not a resource constant
static PAIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// MEASURED M15 — the instrument, not a resource constant
static NO_PARTNER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// MEASURED M15 — the instrument, not a resource constant
static OUT_OF_WINDOW: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// MEASURED M15 — the instrument, not a resource constant
static BLOCKED_MOTION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// MEASURED M15 — the instrument, not a resource constant
static BACKWARD_ONLY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// MEASURED M15 — the instrument, not a resource constant
static LAYOUT_COULD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Same direction and same transfer width — the two things a paired form needs
/// beyond adjacency, so "a layout could have paired these" is not claimed for a
/// load next to a store.
fn same_shape(a: &MInst, b: &MInst) -> bool {
    let shape = |m: &MInst| match m {
        MInst::Load { op, .. } => Some((true, op.bytes())),
        MInst::Store { op, .. } => Some((false, op.bytes())),
        MInst::Reload { w, .. } => Some((true, w.bytes())),
        MInst::Spill { w, .. } => Some((false, w.bytes())),
        _ => None,
    };
    match (shape(a), shape(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn wanted() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var_os("ZCC_LDSTP").is_some())
}

/// Why did `insts[i]` not pair? Looks far past `WINDOW` on purpose: the point is
/// to separate "no partner exists" from "a partner exists and this pass did not
/// reach it".
fn residual(offs: &[i32], insts: &[MInst], taken: &[bool], i: usize) {
    if !wanted() || !matches!(insts[i], MInst::Load { .. } | MInst::Store { .. } | MInst::Reload { .. } | MInst::Spill { .. }) {
        return;
    }
    let far = 200usize;
    // forward: a partner AFTER i, beyond the window
    let mut fwd = None;
    let hi = (i + far).min(insts.len().saturating_sub(1));
    for j in (i + 1)..=hi {
        if !taken[j] && fuse(offs, &insts[i], &insts[j]).is_some() {
            fwd = Some(j);
            break;
        }
    }
    // backward: a partner BEFORE i and FARTHER than the window, so it is out of
    // reach by construction rather than by encoding — this pass only ever moves
    // the SECOND access back to the first. A nearer backward partner is the SAME
    // event already counted at that partner's own turn, so it is not counted
    // twice.
    let mut back = false;
    let lo = i.saturating_sub(far);
    for j in lo..i.saturating_sub(WINDOW) {
        if !taken[j] && fuse(offs, &insts[j], &insts[i]).is_some() {
            back = true;
            break;
        }
    }
    // ADJACENT IN TIME, NOT IN ADDRESS. The pairable-ness of two accesses is a
    // property of the FRAME LAYOUT, not only of the schedule: two frame accesses
    // standing next to each other that name different, non-adjacent slots could
    // have been paired if the slots had been laid out together. This counts the
    // opportunity a layout row would have; it is an upper bound, since making
    // one pair adjacent may break another.
    let near_frame = frame_range(offs, &insts[i]).is_some()
        && ((i + 1)..=(i + WINDOW).min(insts.len().saturating_sub(1))).any(|j| {
            !taken[j]
                && frame_range(offs, &insts[j]).is_some()
                && fuse(offs, &insts[i], &insts[j]).is_none()
                && same_shape(&insts[i], &insts[j])
        });
    use std::sync::atomic::Ordering::Relaxed;
    match fwd {
        None if back => { BACKWARD_ONLY.fetch_add(1, Relaxed); }
        None => {
            NO_PARTNER.fetch_add(1, Relaxed);
            if near_frame {
                LAYOUT_COULD.fetch_add(1, Relaxed);
            }
        }
        Some(j) if j > i + WINDOW => { OUT_OF_WINDOW.fetch_add(1, Relaxed); }
        Some(_) => { BLOCKED_MOTION.fetch_add(1, Relaxed); }
    }
}

pub fn residual_report() {
    if !wanted() {
        return;
    }
    use std::sync::atomic::Ordering::Relaxed;
    eprintln!(
        "[ldstp] paired={} | unpaired: no-partner={} (of which {} sit NEXT TO another frame access of the same shape — a LAYOUT could pair them) out-of-window={} motion-blocked={} partner-is-BEHIND={}",
        PAIRED.load(Relaxed),
        NO_PARTNER.load(Relaxed),
        LAYOUT_COULD.load(Relaxed),
        OUT_OF_WINDOW.load(Relaxed),
        BLOCKED_MOTION.load(Relaxed),
        BACKWARD_ONLY.load(Relaxed),
    );
}

/// May `y` be moved back across everything in `between`?
///
/// COMMUTING SQUARE, and every clause is a way the answer is no:
///   * MEMORY, and this is the clause that decides whether the pass fires at
///     all. Refusing every memory instruction in between makes the window
///     useless: in a spill RUN the things between two frame accesses are the
///     other spills, which is exactly the case worth pairing (measured: +14
///     pairs on sqlite with the blanket refusal, against +479 from the frame
///     layout alone). Two accesses may be reordered when they cannot observe
///     each other, and there are two ways to know that:
///       – both only READ. Loads do not conflict with loads, ever.
///       – both name FRAME OBJECTS whose byte ranges are DISJOINT. After
///         `frame` every slot has a number, so this is decidable rather than an
///         alias guess: distinct non-overlapping ranges of the frame are
///         distinct objects (C99 6.2.4).
///     Anything else — a call, a pointer access, `StackAlloc` moving sp — is a
///     barrier.
///   * `y`'s TRANSFER REGISTER may not be WRITTEN in between — the pair writes
///     it at the earlier point, so a later write in between would win where it
///     used to lose (a load), or the stored value would be the wrong one (a
///     store, whose source is now read early).
///   * for a LOAD, `y`'s destination may not be READ in between either: that
///     read currently sees the OLD contents of the register and would see the
///     loaded value instead.
///   * `y`'s BASE register may not be written in between.
/// `x` itself does not move, so nothing has to be said about it.
fn hoistable(offs: &[i32], between: &[MInst], y: &MInst) -> bool {
    let (load, xfer, base) = match y {
        MInst::Load { dst, mem, .. } => (true, *dst, mem_base(mem)),
        MInst::Store { src, mem, .. } => (false, *src, mem_base(mem)),
        MInst::Reload { dst, .. } => (true, *dst, None),
        MInst::Spill { src, .. } => (false, *src, None),
        _ => return false,
    };
    let yr = frame_range(offs, y);
    between.iter().all(|k| {
        if matches!(k, MInst::StackAlloc { .. }) {
            return false;
        }
        let mem_ok = match k.effect() {
            MemEffect::None => true,
            // two reads cannot observe each other
            MemEffect::Read if load => true,
            MemEffect::Read | MemEffect::Write => match (frame_range(offs, k), yr) {
                (Some((a, an)), Some((b, bn))) => a + an <= b || b + bn <= a,
                _ => false,
            },
            MemEffect::Barrier => false,
        };
        if !mem_ok {
            return false;
        }
        let mut ok = true;
        k.visit(&mut |r, c| {
            let writes = matches!(c, Constraint::Def | Constraint::DefFixed(_));
            if r == xfer && (writes || load) {
                ok = false;
            }
            if writes && Some(r) == base {
                ok = false;
            }
        });
        ok
    })
}

/// The byte range this instruction touches inside the FRAME, when it provably
/// touches the frame and nothing else. A slot has a number after `frame`, so two
/// such ranges either overlap or name different objects — no alias oracle
/// required. Anything reached through a register base could be anywhere.
fn frame_range(offs: &[i32], i: &MInst) -> Option<(i32, i32)> {
    let (mem, size) = match i {
        MInst::Load { op, mem, vol: false, .. } | MInst::Store { op, mem, vol: false, .. } => {
            (mem.clone(), op.bytes() as i32)
        }
        MInst::Pair { w, mem, .. } => (mem.clone(), 2 * w.bytes() as i32),
        MInst::Spill { slot, w, .. } | MInst::Reload { slot, w, .. } => {
            return Some((*offs.get(*slot as usize)?, w.bytes() as i32));
        }
        _ => return None,
    };
    match mem {
        AddrMode::Slot { slot, off } => Some((offs.get(slot as usize)? + off, size)),
        _ => None,
    }
}

fn mem_base(m: &AddrMode) -> Option<Reg> {
    match m {
        AddrMode::BaseImm { base, .. }
        | AddrMode::BaseReg { base, .. }
        | AddrMode::PreIdx { base, .. }
        | AddrMode::PostIdx { base, .. }
        | AddrMode::SymLo12 { base, .. } => Some(*base),
        AddrMode::Slot { .. } | AddrMode::SpArg { .. } | AddrMode::FrameWb { .. } => None,
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
