// divmagic — division by a constant becomes a multiply (MECHANISM.md §G4).
// THEORY A7b — optimization: this pass ships its commuting square
//
// WHY THIS EXISTS TWICE. `MEASURED M25` built this row, proved it, measured it and
// REMOVED it, on one sentence: "AND THE DIVIDER ON THIS CORE IS NOT SLOW… the
// folklore is a Cortex-A53-era fact; it is not a fact about this core." Every word
// was true about the Apple M1 Pro that measured it. On a Neoverse V2 — the core
// almost every real AArch64-Linux machine has — `latency.sh` measures `udiv` at
// **4.98 dependent adds**, and `a2_udiv_mod` runs at **4.50x** gcc -O1 where the M1
// read 1.12x (`MEASURED M46`). The theorem never changed; the machine did.
//
// AND THE SECOND PRECONDITION M25 SET IS NOT NEEDED. It asked that the emitted
// sequence reach gcc's five instructions rather than nine, three of the nine being
// constant materialization inside the loop. Hand-edited on Neoverse V2, with the
// magic constant rebuilt EVERY ITERATION — the shape this pass emits without any
// hoist — `a2_udiv_mod` still goes 4.455x → **1.111x**. A `udiv` is expensive
// enough that four extra one-cycle instructions do not begin to pay for it. The
// row is independent of `const_share`'s loop hoist, which is why it can ship while
// that one stays off (`M44`).
//
// COMMUTING SQUARE. ⟦Bin{UDiv,n,d}⟧ = ⟦the sequence below⟧ for every n of the type,
// and the proof is not an argument about magic numbers — it is EXHAUSTION. A
// 32-bit divisor is checked against real division at the boundaries of the
// numerator and over a dense divisor range; the transcription is Hacker's Delight
// §10-9 (`magicu`) and §10-4 (`magic`), and `tests.rs` re-derives every constant it
// uses rather than trusting this file. A pass that computes the wrong multiplier is
// not a slow compiler, it is a wrong one, so nothing here is believed on its face.
//
// WHAT IS REFUSED, and each refusal is a real boundary rather than an omission:
//   * `d == 0` — the program's own fault (C99 6.5.5p5), left where C put it.
//   * `|d| == 1` — `fold` already rewrites it; a magic number for 1 is degenerate.
//   * `|d|` a power of two — `fold` already emits the shift, which is one
//     instruction against this sequence's four.
//   * `INT_MIN` as a signed divisor — `abs` is not representable, and the only
//     quotient values are 0 and 1; the shift form `fold` reaches is better.
use super::*;

/// Hacker's Delight §10-9 `magicu`, at any width, in `u128` so the doubling steps
/// never wrap silently. Returns `(M, s, add)`: the quotient is
/// `(n·M >> W) >> s`, or with `add` set, `((((n − h) >> 1) + h) >> (s−1))` where
/// `h = n·M >> W` — the correction the algorithm needs when `M` does not fit in W
/// bits, which is the case for `d = 7`.
fn magicu(d: u128, w: u32) -> (u128, u32, bool) {
    debug_assert!(d > 1 && w >= 2);
    let two_w: u128 = 1u128 << w;
    let ones: u128 = two_w - 1;
    let half: u128 = 1u128 << (w - 1);
    // nc = -1 - (-d) % d, in W-bit unsigned arithmetic
    let nc = ones - ((two_w - d) % d);
    let mut p = w - 1;
    let mut q1 = half / nc;
    let mut r1 = half - q1 * nc;
    let mut q2 = (half - 1) / d;
    let mut r2 = (half - 1) - q2 * d;
    let mut add = false;
    loop {
        p += 1;
        if r1 >= nc - r1 {
            q1 = 2 * q1 + 1;
            r1 = 2 * r1 - nc;
        } else {
            q1 *= 2;
            r1 *= 2;
        }
        if r2 + 1 >= d - r2 {
            if q2 >= half - 1 {
                add = true;
            }
            q2 = 2 * q2 + 1;
            r2 = 2 * r2 + 1 - d;
        } else {
            if q2 >= half {
                add = true;
            }
            q2 *= 2;
            r2 = 2 * r2 + 1;
        }
        let delta = d - 1 - r2;
        if p >= 2 * w || !(q1 < delta || (q1 == delta && r1 == 0)) {
            break;
        }
    }
    // `M` IS A W-BIT VALUE AND THE MASK IS NOT COSMETIC. Hacker's Delight computes
    // `q2` in W-bit unsigned arithmetic, so `q2 + 1` WRAPS there; carried in `u128`
    // it does not, and `d = 7` comes out as the 33-bit `0x1_2492_4925`. The missing
    // `2^W` term is exactly what the `add` correction puts back — which is why `add`
    // is set on precisely the divisors whose multiplier overflows. Masking here
    // reproduces the wrap, and the constant becomes `0x2492_4925`: gcc's.
    ((q2 + 1) & ones, p - w, add)
}

