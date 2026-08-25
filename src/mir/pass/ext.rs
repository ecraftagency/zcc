// ext_lattice (REARCH.md §8) — the extension that has already happened.
//
// A64 narrows and widens in the LOAD: `ldrb` zero-extends its byte into the
// whole register, `ldrsh` sign-extends its halfword, and every 32-bit form
// zeroes bits 63:32 (DDI 0487 B1.2.1). A C program that reads an `unsigned
// char` and promotes it to `int` therefore asks for a widening the machine has
// already done, and isel — which lowers one HIR instruction at a time — emits
// the `uxtb` anyway. sqlite paid 3,918 `uxtb` and 1,357 `uxth` for that, against
// gcc's zero.
//
// The pass is a forward dataflow over MIR-SSA carrying one fact per value: how
// many low bits it actually has, and whether the bits above them are zero or a
// copy of the sign. An extension whose source already satisfies it becomes a
// `Copy`, which biased colouring then removes entirely.
//
// COMMUTING SQUARE. The replacement is legal exactly when the fact holds, and
// the fact is established only by instructions whose ARCHITECTURAL definition
// establishes it — a load form, a narrower extension, a mask, or a literal.
// Nothing is inferred from context, so there is no case analysis to get wrong.
use crate::mir::*;
use std::collections::HashMap;

/// What is known about a value's bits.
///
/// The subtlety this type exists to make explicit: a `w`-form instruction writes
/// 32 bits and ZEROES bits 63:32 (DDI 0487 B1.2.1). So `sxtb w0, w1` produces a
/// value that is sign-extended within its low 32 bits and zero-extended above
/// them, and treating that as "sign-extended from 8 bits" — which is what a
/// single (bits, zero) pair says — makes a later `sxtw` look redundant when it is
/// the very instruction that would set the upper half. That was a wrong-code bug
/// (yarpgen s0009 and 44 others), and the third field is the fix.
#[derive(Clone, Copy, PartialEq)]
struct Known {
    /// the number of significant low bits
    bits: u32,
    /// bits above `bits` are zero (as opposed to a copy of bit `bits-1`)
    zero: bool,
    /// the fact holds inside the low 32 bits only; bits 63:32 are zero
    w32: bool,
}

impl Known {
    /// The same fact stated about the whole 64-bit register.
    fn wide(self) -> Known {
        if self.w32 && !self.zero {
            // sign-extended within 32 bits, zero above: as a 64-bit value that is
            // simply "32 significant bits, zero above"
            Known { bits: 32, zero: true, w32: false }
        } else {
            Known { w32: false, ..self }
        }
    }
    /// What survives a write that keeps only the low 32 bits.
    fn narrow(self) -> Known {
        if self.bits <= 32 {
            Known { w32: true, ..self }
        } else {
            Known { bits: 32, zero: true, w32: true }
        }
    }
}

pub fn run(f: &mut MFunc) {
    let mut known: HashMap<VReg, Known> = HashMap::new();
    let cfg = crate::mir::verify::cfg(f);
    let width = |r: Reg| -> Width {
        match r {
            Reg::V(v) => f.vregs[v as usize].width,
            Reg::P(_) => Width::W64,
        }
    };
    // SSA: a definition dominates its uses, and reverse postorder visits a
    // dominator before what it dominates.
    for &b in &cfg.rpo {
        let bi = b as usize;
        for i in 0..f.blocks[bi].insts.len() {
            if let Some(rep) = redundant(&f.blocks[bi].insts[i], &known) {
                f.blocks[bi].insts[i] = rep;
            }
            plain_operand(&mut f.blocks[bi].insts[i], &known);
            record(&f.blocks[bi].insts[i], &mut known, &width);
        }
    }
}

/// The fact an instruction establishes about its destination.
fn record(i: &MInst, known: &mut HashMap<VReg, Known>, width: &dyn Fn(Reg) -> Width) {
    let mut set = |r: Reg, bits: u32, zero: bool| {
        if let Reg::V(v) = r {
            let k = Known { bits, zero, w32: false };
            known.insert(v, if width(r) == Width::W32 { k.narrow() } else { k });
        }
    };
    match i {
        MInst::Load { op, dst, .. } => {
            match op {
                MemOp::B => set(*dst, 8, true),
                MemOp::SB | MemOp::SBX => set(*dst, 8, false),
                MemOp::H => set(*dst, 16, true),
                MemOp::SH | MemOp::SHX => set(*dst, 16, false),
                MemOp::W => set(*dst, 32, true),
                MemOp::SW => set(*dst, 32, false),
                _ => {}
            }
        }
        MInst::Ext { op, w, dst, .. } => {
            let (bits, zero) = match op {
                ExtOp::Sxtb => (8, false),
                ExtOp::Sxth => (16, false),
                ExtOp::Sxtw => (32, false),
                ExtOp::Uxtb => (8, true),
                ExtOp::Uxth => (16, true),
            };
            if let Reg::V(v) = dst {
                let k = Known { bits, zero, w32: false };
                known.insert(*v, if *w == Width::W32 { k.narrow() } else { k });
            }
        }
        // A mask by a literal bounds the result from above.
        MInst::Alu { op: AluOp::And, dst, b: Rhs::Imm(m), .. } if *m > 0 => {
            set(*dst, 64 - (*m as u64).leading_zeros(), true);
        }
        MInst::MovImm { dst, imm, .. } if *imm >= 0 => {
            set(*dst, (64 - (*imm as u64).leading_zeros()).max(1), true);
        }
        MInst::Copy { w, dst, src } => {
            if let (Reg::V(s), Reg::V(d)) = (src, dst) {
                match known.get(s).copied() {
                    Some(k) => {
                        let k = if *w == Width::W32 { k.narrow() } else { k };
                        known.insert(*d, k);
                    }
                    None => {
                        known.remove(d);
                    }
                }
            }
        }
        _ => {
            // anything else: the destination's bits are unknown
            i.visit(&mut |r, c| {
                if let (Reg::V(v), Constraint::Def | Constraint::DefFixed(_)) = (r, c) {
                    known.remove(&v);
                }
            });
        }
    }
}

