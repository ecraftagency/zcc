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

## M11-correction. "Locally evictable" counts ONE of three conditions

**WHAT THE REPORT SAYS.** `ZCC_HINT=1` prints, of the hints refused because the
wanted register was occupied, how many have an occupant that "dies in this block
(locally evictable)". On sqlite that is 8,696 of 14,764, and the FULL-RANGE line
then says a register is free across the occupant's whole range in 100% of them.

**WHY THAT IS NOT A CEILING.** `HINT_OCC_LOCAL` tests only that the occupant's
LAST USE is in this block. A value can die here and still be LIVE-IN, its range
reaching back through dominating blocks the colourer walked earlier and keeps no
occupancy record of. Recolouring one of those changes its register in those
blocks too. Measured, by building the mechanism and running it:
`regalloc::verify` stopped the compile at
`unixShmSystemLock: V(4) and V(25) are both live at bb0[3] and both hold Gpr9`.

**THE REAL NUMBER.** Restricted to occupants DEFINED in this block, dying in it,
and not live-out — the case a block-local history can actually justify — the
recolour fires **7 times in the whole of sqlite**, for −37 instructions. Seven,
against a reported eight thousand six hundred.

**WHAT USES IT.** Nothing, now: the mechanism was reverted (`SPILL.md` §4b).
The entry exists so the next reader of that column knows it is an upper bound on
an upper bound, and so the row is not attempted a seventh time on the strength
of the same number.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

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

---

## M13. The argument registers go LAST in the caller-saved half

**VALUE.** `GPR_ORDER` offers x8–x15 before x0–x7. The allocatable SET is
AAPCS64 §6.1.1 and does not change; only the order `assign` walks when a value
has no coalescing hint.

**WHY IT COULD MATTER.** x0–x7 are the only registers a call can demand by
name. `assign` picks `hint.or_else(|| alloc_order.find(free))`, so with x0
first every unhinted value in the function takes an argument register before
anything else — and the argument that later wants x0 finds it occupied, which
is one `mov` per refusal. The instrument (`ZCC_HINT=1`) had already measured
the refusals: **34,569 hints wanted, 55.4% taken, 15,348 refused because the
register was OCCUPIED**, never for want of a free register (0 refusals had no
spare).

**METHOD.** sqlite compiled with both orders, same binary otherwise:

| | x0-first | x8-first |
|---|---|---|
| reg-reg `mov` | 31,352 | **30,669** |
| of those, writing x0–x7 | 22,829 | **19,985** |
| file instructions | 175,407 | **174,677** |
| hint hit rate | 55.4% | 56.7% |

−730 instructions, 1.1167× → 1.1120× against gcc -O1.

**WHAT IT DOES NOT FIX, and the number that says so.** The hit rate moves by
1.3 points. 14,879 hints are still refused because the wanted register is
occupied, and for **8,784** of them the occupant dies inside the same block
with a register free across its WHOLE range — the colourer computes that in
its statistics replay and acts on none of it. Reordering cannot reach those:
they need the occupant RE-COLOURED, which greedy colouring in dominance order
does not do. That is the open lever, and it is larger than this one.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `regalloc/color.rs::assign`'s fallback scan — and nothing
else: masks, `k`, and the callee-saved set are all order-independent.

---

## M14. A copy of 32 bytes or less is cheaper open-coded than called

**VALUE.** `INLINE_COPY_MAX = 32` in `isel/lower.rs`. An `Inst::MemCpy` of this
many bytes or fewer becomes loads and stores; anything larger stays a call to
`memcpy`.

**WHY THERE IS A DECISION AT ALL.** C says a by-value parameter IS a local
object, so the frontend homes one by copying the incoming registers into the
local's storage. For a four-`int` struct that is a sixteen-byte `MemCpy`, and
lowering it to `bl memcpy` costs far more than the copy: the call itself, a
frame and an x30 save in what would otherwise be a LEAF function, and a
clobbered caller-saved half at the point where the argument registers are still
live. `e3_struct_byval` was **2.630× gcc -O1 on the clock — the worst program in
the taxonomy suite on both axes** — for a copy gcc does not perform at all.