/// Hacker's Delight §10-4 `magic`, signed, at any width. Returns `(M, s)` with `M`
/// carried as the W-bit two's-complement pattern: the quotient is
/// `(n·M >> W)`, corrected for the sign of `d` versus the sign of `M`, then
/// arithmetic-shifted by `s` and rounded toward zero by adding its own sign bit.
fn magics(d: i128, w: u32) -> (i128, u32) {
    debug_assert!(d.unsigned_abs() > 1 && w >= 2);
    let two_w1: u128 = 1u128 << (w - 1); // 2^(W-1)
    let ad: u128 = d.unsigned_abs();
    let t: u128 = two_w1 + if d < 0 { 1 } else { 0 };
    let anc: u128 = t - 1 - t % ad;
    let mut p = w - 1;
    let mut q1 = two_w1 / anc;
    let mut r1 = two_w1 - q1 * anc;
    let mut q2 = two_w1 / ad;
    let mut r2 = two_w1 - q2 * ad;
    loop {
        p += 1;
        q1 *= 2;
        r1 *= 2;
        if r1 >= anc {
            q1 += 1;
            r1 -= anc;
        }
        q2 *= 2;
        r2 *= 2;
        if r2 >= ad {
            q2 += 1;
            r2 -= ad;
        }
        let delta = ad - r2;
        if !(q1 < delta || (q1 == delta && r1 == 0)) {
            break;
        }
    }
    // Same wrap as `magicu`: `q2` is W-bit there, so the increment is masked before
    // the sign is applied.
    let m = ((q2 + 1) & (two_w1 * 2 - 1)) as i128;
    let m = if m >= two_w1 as i128 { m - (two_w1 as i128) * 2 } else { m };
    (if d < 0 { -m } else { m }, p - w)
}

/// THEORY A7b  SQUARE divmagic_replaces_a_constant_divide_with_a_multiply — the
/// division rewritten as a multiply
///
/// TWO PROOFS, and they answer different questions. The batteries at the bottom of
/// this file check the CONSTANTS — that `(n·M >> W) >> s` is `n / d` for a dense
/// range of divisors at the boundaries of the numerator — which is a statement
/// about Granlund–Montgomery and not about this pass. The square in `tests.rs`
/// checks the PASS: `⟦f⟧ = ⟦divmagic f⟧` through the reference interpreter, which
/// is what catches a correct multiplier wired to the wrong operand.
pub fn run(f: &mut Func) -> bool {
    if std::env::var_os("ZCC_NODIVMAGIC").is_some() {
        return false;
    }
    let mut changed = false;
    for b in 0..f.blocks.len() {
        let mut i = 0;
        while i < f.blocks[b].insts.len() {
            let site = match &f.blocks[b].insts[i] {
                Inst::Bin { dst, op, ty, a, b: Operand::Imm(k) } => {
                    match (op, ty) {
                        (
                            BinOp::UDiv | BinOp::URem | BinOp::SDiv | BinOp::SRem,
                            Ty::I32 | Ty::I64,
                        ) => Some((*dst, *op, *ty, *a, *k)),
                        _ => None,
                    }
                }
                _ => None,
            };
            let Some((dst, op, ty, num, k)) = site else {
                i += 1;
                continue;
            };
            let w = if ty == Ty::I32 { 32 } else { 64 };
            let signed = matches!(op, BinOp::SDiv | BinOp::SRem);
            if !worth_it(k, w, signed) {
                i += 1;
                continue;
            }
            let n = expand(f, b as BlockId, i, dst, op, ty, num, k, w, signed);
            super::refresh_block_defs(f, b as BlockId);
            i += n;
            changed = true;
        }
    }
    changed
}

