// Immediate legalization: given a constant and the instruction that wants it,
// either an encodable operand field or the instruction sequence that
// materializes it. Every predicate consulted here lives in `mir::isa` (Side-II);
// this file only chooses between them.
use crate::mir::{AluOp, MInst, Reg, Rhs, Width, isa};

/// Can `k` ride in the instruction's immediate field?
pub fn as_rhs(op: AluOp, k: i64, w: Width) -> Option<Rhs> {
    match op {
        // add/sub take imm12 (optionally << 12), and each can absorb the other's
        // sign, but the caller owns that choice — here only the literal field.
        AluOp::Add | AluOp::Sub => isa::add_imm(k).map(|_| Rhs::Imm(k)),
        AluOp::And | AluOp::Orr | AluOp::Eor | AluOp::Bic | AluOp::Orn | AluOp::Eon => {
            if isa::logical_imm(k as u64, w.is64()) {
                Some(Rhs::Imm(k))
            } else {
                None
            }
        }
        // a shift amount is an immediate field of its own, modulo the width
        AluOp::Lsl | AluOp::Lsr | AluOp::Asr => {
            let bits = if w.is64() { 64 } else { 32 };
            if (0..bits).contains(&k) {
                Some(Rhs::Imm(k))
            } else {
                None
            }
        }
        AluOp::Mul | AluOp::SDiv | AluOp::UDiv => None,
    }
}

/// Put `k` into `dst`. One MIR instruction; the emitter expands it into the
/// `movz/movn/movk` chain, whose length is already known here.
pub fn materialize(dst: Reg, k: i64, w: Width) -> MInst {
    MInst::MovImm { w, dst, imm: k }
}

/// The instruction count `materialize` will cost — the cost model reads this,
/// never the emitted text.
pub fn materialize_cost(k: i64, w: Width) -> usize {
    isa::mov_chain(k, w.is64()).len()
}

/// A floating constant: `fmov #imm8` when the 8-bit form covers it, otherwise
/// the bit pattern through a GPR (`mov`-chain + `fmov d, x`), which needs no
/// literal pool and no relocation.
pub fn fp_is_imm8(bits: u64, w: Width) -> bool {
    isa::fp_imm8(bits, w)
}
