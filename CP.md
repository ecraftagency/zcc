# CP.md — the compile-speed campaign (transient)

> **Lifecycle (anti-bloat).** This is a TRANSIENT working doc, the compile-speed twin of `OPT.md`'s
> role for the optimizer. It holds the plan + scoreboard for the §CP campaign only. **Delete it when
> the campaign closes.** Before deleting, cook the durable results into the permanent record: any new
> algorithm that becomes load-bearing (bitset liveness, worklist dataflow, memoized SCEV) is a
> Side-I theorem and its provenance belongs in `THEORY.md`; the measured phase profile and final
> baseline belong in `REARCH.md` §13/§CP-closeout. `REARCH.md` keeps only a one-line pointer here
> while this runs, and that pointer is removed at deletion. This doc introduces NO new plan
> numbering that forks the R-ladder — it is the CP2.x detail of one REARCH side-campaign, edited in
> place here (anti-fragmentation law still binds).

## §CP — THE COMPILE-SPEED CAMPAIGN (opened 2026-08-25; a side campaign, orthogonal to R4)

**Why.** Gating R4.2 surfaced **6 yarpgen CTIMEOUT** (compile > 300 s: s0007, s0025, s0035, s0075,
s0231, s0228) where the fuzz suites are meant to be ~constant. Isolated to the OPTIMIZER + BACKEND,
not R4.2 (that change is in `destruct`, after the optimizer; `ZCC_O0=1` compiles s0007 in **12 s**
vs **259 s** opt-on). The trigger is a class of yarpgen function that is pathologically large — `init`
in s0007 is **7,266 blocks / 27,999 values / 1,643 loops / 1,643 SROA pieces** (sqlite's largest is
6,231 blocks / 59 loops / 328 pieces) — and several passes plus the register allocator are
super-linear in one of those dimensions.

**Goal (user, 2026-08-25):** MAINTAIN MAXIMUM OPTIMIZATION — no output change, no de-optimizing size
cap. Replace every super-linear site with the right algorithm (N² → N log N → N), trading MEMORY for
speed where it helps (bitsets, hash indices, incremental maps). The per-fix gate is **byte-identical
`.s`** over a corpus: identical bytes ARE the proof that output speed/size are untouched (the dual of
the refactor gate). A size cap that skips a pass is NOT allowed here — that is a different tool and it
loses optimization.

**FIRST, THE BUILD FACT (Law-2 measurement exception).** Every alarming compile number this session
was a **debug** zcc. `tests/box.sh` / `tests/fullsuite.sh` build the musl ELF debug; debug Rust is
**~9× slower**. Measured in-box (aarch64 musl), sqlite `-O1 -S`, byte-identical output (217,160 insns):
**debug 112 s → RELEASE 12 s**; old-main (rc3) debug was ALSO 112 s (no branch regression). The 6
yarpgen "CTIMEOUT" seeds: debug 259–300 s → **release 36–56 s, 0 CTIMEOUT**. gcc-O1 in-box = 7 s, so
release zcc is **1.7× gcc**. **Rule: TIME with a release zcc.** So §CP is a POLISH (12 s → ~7 s), not
a fire — but the quadratics below are real and DO scale the 12 s.

**MEASURED phase profile (RELEASE, `ZCC_TIME=1`, phase totals over the whole module):**

| phase | sqlite -O1 (~12 s) | s0025 -O1 (~29 s) | share |
|---|---|---|---|
| **`regalloc` (of which `spill`)** | **6.7 s (spill 6.1 s)** | **18.6 s (spill 18.5 s)** | **51 % / 64 %** |
| `hir::pass` (the HIR optimizer) | 3.2 s | 7.4 s | 27 % / 25 % |
| `mir::pass` | 0.7 s | 3.3 s | 6 % / 11 % |
| isel · emit · frame · verify · cfg · domtree | each < 0.2 s | each < 0.03 s | negligible |

