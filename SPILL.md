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
| S0 | **A shape-matched kernel in the exec suite** (§4a). geo40 cannot currently SEE this defect — that is why it reads 0.9494× while the same compiler reads 1.4–2.0× on real sqlite. Build one kernel with `sqlite3VdbeExec`'s shape and admit it to the suite, so the standard metric gates S1 instead of hiding it. | the kernel reproduces zcc/gcc ≈ **1.7–1.8×**, matching the real function | ⬜ |
| S1 | **The trace-distance model.** ~~Loop-weighted eviction~~ — the measurement (§1) refuted that framing: the defect was the `usize::MAX` a back edge produces, not a missing weight. Shipped `spill.rs::Trace`: Belady's distance measured along the execution trace (a use behind, inside this loop, is one wrap away; a use outside costs the remaining trips) and over the SSA WEB (the granularity at which eviction is paid, since `Sim::More` retires a whole web). | `nestjoin.c` **8 ms → 1 ms = gcc**; inner loop 11 insns → 8, frame ops 6 → 2 | ✅ |
| S2a | **The invariant reload.** ✅ The mechanism that carries a memory-resident value through a loop in a register — the cold-edge phi — was already built and was being REFUSED by its own pruning gate, which asked for a read strictly AFTER the block head when the read is AT the head, and answered `usize::MAX` for a value read only across the back edge. The same trace query S1 installed fixes it. | `nestjoin` inner loop 8 insns → **7**, zero reloads; at 36M iterations **12 ms → 11 ms = gcc's 11** | ✅ |
| S2b | **The accumulator's store.** The one frame op left: every definition of a memory-resident value emits a `Spill` immediately after it (`apply`, ~line 2138), so an accumulator that never leaves its register still writes its slot every iteration. It is off the dependence chain, so measure the ceiling by hand before building anything — sinking a store to the loop's exit edges is a dataflow pass, not a peephole. | no `str` to a slot inside a loop whose slot no instruction in that loop reads | ⬜ |
| S3 | **`sqlite3VdbeExec`.** Re-measure after S1+S2. | distinct frame slots 235 → **< 80**; function ratio 1.78× → **< 1.2×** | ⬜ |
| S4 | **The copy residual.** +1,252 reg-reg mov in that function. Only after S1–S3, because eviction pressure changes once hot values stop moving. | reg-reg mov in `VdbeExec` < 800 | ⬜ |
| S5 | **`ldp`/`stp` pairing.** File-wide gcc 12,305 pairs vs zcc 7,266 — **−5,039 instructions**, the cheapest untouched size win, and it touches no allocator theorem. | file ratio ≤ 1.09× | ⬜ |

### §4a S0 — the shape-matched kernel

**Why it comes first.** Every existing geo40 kernel fits in L1i and stays under
register pressure, so none of them can express the defect. Gating S1 on a suite
that is blind to it would let a real regression pass and a real win go unseen.
`tests/bench/nestjoin.c` proves the mechanism but is a nested loop, not the
interpreter shape that carries sqlite's cost.

**Step 1 — extract the true shape.** `sqlite3VdbeExec` starts at line 93917 of
the cached amalgamation. A naive `awk '/^}/{exit}'` stops after 267 lines on a
nested brace; extract it properly and record: number of locals live across the
dispatch, opcode-case count, loop nest depth inside the cases, and how many
values are live ACROSS the switch. Those four numbers are the spec.

**Step 2 — write the kernel to that spec.** A switch-dispatch interpreter: a
`while` loop over a synthetic bytecode program, a switch with the measured number
of cases, the measured number of VM-state locals held live across every case, and
an inner loop in the cases that carry one. Drive it with enough bytecode to time
cleanly at kernel scale (target ~5–50 ms under gcc, so the suite stays fast).

**Step 3 — ADMISSION IS CONDITIONAL.** The kernel joins the suite only if it
reproduces the real function's ratio, **1.7–1.8× zcc/gcc**. A kernel that
compiles to 1.0× is decorative: it proves the shape was not captured, and
admitting it would dilute the geomean with a program that tests nothing. Iterate
on the spec until the ratio matches, or report that the shape could not be
reproduced and say why.

**Step 4 — re-baseline.** geo40 becomes geo41 and the headline number MOVES,
because a program that was previously absent is now included. State the new
baseline explicitly and never compare a post-S0 geomean against 0.9494×; they
are different suites. This is the honest version of "widen the surface" from
Law 3c — one program chosen because it carries a known defect, rather than 100
chosen at random.

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
| geo40 EXEC geomean | **0.9565×** at HEAD (18 timed, median 1.000, 0 DIVERGE) | 2026-08-27 |
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
