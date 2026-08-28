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

**BANKED — four rows, and the shape of the day is that the geomean is the wrong
scoreboard.** Suite EXEC **1.0204**, INSN **1.0688**, gate 15/0. But 11 of 49
programs are still above 1.1× and the worst is 1.44, so the 2% left in the
geomean is not where the work is.

| row | what it was | effect |
|---|---|---|
| `ZCC_CSBIAS` | partner-aware callee-saved bias (`M27`) | sqlite runtime 0.9740 |
| `promote::sink_stores` | the latch store, sunk into its producer (`M29`) | n7 1.370 → **1.195** |
| `cost::weighted` + `ZCC_WCOST` | the executions model (`M30`) | the instrument, not a row |
| isel commutative-immediate swap | `97 + x` lowered as `add x, #97` (`M30`) | EXEC −0.002, INSN −0.0018, both pairs |

**THE METHOD THAT PRODUCED ALL OF IT, and it is repeatable:** rank the failing
program's blocks by `ZCC_WEIGHTS=1 ZCC_WCOST=1`, read the top two against gcc's
same loop, hand-edit the `.s`, verify the output, time it with alternating
best-of-N — and only then touch the compiler. n7's win was 13% of a program for
0.0008 of the INSN geomean; m1's two hottest blocks are 58% of that program and
the static count ranks them nineteenth.

**BANKED — the switch-arm order** (`M31`, `isel::order_switch_arms`): staying
arms first, following `v` through a `Select` as well as an edge (that second half
matters — `ifconv` turns `if (--want==0) st = S_CR;` into a select, which is
exactly where the hot arm's old state hides). **m2_http_parse 1.318 → 1.229**,
INSN unchanged, gate 15/0.

**AND ITS RESIDUAL IS A PROFILE, chased to the end.** Identification is solved —
5 of m1's 6 arms and 7 of m2's 9 now qualify — so what is left is RANKING, and
nothing static separates them: hand-ordering m2's hot arm first measures 0.8198
(→ **1.008**), a `{4,5,6,7}`-cycle rule measures 0.8927 (→ 1.097) but its premise
is false because `S_DONE → S_METHOD` makes all nine states one SCC. Which arm is
hot is a property of the INPUT. **Do not build another static rule here** — it is
the project's first row whose answer is PGO, worth ~0.11 of the suite's 0.97 log
mass.

**OPEN, MEASURED, AND THE CHEAP FIX IS REFUTED** (`M32`): the inliner refuses
`n1_btree_page`'s four-instruction `get2` at all **seventeen** call sites where
gcc inlines it everywhere. Inlining it in the SOURCE measures **0.9567**
(n1 1.312 → 1.255) and thirteen instructions FEWER. It is upstream of n1's
constant row: the calls are what make all ten callee-saved registers live, which
is why the loop-invariant LCG constants are rebuilt eight instructions per
iteration in a block worth 54% of the program.

The diagnosis stands — `body_size` counts HIR nodes, `call_cost` counts machine
instructions, and the mismatch is systematic against inlining. The obvious fix
(subtract zero-extensions, DDI 0487 B1.2.1) did NOT admit `get2` and DID admit
others: INSN 1.0688 → 1.0739, reverted. What the row needs is
`args + 3 + |live across the site|`, and HIR has no general liveness. **Build the
liveness or leave the row** — every substitute is a threshold somebody picked,
which is what `call_cost`'s own comment refuses.

**THE SCOREBOARD IS THE TAIL, NOT THE GEOMEAN.** Σln(rᵢ) ≈ 0.97 over 49
programs, and essentially all of the positive mass is the 11–13 programs above
1.1× (m1 0.35, n1 0.28, m2 0.22, n7 0.18, k1 0.17, …≈1.8 between them), offset by
the programs already below 1.0 (a1 −0.13, n4 −0.06). Take that tail to parity and
Σln ≈ −0.83, i.e. geomean ≈ **0.983 — sub-1×**. So the path is NOT seventeen
0.2% levers; it is eleven programs each needing a 10–40% PROGRAM-level win, which
is the size of the two taken today. Count programs leaving the tail, not geomean
deltas.

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
