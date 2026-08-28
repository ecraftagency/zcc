# PLAN — the one grind in progress

**THIS FILE IS ALLOWED TO BE WRONG.** Every line is a HYPOTHESIS about a
compiler that does not exist yet. Nothing here may be cited from `src/`, and
nothing here is evidence of anything.

**The contract.** It holds **one grind**, not a list. At most 100 lines. A row
that cannot be stated in ten lines is not understood well enough to be here. It
is TRUNCATED, never appended to, and when the grind closes the file is emptied
first — every row leaving by exactly one of two doors: baked into `MECHANISM.md`
because it won, or written into its Part F as a refutation because it lost.

An open row belonging to a campaign that has already closed is not a plan, it is
a FACT about what was not built; it lives in that campaign's status table in
`MECHANISM.md` Parts C and D. It does not come back here.

**Why the cap and the one-grind rule exist:** `REARCH.md` reached 3,194 lines by
accumulating every campaign's plan beside every campaign's results, until no
reader could tell which lines were true of the compiler that was green. It was
dismantled on 2026-08-28.

---

## THE GRIND: the tail of a suite that is now ninety programs wide

**THE SUITE WAS WIDENED and the number went up seven points** (`M35`): EXEC
1.020 → **1.0916**, INSN 1.0701 → **1.0892**, 29 of 90 above 1.1×. Nothing got
slower — the measurement caught up with the compiler, because every row shipped
before this was measured on the 49 programs it was tuned against.

**The compiler held.** Forty-one new programs written against documented blind
spots, never tuned for, spread 0.918–5.045, median 1.038, and **zcc beats gcc -O1
on eight of them**.

**⭐ START HERE: `x1_goto_cleanup`, 5.045× — three times worse than anything this
project has measured**, and it is not exotic: it is the `goto out;` cleanup
ladder, C's only error mechanism, in every kernel driver and every library entry
point. Its CFG is a fan-in, many early exits converging on one block with values
live across all of it. Nothing in the old suite had that shape. Next after it:
`z2_rle` 1.61, `u4_popcnt64` (INSN 2.88), `q2_deep_rec` 1.41, `o2_fp_stencil`
1.41, `o3_fp_mixed` 1.40.

**Method, unchanged and it is what found everything:** rank the program's blocks
by executions (`ZCC_WEIGHTS=1 ZCC_WCOST=1`), read the top two against gcc's same
loop, hand-edit the `.s` and verify the output, time it with alternating
best-of-N — and only then touch the compiler.

**The gate every row owes:** a commuting square, both axes, and for EXEC an
interleaved A/B in ONE box session (`tests/bench/abpair.sh`). Two `realprog.sh`
runs are not a comparison: on 2026-08-28 the gcc side moved 7.6% between two of
them.
