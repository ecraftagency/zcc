# PLAN — what is not proven yet

**THIS FILE IS ALLOWED TO BE WRONG.** Every line is a HYPOTHESIS about a
compiler that does not exist yet. Nothing here may be cited from `src/`, and
nothing here is evidence of anything.

**The contract, and the cap is the whole of it.** At most 100 lines. A row leaves
by exactly one of two doors — baked into `MECHANISM.md` because it won, or
written into `MECHANISM.md` Part F as a refutation because it lost — and a row
that cannot be stated in ten lines is not understood well enough to be here. The
file is TRUNCATED, never appended to: `REARCH.md` grew to three thousand lines
because nothing ever left it.

---

## The open campaign — the register copy (`MECHANISM.md` Part E)

**C0 — attribute the copies before touching anything.** The census counts what
reaches the assembler; it does not say which mechanism minted each copy. Three
sources want opposite fixes: SSA destruction colouring a phi's two ends
differently; a parallel copy that is a genuine permutation (costs copies however
it is coloured — NOT a coalescing failure); and a `Copy` minted by
`mir/pass/ext.rs` that colouring never erased. Instrument `regalloc/destruct.rs`
and `regalloc/color.rs` with counters under `ZCC_TIME`, over the suite and over
sqlite, and produce the table. **No code is written before it exists** — three
rows were lost on 2026-08-28 by inverting this order.

**C1 — the identity copy.** 67 copies name the same register at both ends (gcc:
none); `k1_dispatch` ends every switch arm with `mov w10, w10`. Not soundly
removable in `emit.rs`: a `w`-form write zeroes bits 63:32 and at `Width::W32`
the lattice in `ext.rs` proves a fact about the low half only. Restate the fact
at full width there instead.

**C2 — eviction / priority colouring.** 14,615 hints were refused on sqlite
because the register was OCCUPIED, not because they were absent or misordered;
three ordering fixes are already refuted. Needs C0 first: a refusal whose
occupant is a hinted phi is a different problem from one whose occupant is a
reload.

**C3 — split the PARAMETER at the terminator**, not the web. `evict_params`
strips `has_def`, so a loop-header phi can never carry an accumulator. Judge
against the 1.10× sqlite floor, not 1.0.

**C4 — ABI argument placement.** +98 copies on the suite, 40% of sqlite's size
gap on its own. A separate front; do not fold it into C2's measurement.

## Open rows carried from closed campaigns

- **S4 — the copy residual in `sqlite3VdbeExec`** (+1,252 reg-reg `mov`). Same
  family as C2; likely subsumed by it. Gate: under 800.
- **CP2.4 — profile `simulate`.** The spiller's per-call cost, not its round
  count, is what remains. Profile before touching.
- **CP2.6b / CP2.7 / CP2.10** — a quadratic each: `LoopForest` parent nesting,
  `rotate::force` rebuilding the CFG per rotation, `destruct`'s parallel-copy
  sequencing by `.position().any()`. All below the knee on today's corpus.
- **`ldp` may write its own base** — a missed pair, measured, in
  `mir/pass/ldstp.rs::fuse`.
- **`M4` unsettled**: the jump-table crossover is not a function of case count
  (`isel/lower.rs::MIN_CASES`). **`M7`, `M8`** never swept on this corpus.

## The documentation consolidation (stage 2)

`REARCH.md` is 3,194 lines and 41 places in `src/` point into it. Triage it:
settled theorems to `THEORY.md`, target facts to `ARM64.md` (from
`src/arm64_elf.md`), living machinery to `MECHANISM.md`, the rest deleted.
`MILESTONES.md` folds into `README.md`. The end state is five documents the
source may point at, and `REARCH.md` is not one of them.