**METHOD.** The threshold trades size against that cost, so it was swept rather
than chosen. sqlite compiled at nine settings, everything else identical:

| bound (bytes) | sqlite instructions |
|---|---|
| 0 (always call) | 174,677 |
| 8 | 174,659 |
| 16 | 174,604 |
| **32** | **174,572** |
| 48 | 174,584 |
| 64 | 174,604 |
| 128 | 174,703 |
| 256 | 174,703 |

A clean minimum at 32, and past 64 the open-coded form is worse than not
inlining at all — which is the shape the trade predicts, since a call is four
instructions whatever the length while the expansion grows with it.

**WHAT IT BOUGHT ON THE CLOCK.** `e3_struct_byval` 2.630× → **1.953×**, and its
instruction ratio 1.724 → 1.621. The taxonomy suite's EXEC geomean 1.0403 →
**1.0304** over 25 timed programs.

**WHAT IS STILL WRONG THERE, because the row is not exhausted (Law 4).** zcc
still round-trips the struct through memory twice: the incoming registers go to
the argument home, the home is copied to the local, and the fields are then
loaded back. gcc keeps the whole struct in x0/x1 and extracts the four `int`s
with `sxtw` and `asr #32`, touching memory not at all. Closing that needs the
local copy to be recognised as redundant when the parameter is never modified,
and small aggregates to live in registers (SROA) — neither is this row.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `isel/lower.rs::copy_inline`, reached from `Inst::MemCpy`.
The expansion emits two loads then two stores per sixteen bytes so that
`mir/pass/ldstp.rs` sees adjacent same-kind accesses and fuses them.

---

## M15. The `ldp`/`stp` residual, and why the layout cannot collect it

**VALUE.** On sqlite, `ZCC_LDSTP=1`:

```
paired=7616 | unpaired: no-partner=44137 (of which 3020 sit NEXT TO another
frame access of the same shape — a LAYOUT could pair them)
out-of-window=123  motion-blocked=886  partner-is-BEHIND=126
```

gcc -O1 emits 12,637 pairs to zcc's 7,616. This is the Law-4 residual of the
pairing theorem, classified: 41,117 accesses have no partner at any distance and
are a FUNDAMENTAL limit; 1,135 are convenience truncations of how this pass
looks (window, motion rule, direction); and 3,020 are refused only because the
two slots are not neighbours in the frame.

**THE 3,020 IS AN UPPER BOUND ON AN UPPER BOUND, and it was tested.** Two
orderings of the spill group were built and measured against the creation order
that §13o leaves in place:

| spill-slot order | pairs | sqlite instructions |
|---|---|---|
| creation order (shipped) | **7,616** | **174,572** |
| heaviest disjoint affinity pairs | 7,516 | 174,730 |
| first-access position | 7,435 | 174,882 |

Both alternatives are WORSE. The count says "these two could be adjacent" one
pair at a time and cannot say that making them adjacent separates two others —
`ldp`/`stp` consume RUNS, and a disjoint matching cuts a four-slot run into two
pairs where creation order had three. The allocator mints spill slots in an
order already correlated with the order they are accessed in, which is why the
inherited order is hard to beat.

**THE PREMISE OF THE WHOLE ROW WAS WRONG, and here is the arithmetic.** "gcc
emits 12,637 pairs to zcc's 7,616, so 5,130 instructions are being left on the
table" counts gcc's PAIRS as if each one zcc lacks were an instruction zcc could
delete. A pair only saves an instruction when the two accesses exist. Counted
properly, on sqlite:

| frame traffic | zcc | gcc -O1 |
|---|---|---|
| paired instructions (`ldp`/`stp` on sp/x29) | 7,097 | 11,456 |
| single `ldr` | 8,862 | 7,976 |
| single `str` | 6,111 | 5,288 |
| **total frame instructions** | **22,070** | **24,720** |
| accesses those instructions cover | 29,167 | 36,176 |

