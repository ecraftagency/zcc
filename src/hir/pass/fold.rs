// Constant folding and the algebraic rewrite table (REARCH §4 row 4).
// THEORY A7b — optimization: this pass ships its commuting square
//
// TWO rules govern this file, and together they are why it needs no separate
// proof of its own beyond the corpus square:
//
//  1. **Constant folding IS the semantics, restricted to constants.** The
//     evaluation below calls `interp::eval_{bin,un,cmp,cvt}` — the very
//     functions ⟦hir⟧ uses. There is no second transcription of what `sdiv`
//     means, so a folder/interpreter disagreement is not expressible. A trap
//     (`Trap::DivZero`) folds to nothing: refining ⊥ is legal but pointless, and
//     leaving the division in place keeps the program's fault where C put it.
//
//  2. **Every algebraic row is an identity over the FULL type, not a convenient
//     subset** (Article E's resource-fidelity question). `x & -1 → x` holds
//     because an integer value of type τ is carried sign-extended, so `Imm(-1)`
//     is all-ones at every width. Float rows are ABSENT on purpose: `x + 0.0`
//     is not `x` at `x = -0.0`, and `x - x` is not `0.0` at NaN, so the only
//     float rewrite here is exact constant evaluation.
use super::super::interp::{eval_bin, eval_cmp, eval_cvt, eval_un, mask};
use super::*;

fn imm(o: Operand) -> Option<i64> {
    match o {
        Operand::Imm(k) => Some(k),
        _ => None,
    }
}

/// The bit pattern of a constant operand of type `ty`, if it is one.
fn bits(o: Operand, ty: Ty) -> Option<u64> {
    match o {
        Operand::Imm(k) if !ty.is_float() => Some(k as u64),
        Operand::Fimm(k) if ty.is_float() => Some(k),
        _ => None,
    }
}

fn cst(v: u64, ty: Ty) -> Operand {
    if ty.is_float() {
        Operand::Fimm(v)
    } else {
        Operand::Imm(mask(v, ty) as i64)
    }
}

/// The value this instruction is equal to, when that value is an operand already
/// available at this point. `None` = keep the instruction.
pub fn fold_inst(inst: &Inst) -> Option<Operand> {
    match inst {
        Inst::Bin { op, ty, a, b, .. } => fold_bin(*op, *ty, *a, *b),
        Inst::Un { op, ty, a, .. } => bits(*a, *ty).map(|x| cst(eval_un(*op, *ty, x), *ty)),
        Inst::Cmp { op, ty, a, b, .. } => fold_cmp(*op, *ty, *a, *b),
        Inst::Cvt { op, from, to, a, .. } => {
            bits(*a, *from).map(|x| cst(eval_cvt(*op, *from, *to, x), *to))
        }
        Inst::Select { ty, c, a, b, .. } => match imm(*c) {
            Some(0) => Some(*b),
            Some(_) => Some(*a),
            None if a == b => Some(*a),
            // NOT here: `select c, 1, 0 → c`. A select tests its condition ≠ 0,
            // exactly as a branch does, so the rewrite holds only when `c` is
            // ALREADY 0 or 1 — and `fold_inst` sees an operand, not the
            // instruction that produced it. `c` is routinely a whole value
            // (`x && y` tests one), and the rewrite would then return 3 where C
            // says 1. Belongs in gvn, which has the definition to hand; recorded
            // as a residual rather than shipped unsound (torture pr10352-1).
            None => None,
        },
        _ => None,
    }
}

fn fold_bin(op: BinOp, ty: Ty, a: Operand, b: Operand) -> Option<Operand> {
    if let (Some(x), Some(y)) = (bits(a, ty), bits(b, ty)) {
        // A trapping fold is declined: see rule 1.
        return eval_bin(op, ty, x, y).ok().map(|v| cst(v, ty));
    }
    if ty.is_float() {
        return None; // rule 2
    }
    let (ka, kb) = (imm(a), imm(b));
    let zero = Some(Operand::Imm(0));
    use BinOp::*;
    match op {
        Add | Or | Xor | Shl | LShr | AShr | Sub if kb == Some(0) => Some(a),
        Add | Or if ka == Some(0) => Some(b),
        Xor if ka == Some(0) => Some(b),
        Sub if a == b => zero,
        Xor if a == b => zero,
        And if a == b => Some(a),
        Or if a == b => Some(a),
        Mul if kb == Some(0) || ka == Some(0) => zero,
        Mul if kb == Some(1) => Some(a),
        Mul if ka == Some(1) => Some(b),
        And if kb == Some(0) || ka == Some(0) => zero,
        And if kb == Some(-1) => Some(a),
        And if ka == Some(-1) => Some(b),
        Or if kb == Some(-1) || ka == Some(-1) => Some(Operand::Imm(-1)),
        SDiv | UDiv if kb == Some(1) => Some(a),
        SRem | URem if kb == Some(1) => zero,
        _ => None,
    }
}