/// An extension its source has already undergone becomes a copy.
fn redundant(i: &MInst, known: &HashMap<VReg, Known>) -> Option<MInst> {
    let (op, w, dst, src) = match i {
        MInst::Ext { op, w, dst, src } => (*op, *w, *dst, *src),
        _ => return None,
    };
    let k = match src {
        Reg::V(v) => *known.get(&v)?,
        Reg::P(_) => return None,
    };
    // A `w`-form extension only has to be right inside the low 32 bits, which is
    // exactly what the recorded fact says; an `x`-form has to be right over the
    // whole register, so a fact that only holds below bit 32 is restated first.
    let k = if w == Width::W32 { k } else { k.wide() };
    let (need, zero) = match op {
        ExtOp::Sxtb => (8, false),
        ExtOp::Sxth => (16, false),
        ExtOp::Sxtw => (32, false),
        ExtOp::Uxtb => (8, true),
        ExtOp::Uxth => (16, true),
    };
    // A ZERO-extension is satisfied by a value already known zero above a
    // narrower point. A SIGN-extension is satisfied either by the same sign
    // property, or by a value that is zero above a STRICTLY narrower point —
    // then bit `need-1` is zero and the two agree.
    let ok = if zero {
        k.zero && k.bits <= need
    } else {
        (!k.zero && k.bits <= need) || (k.zero && k.bits < need)
    };
    if !ok {
        return None;
    }
    Some(MInst::Copy { w, dst, src })
}

/// The same fact, applied to an extension that rides INSIDE an operand.
///
/// `add x1, x1, w0, sxtw` and `add x1, x1, x0` are the same value when the
/// extension is a no-op — and they are NOT the same instruction: the
/// extended-register form is a 2-cycle operation where the plain one is 1
/// (DDI 0487 C6.2.4 is silent on timing; the latency is the measured Side-II
/// fact of §13n's "MISSING DUAL"). On a LOOP-CARRIED recurrence that is the
/// whole difference — `s += a[i] & 31` runs at half speed for an extension
/// that does nothing (d2_nested_loops, 2.11×). `cost = |MIR|` scores the two
/// identically, which is exactly why this is proven on the lattice and not
/// discovered on the clock.
///
/// COMMUTING SQUARE: the rewrite fires only when the recorded fact says bits
/// 63:32 ALREADY hold what the extension would put there, so the two operands
/// denote the same 64-bit value; the destination, the flags and every other
/// field are untouched. The fact itself is established only by instructions
/// whose architectural definition establishes it (see `record`).
fn plain_operand(i: &mut MInst, known: &HashMap<VReg, Known>) {
    let (w, b) = match i {
        MInst::Alu { w, b, .. } | MInst::Cmp { w, b, .. } => (*w, b),
        _ => return,
    };
    // the fold is about the 64-bit form only: at `w` there is nothing above
    // bit 31 for an extension to write
    if !w.is64() {
        return;
    }
    let (r, e) = match b {
        Rhs::Extended(r, e, 0) => (*r, *e),
        _ => return,
    };
    let k = match r {
        Reg::V(v) => match known.get(&v) {
            Some(k) => *k,
            None => return,
        },
        Reg::P(_) => return,
    };
    // `uxtw` is a no-op when bits 63:32 are already zero; `sxtw` when they
    // already hold bit 31 — which a `w`-form write with a clear bit 31 gives
    // (top zero, sign zero), as does a value already sign-extended over 64.
    let noop = match e {
        ExtKind::Uxtw => k.w32 || (k.zero && k.bits <= 32),
        ExtKind::Sxtw => (k.w32 && k.zero && k.bits <= 31) || (!k.w32 && !k.zero && k.bits <= 32),
        _ => false,
    };
    if noop {
        *b = Rhs::Reg(r);
    }
}
