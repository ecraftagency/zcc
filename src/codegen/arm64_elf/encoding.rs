//! Side-II spec tables for AArch64/ELF: the register files (AAPCS64 §6.1.1),
//! immediate/condition-code encoders, and symbol mangling. Every item here is a
//! constant or encoding transcribed from a spec line (Law 1, Side II) — no algorithm.
use crate::ir::{Op, Val};
use crate::opt::ClassBudget;

// Stage 5b — AAPCS64 §6.1.1 register files partitioned for `opt::abi_alloc`. The
// SPEC-TABLE side of the pass (the algorithm lives in opt.rs): a color index maps to a
// physical register here. A color ≥ ncaller is callee-saved ⟹ obliges a prologue/
// epilogue save/restore.
//   FP: caller-saved v16–v31 then callee-saved v8–v15 (only d8–d15 preserved across a bl).
// GP allocation budgets — §3 keystone (the caller/callee split). The hazard that once forced
// ncaller=0 is now precisely scoped: the emitter's fixed scratch that CLOBBERS x10–x15 lives in
// exactly three body instructions — Overflow (ext.rs overflow_emit uses x14/x15), Sync, VaArg
// (+ the prologue struct-copy, which runs before any home is live). This is the same clobber
// that hung pr64006 when x14/x15 were pooled blindly (ext.rs was outside the first grep) — but
// the cure is to gate on it, not to abandon the whole caller-saved file. A function FREE of
// those three therefore uses x10–x15 as 6 CALLER-saved homes (WIDE, ncaller=6); a function
// containing one falls back to NARROW (x19–x28 only, ncaller=0). x10–x15 are not argument
// registers, so opening them adds no call-marshalling shuffle hazard, and color_abi's crossing[]
// already confines any call-crossing temp to the callee-saved band.
pub(super) const GP_BUDGET: ClassBudget = ClassBudget { k: 10, ncaller: 0, narg: 0 }; // NARROW: x19–x28
// WIDE caller file = x0–x7 (colors 0–7, the ARGUMENT/result registers), callee x19–x28 (8–17).
// Opening the arg registers as homes is what enables VALUE-PLACEMENT TARGETING (an arg temp
// homed at x{i} elides its marshal mov; a call result homed at x0 elides its capture mov) — the
// ~27k-mov / ~8% sqlite lever. It reintroduces two call-boundary hazards the prior x10–x15 file
// dodged, both handled: the marshalling shuffle (marshal_call_args' parallel move) and the
// indirect-callee clobber (ir_call snapshots the pointer to x17 before marshalling). crossing[]
// confines any call-crossing temp to the callee band; narg=8 keeps PARAM temps off the arg
// registers (Inst::Param delivery stays a permutation-free arg→home copy). The backend funnel
// scratch that used to live in x0–x5 was RELOCATED to x10–x15 (via the `fnl` base; disjoint from
// this home file AND from the x9 internal address scratch); the heavy paths (Overflow/Sync/VaArg/
// Asm, NARROW-only) keep their x0–x5 funnel — they never run WIDE (fnl=0).
pub(super) const GP_BUDGET_WIDE: ClassBudget = ClassBudget { k: 18, ncaller: 8, narg: 8 }; // WIDE: x0–x7 | x19–x28
pub(super) const FP_BUDGET: ClassBudget = ClassBudget { k: 24, ncaller: 16, narg: 0 };
pub(super) fn fp_phys(idx: u32) -> u32 {
    if idx < FP_BUDGET.ncaller { 16 + idx } else { 8 + (idx - FP_BUDGET.ncaller) }
}
// GP register name in x/w form. Reg 31 is the ZERO register (XZR/WZR) in every operand
// position these helpers emit (load/store Rt, data-processing Rm/Rn-of-flag-forms) — NOT
// sp; callers must never pass 31 where the encoding reads it as sp (add/sub-immediate Rn).
pub(super) fn xr(n: u32) -> String {
    if n == 31 { "xzr".into() } else { format!("x{n}") }
}
pub(super) fn wr(n: u32) -> String {
    if n == 31 { "wzr".into() } else { format!("w{n}") }
}
// Add/Sub with a small constant right operand → (mnemonic, magnitude) for the AArch64
// imm12 form. Side-II: the imm12 field is an *unsigned* 0..4096; a negative Add becomes a
// Sub and vice versa. Returns None when the operand is not an in-range immediate.
// Relational Op → AArch64 condition suffix (unsigned picks lo/ls/hi/hs). None ⟹ not a
// comparison. The mapping is identical to ir_bin_r/try_bin_imm's cset cond — the fused
// compare-branch reuses it so `cmp;b.cc` carries the exact condition `cmp;cset;cbnz` did.
pub(super) fn rel_cond(op: Op, u: bool) -> Option<&'static str> {
    Some(match (op, u) {
        (Op::Eq, _) => "eq", (Op::Ne, _) => "ne",
        (Op::Lt, true) => "lo", (Op::Lt, false) => "lt",
        (Op::Le, true) => "ls", (Op::Le, false) => "le",
        (Op::Gt, true) => "hi", (Op::Gt, false) => "gt",
        (Op::Ge, true) => "hs", (Op::Ge, false) => "ge",
        _ => return None,
    })
}