/// The refusals of the header, in one predicate so the reason a site is skipped is
/// readable at the call site.
fn worth_it(k: i64, w: u32, signed: bool) -> bool {
    if signed {
        // INT_MIN has no representable absolute value, and `fold` reaches a better
        // form for it and for the powers of two.
        if k == 0 || k == i64::MIN || (w == 32 && k == i32::MIN as i64) {
            return false;
        }
        let ad = k.unsigned_abs();
        ad > 1 && !ad.is_power_of_two()
    } else {
        // An unsigned divisor is the low W bits of the immediate.
        let d = if w == 32 { (k as u32) as u64 } else { k as u64 };
        d > 1 && !d.is_power_of_two()
    }
}

/// Append one binary instruction to the buffer and hand back its value.
fn bin(
    f: &mut Func,
    insts: &mut Vec<Inst>,
    b: BlockId,
    op: BinOp,
    ty: Ty,
    a: Operand,
    rhs: Operand,
) -> Operand {
    let v = f.new_value(ty, Def::Inst(b, 0));
    insts.push(Inst::Bin { dst: v, op, ty, a, b: rhs });
    Operand::Val(v)
}

/// Append one conversion and hand back its value.
fn cvt(
    f: &mut Func,
    insts: &mut Vec<Inst>,
    b: BlockId,
    op: CvtOp,
    from: Ty,
    to: Ty,
    a: Operand,
) -> Operand {
    let v = f.new_value(to, Def::Inst(b, 0));
    insts.push(Inst::Cvt { dst: v, op, from, to, a });
    Operand::Val(v)
}

