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

## THE GRIND: the register copy that is half the instruction gap

The facts are `MECHANISM.md` Part E. Measured there: `mov reg,reg` is **70% of
zcc's entire instruction excess** over gcc -O1, and **519 of those sit at a block
edge against gcc's 56** — half the total gap on its own. sqlite says the same
independently (+10,464 of a 20,264 gap).

**C0 — attribute the copies before touching anything.** The census counts what
reaches the assembler; it does not say which mechanism minted each copy. Three
sources want opposite fixes: SSA destruction colouring a phi's two ends
differently; a parallel copy that is a genuine permutation (costs copies however
it is coloured — NOT a coalescing failure); and a `Copy` minted by
`mir/pass/ext.rs` that colouring never erased. Instrument `regalloc/destruct.rs`
and `regalloc/color.rs` with counters under `ZCC_TIME`, over the suite and over
sqlite, and produce the table. **No code is written before it exists.** Three
rows were lost on 2026-08-28 by inverting this order.

**C1 — the identity copy.** 67 copies name the same register at both ends (gcc:
none); `k1_dispatch` ends every switch arm with `mov w10, w10`. Not soundly
removable in `emit.rs`: a `w`-form write zeroes bits 63:32, and at `Width::W32`
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

**The gate every row owes:** a commuting square, both axes (EXEC before size,
Law 0), interleaved pairs inside one box session for EXEC, and the deterministic
INSN geomean for any size claim.
