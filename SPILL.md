# SPILL.md — the spill-placement campaign

The plan of record for closing zcc's real-program performance gap. Opened
2026-08-27, after the sqlite-segfault night. Read §0, then start at §3.

---

## §0 BOOT — the one paragraph that matters

zcc's spiller ranks eviction by **raw static next-use distance**. Every
instruction counts 1, whether it runs once or four million times, so the loop
index, the loop pointer and the accumulator get spilled **inside** hot loops
while cold values sit in registers. gcc weights each use by `10^loop_depth` and
therefore never does this. `LoopForest` depth is already computed in
`spill.rs` — it just never reaches the eviction decision.

**This is one missing term in one sort key, not a broken architecture.** Do not
rewrite the allocator (§2).

---

## §1 THE MEASUREMENTS — all taken 2026-08-27, all reproducible

### The ceiling, proven by hand (`scratchpad/nestjoin.c`, 25 lines)

A nested-loop join with 24 unfoldable values live across the inner loop:

| build | time | output |
|---|---|---|
| gcc -O1 | **1 ms** | 4087392 |
| zcc -O1 | **8 ms** | 4087392 |
| zcc, inner loop hand-edited | **1 ms** | 4087392 |

The hand-edit removes **five instructions** and closes **the entire 8× gap**.
That is the whole campaign in one number: the shape is worth everything.

Before (zcc, 4,000,000 iterations, 6 of 11 instructions are frame traffic):

```
.Ljoinit_6:
    ldr x2, [sp, #80]      <- reload pb       the POINTER
    ldr x3, [sp, #240]     <- reload j        the LOOP INDEX
    ldr w2, [x2, x3, lsl #2]
    cmp w2, w0
    ldr x2, [sp, #144]     <- reload hits     the ACCUMULATOR
    csinc x2, x2, x2, ne
    str x2, [sp, #144]     <- spill hits
    add x2, x3, #1
    str x2, [sp, #240]     <- spill j
    cmp x2, x1
    b.lt .Ljoinit_6
```

After (hoist the three into x4/x5/x7 before the loop, sink after):

```
    ldr x4, [sp, #80]      / mov x5, xzr / ldr x7, [sp, #144]
.Ljoinit_6:
    ldr w2, [x4, x5, lsl #2]
    cmp w2, w0
    csinc x7, x7, x7, ne
    add x5, x5, #1
    cmp x5, x1
    b.lt .Ljoinit_6
    str x7, [sp, #144]     / str x5, [sp, #240]
```

Note what the allocator did: it kept the COLD `c0..c23` in x6/x8/x10/x12/x14/
x15/x20/x22/x24 across the loop and spilled the hot three. Exactly inverted.

### The same defect at scale — `sqlite3VdbeExec`

| | zcc | gcc -O1 |
|---|---|---|
| instructions | 10,766 | 6,040 (**1.78×**) |
| **distinct frame slots** | **235** | **43** |
| frame accesses | 1,862 | 515 (**3.6×**) |
| reg-reg mov | 1,736 | 484 (3.6×) |
| callee-saved used | x19–x28 (all) | x19–x28 (all) |

+4,726 instructions — **25% of the whole 19,079-instruction sqlite gap in one
function**, and it is the function every query runs. Across functions present in
both compilers zcc is only **1.045×**; the file-wide 1.1238× is mostly gcc
inlining small statics away, which is a different lever entirely.

⚠️ **This corrects a recorded belief.** "zcc spills less than gcc file-wide" is
true *on average* and hid the opposite where it counts. Never judge spilling by
a file-wide average again.

### The code

`spill.rs::next_use` returns a position from `linear_positions`, which numbers
instructions in reverse-postorder, unweighted. The eviction key is

```rust
cand.sort_by_key(|r| (droppable(r), next_use(&uses, r.v as usize, head)))
```

`lf.depth` appears three times in the file: the fixpoint round budget, a
cold-edge reload placement test, and a reporting histogram (`inloop`, ~line 526).
**Never in the decision.**

### ⚠️ WHAT THE DEFECT ACTUALLY WAS — measured 2026-08-27, and it is not §0's story