/// Replace the division at `blocks[b].insts[at]` with the multiply sequence, the
/// LAST instruction of which defines the original `dst` so every existing use is
/// untouched. Returns the number of instructions written.
#[allow(clippy::too_many_arguments)]
fn expand(
    f: &mut Func,
    b: BlockId,
    at: usize,
    dst: ValueId,
    op: BinOp,
    ty: Ty,
    num: Operand,
    k: i64,
    w: u32,
    signed: bool,
) -> usize {
    let wide = Ty::I64;
    // The sequence is built into a buffer and spliced over the division in one go.
    // Every definition site is written as index 0 and corrected afterwards by
    // `refresh_block_defs`, which is the same contract `licm` and `gvn` use.
    let mut insts: Vec<Inst> = Vec::new();

    let q = if !signed {
        let d = if w == 32 { (k as u32) as u128 } else { k as u64 as u128 };
        let (m, s, add) = magicu(d, w);
        let hi = if w == 32 {
            // The 32x32 product fits in 64 bits, so the high half is a plain
            // 64-bit multiply and shift — no `umulh` and no widening opcode.
            let z = cvt(f, &mut insts, b, CvtOp::Zext, ty, wide, num);
            let p = bin(f, &mut insts, b, BinOp::Mul, wide, z, Operand::Imm(m as i64));
            let h = bin(f, &mut insts, b, BinOp::LShr, wide, p, Operand::Imm(32));
            cvt(f, &mut insts, b, CvtOp::Trunc, wide, ty, h)
        } else {
            bin(f, &mut insts, b, BinOp::UMulHi, ty, num, Operand::Imm(m as i64))
        };
        if add {
            // ((n - h) >> 1) + h, then >> (s-1). `s >= 1` whenever `add` is set.
            let t = bin(f, &mut insts, b, BinOp::Sub, ty, num, hi);
            let t = bin(f, &mut insts, b, BinOp::LShr, ty, t, Operand::Imm(1));
            let t = bin(f, &mut insts, b, BinOp::Add, ty, t, hi);
            bin(f, &mut insts, b, BinOp::LShr, ty, t, Operand::Imm(s as i64 - 1))
        } else if s == 0 {
            hi
        } else {
            bin(f, &mut insts, b, BinOp::LShr, ty, hi, Operand::Imm(s as i64))
        }
    } else {
        let d = if w == 32 { (k as i32) as i128 } else { k as i128 };
        let (m, s) = magics(d, w);
        let hi = if w == 32 {
            let z = cvt(f, &mut insts, b, CvtOp::Sext, ty, wide, num);
            let p = bin(f, &mut insts, b, BinOp::Mul, wide, z, Operand::Imm(m as i64));
            let h = bin(f, &mut insts, b, BinOp::AShr, wide, p, Operand::Imm(32));
            cvt(f, &mut insts, b, CvtOp::Trunc, wide, ty, h)
        } else {
            bin(f, &mut insts, b, BinOp::SMulHi, ty, num, Operand::Imm(m as i64))
        };
        // The multiplier's sign can disagree with the divisor's; the correction is
        // exactly Hacker's Delight's, and it is why `magics` returns a SIGNED M.
        let hi = if d > 0 && m < 0 {
            bin(f, &mut insts, b, BinOp::Add, ty, hi, num)
        } else if d < 0 && m > 0 {
            bin(f, &mut insts, b, BinOp::Sub, ty, hi, num)
        } else {
            hi
        };
        let q0 = if s == 0 { hi } else { bin(f, &mut insts, b, BinOp::AShr, ty, hi, Operand::Imm(s as i64)) };
        // Truncation toward zero: add the sign bit back, since the arithmetic shift
        // floors and C99 6.5.5p6 truncates.
        let sign = bin(f, &mut insts, b, BinOp::LShr, ty, q0, Operand::Imm(w as i64 - 1));
        bin(f, &mut insts, b, BinOp::Add, ty, q0, sign)
    };

    // `a % b` is `a - (a / b) * b` (C99 6.5.5p6) — the same identity isel uses for
    // the `msub` it emits today, only with the divide now a multiply.
    let last = if matches!(op, BinOp::URem | BinOp::SRem) {
        let m = bin(f, &mut insts, b, BinOp::Mul, ty, q, Operand::Imm(k));
        bin(f, &mut insts, b, BinOp::Sub, ty, num, m)
    } else {
        q
    };

    // The last instruction must define the ORIGINAL dst, so no use has to be
    // rewritten and no copy is introduced.
    let Operand::Val(lastv) = last else { unreachable!("the sequence always ends in a value") };
    debug_assert_eq!(
        match insts.last() {
            Some(Inst::Bin { dst, .. }) | Some(Inst::Cvt { dst, .. }) => *dst,
            _ => unreachable!(),
        },
        lastv
    );
    match insts.last_mut() {
        Some(Inst::Bin { dst: d, .. }) | Some(Inst::Cvt { dst: d, .. }) => *d = dst,
        _ => unreachable!(),
    }
    // The value allocated for the final result is now unused; leaving it costs one
    // slot in `values` and nothing in the output (it has no definition and no use,
    // so `verify` never sees it as live). Removing it would renumber every later
    // value for no gain.
    let n = insts.len();
    f.blocks[b as usize].insts.splice(at..=at, insts);
    n
}

#[cfg(test)]
mod magic_tests {
    use super::*;

