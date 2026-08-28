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

## THE GRIND: what is left of the register copy, after C0 measured it

**C0 IS DISCHARGED and it moved the ground.** The attribution table is
`MECHANISM.md` Part F `M26-correction`; the refusal census is `M27`. Two things
this campaign was built on turned out to be instrument defects, so the rows below
are the ones that survive:

- the copy family is **44% of the gap, not 70%** — 289 of the suite's 518
  "block-edge copies" are `mov wN, wzr`, a constant zero gcc pays one instruction
  for as well;
- the hint refusals are **not occupancy** (3 of 488 on the suite, 136 of 14,640
  on sqlite) — they are the AAPCS64 half and the zero register. **C2 as written
  is retired**: eviction and priority colouring aim at a bucket that is not there.

**BANKED.** C0-ROW-1, partner-aware half selection (`ZCC_CSBIAS`, default 1):
sqlite runtime 0.9740 interleaved, sqlite −467 instructions, suite −6, EXEC
unchanged, gate 15/0.

**WHAT IS LEFT, and it is 203 instructions of a 953 gap on the suite.**

**C1 — the residual FREE.** 203 pairs on the suite, 4,965 on sqlite, where both
ends are virtual and the argument dies on the edge, so the merge is legal.
`ZCC_CSBIAS` addresses the ABI-banned share (72 / 1,554). The remainder is 77
genuinely occupied and 42 where the transitive walk reached a different partner
first. Re-take `ZCC_HINT` with the row default-on before proposing anything: the
denominator has moved.

**C4 — ABI argument placement.** A separate front and, on sqlite, the larger one:
`ZCC_MOVKIND` counts 24,374 marshalling copies against 7,791 edge copies, and
x0–x7 traffic is 22,813 against gcc's 14,626. Do not fold it into C1's
measurement.

**MEASURED AND CLOSED THIS SESSION** (`M28`): the IVX trip-count gate is not the
binding one — removing it changes the suite by zero instructions — and SCEV's own
coverage is 78% of that pass's residual. Do not spend a session on the gate.

**AND THE ROW C0 UNCOVERED, which is not a copy row at all.** `M26-correction`
inverted the constant-materialization column: zcc emits **951 against gcc's 790,
+161**, where the old table read −182 and was taken as proof the constant-sharing
row worked. It is now the second-largest family in the gap and nothing has ever
been aimed at it. First question, unanswered: `isa::mov_chain` knows
`movz`/`movn`/`movk` and does NOT consider `orr wd, wzr, #imm`, though
`isa::logical_imm` — the encodability test — already exists and is used only for
ALU operands. Count the constants whose chain is ≥2 and which `logical_imm`
accepts before writing anything.

**BANKED, and it is the session's largest win.** `promote::sink_stores`
(`MEASURED M29`): a loop-carried slot promoted to a register left its store alone
in the latch's split block, three executed instructions and a taken branch in a
fifteen-instruction body running 5.76M times. **n7_nested_subq 1.370 → 1.195**,
suite EXEC 1.0235 → 1.0210, gate 15/0.

**THE INSTRUMENT THIS EXPOSED, and it is the next thing to build.** The INSN
geomean moved 1.0714 → 1.0706 for that. A static count weighs a latch executed
5.76M times exactly as it weighs a cold arm, so **`cost = |MIR|` cannot rank a
codegen row by time** — a third blindness beside Law 3c's chains. zcc already
carries the frequencies (`hir::freq::annotate` → `MBlock.weight`): build
`Σ_b weight(b)·|insts(b)|` and rank the remaining rows on it. Every 1.3–1.4×
program left (m1 1.433, n1 1.316, m2 1.318, a2/a3 ~1.11) should be re-read with
that number in hand rather than by eye.

**REFUTED THE SAME SESSION** (`M28`): the sign-extended index. zcc emits 141
memory operands of the form `[base, wN, sxtw]` against gcc's 11, 78 in
`k1_dispatch` alone — and rewriting 73 of them by hand, output identical and
instruction count identical, bought **0.5%** (0.9946, best-of-40 alternating; a
first best-of-10 said 0.9798 and was noise). The extension is free inside an
addressing mode on this core, which is a correction to `M1`'s SCOPE: that 2-cycle
fact is about an ALU operand. Do not open induction-variable widening on the
strength of this count.

**The gate every row owes:** a commuting square, both axes, and for EXEC an
interleaved A/B in ONE box session (`tests/bench/abpair.sh`). Two `realprog.sh`
runs are not a comparison: on 2026-08-28 the gcc side moved 7.6% between two of
them.