§0 above says "one missing weight in one sort key". That diagnosis was made by
reading. Instrumenting every eviction site (a temporary `eprintln!` at each
`newsp.push`) said something sharper, and a session that trusts §0's wording
will build the wrong mechanism:

```
SPILL joinit site=TERMARG bb29 depth2 v484 nextuse-1 from89   <- inner-loop latch
SPILL joinit site=TERMARG bb31 depth1 v439..v452 nextuse-1    <- outer-loop latch
```

`nextuse-1` is `usize::MAX`. **A back edge runs backwards in reverse postorder**,
so a value carried around a loop is read at a LOWER position than the latch that
passes it on; `partition_point(|&p| p <= from)` finds nothing and `next_use`
answers *never used again* — the strongest possible reason to evict, handed to
precisely the values that are used most. It was not that hot values were
under-weighted. **They were ranked as dead.**

A second blindness sat behind it. mem2reg splits one C variable into a chain of
SSA values joined by block parameters, and every link of that chain has exactly
ONE use: being passed to the next link. Asked of the vreg, "how far to the next
use of `c0`?" and "of `j`?" both answer 1 — twenty-four cold values and three
hot ones become indistinguishable at the exact edge where the choice is made.
Measured: every candidate at the preheader's terminator reported distance 3.

Both are fixed by measuring the distance the way Belady's theorem defines it —
along the TRACE, over the WEB (`spill.rs::Trace`). Neither is a weight.

---

## §2 WHY NOT A REWRITE

The user asked. The answer is no, and the reason is evidence, not conservatism.

CCC (the AI compiler benchmarked at 737×–158,000× on sqlite) needs a rewrite: it
has no allocator, uses "a single shuttle register", and produces 11,000-byte
frames for 32 variables. zcc is 1.4–2.0× on the same program, with:

* SSA-form allocation on a **chordal** interference graph, where greedy colouring
  along a dominator preorder is **optimal in k by construction** (THEORY A7);
* Braun–Hack spilling, live-range splitting, rematerialization, biased colouring;
* a commuting square `⟦mir_v⟧ = ⟦mir_p⟧` and structural obligations checked on
  every compile.

This is the modern design — the same family LLVM uses. A rewrite would spend
weeks re-deriving what A7 already proves and would put every correctness square
back in play; both bugs fixed on 2026-08-27 lived **at allocator seams**. The
measured defect is a cost model, and cost models are replaceable in isolation.

---

## §3 THE METHOD — this is the part that decides success

**Five previous attempts at this area all failed.** Every one of them edited
`color.rs` directly and was reverted: hint-set without re-check; rollback;
excluding the ParallelCopy path; a separate post-colouring pass (fired 0 times —
ABI args are `ParallelCopy` pairs, not `UseFixed`); retargeting (collides on
simultaneity). **Do not retry any of them.**

What worked instead — the method that took geo40 below 1×:

> **Never patch the compiler to test a codegen theory. Hand-edit the `.s`, link
> it, run it, time it. Prove the shape wins first; only then build the mechanism
> that produces that shape.**

So every phase below is: **(a) hand-edit to the target shape and measure the
ceiling → (b) only if the ceiling is worth it, build the minimal mechanism →
(c) prove it with a non-vacuous square → (d) full gate + seal.**

Phase 1's ceiling is already measured (§1): 8 ms → 1 ms. That is why it is
Phase 1.

**Non-vacuity is mandatory.** On 2026-08-27 two fixtures passed with their fix
disabled and were withdrawn. A test that passes without the change is not a
proof; `tests/provenance.sh` exists to refuse exactly that.

---

## §4 THE LADDER

Status lives HERE, edited in place. Do not open a new numbering elsewhere.

