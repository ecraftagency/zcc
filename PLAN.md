# PLAN — the one grind in progress

**THIS FILE IS ALLOWED TO BE WRONG.** Every line is a HYPOTHESIS about a
compiler that does not exist yet. Nothing here may be cited from `src/`, and
nothing here is evidence of anything.

**The contract.** It holds **one grind**, not a list. At most 100 lines. A row
that cannot be stated in ten lines is not understood well enough to be here. It
is TRUNCATED, never appended to, and when the grind closes the file is emptied
first — every row leaving by exactly one of two doors: baked into `MECHANISM.md`
because it won, or written into its Part F as a refutation because it lost.

---

## THE GRIND: the counted reduction, four lanes wide

**WHAT THE COUNTERS SAY, and it decided the grind.** `perf` over the twelve worst
programs against gcc -O2: every one is COUNT-driven, none is chain-driven. zcc's
IPC is 4.1–6.7 and it retires 1.4x–4.6x the instructions. Seven of the twelve are
programs gcc VECTORIZED and zcc did not; five of those seven are one shape:

    a1_int_mix  17.4 insn/elem vs 4.25    a3_sdiv_mod 17.3 vs 4.73
    d4_goto      6.1 vs 2.84              e2_many_args 29.2 vs 6.40
    h2_revbits 171.3 vs 51.0

A counted loop carrying a counter and ONE accumulator over a body that is
otherwise a function of the counter. `vecprobe` finds 34 such loops in the suite
and 11 in sqlite, and that census UNDERCOUNTS — it skips loops with no memory
access, which is four of the five above.

**PRICED BEFORE BUILDING.** Assume the 4-lane form lands at 1.5x gcc's dynamic
count (missing forms, guard and tail) and IPC falls to 3.0:

    a1 2.757 -> 1.470   a3 2.667 -> 1.516   d4 2.108 -> 1.309   e2 2.752 -> 1.496

Suite 1.2091 -> **1.176**; optimistic bound (all five at 1.000) **1.153**.

**ROW 1 IS BUILT AND IS NOT ENOUGH — that was predicted, and it measured.**
`hir::pass::redjam` unrolls the counted reduction by four with four partial
accumulators, default-OFF. Correct: 4/4 targets match gcc, trip counts 0..24,
99..101, 1000 match, the `i0+3s` overflow boundary matches, the I64 guard is
proven load-bearing by excision (remove it and zcc loops forever), and it carries
its square.

**WHAT IT BOUGHT, on the only instrument that can see it.** Six BASE readings of
the same unchanged binary came back 1.2091, 1.2014, 1.2107, 1.2097, 1.2105,
1.1990 — a spread of 0.012, so the suite EXEC geomean cannot resolve anything
below about 1.5% and it buried this row entirely. `perf`'s dynamic instruction
count is deterministic and says exactly what happened:

    a1 34.8M -> 29.8M (0.856)   a3 51.8M -> 46.6M (0.898)
    d4 48.8M -> 40.8M (0.836)   e2 116.8M -> 102.9M (0.881)

12-16% of the executed instructions, on every target, against gcc's 8.4M / 14.2M
/ 22.7M / 25.7M. The row works and is three to four times short of the referee —
which is the shape the price predicted. Static cost is INSN 1.0147 -> 1.1782, so
it does not ship alone.

**ROW 2 IS FOUR SLICES, NOT ONE.** The lanes must become vector arithmetic. What
MIR already has: `VIntOp{Add,Sub,Mul,And,Or,Eor}`, `VDup`, `VExt`, `VAddv`,
`MemOp::Q`, `Arr::V4S/V2D`. What each target still needs:

| slice | new ISA forms | unlocks | worth alone |
|---|---|---|---|
| 1 | `sshll`/`sshll2` (4x i32 -> 2x2 i64 accumulator + exit reduce), `cmeq`, `bsl` | `d4_goto` | ~ -0.5% |
| 2 | `shl`/`sshr`/`ushr` | part of `a1`, `a3` | — |
| 3 | `smull`/`smull2`/`smlal` | `e2_many_args` | ~ -0.5% |
| 4 | the vector magic-division sequence | `a1`, `a3` | ~ -1.0% |

**All twenty-two forms were checked against `as` on the Graviton4 box and
assemble.** Slice 1 is first because the widening and the horizontal reduce are
shared by every other slice.

**THE MATCHER, and it is the hard half.** Larsen-Amarasinghe seeded from a known
root: start at the four accumulator parameters `redjam` leaves, walk the four def
chains backward in lockstep, requiring the same opcode at each step and operands
that are either (a) one value shared by all four (a `dup`), (b) themselves an
isomorphic four-tuple, or (c) the counters `i, i+1, i+2, i+3` (the seed vector).
Build packs bottom-up; refuse and leave the scalar lanes wherever isomorphism
fails. `slp.rs` is NOT this — it is FP-only and keyed on adjacent memory.

**WHAT IS OUT OF REACH AND WHY.** `h2_revbits` needs its outer loop vectorized
with `revbits` inlined, and `inline` refuses a callee containing a loop; its inner
loop carries three values. `o1_fp_dot` and `o2_fp_stencil` are FP reductions —
reassociation changes the value there and gcc does not do it either; gcc wins
those by unrolling the OUTER loop, which is a different row.

**REFUTED THIS GRIND, so nobody re-runs them.** `vecmap` switched on buys nothing
(EXEC 1.2014->1.2021, 1.2107->1.2081) and costs INSN 1.0147->1.1782; it fires on
ZERO of the seven hot programs because every one carries a value. A `smull`
widening-multiply isel row: 10 sites in the whole suite, all in `e2` alone.
`ZCC_HOISTMIN=1`, the seam that lifts a one-instruction constant out of a loop:
pairs 1.2105->1.2034 and 1.1990->**1.2185**, opposite signs, against a steady INSN
1.0147->1.0320 — noise bought with 1.7% size. `MEASURED M44`'s default stands.

⚠️ **THREE MEASUREMENTS WERE LOST TO THE INSTRUMENT THIS GRIND**, all mine: two
`exectime.sh` runs at once (it uses fixed `/tmp/g`, `/tmp/z`, so they overwrite
each other and print DIVERGE for the UNMODIFIED compiler), a runaway test binary
pinning a core through an A/B, and a `k=0` case whose own harness was UB. Check
`ps` and the harness's own summary line before believing a spread.
