# MEASURED — target facts with no spec to cite

Law 1 says every line of `src/` is either a theorem (Side I) or a constant
transcribed from a spec line (Side II). `THEORY.md` holds both, and Side II's
entries are **citations** — a section number in ISO 9899, AAPCS64, DDI 0487 or
the ELF ABI that a reader can look up.

Some facts have no such line to cite. **Apple publishes no Software Optimization
Guide for the M1**, so an instruction's latency, or whether a transform pays on
this machine, cannot be referenced — it can only be MEASURED. Those facts live
here rather than in `THEORY.md`, so that Law 1's two-side claim stays literally
true: `THEORY.md` II-* is cited spec and nothing else.

An entry is not an opinion. Each carries:

* **VALUE** — the number or verdict the compiler acts on;
* **METHOD** — the instrument and the command, so it can be re-taken;
* **WHEN / WHERE** — the date and the machine, because a measured fact is only
  true of the machine that produced it;
* **WHAT USES IT** — the site in `src/` that reads it, so a change here has a
  visible blast radius.

**THE STANDING CAUTION.** Every number below was taken on **Apple M1 Pro cores
under Docker**, while the notional target is generic AArch64-Linux. A measured
fact is evidence about the measuring machine first and about the target second.
Where the two could differ, say so in the entry.

Cite an entry from code as `MEASURED M<n>`, exactly as a spec fact is cited as
`THEORY II-<n>`. `tests/provenance.sh` checks that every citation names an entry
that exists.

---

## M1. Extended-register ALU latency — 2 cycles against 1

**VALUE.** On this machine `add xN, xN, wM, sxtw` has a 2-cycle latency where
`add xN, xN, xM` has 1. The two are the same instruction COUNT, so `cost = |MIR|`
scores them identically and always will.

**METHOD.** j3_prefix_sum's loop-carried recurrence is `acc += ext(load)`. With
the extension in the ALU the recurrence bound is 2.0; with `ldrsw` doing the
extension in the load it is 1.0. Predicted from that table alone, with no build:
**2.0**. Measured: **1.940** — a 3% error. After the transform: **1.000**.

**WHEN / WHERE.** 2026-08-25, M1 Pro under Docker, `tests/bench/exectime.sh`.

**WHAT USES IT.** `isel/lower.rs`'s extending-load row prefers the extension in
the LOAD over the ALU operand; `mir/pass/ext.rs::plain_operand` drops an operand
extension the lattice proves is a no-op. Neither is justified by instruction
count — both are justified by this entry.

**CAUTION.** A core with a different extended-register path would not show this.
The transform is never WRONG there, only unmotivated.

---

## M2. The UNIT-STRIDE pointer / 64-bit induction variable is NEGATIVE on this target

**VALUE.** Rewriting a recomputed `[base, w, sxtw #k]` address into a pointer
walked by a post-index writeback makes zcc measurably WORSE. `hir/pass/iv.rs`
ships that half default-OFF because of this entry.

**SCOPE, narrowed 2026-08-26.** This entry is about a step EQUAL to the access
size, and only that. It is what the A/B below varied, and it is the only case
A64's scaled index reaches: `ldr Xt,[Xn,Xm,lsl #3]` scales by the access size
and by nothing else (DDI 0487 C6.2.130). An address whose step the mode cannot
express — `B[k][j]` walking a 240 x 8-byte ROW, step 1920 — is rebuilt with a
MULTIPLY on every iteration, so replacing it with an `add` costs the same
instruction count and removes a multiply from in front of a load. That half
ships ON and has its own fact, M9; nothing here measured it.

**METHOD.** `ZCC_IV=1` A/B over the 35-program suite, twice, on two different
compilers. §13k (pre-R4.7): EXEC ≥30 ms 1.3789 → 1.4087, INSN 1.2419 → 1.2454,
sqlite +1,276. Re-taken post-R4.7 (2026-08-25): INSN **1.1493 → 1.1538**, EXEC
**1.2044 → 1.2140**, programs above 1.1× 8 → 9.

**WHY, and this is the part that generalizes.** A64's scaled-index addressing
form makes rebuilding an address from a counter FREE — there is nothing to
strength-reduce. R4.7 then removed the one thing that was not free about it (the
`sxtw` feeding the loop-carried chain, M1), which is why the re-measurement is
worse than the first.

**WHEN / WHERE.** 2026-08-25, M1 Pro under Docker.