**zcc emits 2,650 FEWER frame instructions than gcc -O1.** gcc has more pairs
because it has 7,009 more frame accesses to pair — it spills more file-wide,
which is a fact already on the record. There was never a 5,130-instruction
opportunity here.

What is real is pairing EFFICIENCY: 0.757 instructions per frame access against
gcc's 0.683. Matching that on zcc's own accesses would be ~2,100 instructions,
and the census above says ~1,009 of those are reachable (886 motion-blocked, 123
out of window).

**AND IT IS NOT SCHEDULING.** An earlier version of this entry blamed gcc's lead
on instruction scheduling. Measured instead of asserted: at `-O1` gcc reports
`-fschedule-insns [disabled]` and `-fschedule-insns2 [disabled]`, and forcing
`-fschedule-insns2` on at `-O1` moves sqlite's pair count by **2 instructions**
and its instruction count by **zero** (157,074 either way). 91% of gcc's pairs
are sp/x29-based — prologue, epilogue and spill runs, emitted adjacent by the
frame expander, with no scheduler involved. Scheduling is an `-O2` transform and
is out of scope against an `-O1` reference.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** Nothing in the compiler: these are counters behind
`ZCC_LDSTP`. The entry exists so the next reader of the 3,020 knows it was
tried, and so the row is not attempted again on the strength of the number
alone.


---

## M16. `sqlite3VdbeExec` IS the sqlite gap — 85% of it, in one function

**VALUE.** On sqlite's worst workload (`p01_insert`, 100,000 rows through a
recursive CTE, in-memory), compiling **one function** with gcc and the other
1,259 with zcc takes the program from **1.98× to 1.15×**. That one function is
`sqlite3VdbeExec`, the VDBE interpreter loop every statement runs.

| function taken from gcc | ratio | closes |
|---|---|---|
| **`sqlite3VdbeExec`** | 1.145–1.165 | **83.2 / 83.9 / 85.2 / 88.5%** (four runs) |
| `sqlite3BtreeInsert` | 1.87–1.95 | 1.8–2.1% |
| `balance_nonroot` | 1.96 | 1.7% |
| `sqlite3VdbeRecordCompare` | 1.91 | 0.4% |
| `sqlite3VdbeMemGrow` | 2.00 | −0.5% |
| `sqlite3BtreeMovetoUnpacked` | 2.04 | −4.4% |

Everything that is not the interpreter is at or below the noise floor.

**METHOD.** `tests/bench/localize.sh` — attribution by LINKER, because this box
exposes no PMU (`/sys/bus/event_source/devices` carries software events only, and
forcing gcc's own scheduler on at -O1 moves nothing, so there is no profiler to
borrow). The same source is compiled by both compilers; every global in the gcc
object is weakened except the chosen names; those names are weakened in the zcc
object; the zcc object is linked first. A strong definition beats a weak one, so
the chosen functions come from gcc and every other name from zcc. The output is
compared against the pure-gcc build before any time is reported.

**WHAT IT COST TO LEARN, and why it is worth an entry.** Seven optimization rows
shipped on 2026-08-27 moved the 42-program taxonomy suite from 1.0400 to 1.0190
and moved real sqlite execution by **nothing** (1.679 → 1.649, ranges
overlapping). Every one of those rows was aimed at a shape found in a KERNEL,
because kernels are the only programs small enough to diff by hand. This entry
is the first fact about WHERE sqlite's time actually goes, and it says the
kernels were never going to reach it.

**⚠️ WHAT THE NUMBER IS NOT.** `-DSQLITE_PRIVATE=` externalizes sqlite's 1,260
internal functions so they have symbols to select by, and that costs BOTH
compilers their static-function inlining. The hybrid is therefore a slightly
different program from the shipping build — read these ratios against the
baselines the script prints under the same flag (gcc 43,070 µs / zcc 85,634 µs),
never against `realprog.sh`'s.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** Nothing in the compiler — it is an instrument. What it
DIRECTED, within the hour it was built: `sqlite3VdbeExec`'s 196-case dispatch
was found to be a 183-deep linear compare chain (gcc: one indirect branch),
because the jump-table row refused any switch whose arms carry edge arguments.
Fixing that took **sqlite's SQL geomean from 1.651 to 1.159** and this workload
from 1.988× to 1.279×. The instrument paid for itself the same day.

