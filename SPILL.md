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
| S5 | **`ldp`/`stp` pairing.** ⛔ RE-CLOSED, and for a different reason than the first time — the row's premise was arithmetic that did not hold. "gcc emits 12,637 pairs to zcc's 7,616" counts gcc's PAIRS as if each one zcc lacks were an instruction zcc could delete, but a pair only saves an instruction when the two accesses exist. Counted properly, zcc emits **22,070 frame instructions to gcc's 24,720** — zcc is **2,650 AHEAD**; gcc has more pairs because it has 7,009 more frame accesses, i.e. it spills more. The real quantity is efficiency (0.757 instructions per access against 0.683), of which the census says ~1,009 are reachable. The first closure blamed gcc's lead on SCHEDULING; that was asserted, not measured, and it is false: at `-O1` gcc has `-fschedule-insns2` disabled, and forcing it on moves sqlite by 2 instructions and 0 in total count. | true ceiling ~1,009 pairs, not 5,130 | ⛔ |

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

## §4c THE NEXT SESSION STARTS HERE — the copy-coalescing campaign

**State at hand-off.** sqlite exec **1.159×** gcc -O1 (was 1.651 at the start of
2026-08-27). Size 1.1052×. The 42-program suite 1.0206. Everything in `§6` is
measured; do not re-take it.

**BEFORE ANYTHING ELSE.** `mir/pass/slotmerge.rs` is committed but the FULL GATE
WAS NOT RUN on it — the session ended first. It has: cargo 186/0, provenance
PASS, `localize.sh`'s output check green on sqlite (which is what caught its
predecessor's miscompile), and `determinism` NOT run. **Run
`FUZZ_N=300 sh tests/fullsuite.sh all` first, before adding anything.**

**THE TARGET, measured (`MEASURED M20`).** In `sqlite3VdbeExec`:

```
mov <callee-saved>, <callee-saved>      zcc 325   gcc 8     <- the gap
mov <callee-saved>, x0..x7 after a call zcc  38   gcc 26    <- ABI-forced, near-equal
mov -> x0..x7 (argument marshalling)    zcc 645   gcc 379
```

The excess is NOT the ABI. A result live across a later call must move to a
callee-saved register and gcc does that too. What zcc does 325 times and gcc 8 is
shuffle a value from one callee-saved register to another — coalescing failure,
and unlike the frame rows these copies EXECUTE.

**THE ORDER OF WORK, and step 1 is not code.**

1. **MEASURE THE CEILING BY HAND.** Take one hot arm of the dispatch, delete its
   callee-saved shuffles in the `.s` by renaming registers, link, check the
   output, time it. That number decides whether the campaign is worth 7 points or
   1. Everything on 2026-08-27 that skipped this step was refuted; everything
   that did it shipped. `MEASURED M20` says 325 is an upper bound on what the ABI
   does not force, NOT on what a colouring could avoid.
2. **Diagnose ONE shuffle.** Why did the colourer put the value somewhere its
   copy partner is not? `ZCC_HINT=1` already reports the refusals; the answer for
   the block-local case is in `§4b` and it is that recolouring the occupant needs
   interference the allocator does not carry.
3. **The mechanism, if the ceiling justifies it.** Post-colouring recolouring
   with WHOLE-FUNCTION occupancy, which is what `§4b`'s attempt lacked: build,
   per physical register, the set of program points where it is held, then for a
   copy `D = S` recolour `D` to `S`'s register when that register is free across
   `D`'s entire live range and the caller/callee partition allows it. The copy
   becomes a self-move and `destruct::sequentialize` already deletes those.
4. **Verify with `localize.sh` before timing anything.** It compares program
   output against the gcc build and refuses to report a number otherwise. It is
   the only instrument in the tree that caught the slot-merge miscompile — 185
   unit tests and all 42 suite programs passed it.

**WHAT IS NOT THE PATH TO 1×, measured on 2026-08-27 so nobody re-tries it:**