**The spiller is HALF the compile.** `regalloc::spill::spill_with` (`src/regalloc/spill.rs`, 1495 lines)
is #1 by a wide margin on BOTH real code and the fuzzer monster — its `for _ in 0..bound` fixpoint
re-runs an O(function) decision over `BTreeSet`s (log-factor everywhere), so it is at least
O(bound × n log n). That is the campaign's first target, ahead of everything HIR.

**Root-cause anatomy of the spiller (`spill_with`, measured this session).** The fixpoint reruns the
whole pipeline from scratch every round:

```
for _ in 0..bound {              // bound = f.vregs.len() + 2   (s0007: ~28k)
    cfg = crate::mir::verify::cfg(f);   // CFG rebuilt from scratch every round
    lv  = live::compute(f, &cfg);       // full BTreeSet dataflow, clone/block/round
    simulate(f, &lv, &cfg, &spilled, …) // full-function pass, BTreeSet residency
}
```

Three compounding costs: (1) CFG + liveness are recomputed over the whole function EVERY round
though block structure is loop-invariant until `apply`; (2) `live::compute` is a `while changed`
round-robin over all blocks with `BTreeSet<usize>` cloned per block per round (log factor +
pointer-chase + allocation); (3) `spilled` / `physlive` / the simulate residency sets are all
`BTreeSet`. Liveness is the KEYSTONE — it runs inside this fixpoint AND is used again by `color.rs`,
so fixing it once pays in two places.

**Measured catalog (worst wall-time first; each fix must be byte-identical):**
| site | cost class | fix (memory-for-speed) | output |
|---|---|---|---|
| **`regalloc::spill::spill_with`** | **#1 — 51 % (sqlite) / 64 % (s0025)**; `for _ in 0..bound` fixpoint × O(n) BTreeSet work | bound the fixpoint / dirty-worklist; BTreeSet→bitset/Vec where order is not needed | identical |
| `hir::pass` (the optimizer, all rounds) | #2 — 27 %; the sroa/rotate/licm/scev O(n²) sites below live here | the rows below | identical |
| `mir::pass` | #3 — 6–11 % | profile which MIR pass | identical |
| `sroa` mem2reg DF construction | O(preds × domdepth × `df.contains`-Vec) | bitset frontier + Cytron IDF | identical |
| `LoopForest::new` nesting | O(loops² × body) + per-header `vec![false;n]` | near-linear parent (of[]-based), reused scratch | identical |
| `rotate::force` | O(iters × full CFG/dom/loop rebuild) | batch, or incremental invalidation | identical |
| `licm` hoist scan | O(hoists × body) restart-scan | worklist, not restart | identical |
| `scev::eval_fuel` | unmemoized, up to 2^16 per `eval` on DAGs | memoize `(ValueId, fuel)` | identical |
| `sroa` `ever.contains` | **✅ SHIPPED 3894fb5** — Vec→bitset, reused across pieces | — | identical |
| `licm` `refresh_defs` | **✅ SHIPPED 3894fb5** — full-`Func` per hoist → scoped `refresh_block_defs` | — | identical |