**A CAUTION FOR THE NEXT READER.** Attribution to a function is not attribution
to a defect. The histogram of that function (`xray.sh`) named classes — `mov`
+1148, `mov #imm` +485, `str` +353 — and three rows built from those classes
were each refuted at ~1%. What worked was narrowing the window to what the
workload actually executes (`EXPLAIN`) and then counting ONE mnemonic (`br`) in
both assemblies.


---

## M17. The pass audit — which passes pay, which refuse, which are dead weight

**METHOD.** `ZCC_NOPASS=<name>` disables one pass. Compile sqlite with each
disabled in turn and compare: a pass whose removal costs nothing is a pass that
is refusing everything, and a pass whose removal SHRINKS the program is buying
its size with something else — or with nothing. No instrumentation is needed;
the bisection tool already in the tree answers it.

**SIZE, sqlite (baseline 173,611 instructions):**

| pass | instructions if removed |
|---|---|
| `sroa` | **+18,635** |
| `gvn` | +9,728 |
| `cfg` | +6,706 |
| `mem` | +3,122 |
| `ifconv` | +1,424 |
| `sccp` | +645 |
| `purecall` | **0 — inert on this program** |
| `iv` | −944 |
| `inline` | −1,980 |
| `licm` | −1,998 |
| `rotate` | **−4,786** |

**SPEED, `p01_insert`.** Four passes cost size, so the question is what they buy:

| disabled | speed |
|---|---|
| `inline` | **+7.7% slower** — it earns its size |
| `rotate`, `licm`, `iv` together | **−0.2% to −1.1%** — noise |

**THE FINDING.** `rotate`, `licm` and `iv` add **7,728 instructions to sqlite —
4.5% of it — for no measurable speed.** And they are not optional: disabling them
on the 42-program taxonomy suite takes EXEC from **1.0206 to 1.4236**, with
`l2_nested_join` at **10.889×** and 26 of 42 programs above 1.1×. They are worth
40% of execution on loop code.

So this is not a deletion, it is a **missing profitability gate**: three loop
passes that pay enormously on loops and inflate everything else. The row is to
make them decline a transform that cannot pay, not to switch them off.

**A SECOND FINDING, smaller.** `purecall` changes sqlite by zero instructions —
it fires nowhere in 173,611 instructions of real C. Either its precondition is
too narrow or the shape does not occur outside the suite; it should be measured
before it is trusted.