* frame-size work. Slot coalescing took `VdbeExec` from 203 slots to 116 and cut
  6,832 bytes of stack; the clock moved 1.279 → 1.276. Fewer ADDRESSES is not
  fewer ACCESSES, and the access count (1,629 against gcc's 598) is what costs.
* cold-path work. The rotation gate removed 662 instructions from loops that by
  definition do not execute.
* `madd`→shifts, `cset`/`cmp` folding, dispatch reordering, dispatch trees,
  small-struct SROA, invariant-constant hoisting — each priced by hand-edit and
  each worth ~1% or less. `arm64_elf.md` §6.1 records why.

**THE HONEST SIZING.** `VdbeExec` is ~47% of the remaining 15.9 points ≈ 7.5.
The tail (`MemShallowCopy` ~8%) is ~1.3. The rest is below the attribution
instrument's noise floor, which is what a systemic allocation problem looks like
from a distance. **1× is not reachable without this campaign**, and it may end at
"chordal colouring in dominance order cannot revisit, so this needs a different
allocator" — which is a REARCH decision, not a row.

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
| `ldp`/`stp` file-wide | zcc 7,266 / gcc 12,305 — ⚠️ **NOT a 5,039 opportunity**, see `MEASURED M15`: counted as instructions rather than pairs, zcc emits 22,070 frame instructions to gcc's 24,720 and is 2,650 AHEAD | S5 |
| geo40 EXEC geomean | **0.9565×** — SUPERSEDED, see §4a: 18 timed under a 5 ms floor | 2026-08-27 |
| **geo41 EXEC geomean** | **1.0403×** (25 timed at a 189 µs floor, median 1.004, worst `e3_struct_byval` 2.630, 6 above 1.1×) | 2026-08-27 |
| **geo41 INSN geomean** | **1.0421×** (all 36, median 0.991, worst `e3_struct_byval` 1.724, 12 above 1.1×) | 2026-08-27 |
| geo40 INSN geomean | **1.0432×** (deterministic, all 35, worst `e3_struct_byval` 1.759×) | 2026-08-27 |
| geo40 worst exec | `d1_switch` 1.111× | 2026-08-27 |
| realprog total | 1.410× Apple / 2.03× Graviton | report |

### ⭐ THE HEADLINE — sqlite exec 1.651 → 1.159, and what actually did it

Three interleaved runs of each binary, `realprog.sh` at microsecond resolution,
session start (`d85aac9`) against `5ed5648`:

| | session start | after the jump-table row |
|---|---|---|
| **SQL geomean, 11 phases** | 1.6282 / 1.6743 → **1.651** | 1.1524 / 1.1646 → **1.159** |
| TOTAL (sum-weighted) | 1.490 | **1.164** |
| worst phase `p01_insert` | 2.818 / 2.593 | **1.313 / 1.301** |
| median phase | 1.551 / 1.578 | **1.154 / 1.147** |
| phases above 1.1× | 10 of 11 | 9 of 11 |

**65% slower than gcc -O1 became 16% slower, from one condition in `isel`.**
`sqlite3VdbeExec` dispatches 196 opcodes and every arm carries edge arguments,
so the jump-table row refused it and the hottest dispatch in the program was a
183-deep linear compare chain, walked ~1.4 million times per 100,000-row INSERT.

**AND HERE IS THE LESSON, which cost a day to learn.** The seven rows shipped
before it — trace-distance eviction, the phi gate, argument registers last,
inline of composite parameters, the parameter-copy elision, if-conversion —
were all real, all gated, all measured on their own programs, and together they
moved sqlite **by nothing** (1.679 → 1.649, ranges overlapping). Every one had
been aimed at a KERNEL, because kernels are the only programs small enough to
diff by hand. The row that moved sqlite was aimed at sqlite.

The chain that found it, in order, and none of the steps is skippable:

1. `localize.sh` — attribution by linker: **85% of the gap in one function**
   (`MEASURED M16`). Static instruction counts had said `VdbeExec` was 25% of
   the *size* excess; they could not say it was 85% of the *time*.
2. `xray.sh` — that function's mnemonic histogram against gcc. **Necessary but
   not sufficient**: a histogram names CLASSES, not sites. Three hypotheses
   drawn from it (`madd`→shifts, `cset`/`cmp` folding, dispatch reordering) were
   built or hand-edited and each refuted at ~1%.
3. **Narrowing the window.** `EXPLAIN` gave the opcodes the workload actually
   runs; counting `br` in the two assemblies gave the answer in one line —
   gcc 1, zcc 0.

Step 3 is the one that mattered, and it is the cheapest of the three.

### The pre-jump-table state, kept because it is what the lesson is about

Three interleaved runs of each binary, `realprog.sh` at microsecond resolution,
session start (`d85aac9`) against HEAD (`47f8e77`) — seven shipped rows apart:

| | session start | HEAD |
|---|---|---|
| **SQL geomean over 11 phases** | 1.7179 / 1.6558 / 1.6620 → **1.679** | 1.6493 / 1.6363 / 1.6626 → **1.649** |
| TOTAL (sum-weighted) | 1.498 / 1.477 / 1.548 → 1.508 | 1.474 / 1.424 / 1.500 → 1.466 |
| worst phase | `p01_insert` 2.67–3.01× | `p01_insert` 2.79–2.92× |
| phases above 1.1× | 10–11 of 11 | 10–11 of 11 |

**The ranges overlap** (old 1.656–1.718, new 1.636–1.663), so a 1.8% shift
against a 3.7% spread is not a result. Say it plainly: the session moved the
taxonomy suite from 1.0400 to 1.0190 and sqlite's SIZE from 1.1216× to 1.1085×,
and did not measurably move real sqlite EXECUTION.

⚠️ **THE STANDING LESSON, and it is the one to read first.** Every row shipped
today was aimed at a shape found in a KERNEL — a by-value struct parameter, a
parser's dispatch arm, a nested-loop join. Each was real and each paid on its own
program. None of them was aimed at sqlite, and sqlite did not move. The 1.11×
size against 1.65× exec split said this in advance: **the remaining real-program
gap is not instruction count**, so rows found by counting instructions cannot
close it.

**What that makes necessary.** A localizer — WHICH FUNCTIONS carry the 1.65×.
`-DSQLITE_PRIVATE=` already exposes all 1,260 internal functions as symbols in
both compilers, and `objcopy --weaken-symbols=<list>` allows a hybrid link:
weaken every global in gcc's object except a chosen set, weaken exactly that set
in zcc's, link the two, and the chosen functions come from gcc while everything
else comes from zcc. One link and one run per experiment, no recompiles, so
binary-searching 1,260 functions is about eleven cycles. (An earlier attempt to
split the amalgamation into its original translation units failed — 47 of 102
units do not compile because the headers interleave — and `objcopy
--only-section` destroys the symbol table. The weaken-list route avoids both.)

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
