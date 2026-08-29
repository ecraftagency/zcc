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

## THE GRIND: an ANALYSIS layer — the re-architecture the measurements keep asking for

**THE SHAPE OF THE ARGUMENT, and it is not "zcc needs more passes".** Count what
is already built and NOT RUNNING:

| capability | where | why it is off |
|---|---|---|
| **SLP vectorizer** | `mir/pass/slp.rs:63` | *"R5.3's A/B SEAM (`ZCC_SLP`). Off, no `VAlu` is ever built"* |
| **TBAA alias oracle** | `hir/mod.rs:236` | *"R5.2's A/B SEAM (`ZCC_TBAA`). Off, every access is stamped `ACLASS_ANY`"* |
| **loop-constant hoist** | `mir/pass/const_share.rs` | reverted by `M44` for **+28.7% of sqlite's compile time** |
| **tailjump**, at its target | `hir/pass/tailjump.rs` | fenced by missing SSA reconstruction (`M53`) |
| **unroll**, past its limit | `hir/pass/unroll.rs:242` | *"SSA reconstruction this row does not do"* |

**Five advanced capabilities, present, not delivering.** TBAA being off is the
sharpest of them: every memory pass in the compiler — `mem.rs`, `licm`, and
`loopmem` written on 2026-08-29 — runs against an oracle where everything may
alias everything, because the field that would say otherwise is stamped `ANY`.

The HIR/MIR split gave each half of the compiler its own theorems and its own
passes, and the seam is why a second target adds a second MIR instead of a
conditional. This grind is the same move one level in: every loop pass in
`hir/pass/` privately rebuilds the same three analyses, so there is no seam, no
sharing, and no place to put a fourth analysis when one is needed. **The structure
is what is holding the passes down, not their absence.** Three measurements on
2026-08-29 point at that one fact.

* **`M44`** — the loop-constant hoist was reverted for **+28.7% of sqlite's
  compile time**. It builds `cfg` + `DomTree` + `LoopForest` per function, and
  `const_share::run` immediately before it has already built the first two. A
  cycle-test early-out was written to skip loopless functions and measured at
  **ZERO**: an amalgamation's functions mostly DO have loops, so the cost is the
  analysis itself and not the wasted calls.
* **`M53` and `unroll.rs:242`** — two passes carry a comment saying they are
  blocked by the same missing capability, SSA reconstruction. `tailjump` refuses
  any block whose definitions are used past an immediate successor and therefore
  stops at the one block worth duplicating, with `M52`'s counters saying **1.95×
  the instructions** are on the table at `m2_http_parse`.
* **The 1.1× target** — 33 of 96 programs are above 1.3× against `gcc -O2` and
  they are dominated by vectorization. `slp.rs` is BUILT; what it lacks is an
  alias oracle sharp enough to prove two accesses independent (TBAA, also built,
  also off) and a loop-level dependence test to sit beside it. Both want the same
  substrate, and building them on per-pass rebuilds would make the compile-time
  problem structural rather than incidental.

**WHAT THE LAYER IS.** One owner for dominance, loops, and SSA repair, held
across a function's pass pipeline and INVALIDATED rather than rebuilt:

  1. **A cached analysis handle.** `cfg`, `DomTree`, `LoopForest` computed once
     per function and invalidated by the passes that change the CFG. Most passes
     do not change it at all.
  2. **`reconstruct(f, &[ValueId])` as a first-class operation.** `sroa.rs:256`
     already computes iterated dominance frontiers and places pruned parameters
     for memory pieces (Cytron et al. 1991 §4.2, "the runner formulation").
     **START HERE: read `promote` and answer one question — does it factor, or is
     its pruning too tied to a `Piece`?** That answer decides whether this grind
     is small or medium, and it is one reading of one function.
  3. **A place for dependence analysis to live**, which is what the vectorizer
     will need and what nothing today could host.

**THE SEAM RULE, carried over from Article B.** A pass may READ the layer and must
DECLARE what it invalidates. A pass that rebuilds an analysis privately is the
defect this grind exists to remove, and the reviewer's question for every diff is
"what did this pass rebuild that it could have read?"

**HOW IT IS PROVEN, and this is not negotiable for a re-architecture.** The
HIR/MIR split shipped byte-identical (`refactor_gate`), and so does this: caching
an analysis must not change one instruction. Two witnesses per Article E — all 96
suite programs and `sqlite3.c` at 742k lines — plus the 16-stage gate NATIVELY on
the Graviton box. **Any codegen change is a bug in the caching, not a bonus.**

**THE PAYOFF, priced before the build so it can be checked after:**
* compile time — `M44`'s +28.7% is the visible half; the invisible half is every
  other loop pass paying the same toll. sqlite compiles in **6.77 s** today.
* the hoist row becomes affordable again and can be re-measured on its merits
  (it was −0.74% EXEC on the suite, neutral on the application).
* `tailjump` reaches its target and `unroll` loses its stated limitation.
* `ZCC_SLP` and `ZCC_TBAA` become answerable questions rather than seams nobody
  can afford to turn on — each is one A/B on the Graviton box once the analysis
  they need is cheap and shared.

**WHAT IT DOES NOT DO.** It buys no exec by itself. A re-architecture is ranked
`better ground for optimization ∧ easier proof` (Article G), and it must be
reported that way — no number is claimed for it beyond compile time.

**THE FALLBACK IF THE FACTORING IS HARD.** "Remat, then duplicate" for `tailjump`
alone: copy the dispatch's load down into each arm that uses it, removing the
live-out. Free on the clock — every path still executes one load, only static size
grows. Built and REVERTED on 2026-08-29 when the first cut deleted the load
without placing it and `m2` hung. **The idea is sound and that implementation was
not**; it starts from the failure, which was in the use-rewriting.

**Method, unchanged and it is what found everything this month:** census the
corpus BEFORE building (`M51`); hand-edit the `.s` and verify the output before
touching the compiler; interleaved best-of-N on an idle box; `perf` when the axes
disagree (`M52`). And read the harness's own summary line — three times in one
session an instrument spoke and was not heard.