**WHAT USES IT.** `hir/pass/iv.rs::ENABLED = false` — which now gates the
unit-stride half alone (`strengthen`'s `unit` parameter), not the whole pass.

**RE-ENTRY TRIGGER.** §13k's own gate: a cost model that can say WHEN a
writeback pays. Until one exists this stays off. j5_insertion_sort is the one
program where it would pay, which is a statement about j5, not about the target.

---

## M3. The copy-partner graph saturates at depth 3

**VALUE.** `regalloc/color.rs` follows the copy-partner graph three hops looking
for a coloured member to bias toward. Three, not one and not eight.

**METHOD.** Swept on sqlite, `ZCC_CODEPTH=<n>`, whole-module instruction count:

| depth | 1 | 2 | **3** | 5 | 8 | 16 |
|---|---|---|---|---|---|---|
| insns | 188,659 | 187,260 | **187,097** | 187,081 | 187,104 | 187,104 |

Depth 1 is the old one-hop behaviour. It is flat from 3 on; 5 buys 16
instructions and 8 gives them back.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker.

**WHAT USES IT.** `regalloc/color.rs`, the `depth` bound in `assign`.

**CAUTION.** This is a property of sqlite's copy graph, not of the ISA. A corpus
with longer copy chains would move it.

---

## M4. The jump-table crossover is ~24 arms, and a BALANCED TREE never wins

**RE-TAKEN 2026-08-26, and the earlier entry is superseded below.** The first
attempt could not separate the forms because it swept a synthetic whose arms did
too little work. This one sweeps 4…64 arms with a pseudorandom index AND with a
repeating one, and the two agree.

**VALUE.** `isel/lower.rs::MIN_CASES = 24`, was 4 (chosen by taste at R3.3).

**METHOD.** Three dispatch forms, same program, ms best-of-7, outputs compared
first. Unpredictable index:

| arms | gcc | chain | tree | table |
|---|---|---|---|---|
| 4 | 21 | **46** | 53 | 54 |
| 8 | 36 | **54** | 69 | 62 |
| 16 | 49 | **62** | 84 | 65 |
| 32 | 50 | 71 | 98 | **67** |
| 64 | 53 | 87 | 111 | **68** |

Crossover, both index kinds (chain / table): 16 → 62/65 and 11/12 · 20 → 66/67
and 12/12 · 24 → 68/**67** and 14/**12** · 28 → 70/**67** and 15/**12**. The
chain is better or equal to 20 arms and the table wins from 24, whether the
index repeats or not. 21…23 were not measured and the constant does not pretend
otherwise — 24 is the first size where the table actually wins.

**THE BALANCED SEARCH TREE IS REFUTED.** It was built, proven and measured, and
it loses at EVERY size from 4 to 64 — at 16 arms, chain 62 ms, table 65, tree 84.
It asks strictly fewer questions (4 against 7 on d1_switch) and takes more time,
because the chain's tests FALL THROUGH while the tree spends a taken branch per
level and scatters the arms. Law 3c pointing the other way: fewer questions is
not less time either. The code was removed rather than kept behind a flag,
because no measured size wants it.

**RESULT.** d1_switch **1.500 → 1.200**; geo40 EXEC 1.0240 → **1.0180**; sqlite
173,344 → 173,519 (+175, +0.1%), which is the Law 0 ordering — `exec > size`.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, gcc 14.2.0.

**WHAT USES IT.** `isel/lower.rs::MIN_CASES`, and `jump_table`'s density test.

**OPEN, and FOUR HYPOTHESES REFUTED.** d1 sits at 1.200, and none of the obvious
explanations survives a controlled hand-edit (same file, one change, outputs
compared, best-of-11):

| d1 variant | ms |
|---|---|
| gcc -O1 | 10 |
| **zcc, compare chain (what ships)** | **12** |
| gcc's dispatch shape transcribed verbatim into zcc | 13 |
| `csel` on the last arm (if-converted arm body) | 12 |
| `tbnz` range split | 15 |
| counter widened to 64 bits, arms read `x1` (no `sxtw`) | 13 |

Transcribing gcc's own shape makes zcc SLOWER. The branchless arms buy nothing.
The range split hurts. Widening the counter — which removes three
extended-register operands (`MEASURED M1`) from the loop-carried accumulator —
also loses. Whatever the last 2 ms is, it is not the switch and not the arms, and
four experiments did not find it.

QUARANTINED at 1.200 rather than guessed at further. The re-entry is **R4.18**,
the time-dual cost model: this is precisely the case it exists for — a program at
INSN 1.077 whose remaining time gap no instruction-level reasoning has located.

---

## M4-superseded. A jump table and a compare tree are indistinguishable by case count

**VALUE.** `isel/lower.rs::MIN_CASES = 4` is UNSETTLED. The measurement does not
support any constant derived from the case count, so the R3.3 value stands
unchanged rather than being replaced by a fitted one.

**METHOD, and it is the disagreement that is the finding.**

* d1_switch (8 cases), repeatedly and directly: jump table **15 ms**, compare
  tree **12 ms** — the tree wins by 20% while emitting **12 MORE instructions**
  (95 against 83). The table's indirect branch is unpredictable.
* A synthetic sweep at 4, 6, 8, 12, 16, 24 and 32 cases, with a pseudorandom
  (unpredictable) index: table and tree within **1 ms of each other at every
  case count**.
* Whole-suite A/B: `ZCC_JT=9` moved the EXEC geomean 1.0899 → 1.0639, but d1
  alone moves 13% and the geomean would need 35% from it — the rest is
  cross-program noise.

**THE CONCLUSION.** The case count is not the variable. Something about d1's
switch — not how many arms it has — decides it, and no constant over arm-count
would be honest. `ZCC_JT` is left in place as the instrument.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker.

**WHAT USES IT.** `isel/lower.rs::MIN_CASES`, and `jump_table`'s density test.

---

## M5. The `ldp`/`stp` pairing window saturates within ten instructions

**VALUE.** `mir/pass/ldstp.rs::WINDOW = 10`.

**METHOD.** Distance distribution of pairable frame accesses on sqlite, after
the spills-first frame layout: 433 adjacent, then 302, 299, 144, 117, 116, 107,
102, 97, 90 at distances 2…10 — 1,807 in total, of which 761 are refused by the
paired form's imm7 range regardless of distance. The tail beyond ten is flat.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, `tests/bench/excess.sh`.

**WHAT USES IT.** `mir/pass/ldstp.rs::WINDOW`.

---

## M6. `alg.sh` is bound by zcc's compile time, not by the harness

**VALUE.** The expression-algebra gate does not scale past two workers, and the
reason is zcc, not the script.

**METHOD.** `ALG_JOBS` sweep: 1 → 98s, 2 → 57s, 4 → 54s, 8 → 54s, 16 → 57s.
Profiled: generation 147ms, the eleven `run` cases compile in **zcc 73.0s
against cc 4.2s** (17×) on 3.4k-line files, the runs take 8ms, concatenation and
diff 0ms.

**WHY.** These are exhaustively generated op × type × corner files — one huge
function each — which is the same superlinear compile-time shape that produced
the yarpgen CTIMEOUTs before the release-build fix.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker.

**WHAT USES IT.** `tests/alg.sh`'s comment, and §CP's target list. Nothing in
`src/` reads this; it is here so the next person to look at gate speed does not
re-derive it.

---

## M7. `MAX_HEADER_INSTS = 20` is gcc's number, not a spec's

**VALUE.** `hir/pass/rotate.rs::MAX_HEADER_INSTS = 20` — the largest loop header
worth copying.

**METHOD.** NOT measured here. It is gcc's own -O1 value for the same transform
(`--param max-loop-header-insns`, default 20, read by `-ftree-ch`), taken
because rotation trades a STATIC copy of the header for a DYNAMIC branch per
iteration — an exchange rate, so no bound falls out of the theorem.

**WHY IT IS HERE AND NOT IN THEORY.md.** gcc's default is not a specification.
Recording it as a Side-II citation would be inventing provenance, which is the
Article E failure this file exists to prevent.

**WHAT USES IT.** `hir/pass/rotate.rs`.

**OPEN.** Never swept on this corpus. A sweep would move it from "gcc's number"
to a measured one; until then it is honestly labelled.

---

## M8. The if-conversion arm bound is 2, and it is REASONED, not measured

**VALUE.** `hir/pass/ifconv.rs::ARM_LIMIT = 2` — the most instructions an arm may
hold and still be if-converted into a `select`.

**METHOD.** NOT measured. It is a reading of the trade: converting replaces a
compare, a taken branch and the pipeline bubble a misprediction costs with
unconditional work on both arms, so the bound is "fewer instructions than a
mispredict costs". Two is the conservative reading, and the shape this pass
exists for — a join parameter and nothing else — needs none at all.

**WHY IT IS HERE AND NOT IN THEORY.md.** There is no spec line for the cost of a
branch misprediction on this core, and none was measured. Recording it as a
Side-II citation would be inventing provenance. Labelled honestly instead.

**WHAT USES IT.** `hir/pass/ifconv.rs`.

**OPEN.** Never swept. A sweep over the suite would move it from "the
conservative reading" to a measured entry — and `csel` sits at 599 against gcc's
542, so the bound is not currently costing much either way.

## M9. A ROW-STRIDED pointer IV is POSITIVE on this target

**VALUE.** When a loop's load address advances by a step the addressing mode
cannot express, walking a pointer removes a MULTIPLY from in front of the load
at the same instruction count. `hir/pass/iv.rs` ships this half ON.

**METHOD.** `tests/bench/matmul.c` — `s += A[i][k] * B[k][j]`, where `B[k][j]`
walks a 240 x 8-byte row, step 1920. The k-loop is seven instructions either
way; the difference is one `madd x12,x11,x4,x1` computing the address against
one `add x14,x14,#1920` advancing a pointer. Both forms were HAND-ASSEMBLED from
the same zcc output and linked and run side by side, so nothing but that one
instruction differs, and both print `414714994`:

| k-loop form | ms, best of 5 | vs gcc -O1 |
|---|---|---|
| gcc -O1 (same shape as the pointer walk) | 69 | 1.000 |
| zcc, address rebuilt with `madd` | 113 | 1.638 |
| zcc, pointer walked by `add #1920` | 69 | **1.000** |

Adding gcc's other two tricks on top — post-index writeback for the `A` load and
a pointer-limit exit test instead of a counter, six instructions — changed
nothing: also 69 ms. The whole gap is the multiply.

**WHY.** The multiply sits at the head of a dependence chain that ends in a
strided load, and a strided load is where the machine most needs its address
early. `cost = |MIR|` cannot see this: the instruction COUNT is identical. It is
the same kind of fact as M1, and it is judged the same way — on the clock.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, `zcc-box:latest`, gcc 14.2.0.

**WHAT USES IT.** `hir/pass/iv.rs::strengthen` — the `scaled` test that separates
this half from M2's.

**OPEN.** Stores are still refused (M2's j2_histogram argument, which is about
the unit-stride case). `ZCC_IVDBG=1` prints the residual per refusal reason;
matmul still reports 17 `scev-refused` and 3 `unit-stride-gated`.

---

## M10. Instruction latency on this core, in units of a dependent `add`

**VALUE.** The Side-II table the time model reads (`mir/cost.rs::latency`).

| latency | forms |
|---|---|
| **1** | `add`/`sub`/`and`/`orr`/`eor` (reg or imm), `lsl` (imm or reg), `csel`, `sxtw`, `uxtb`, `ubfx`, `mvn`, `rev`, and **`madd` reached through its ACCUMULATOR** |
| **2** | `add x,x,x,lsl #n` and `add x,x,w,sxtw` — a shifted or extended register operand |
| **3** | `mul`, `madd` reached through a MULTIPLICAND, `ldr` L1 hit (plain or register-offset) |
| **7** | `sdiv`, `udiv` |

**METHOD.** `tests/bench/latency.sh`. Time a loop whose body is 32 copies of one
instruction, each reading the register the previous wrote. The chain cannot
overlap, so wall time is `K x latency x iterations` whatever the core does about
width or reordering — and dividing by the same measurement for `add x0,x0,#1`
cancels the clock, which is why no frequency is needed and the answer is a ratio.
Measured ratios: 1.00 / 2.02 / 3.02 / 7.05, with a `nop` control at **0.12**
confirming the harness is not measuring itself.

**THE ONE THAT CHANGES DESIGN DECISIONS.** `madd` is TWO latencies in one
instruction: 3.02 through a multiplicand, **1.00 through the accumulator**. So
`s += a*b` accumulation is not multiply-bound, and a loop that looks
multiply-heavy may have a one-cycle recurrence. matmul is exactly that, which is
why a recurrence-only model could not see its gap and `Bound` grew a second axis.

**IT RE-DERIVES WHAT WAS ALREADY MEASURED**, which is R4.18's ship condition:

| case | from the table alone | measured on the clock | error |
|---|---|---|---|
| `loops.c`, `mul`+`add` (3+1) becomes `madd` (3) | 4/3 = **1.333x** | 771/565 = **1.365x** | 2.3% |
| j3, extended operand (2) becomes `ldrsw`+`add` (1) | **2.00x** | **1.940x** | 3% |
| matmul, `madd` address vs pointer walk | addr **3 -> 0** | 113/69 = 1.638x | direction |

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, `zcc-box`, gcc 14.2.0.

**WHAT USES IT.** `src/mir/cost.rs`. `ZCC_CYCLES=1` prints the per-loop bounds.

**OPEN.** The FP forms, `Call`, and the `ldp`/`stp` pair are unmeasured and take
the ALU default of 1; a loop containing a call is reported UNSCORED rather than
guessed at. Issue width, ports, the reorder window, cache misses and branch
misprediction are not modelled at all — the recurrence is a LOWER bound, and
programs it scores at 1 while they run slower (j5, g1, d1) are bounded by
something else, which is itself a useful verdict.

---

## M11. Tail-duplicating a loop latch pays only at a MULTI-WAY dispatch

**VALUE.** `mir/pass/layout.rs::duplicate_latch` copies a loop tail into its
predecessors only when **three or more** of them reach it by an unconditional
branch.

**METHOD.** d1_switch's switch arms each end `b .Lwork_3`, and that block is the
whole loop tail — bump the counter, test it, branch back. Every iteration paid
TWO taken branches to reach the top. Hand-validated in zcc's own `.s` before the
pass was written (three passes, output identical at 8000006000000):

| d1_switch | ms |
|---|---|
| gcc -O1 | 10 |
| zcc, arms jump to a shared tail | 12 |
| zcc, tail copied into each arm | **10** |

**THE THRESHOLD, and what it cost to find.** Firing on TWO or more predecessors
— which describes any if-else join — fired on nearly every loop in the suite:

| predecessors required | geo40 EXEC | geo40 INSN | sqlite |
|---|---|---|---|
| ≥ 2 | 0.9430 | **1.3668** (32 of 35 above 1.1×) | +3,906 |
| **≥ 3** | **0.9494** | **1.0432** | **+840** |

33% of size for 2% of time is the trade R4.14 refused at 16-for-7. Three is the
count that distinguishes a multi-way dispatch from a two-armed join, which is
where a second branch per iteration actually repeats.

**AN EARLIER FENCE, AND THE VERIFIER THAT FOUND IT MISSING.** The first cut
tested only "conditional terminator, ≥2 unconditional predecessors" — describing
any join — and duplicating a join that reloads a spilled value moved the reload
above its store on one path. `regalloc::verify` said so at once: "reload of
unstored slot 31". A loop TAIL is a join whose terminator branches BACK to a
block that dominates it, and that is what the pass tests now.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, gcc 14.2.0.

**WHAT USES IT.** `mir/pass/layout.rs::duplicate_latch`.

**OPEN — THE THRESHOLD IS UNSWEPT.** 2 and 3 were measured; 4, 5 and beyond
were not. Three is where the measurement stopped, not where it was shown to be
best, and this entry says so rather than dressing a plausible story as a fact —
`MIN_CASES = 4` sat unswept in `isel/lower.rs` for a milestone and cost d1 50%
when someone finally measured it (`MEASURED M4`). Sweeping 4/5/6 on INSN and
sqlite is deterministic and needs no quiet box.

---

## M12. Assumed trips per loop level is TEN, and the choice is not load-bearing

**VALUE.** `TRIPS = 10` in `regalloc/spill.rs`. The spiller's next-use distance
is measured along the execution trace, and a value whose next use lies OUTSIDE
the current loop is reached only after the iterations still to run; that count
is unknowable statically, so the model assumes ten per nesting level — the same
convention as gcc's `10^depth` block frequency.

**METHOD.** The number is a cost-model parameter, so the honest question is not
"is ten right?" (no static analysis can know) but Article E's: *is this the
spec's number or my convenience's number?* Answered by sweeping it and showing
the decisions barely move. `ZCC_TRIPS` was made to override the constant and
sqlite plus all 35 taxonomy kernels were compiled at 1, 2, 3, 4, 5, 10, 20, 100
and 1000:

| TRIPS | sqlite instructions | taxonomy suite |
|---|---|---|
| 1 | 175,452 | byte-identical throughout |
| 2 | 175,438 | ″ |
| 5 | 175,405 | ″ |
| **10** | **175,394** | ″ |
| 20 | 175,390 | ″ |
| 100 | 175,380 | ″ |
| 1000 | 175,380 (identical bytes to 100) | ″ |

The whole three-orders-of-magnitude sweep moves sqlite by **72 instructions,
0.04%**, monotonically, and saturates at 100 — beyond which no ranking changes
at all. The taxonomy suite does not move by one byte at any value, which is a
second reading of the same fact recorded in `SPILL.md` §4a: none of its kernels
is under enough register pressure to spill, so nothing there can see this
constant. Ten sits on the flat part of a flat curve.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `regalloc/spill.rs::Trace::next_use` — the step that leaves a
loop the value is not wanted in, and only that step.

**WHAT WOULD MAKE IT MATTER.** A body long enough that one loop's remaining
instructions outweigh a factor of ten across a level — deep nests over long
bodies. Nothing in the current corpus is that shape; a program that is would
show up as a sqlite-scale gap between TRIPS=10 and TRIPS=100, which today is
14 instructions.
