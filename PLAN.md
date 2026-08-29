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

## THE GRIND: SSA reconstruction, and the two rows that are waiting on it

**WHY THIS ONE AND NOT A FASTER-LOOKING ONE.** Two passes now carry a comment
saying they are blocked by the same missing capability, and both were measured
before they were blocked:

* `tailjump` (`M53`) copies a state machine's dispatch into each arm. It works,
  it is gated, and it stops at the one block worth duplicating — `m2_http_parse`'s
  dispatch loads the byte its arms read, and that byte is used PAST the arm's
  first block, so a copy would need a phi. `M52`'s hardware counters say **1.95×
  the instructions** are on the table there.
* `unroll.rs:242` names the identical gap in its own words: *"SSA reconstruction
  this row does not do."*

One capability, two rows already priced. That is what a grind is for.

**WHAT IT IS.** After a block is duplicated, a use of the original block's values
must choose between two definitions. Today `tailjump` REFUSES any block whose
definitions are used past an immediate successor, because a block parameter can
carry the choice only that far. The general answer is phi placement at the
iterated dominance frontier of the new definitions — Cytron et al. 1991 §4.2,
which `sroa.rs` already implements for memory pieces (`sroa.rs:275`, "the runner
formulation") and which has to be lifted to arbitrary values.

**START HERE: read `sroa.rs:256 promote`.** It computes frontiers, places pruned
parameters, and renames. The question to answer first is whether it can be
factored into a `reconstruct(f, &[ValueId])` that `tailjump` and `unroll` call,
or whether the pruning it does is too tied to a `Piece`. That answer is one
reading of one function, and it decides whether this grind is small or medium.

**THE CHEAPER PATH THAT WAS TRIED AND FAILED, so it is not tried again blind.**
"Remat, then duplicate" — copy the dispatch's load DOWN into each arm that uses
it, which removes the live-out entirely and is free on the clock, since every
path still executes exactly one load and only the static count grows. It was
built and reverted inside an hour on 2026-08-29: the first cut deleted the load
from the dispatch without placing it correctly, and `m2_http_parse` went from one
`ldrb` to none and hung. **The idea is sound and that implementation was not.**
If the factoring above turns out to be hard, this is the fallback — but it starts
from the failure, which was in the use-rewriting, not in the reasoning.

**THE MEASUREMENT THE ROW OWES.** `m2_http_parse` against `gcc -O2` on the
Graviton box: it is **2.075×** today. `perf stat` before and after, not just the
clock — `M52` is the entry that exists because the assembly said "branch
prediction" and the counters said "instruction count", and the row was nearly
built against the wrong cost model.

**AND WHAT THIS GRIND DOES NOT REACH, stated so nobody expects it to.** The
target of 1.1× against `gcc -O2` is not reachable by this row or by any row of
its kind. The distribution on 2026-08-29 is 19 programs FASTER than gcc -O2, 39
within 10%, and 33 above 1.3× — and the 33 are dominated by three
transformations zcc does not have at all: vectorization (`z4_matmul_int` 3.44,
`o2_fp_stencil` 2.76), loop-idiom (`g1_memcpy_loop` 30.6, though `M51` measured
that shape as absent from 435 real source files), and loop deletion (two programs
leave the geomean entirely because `-O2` removes their loop). **The grind that
reaches 1.1× is a vectorizer.** This one is its prerequisite only in the sense
that it is smaller, bounded, and already paid for by two measured rows.

**Method, unchanged and it is what found everything this month:** census the
corpus BEFORE building (`M51` — the previous obviously-right row fired on two
initialisation loops and nothing else in 435 files); hand-edit the `.s` and verify
the output before touching the compiler; interleaved best-of-N on an idle box, and
`perf` when the axes disagree.

**The gate every row owes:** a commuting square named on `pub fn run` and a test
that actually executes it (`provenance.sh` is RED without both), `ubscan` green,
and the full gate run NATIVELY on the Graviton box — 16 stages, and the box is
`terraform apply -var instance_type=c8gd.2xlarge` in us-west-2, ~$0.0016/min.

**Rows that belong to the closed suite-tail grind and were never built** —
`u4_popcnt64` (INSN 2.88), `q2_deep_rec`, `o2_fp_stencil` — are facts about what
was not built, not plan rows. They live in `MECHANISM.md`, not here.