| # | row | gate | status |
|---|---|---|---|
| S0 | **A shape-matched kernel in the exec suite** — and, it turned out, an INSTRUMENT that could see it. Two things were hiding this defect from geo40, not one: no kernel in the suite spills, AND the harness timed with `date +%s%N` and then divided by 1,000,000, throwing the nanoseconds away before declaring everything under 5 ms unmeasurable. See §4a. | kernel `k1_vdbe_dispatch` reads **exec 1.939× / insn 1.561×**; timed programs 18 → 25 | ✅ |
| S1 | **The trace-distance model.** ~~Loop-weighted eviction~~ — the measurement (§1) refuted that framing: the defect was the `usize::MAX` a back edge produces, not a missing weight. Shipped `spill.rs::Trace`: Belady's distance measured along the execution trace (a use behind, inside this loop, is one wrap away; a use outside costs the remaining trips) and over the SSA WEB (the granularity at which eviction is paid, since `Sim::More` retires a whole web). | `nestjoin.c` **8 ms → 1 ms = gcc**; inner loop 11 insns → 8, frame ops 6 → 2 | ✅ |
| S2a | **The invariant reload.** ✅ The mechanism that carries a memory-resident value through a loop in a register — the cold-edge phi — was already built and was being REFUSED by its own pruning gate, which asked for a read strictly AFTER the block head when the read is AT the head, and answered `usize::MAX` for a value read only across the back edge. The same trace query S1 installed fixes it. | `nestjoin` inner loop 8 insns → **7**, zero reloads; at 36M iterations **12 ms → 11 ms = gcc's 11** | ✅ |
| S2b | **The accumulator's store.** ⛔ REFUSED BY ITS OWN CEILING, and no code was written — the §3 method working as intended. Hand-edited the store out of `nestjoin`'s inner loop and timed it at microsecond resolution: gcc 11,599 µs, zcc 11,634 µs, zcc-with-the-store-sunk **11,594 µs**. A 0.34% difference, because the store is off the dependence chain and retires into the write buffer (Law 3c: count is not cost, in the direction that says DON'T build it). A store-sinking dataflow pass is not worth 0.34% of one program. | ceiling measured at **0.34%**; not built | ⛔ |
| S3 | **`sqlite3VdbeExec`.** Re-measured after S1+S2a. **Gate NOT met**, and the reason was already on the record: slots fell 244 → **199** (−18%) while the function's ratio moved only 1.823× → **1.786×**, because `excess.sh` had already shown the gap in that function is COPIES, not spill traffic (+10,464 reg-reg `mov` file-wide against +1,741 frame accesses). Spill ranking was never going to close it. | wanted slots <80 (got 199) and ratio <1.2× (got 1.786×) | ⛔ |
| S4a | **The argument registers go last.** ✅ `assign` picks `hint.or_else(alloc_order.find(free))` and `alloc_order` began at x0, so every unhinted value in the function took an argument register before anything else and the argument that wanted it paid a `mov`. Reordering the caller-saved half to x8–x15 then x0–x7 (`MEASURED M13`) — no set, mask or ABI changes. | sqlite 175,407 → **174,677** (1.1167× → **1.1120×**); movs into x0–x7 22,829 → 19,985; geo40 INSN 1.0432 → **1.0301** | ✅ |
| S4b | **Re-colour the occupant.** ⛔ BLOCKED — attempted, refuted by the verifier, and the ceiling it was aimed at turns out not to exist. See §4b. | attempted; **7 recolours in all of sqlite**, −37 instructions | ⛔ |
| S4 | **The copy residual.** +1,252 reg-reg mov in that function. Only after S1–S3, because eviction pressure changes once hot values stop moving. | reg-reg mov in `VdbeExec` < 800 | ⬜ |
| S6 | **A small copy is not a libcall.** ✅ Added IN PLACE, not as a new numbering: S0's instrument made `e3_struct_byval` visible at **2.630× exec**, the worst program in the suite on both axes, and the cause was `isel/lower.rs` lowering EVERY `Inst::MemCpy` to `bl memcpy` — including the 16-byte home of a by-value struct parameter, which made a leaf function build a frame and call libc four million times. Now open-coded up to 32 bytes (`MEASURED M14`, the measured minimum of a nine-point sweep), emitted as two loads then two stores so `ldstp.rs` fuses them. | `e3_struct_byval` 2.630× → **1.953×** (insn 1.724 → 1.621); sqlite 174,677 → **174,572**; suite EXEC 1.0403 → **1.0304** | ✅ |
| S5 | **`ldp`/`stp` pairing.** ⛔ BLOCKED — the residual is now measured and classified (`MEASURED M15`), and the class that looked reachable is not. Of sqlite's unpaired frame accesses, 41,117 have no partner at any distance (fundamental), 1,009 are convenience truncations of the window and motion rule, and 3,020 are refused only because the two slots are not neighbours. Two spill-slot orderings were built to collect that 3,020 — heaviest disjoint affinity pairs, and first-access position — and BOTH are worse than the creation order already shipped: 7,516 and 7,435 pairs against 7,616, and +126 / +310 instructions. `ldp`/`stp` consume RUNS, and a disjoint matching cuts a four-slot run into two pairs where the inherited order had three. gcc's remaining lead is that it SCHEDULES; zcc has no scheduler. | census shipped behind `ZCC_LDSTP`; layout attempts reverted | ⛔ |

### §4a S0 — what was actually wrong with the instrument

**The suite could not see the defect for two reasons, and only one was planned
for.** The first is the one this row was written about: every geo40 kernel fits
in the register file and spills nothing. That is now proven rather than assumed —
the whole 35-program corpus is byte-identical across a 1000× sweep of the
spiller's one cost constant (`MEASURED M12`), which is only possible if no
allocation decision in any of them is pressure-bound.

**The second was the harness.** `exectime.sh` timed with `date +%s%N` — a
nanosecond clock — and then wrote `(t1-t0)/1000000`, truncating to whole
milliseconds, with a shell `fork` for `date` sitting between the two readings.
On the strength of that truncation it declared everything under 5 ms
"startup-dominated" and skipped it. **Fifteen of the thirty-five programs never
produced an exec number at all.** The resolution was never missing from the
machine: `clock_gettime(CLOCK_MONOTONIC)` is a vDSO read here, the counter
behind it runs at 24 MHz (41.7 ns/tick, 0.5% run-to-run over ten million
iterations), and the real floor — `fork`+`execve` of `/bin/true`, best of 20 —
is **189 µs**, reproducible to the microsecond. `tests/bench/timeit.c` measures
that floor on every run and prints it, so the cutoff is a measured number times
a margin rather than a constant someone chose.

What that changed, at the SAME tree:

| | old instrument | µs instrument |
|---|---|---|
| programs timed | 18 | **25** |
| EXEC geomean | 0.9500 | **1.0165** |
| worst exec | `d2_nested_loops` 1.111 | `e3_struct_byval` **2.642** |

⚠️ **The sub-1× reading was substantially an artifact of the skipping.**
`e3_struct_byval` was reported as `fast` and dropped; it is 2.6× slower than
gcc. `a2_udiv_mod`, `a3_sdiv_mod` and `a4_shift_mask` were dropped; they are
1.11–1.14×. A geomean over the 18 programs that survived a 5 ms floor was not a
statement about the suite. This is Law 3c's own warning arriving from an
unexpected direction: the narrow surface was narrower than anyone had counted.

**The kernel.** `tests/bench/suite/k1_vdbe_dispatch.c`, generated to the spec
measured from `sqlite3VdbeExec` itself (8,363 lines at amalgamation line 93,917;
**196 arms in one switch**; **42 for/while loops inside them**; brace depth 9;
a VM-state set live across every arm; per-arm locals with mutually exclusive
live ranges). Arms are heterogeneous by construction — integer chain,
struct-field chasing, byte/short work, double arithmetic, compare-and-select —
because a uniform body measures one lowering row 196 times instead of a
dispatch.

**Admission, and the honest shortfall.** Step 3 asked for 1.7–1.8×. On the
arbiter axis it exceeds that: **exec 1.939×**. On instructions it reaches
**1.561×** against the real function's 1.794×, with 86 zcc frame slots to gcc's
37 (the real pair is 199/43). Seven parameter settings were swept; the
instruction ratio plateaus at 1.5–1.6, and adding calls to the arms — VdbeExec
is the most call-dense function in sqlite — LOWERED it, because argument
marshalling costs gcc as much as zcc per call. The residual is heterogeneous
hand-written code over a large frame, which a generator does not reproduce. The
program carries the shape and the exec ratio; the last 0.23× of the instruction
ratio lives in sqlite, where `realprog.sh` measures it.

**Step 4 — the suite is re-baselined and the old numbers do not compare.**
geo40 becomes geo41. At HEAD, 36 programs: **EXEC 1.0403 over 25 timed** (median
1.004, worst `e3_struct_byval` 2.630, 6 above 1.1×) and **INSN 1.0421 over all
36** (median 0.991, worst 1.724, 12 above 1.1×). Never compare either against
0.9494×, 0.9565× or 0.9500×: those are a different program set read through a
different instrument.

### §4b S4b — why the 8,784 was never a ceiling

The row was aimed at a number the colourer prints itself: of sqlite's 14,764
hints refused because the wanted register was OCCUPIED, **8,696 have an occupant
that "dies in this block"**, and the statistics replay says a register is free
across that occupant's whole range in 100% of them. The plan read that as 8,696
removable copies.

**It is not, and the instrument's wording is what misled it.** `HINT_OCC_LOCAL`
tests ONE condition — the occupant's LAST USE is in this block — and labels the
result "locally evictable". A value can die in this block and still have been
LIVE-IN, with its range reaching back through dominating blocks the colourer
walked earlier and keeps no occupancy record of. Recolouring one of those
changes its register in those blocks too, where the new register is very likely
taken.

That is not a deduction; it is what happened. The mechanism was built —
a per-point occupancy history so a refusal could ask what was busy in the part
of the occupant's range already walked — and on the first real program
`regalloc::verify` stopped the compile:

```
unixShmSystemLock: V(4) and V(25) are both live at bb0[3] and both hold Gpr9
```

Restricted to the genuinely local case — occupant DEFINED in this block, dying
in this block, not live-out — it is correct, the full corpus passes, and it
fires **7 times in the whole of sqlite** for **−37 instructions**. Seven, against
a claimed eight thousand.

**So the lever needs global interference**, which this allocator deliberately
does not carry: chordal colouring in dominance order is optimal in k precisely
because it never revisits (THEORY A7). Getting it would mean an interference
graph or live-range splitting at colouring time — a different allocator, not a
row. Reverted; the ~150 lines are not worth 37 instructions and they carry an
edge the verifier had to catch.

**What to fix instead of retrying this.** The instrument should say what it
measures. `HINT_OCC_LOCAL` should require defined-here AND dying-here before it
calls anything "locally evictable", so the next reader is not handed an 8,696
that means something else. Until then, treat that column as an upper bound on an
upper bound.

⚠️ §3 said five previous attempts in this area were refuted. This is the sixth,
and it is the first that says WHY in a form the next session can check: the
number in the report is not the number of removable copies.

---

## §5 HOW TO JUDGE

* **Speed on real programs, not instruction count** (Law 3c). `realprog.sh` per
  phase, and `bench/quickapp.sh` for the SQL statements.
* **Both microarchitectures.** Apple silicon and Graviton disagreed by 40% on the
  same binary (geomean 1.45× vs 2.03×). A win on one is not a win.
* **geo40 must not regress.** It stands at 0.9494× (tag `rc5`). Loop-weighted
  eviction moves pressure *out* of loops and therefore *into* straight-line code;
  the kernels are where that shows up first.
* **A full seal, not the 300-seed gate.** S1 changes what every function spills.
  `c04804` (over-k panic at a `pcopy`) was a one-in-ten-thousand event that the
  300-seed gate never saw. Budget a 10k csmith + 10k yarpgen run on us-east-2 —
  and **tear the box down and verify** (0 instances, 0 volumes, 0 spot requests).

**Abandon criteria.** If S1's real yield is under 20% of the measured ceiling
after one bounded attempt, mark it `BLOCKED: <reason>`, revert to green, bank
anything positive, and advance. A blocker never authorizes a new direction.

---

## §6 THE NUMBERS ARE ALREADY TAKEN — DO NOT RE-TAKE THEM

Everything in §1 was measured on 2026-08-27 against `2d6461a`. **A later session
must not re-measure any of it to "confirm".** Re-measuring a recorded fact costs
an hour, produces the same number, and is the single most common way a session
spends itself without moving the ladder. The facts:

| fact | value | source |
|---|---|---|
| `nestjoin.c` gcc -O1 | 1 ms | §1 |
| `nestjoin.c` zcc -O1 | 8 ms | §1 |
| `nestjoin.c` zcc, hand-edited loop | 1 ms | §1 — **the ceiling** |
| `VdbeExec` distinct frame slots | zcc 235 / gcc 43 | §1 |
| `VdbeExec` instructions | zcc 10,766 / gcc 6,040 | §1 |
| sqlite file ratio | 1.1238× (173,176 / 154,097) | §1 |
| functions in both compilers | 1.045× | §1 |
| `ldp`/`stp` file-wide | zcc 7,266 / gcc 12,305 | S5 |
| geo40 EXEC geomean | **0.9565×** — SUPERSEDED, see §4a: 18 timed under a 5 ms floor | 2026-08-27 |
| **geo41 EXEC geomean** | **1.0403×** (25 timed at a 189 µs floor, median 1.004, worst `e3_struct_byval` 2.630, 6 above 1.1×) | 2026-08-27 |
| **geo41 INSN geomean** | **1.0421×** (all 36, median 0.991, worst `e3_struct_byval` 1.724, 12 above 1.1×) | 2026-08-27 |
| geo40 INSN geomean | **1.0432×** (deterministic, all 35, worst `e3_struct_byval` 1.759×) | 2026-08-27 |
| geo40 worst exec | `d1_switch` 1.111× | 2026-08-27 |
| realprog total | 1.410× Apple / 2.03× Graviton | report |

### After S1 — taken 2026-08-27 with ONE harness across both binaries

The baseline column is not a recorded number: `d85aac9` was rebuilt and run
through the same script in the same box session, because a ratio taken by two
different scripts is not a comparison.

| | before S1 | after S1 | gcc -O1 |
|---|---|---|---|
| `nestjoin.c` best-of-5 | 8 ms | **1 ms** | 1 ms |
| sqlite file instructions | 176,186 | **175,394** | 157,074 (1.1216× → **1.1166×**) |
| `VdbeExec` instructions | 11,014 | **10,841** | 6,041 (1.823× → **1.794×**) |
| `VdbeExec` distinct frame slots | 244 | **200** | 43 |
| `VdbeExec` frame accesses | 1,928 | **1,704** | 598 |
| geo40 EXEC / INSN | 0.9565 / 1.0432 | **0.9474 / 1.0432** | — |

The taxonomy suite's INSN geomean is unchanged **to four decimal places**, and
the whole 35-kernel corpus is byte-identical across a 1000× sweep of the model's
one constant (`MEASURED M12`). That is S0's thesis stated as a measurement: no
kernel in the suite is under enough pressure to spill, so the suite cannot see
this row at all — it can only certify that the row broke nothing.

⚠️ **`realprog.sh`'s ratio is not stable enough to read from one run.** Three
runs of the SAME tree gave totals of 1.390×, 1.467× and (before S1) 1.415×,
while gcc's own total moved 1,181 → 773 ms between them — the box's load
compresses the ratio toward 1. A realprog A/B must interleave the two binaries
in one sequence and be read across runs, never as a single pair. The gate that
DOES resolve S1 is `nestjoin` (8× effect) and the deterministic instruction and
slot counts above.

⚠️ **`exectime.sh` NEEDS `SUITE=`.** It defaults to `/work/tests/bench/suite`
while the repo mounts at `/work/zcc`, and with the wrong path it prints
"EXEC: no timed programs" instead of failing. Always:

```
docker run --rm -e ZCC=/usr/local/bin/zcc -e SUITE=/work/zcc/tests/bench/suite \
  -v "$PWD/target/aarch64-unknown-linux-musl/release/zcc":/usr/local/bin/zcc:ro \
  -v ~/.cache/zcc-suites:/suites -v "$PWD":/work/zcc:ro zcc-box \
  sh /work/zcc/tests/bench/exectime.sh
```

**Re-measure only when the tree has changed in a way that could move the number**
— i.e. AFTER shipping a row, as that row's gate. Never before, and never "to be
sure".

### The first hour

1. Read `spill.rs` around `next_use`, `linear_positions`, and the
   `cand.sort_by_key` at ~line 1317. That is the whole surface of S1.
2. Decide the weighting form on the model *before* editing: what does `10^depth`
   do to a position scale that `next_use` binary-searches with
   `partition_point`? The ordering must stay monotone or the search breaks.
3. Then, and only then, write code — and measure once, at the gate.