    /// EXHAUSTION over the divisor, boundaries over the numerator: the square is
    /// not an argument about the transcription, it is a check of it.
    #[test]
    fn magicu32_agrees_with_real_division() {
        let mut checked = 0u64;
        for d in 3u64..=4000 {
            if d.is_power_of_two() {
                continue;
            }
            let (m, s, add) = magicu(d as u128, 32);
            for n in [
                0u32,
                1,
                2,
                d as u32,
                (d as u32).wrapping_sub(1),
                (d as u32).wrapping_add(1),
                0x7fff_ffff,
                0x8000_0000,
                0xffff_ffff,
                0xffff_fffe,
                12345,
                0x1234_5678,
            ] {
                let hi = (((n as u64) * (m as u64)) >> 32) as u32;
                let q = if add {
                    (((n.wrapping_sub(hi)) >> 1).wrapping_add(hi)) >> (s - 1)
                } else {
                    hi >> s
                };
                assert_eq!(q, n / d as u32, "u32 {} / {} (M={:#x} s={} add={})", n, d, m, s, add);
                checked += 1;
            }
        }
        assert!(checked > 40_000, "the battery must actually run: {}", checked);
    }

    #[test]
    fn magicu64_agrees_with_real_division() {
        for d in [3u64, 5, 7, 10, 11, 13, 100, 997, 1_000_000_007, u32::MAX as u64 + 1 + 3] {
            if d.is_power_of_two() {
                continue;
            }
            let (m, s, add) = magicu(d as u128, 64);
            for n in [0u64, 1, d, d - 1, d + 1, u64::MAX, u64::MAX - 1, 1 << 63, 0x0123_4567_89ab_cdef]
            {
                let hi = (((n as u128) * m) >> 64) as u64;
                let q = if add {
                    ((n.wrapping_sub(hi)) >> 1).wrapping_add(hi) >> (s - 1)
                } else {
                    hi >> s
                };
                assert_eq!(q, n / d, "u64 {} / {} (M={:#x} s={} add={})", n, d, m, s, add);
            }
        }
    }

    #[test]
    fn magics32_agrees_with_real_division() {
        let mut checked = 0u64;
        for da in 3i64..=2000 {
            if (da as u64).is_power_of_two() {
                continue;
            }
            for d in [da as i32, -(da as i32)] {
                let (m, s) = magics(d as i128, 32);
                for n in [
                    0i32,
                    1,
                    -1,
                    d,
                    -d,
                    d.wrapping_sub(1),
                    d.wrapping_add(1),
                    i32::MAX,
                    i32::MIN + 1,
                    -12345,
                    12345,
                ] {
                    let mut h = (((n as i64) * (m as i64)) >> 32) as i32;
                    if d > 0 && m < 0 {
                        h = h.wrapping_add(n);
                    }
                    if d < 0 && m > 0 {
                        h = h.wrapping_sub(n);
                    }
                    let q0 = h >> s;
                    let q = q0.wrapping_add(((q0 as u32) >> 31) as i32);
                    assert_eq!(q, n / d, "i32 {} / {} (M={:#x} s={})", n, d, m, s);
                    checked += 1;
                }
            }
        }
        assert!(checked > 40_000, "the battery must actually run: {}", checked);
    }

    /// The refusals are a predicate, so they get a test of their own: a row that
    /// silently accepted `d = 0` would divide by zero at COMPILE time.
    #[test]
    fn worth_it_refuses_the_degenerate_divisors() {
        for k in [0i64, 1, -1, 2, 4, 8, 1024, -2, -1024] {
            assert!(!worth_it(k, 32, true), "signed {}", k);
            assert!(!worth_it(k, 64, true), "signed64 {}", k);
        }
        assert!(!worth_it(i32::MIN as i64, 32, true));
        assert!(!worth_it(i64::MIN, 64, true));
        for k in [0i64, 1, 2, 256] {
            assert!(!worth_it(k, 32, false), "unsigned {}", k);
        }
        for k in [3i64, 5, 7, 10, 13, 100, -3, -7] {
            assert!(worth_it(k, 32, true), "signed {} should be taken", k);
        }
        for k in [3i64, 5, 7, 10, 13, 100] {
            assert!(worth_it(k, 64, false), "unsigned {} should be taken", k);
        }
    }
}