fn fold_cmp(op: CmpOp, ty: Ty, a: Operand, b: Operand) -> Option<Operand> {
    if let (Some(x), Some(y)) = (bits(a, ty), bits(b, ty)) {
        return Some(Operand::Imm(eval_cmp(op, ty, x, y) as i64));
    }
    if op.is_float() {
        return None; // `x == x` is FALSE at NaN
    }
    let (t, f) = (Some(Operand::Imm(1)), Some(Operand::Imm(0)));
    use CmpOp::*;
    match op {
        Eq | Sle | Sge | Ule | Uge if a == b => t,
        Ne | Slt | Sgt | Ult | Ugt if a == b => f,
        // An unsigned value is never below 0 and always at or above it.
        Ult if b == Operand::Imm(0) => f,
        Uge if b == Operand::Imm(0) => t,
        _ => None,
    }
}

/// Canonicalization: rewrites that change the OPCODE rather than replace the
/// instruction, so they cannot be expressed as "this value equals that operand".
/// Run as part of the ladder, before value numbering, so the canonical form is
/// what gets numbered.
/// The exact reciprocal of a floating constant, when the division by it and the
/// multiplication by it denote the SAME function on every input.
///
/// This is the one float row rule 2 admits, and it admits it because it is not
/// an approximation. A power of two has an all-zero significand, so its
/// reciprocal is also a power of two and is EXACTLY representable — and IEEE 754
/// §5.4 makes both `x / 2^k` and `x · 2^-k` the correctly-rounded result of the
/// same exact real number, so the two agree bit for bit on every finite input,
/// on ±0, on ±∞ and on every NaN payload (a NaN propagates through either
/// operation unchanged). The rounding mode does not enter: the exact result is
/// already representable whenever `x` is normal, and when it is not, both
/// operations underflow to the same value by the same rule.
///
/// TWO exclusions, both about representability rather than about rounding:
///   * a zero or subnormal exponent field, and the infinity/NaN field — those
///     are not powers of two in this sense;
///   * an exponent whose reciprocal would land on the subnormal boundary
///     (`2^-k` with the exponent field at 0), where the reciprocal is no longer
///     exact and the identity fails.
///
/// f2_double_poly divides by 1024.0 in its inner loop: `fdiv` is 10+ cycles on
/// this machine and `fmul` is 3 (§13n R4.14 (1)).
fn exact_reciprocal(bits: u64, ty: Ty) -> Option<u64> {
    let (ebits, mbits) = match ty {
        Ty::F64 => (11u32, 52u32),
        Ty::F32 => (8u32, 23u32),
        _ => return None,
    };
    let emax = (1u64 << ebits) - 1; // the all-ones field: infinity / NaN
    let bias = (1u64 << (ebits - 1)) - 1;
    let sign = bits >> (ebits + mbits);
    let e = (bits >> mbits) & emax;
    let m = bits & ((1u64 << mbits) - 1);
    if m != 0 || e == 0 || e == emax {
        return None; // not a normal power of two
    }
    // value = (-1)^s · 2^(e-bias); reciprocal exponent field = 2·bias - e
    let re = 2 * bias.checked_sub(0)? ;
    let re = re.checked_sub(e)?;
    if re == 0 || re >= emax {
        return None; // the reciprocal is subnormal or overflows
    }
    Some((sign << (ebits + mbits)) | (re << mbits))
}

pub fn canon(f: &mut Func) -> bool {
    let mut changed = false;
    for b in f.blocks.iter_mut() {
        for inst in b.insts.iter_mut() {
            if let Inst::Bin { op, ty, a, b: rhs, .. } = inst {
                // `x / 2^k = x · 2^-k`, EXACTLY — see `exact_reciprocal`.
                if *op == BinOp::FDiv {
                    if let Operand::Fimm(k) = *rhs {
                        if let Some(r) = exact_reciprocal(k, *ty) {
                            *op = BinOp::FMul;
                            *rhs = Operand::Fimm(r);
                            changed = true;
                            continue;
                        }
                    }
                }
                // `x * 2^k = x << k` in two's complement, at every width. Worth a
                // rule of its own because it is what lets isel see an ARRAY
                // INDEX: `a[i]` is `base + i * elemsize`, and only the SHIFT form
                // folds into an addressing mode.
                if *op == BinOp::Mul && !ty.is_float() {
                    if let Operand::Imm(k) = *rhs {
                        if k > 1 && k & (k - 1) == 0 {
                            *op = BinOp::Shl;
                            *rhs = Operand::Imm(k.trailing_zeros() as i64);
                            changed = true;
                            continue;
                        }
                    }
                    if let (Operand::Imm(k), Operand::Val(v)) = (*a, *rhs) {
                        if k > 1 && k & (k - 1) == 0 {
                            *op = BinOp::Shl;
                            *a = Operand::Val(v);
                            *rhs = Operand::Imm(k.trailing_zeros() as i64);
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    changed
}