**⚠️ WHAT THIS ENTRY IS NOT.** Removal cost is not the same as value: a pass can
be worth nothing on its own and load-bearing in combination (its output feeding
another's precondition). These numbers rank suspicion, not merit.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation,
`p01_insert` in memory, best-of-7.

**WHY IT WAS RUN.** Three of the four rows shipped on 2026-08-27 were an
EXISTING pass refusing a common shape — `args_match` refusing composite
parameters (2.2× on one program), `ifconv` requiring exactly two join
predecessors (12.5%), and the jump table refusing arms with edge arguments (30%
of sqlite). None was a missing optimization. The audit exists to find the next
one by measurement instead of by accident.


---

## M18. The peer landscape — zcc against cproc+qbe, and what cproc cannot build

**VALUE.** Over the 42 programs of `tests/bench/suite`, exec geomean against
`gcc -O1` on the same machine, all three compilers producing byte-identical
program output:

| compiler | exec geomean |
|---|---|
| **zcc** | **1.0229** |
| cproc + qbe | **1.5555** |
| | worst: `i1_global_acc` **4.13×** |

zcc is ~1.52× faster than cproc+qbe on this surface. cproc compiled all 42 with
zero failures, so the comparison is over the whole set rather than a subset.

**AND THE PART THAT IS NOT A RATIO.** The comparison could not be run on sqlite,
because **cproc cannot compile the amalgamation.** Two separate walls:

* the GCC atomic builtins sqlite selects when the preprocessor advertises
  `__GNUC__` (`__atomic_load_n`/`__atomic_store_n`). This one is fair to patch —
  sqlite's OWN non-GCC branch is `*(PTR)`, which is what any non-GCC compiler
  takes — and past it lies the second;
* `volatile store is not yet supported`. Patching around THAT would change the
  program's semantics, so the run stops there rather than reporting a number for
  different code.

**AND IT IS UPSTREAM-DOCUMENTED, not a quirk of this setup.** cproc's own
`README.md`, under *What's missing*: "`volatile`-qualified types ([#7], requires
qbe support)" and "`long double` type ([#3], requires qbe support)". Its
`doc/software.md` records that building binutils required patching out "subtle
`volatile` usage" — the same wall. And `grep -ri sqlite` over the whole cproc
repository returns nothing: it does not claim sqlite among the software it
builds. (The small compilers known for compiling sqlite are chibicc and tcc,
both of which implement `volatile`.)

**AND THE OBVIOUS OBJECTION, ANSWERED.** cproc builds Oasis Linux, so how can it
fail on sqlite? Three facts, and they are consistent:

* `cproc/qbe.c:458` refuses UNCONDITIONALLY —
  `if (tq & QUALVOLATILE) error("volatile store is not yet supported")`;
* cproc's `doc/software.md` says of Oasis: *"One of the main goals of cproc is to
  compile the entire oasis linux system (excluding kernel and libc). This is a
  WORK IN PROGRESS, but many packages have PATCHES to fix various ISO C
  conformance issues, enabling them to be built."*;
* Oasis's package tree holds **153 packages and sqlite is not one of them**
  (`api.github.com/repos/oasislinux/oasis/contents/pkg`, checked 2026-08-27).

So Oasis is cproc-built on patched sources, by design, and never had to compile
sqlite. That is the difference the comparison is about: `Article C` asks zcc to
be a DROP-IN, and the amalgamation is compiled here unmodified.

zcc compiles the amalgamation unmodified, which is Article C's whole premise.

**WHAT THIS ENTRY IS FOR, AND WHAT IT IS NOT.** THE ULTIMATUM names `gcc -O1` as
the finish line, and nothing here changes that. cproc+qbe is a PEER — the
nearest comparable project, a small C compiler with a real SSA backend — so this
answers "is zcc actually good, or only good against a toy?" It must never become
a gate: beating a weaker reference is flattering, and a number quoted against it
would be exactly the Law 3c failure of announcing parity from a favourable
surface.

**METHOD.** qbe and cproc built in-box with gcc (clang is not installed there;
the compiler used to BUILD a compiler does not affect the code it GENERATES).
Each program compiled by all three, outputs compared before any timing, then
best-of-5 wall time through `tests/bench/timeit.c`.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, qbe and cproc at their
repository tips.


---

## M19. Static block execution frequency, and the 23% of loops that are cold

**VALUE.** `hir::freq::estimate` gives every block a relative execution
frequency, the entry block scaled to `ENTRY = 10_000`. Wu & Larus, *Static
Branch Frequency and Program Profile Analysis*, MICRO-27 (1994) — the same paper
`M12` cites for the trip-count convention.

`ENTRY` and `CEIL` are a SCALE and a saturation bound, not thresholds: `ENTRY`
is the fixed-point denominator that lets integer division carry a fraction, and
`CEIL` stops a deep nest from overflowing. Neither was tuned and neither can be:
every consumer reads a RATIO against `ENTRY`.

**WHY IT WAS BUILT.** Three decisions in one day could not be made for want of
it, and one of them was refused twice:

* the profitability gate for `rotate`/`licm`/`iv`, which add 7,728 instructions
  to sqlite for no measurable speed and are worth 40% of exec on the taxonomy
  suite (`M17`);
* the spiller's `TRIPS = 10`, a stand-in for exactly this analysis (`M12`);
* "we have no profile", offered three times as a reason not to decide.

**THE MODEL.** Reverse postorder, one pass, no linear system: a loop header takes
its non-back-edge predecessors' sum times `TRIPS`; every other block sums its
predecessors weighted by edge probability. Two structural heuristics ship — an
edge into an `Unreachable` terminator is weighted 1 against 1,000, and a
successor that returns immediately 250 against 1,000. **No statistical heuristic
from the paper is included**, because each is a claim about C programs that this
compiler has not measured.

**WHAT IT FOUND, and it is the point.** Of the 1,387 loops rotation touches in
sqlite:

| loop frequency | count |
|---|---|
| **below entry — runs less than once per call** | **325 (23%)** |
| 1–10× entry | 708 |
| 10–100× | 244 |
| above 100× | 110 |

Loop DEPTH cannot see this: 1,066 of those 1,387 are outermost loops, and so are
most of the taxonomy suite's hot loops. What separates the cold 23% is the GUARD
in front of them, which is what a frequency estimate measures and a depth does
not.

**WHAT IT BOUGHT, first consumer.** `rotate` now declines a loop below entry
frequency: sqlite **173,611 → 172,949** instructions (−662), and the taxonomy
suite's INSN geomean is **unchanged to four decimals** (1.0721) — the hot loops
were never touched, which is the whole claim.

**WHY THE GATE IS AT `ENTRY` AND NO TIGHTER.** The model gives every loop the
same `TRIPS` multiplier per level, so it cannot rank two loops by trip count —
only by the guards above them. A threshold above the entry frequency would start
refusing loops whose only property is being at depth 0, which is what most of the
suite's hot loops are.

**DETERMINISM.** Integers, `Vec` by block id, reverse postorder. No hash
iteration, no floating point. `tests/determinism.sh` checks it end to end.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `hir/pass/rotate.rs`. The gates for `licm` and `iv`, and the
spiller's `TRIPS`, are the obvious next consumers and are NOT yet wired.


---

## M20. The copies that are NOT the ABI's fault — 325 against gcc's 8

**VALUE.** `sqlite3VdbeExec`, register-to-register `mov`, zcc against gcc -O1:

| kind | zcc | gcc | reading |
|---|---|---|---|
| total reg-reg `mov` | 1,757 | 482 | +1,275 |
| writing x0–x7 (argument marshalling) | 645 | 379 | +266 |
| into a callee-saved reg from x0–x7, right after a call | 38 | 26 | **near-equal — ABI-FORCED** |
| **callee-saved ← callee-saved** | **325** | **8** | **the gap** |
| into a caller-saved temp x8–x15 | 25 | 0 | 25 |

**WHAT IT SETTLES.** The copy excess is not argument marshalling and not the
call-result convention. A result that is live across a later call MUST move to a
callee-saved register — gcc obeys that rule too, and does it 26 times to zcc's
38. What zcc does 325 times and gcc 8 is move a value from one callee-saved
register to ANOTHER: pure allocator shuffling, forced by nothing.

**WHY IT MATTERS MORE THAN THE COUNT SUGGESTS.** These execute. Unlike the
frame-size rows measured the same day — slot coalescing (203 → 116 slots,
−6,832 bytes of stack) and the cold-loop rotation gate (−662 instructions) —
both of which moved the clock by nothing, a copy in the dispatch path is
retired on every pass through it.

**WHAT IT DOES NOT SAY.** That 325 is an upper bound on what coalescing can
remove, in the same sense `M11-correction` and `M15` were upper bounds: it counts
copies that the ABI does not force, not copies that a colouring could avoid. Two
callee-saved values that genuinely interfere still need a move between them at a
join. The number to beat is 8; the number reachable is unmeasured, and the first
job of the campaign is to measure it — by hand-editing the copies out of one hot
arm and timing it, before any allocator code is written.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.
