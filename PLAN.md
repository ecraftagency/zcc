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

**Five advanced capabilities, present, not delivering.** TBAA is the sharpest:
every memory pass — `mem.rs`, `licm`, and the `loopmem` written the same day —
runs against an oracle where everything may alias everything, because the field
that would say otherwise is stamped `ANY`.

The HIR/MIR split gave each half its own theorems and passes, and that seam is why
a second target adds a second MIR instead of a conditional. This is the same move
one level in: every loop pass in `hir/pass/` privately rebuilds the same three
analyses. **The structure holds the passes down, not their absence.**

* **`M44`** — the hoist cost **+28.7% of sqlite's compile time** because it
  rebuilds `cfg` + `DomTree` + `LoopForest` per function, after `const_share::run`
  has just built the first two. A cycle-test early-out to skip loopless functions
  measured at **ZERO**: the cost is the analysis, not the wasted calls.
* **`M53`, `unroll.rs:242`** — both blocked by SSA reconstruction. `tailjump`
  stops at the one block worth duplicating, with `M52`'s counters saying **1.95×
  the instructions** are on the table at `m2_http_parse`.
* **The 1.1× target** — 33 of 96 are above 1.3× against `gcc -O2`, dominated by
  vectorization. `slp.rs` is BUILT; it lacks an oracle sharp enough to prove two
  accesses independent (TBAA, also built, also off) and a loop dependence test
  beside it. Both want this substrate.

**WHAT THE LAYER IS.** One owner for dominance, loops, and SSA repair, held
across a function's pass pipeline and INVALIDATED rather than rebuilt:

  1. **A cached handle** — `cfg`, `DomTree`, `LoopForest` computed once per
     function, invalidated by the passes that change the CFG. Most do not.
  2. **`reconstruct(f, &[ValueId])` as a first-class operation.** `sroa.rs:256`
     already computes iterated dominance frontiers and places pruned parameters
     (Cytron et al. 1991 §4.2). **START HERE: read `promote` — does it factor, or
     is its pruning too tied to a `Piece`?** One reading of one function, and it
     decides whether this grind is small or medium.
  3. **Somewhere for dependence analysis to live**, beside the built SLP.

**THE SEAM RULE (Article B).** A pass READS the layer and DECLARES what it
invalidates. The reviewer's question for every diff: *what did this pass rebuild
that it could have read?*

**HOW IT IS PROVEN.** The HIR/MIR split shipped byte-identical
(`refactor_gate`), and so does this: caching an analysis must not change one
instruction. Two witnesses per Article E — 96 suite programs and `sqlite3.c` at
742k lines — plus the 16-stage gate NATIVELY on the Graviton box. **A codegen
change is a bug in the caching, not a bonus** — turning on what the layer ENABLES
is a separate, measured row with its own A/B.

**THE PAYOFF, priced before the build so it can be checked after.** Compile time:
`M44`'s +28.7% is the visible half, every other loop pass paying the same toll is
the invisible one (sqlite compiles in **6.77 s** today). The hoist becomes
affordable and re-measurable on its merits (−0.74% EXEC on the suite, neutral on
the application). `tailjump` reaches its target and `unroll` loses its stated
limitation. And `ZCC_SLP` / `ZCC_TBAA` become answerable questions rather than
seams nobody can afford to turn on — one A/B each on the Graviton box.

**WHAT IT DOES NOT DO.** It buys no exec by itself. Article G ranks a refactor
`better ground for optimization ∧ easier proof`; report it that way.

**FALLBACK if the factoring is hard:** remat-then-duplicate for `tailjump` alone,
`M53`'s last section — built and reverted 2026-08-29, idea sound, implementation
was not.

**Method:** `M51` (census before building), `M52` (`perf` when the axes disagree),
and read the harness's own summary line — three times in one session an instrument
spoke and was not heard.