**Baseline table (RELEASE, in-box, sqlite `-O1 -S`):** gcc 7 s · **zcc 12 s (1.7×)** · target **≤ 7–10 s**
(user's sufficiency bar). The two shipped fixes are IN this 12 s; the spiller is where the next ~5 s is.

**Shipped with R4.2 (byte-identical, "minor compile-speed" per the bank):** the two ✅ rows — `sroa`'s
IDF `ever`/`seen` bitmaps and `licm`'s scoped `refresh_block_defs`. Verified output-neutral: sqlite
**217,160 insns unchanged**, opt-parity 1552/0, torture 0 FAIL, determinism 86×8. On the suite they
cut licm on yarpgen `test` from **10.7 s → 2.4 s** and dropped the CTIMEOUT count under a session's
worth of guard experiments from **6 → 1** (s0025, backend-bound). NOT shipped: the large-function
size guards trialed this session — they de-optimize and violate the campaign goal; the algorithm
fixes below replace them.

## Status

- **Phase 0 (profiler) — DONE.** No new instrument; the pipeline's existing `ZCC_TIME=1` phase timers
  gave the profile above. Reproduce with any release zcc: `ZCC_TIME=1 <compile>` then group the
  `[time]` lines per phase.
- **Phase 1 (rank by measured wall-time) — DONE.** The catalog table is the ranking.
- **Phase 2 (the algorithm fixes) — the CP2.x ladder below. NOT STARTED.**
- **Phase 3 (re-measure) — after each bank + at close:** release sqlite (target ≤ 7–10 s) and the 6
  yarpgen seeds, output byte-identical.

## Phase 2 — the CP2.x ladder (worst-first; each step byte-identical gated)

Ordered by measured share. The spiller is > half the compile, so CP2.1–2.4 come first; within them the
**bitset + worklist liveness (CP2.2) is the keystone** — the single biggest lever, reused by `color.rs`.
Then the HIR sites by their 27 % share, cheapest-high-value first (the exponential SCEV memo).

| # | site | current cost | industrial fix (trade = memory) | class | status |
|---|---|---|---|---|---|
| **CP2.1** | spiller fixpoint invariants | rebuild CFG + liveness every round | build CFG once above the loop (topology invariant across rounds); liveness stays per-round | bound× → 1× | **✅ banked** |
| **CP2.2** ⭐ | `live::compute` (keystone) | `while changed` round-robin + `BTreeSet` clone/block/round | **predecessor worklist** (re-queue preds only when `live_in` changes — Kildall), seeded reverse-RPO | fewer visits | **✅ banked** |
| **CP2.3** | spiller `spilled` set | `BTreeSet<VReg>` contains on the per-operand hot path | dense `Vec<bool>` over the (fixed) vreg index; `physlive` left as-is (order-iterated) | log→O(1) | **✅ banked (small)** |
| **CP2.4** | spiller `simulate` per-call cost | s0025 spill 16.5 s = 3 rounds × `simulate(6555 blk, 10039 spilled)` — the cost is INSIDE `simulate`, NOT round count (measured, `ZCC_ROUNDS`) | profile `simulate`; cheapen the per-point work; a dirty-worklist only caps at 3→1 | needs profile | ⬜ **NEXT — profile-first** |
| **CP2.5** | `scev::eval_fuel` | unmemoized, up to 2^16 per `eval` on DAGs | per-call memo `(ValueId, fuel)` | **exp→linear** | **✅ banked** |
| **CP2.6** | `LoopForest::new` | per-header `vec![false;n]` | one `mark` scratch reused, cleared by body | O(loops×n)→O(Σbody) | **✅ banked (scratch)** |
| **CP2.6b** | `LoopForest::new` parent nesting | O(loops²×body) `body.contains(header)` | per-loop membership bitset → O(1) contains (O(loops²)) | N²×body→N² | ⬜ |
| **CP2.7** | `rotate::force` | full CFG/dom/loop rebuild per rotation | batch rotations, or incremental invalidation | iters×N→N | ⬜ |
| **CP2.8** | `licm` hoist scan | restart-scan per hoist | worklist, no restart | hoists×body→N | ⬜ |
| **CP2.9** | `sroa` DF construction | `contains`-bitmap shipped; DF build still O(preds×domdepth) | Cytron IDF + bitset frontier | N²→~N | ⬜ |
| **CP2.10** | `destruct` parallel-copy seq | `.position().any()` (spill.rs/destruct.rs ~498, 312) | Boissinot in-degree worklist sequencing | N²→N | ⬜ |
### The witness this campaign needed, and did not have (2026-08-28)

The byte-identical gate runs 58 small programs. Six `inline` rows passed it and
sqlite's assembly still moved: the gate-passing compiler emits
`c655fe3e83f79da3a1ddfa83c50e2c06` (289,478 lines) and the rows produced
`4f1b49325f69ce5efcf0abe68d7da714`. Nothing in the corpus inlines the way a
250,000-line translation unit does, so nothing in it could see the difference.

Two lessons, both paid for:

  * **A corpus proof is scoped to the corpus.** For a pass whose behaviour scales
    with function size, a green gate on small programs is not evidence. The
    reference `.s` for sqlite costs one slow run (5,332 s with the defect in
    place) and then every candidate is a one-second `cmp`. Take it FIRST.
  * **Never chain the baseline.** The hash the rows were compared against was set
    by the first candidate, not by the reference — so the chain agreed with
    itself while drifting away from the compiler it was supposed to reproduce.

`cfg`'s branch-threading carries the same warning from the other direction: it
was batched twice, and both times all 58 programs stayed identical while sqlite
changed. The second attempt rebuilt the use-count table from scratch after every
rewrite, which proves the cause is the interleaving of `run`'s six identities and
not stale bookkeeping — that fixpoint is not confluent.

| **CP2.11** ⭐ | `inline::run_module` — THE WALL, found 2026-08-28 | sqlite did not finish in 20 min against gcc -O1's 6 s. Five whole-program costs, all inside the per-splice `loop`: `live_across` (a whole-function dataflow) every splice; `has_loop` (CFG+domtree+loops of the CALLEE) and `body_size` and `inlinable` per CANDIDATE; `loop_blocks` (CFG+domtree+loops of the caller) every splice; and the site scan restarted from block 0 after every splice | (a) liveness asked LAST and only where it can win, at most once per splice; (b) per-callee facts memoised, invalidated only for the caller a splice rewrote; (c) `inloop` carried across splices — a splice appends, so the new blocks lie in exactly the loops `b` lies in; (d) the set is a **sparse set** (Briggs–Torczon, generation-stamped clear) not a `HashSet` and not a bitset — both of those are sized by how many values EXIST while the live set is a few dozen; (e) the scan RESUMES at the last site taken, since a refusal is a property of the callee and does not change | S×(N+V) → ~N | 🔬 **in flight — each step byte-identical (58 programs)** |

**Overlap guard (already shipped, do NOT redo):** the `sroa` `ever/seen`-`contains` bitmap and the
`licm` scoped `refresh_block_defs` landed in 3894fb5. CP2.9 is the REMAINING sroa work (the IDF/DF
construction algorithm), and CP2.8 is the REMAINING licm work (the hoist restart-scan) — neither
touches the shipped code.

## Scoreboard — first batch banked (2026-08-25)

CP2.1, CP2.2, CP2.5, CP2.6 shipped together (`src/regalloc/spill.rs`, `src/regalloc/live.rs`,
`src/hir/pass/scev.rs`, `src/cfg.rs`). All four are pure algorithm swaps, no output change.

**Measured, in-box (aarch64 musl, RELEASE, `-O1 -S`):**
| target | baseline | after batch 1 | Δ |
|---|---|---|---|
| sqlite wall | 12 s (1.7× gcc) | **9.99 s (1.43×)** | **−17 %** |
| sqlite `spill` | 6.1 s | **4.54 s** | **−26 %** |
| sqlite `regalloc` | 6.7 s | 5.08 s | −24 % |
| sqlite `hir::pass` | 3.2 s | 3.22 s | flat (few loops) |
| s0025 wall | 29 s | **23.14 s** | **−20 %** |
| s0025 `hir::pass` | 7.4 s | **2.69 s** | **−64 %** (scev+loopforest) |

Spill wins come from CP2.1+2.2 (real code); the −64 % HIR win on the yarpgen loop monster comes from
CP2.5+2.6, invisible on sqlite by design. Output identical: sqlite **217,160** insns, s0025 **31,651**.

**Correctness gate (batch 1):** byte-identical `.s` proven over — 57 host corpus (default opt),
7 freestanding stress at `-O1` (loops → scev/loopforest exercised), **1000 csmith at `-O1` patched
vs pristine (0 differ)**, in-box sqlite + s0025 (identical output). torture **1378 pass, 0 FAIL**.
(Full yarpgen-seed sweep skipped in this session — the pathological seeds are ~40 s each and the pure
byte-identical proof already covers the loop path; run it at campaign close.)

**Batch 2 (CP2.3) banked:** `spilled` `BTreeSet<VReg>` → dense `Vec<bool>` (contains on the
per-operand hot path is now O(1); `apply` still mints slots in ascending-vreg order, byte-identical).
Marginal by design: s0025 spill 16,954 → 16,493 ms (−3 %), sqlite neutral (low pressure). The bitset
removed the log factor, but the spiller's dominant cost on the high-pressure yarpgen function is the
NUMBER OF FIXPOINT ROUNDS (each re-simulates the whole function), not the per-lookup constant.

**Measurement correction (Law-2, `ZCC_ROUNDS`):** the spiller fixpoint runs only **3 rounds** on
s0025 (6555 blocks, 10039 spilled) and ≤5 on sqlite's biggest functions — it is NOT round-count
bound. The 16.5 s is `3 × simulate(...)`; the cost lives INSIDE one `simulate` call, superlinear in
the pressure (10039 memory-resident values), not in the number of rounds. So a dirty-worklist that
cut 3→1 caps at −66 % and carries real byte-identical risk (the per-block plan depends on cross-block
entry sets) — it is NOT the first move.

**Next ⬜ = profile `simulate` itself** (`ZCC_TIME`-style coarse timers around its setup vs its RPO
per-point loop, then within the loop) to find the construct that scales with the resident-value count,
and cheapen THAT (same memory-for-speed pattern as CP2.3 — a bitset/index where a set/scan sits on the
per-point path). `physlive` is bounded by the ~32 physical registers, so it is unlikely to be the
sink; the suspect is value-level residency tracking (`w` / `held` / `room`) over the up-to-nsp working
set. Measure before converting. Then CP2.6b / CP2.7–2.10 for the HIR tail.

## The per-fix loop (constitution's iteration process; unchanged)

For each CP2.x, worst-first, one at a time:

1. **Predict** the Δ on the complexity model (state the class change, e.g. `bound×n log n → n·E`).
2. **Baseline first** (TDD-shaped): record the RELEASE `ZCC_TIME=1` time for the target phase on
   sqlite + s0025, and snapshot the md5 corpus.
3. **Implement** the algorithm swap (memory-for-speed; no output change, no size cap).
4. **Gate — byte-identical `.s`** via `tests/refactor_gate.sh` over the fixed corpus (the proof output
   is untouched, Article G refactor dual) **PLUS the full correctness gate** (cargo + torture +
   opt-parity + csmith300 + yarpgen300 + determinism). Byte-identical alone is necessary, not
   sufficient — a correctness regression can still be byte-identical by luck, so the full gate stays
   mandatory.
5. **Re-measure** RELEASE sqlite (target ≤ 7–10 s) + s0025; record the number.
6. **Bank** (commit, number recorded) or, on a wall, quarantine that CP2.x with a `BLOCKED:` note and
   advance — never fork the plan.

**Standing caution (from §CP + §13n):** the allocator is where the nastiest defects live. Any CP2.x
that weakens an allocator invariant ships its verifier check (`mir::verify` virtual mode) in the same
commit. CP2.4's convergence argument is the one at risk of getting hairy — bounded Law-2 attempt; if
the dirty-worklist termination proof does not close, ship the CP2.1–2.3 gains and mark CP2.4 residual.

## Running the campaign

Each CP2.x is near-independent and has an objective acceptance gate (byte-identical + full gate), so
it maps cleanly onto `superpowers:subagent-driven-development`: one fix per subagent, the gate as the
acceptance criterion, `superpowers:verification-before-completion` as the proof-before-bank step.
Keep the scoreboard here (mark each row `✅ banked <sha>` / `BLOCKED: …`), edited in place.