// The negated condition (ARMv8 flips the cond field's low bit). Used when the THEN edge is
// the fall-through, so we branch to ELSE on ¬cond. Total over rel_cond's range.
pub(super) fn inv_cond(cc: &str) -> &'static str {
    match cc {
        "eq" => "ne", "ne" => "eq",
        "lt" => "ge", "ge" => "lt", "le" => "gt", "gt" => "le",
        "lo" => "hs", "hs" => "lo", "ls" => "hi", "hi" => "ls",
        _ => unreachable!(),
    }
}

pub(super) fn add_sub_imm12(op: Op, b: Val) -> Option<(&'static str, u64)> {
    let Val::Imm(k) = b else { return None };
    let mnem = match (op, k >= 0) {
        (Op::Add, true) | (Op::Sub, false) => "add",
        (Op::Add, false) | (Op::Sub, true) => "sub",
        _ => return None,
    };
    let mag = k.unsigned_abs();
    (mag < 4096).then_some((mnem, mag))
}

// Side-II: ARMv8 logical-immediate encoding (`and/orr/eor #imm`). A 64-bit value is
// encodable iff it is a rotation of a run of `ones` set bits (0<ones<size) within a
// power-of-two element `size ∈ {2,4,8,16,32,64}`, replicated across the register. Neither
// all-zeros nor all-ones is encodable. We always emit the x-form ⟹ check over 64 bits.
pub(super) fn is_logical_imm(val: u64) -> bool {
    if val == 0 || val == u64::MAX {
        return false;
    }
    let mut size = 2u32;
    while size <= 64 {
        let mask = if size == 64 { u64::MAX } else { (1u64 << size) - 1 };
        let elem = val & mask;
        // val must be `elem` replicated every `size` bits
        let mut replicated = true;
        let mut s = size;
        while s < 64 {
            if (val >> s) & mask != elem {
                replicated = false;
                break;
            }
            s += size;
        }
        if replicated {
            let ones = elem.count_ones();
            if ones == 0 || ones == size {
                return false; // degenerate at this (smallest) element ⟹ not encodable
            }
            // elem must be a rotation of the contiguous low-ones pattern (2^ones − 1)
            let target = (1u64 << ones) - 1;
            let mut rot = elem;
            for _ in 0..size {
                if rot == target {
                    return true;
                }
                rot = ((rot >> 1) | (rot << (size - 1))) & mask; // rotate-right within `size`
            }
            return false;
        }
        size <<= 1;
    }
    false
}

pub(super) const EPILOGUE: &str = "\tmov sp, x29\n\tldp x29, x30, [sp], #16\n\tret\n";

// EXT(gcc): symbol emit — a \x01 prefix (asm-label / && label) = name already complete; ELF has no '_' prefix
pub(super) fn sym(n: &str) -> String {
    match n.strip_prefix('\x01') {
        Some(raw) => raw.to_string(),
        None => n.to_string(),
    }
}
