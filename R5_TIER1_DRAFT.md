# R5 Tier 1 — batch implementation draft (2026-08-27)

Broad-speed opening hand, ranked broad-speed ÷ effort. **Most of Tier 1 is
WIRING existing hooks, not building.** Each row lists: files, the exact change,
what already exists to reuse, the proof obligation (a commuting square, for the
gate you run after the batch), and a default-off toggle name so the batch can be
A/B'd and bisected.

Toggle convention: an env var `ZCC_R5=<csv>` or per-row `ZCC_<NAME>=1`, mirroring
the existing `ZCC_NOPASS=ifconv` style. Default OFF until gated.

---

## §0 — BOOT (revised 2026-08-27 23:00, after the night's commits)

The body of this document was written at 11:47. Five commits landed after it, and
two of its premises moved. Read this section before the rows.

**Tree state.** Branch `mir-rearch`, HEAD `678e700` (pushed). The night's
correctness work — `141b824` countdown phi, `4de446c` cost-aware eviction,
`01f2ae4` + `aab7da7` diagnostics — is in. So is `678e700`, four compile-speed
fixes of one class (a linear scan where an indexed lookup belongs).

**Order to implement (unchanged): R5.1 → R5.2 → R5.5 → R5.4 → R5.3.**
Cheapest wiring first; SLP last because it adds `V128` and is the only real infra.

**Status per row.**

| row | status |
|---|---|
| R5.1-A populate `Block.weight` | **VERIFIED + WRITTEN, uncommitted.** `freq::annotate` + the `hir::weight` phase in `compile.rs`, gated `ZCC_WEIGHTS=1`. Byte-identical with the toggle BOTH on and off (58 programs) — it must be, nothing reads `weight` yet. That is its commuting square. |
| R5.1-B layout by weight | not started; see the correction below |
| R5.1-C spiller by weight | **STALE — do not apply as written** |
| R5.2 TBAA | not started; its premise (aclass is vacuous) is still UNVERIFIED — grep first |
| R5.3 SLP | not started |
| R5.4 scheduling | not started |
| R5.5 VRP | not started |

**CORRECTION 1 — R5.1-A's premise is confirmed, and it is the keystone.**
`hir::Block.weight` defaults to `1` (`hir/mod.rs:675`) and the nine sites that
touch it only PROPAGATE a neighbour's value (`dom.rs:42`, `licm.rs:453`,
`cfg.rs:384`, `inline.rs:385,471`, `rotate.rs:388`, `layout.rs:152`,
`isel/lower.rs:2565`, `frame.rs:297`). Nothing ever called `freq::estimate`.
A grep for `.weight` excluding assignments returns NOTHING: the field is written
by nine sites and read by none. Plumbed and dead. R5.1/R5.4/R5.6 all want it.

**CORRECTION 2 — R5.1-C is stale and would revert a correctness fix.**
It says to replace `lf.depth` in the spiller's eviction ranking. That ranking was
rewritten the same evening in `4de446c`: a victim is now weighed by what it COSTS
to restore (rematerializable values rank cheap), not by distance alone. That fix
closed `c6837`/`c04804`. **Re-read `spill.rs` eviction before touching it**, and
treat weight as a NEW factor beside the cost term, not a replacement for it.

**CORRECTION 3 — layout's current order is plain RPO; its depth sort is dead.**
`mir/pass/layout.rs:36-37`:
```rust
order.sort_by_key(|&b| (cfg.rpo_num[b], Reverse(lf.depth[b])));   // dead
order.sort_by_key(|&b| cfg.rpo_num[b]);                           // decides everything
```
`rpo_num` is unique per block, so the second sort fully determines the order and
the first cannot affect it. The comment above it claims the pass keeps loop
bodies contiguous by visiting deeper successors first. It does not. R5.1-B is
therefore a clean slate, and the dead sort should go with it.

**OPEN DECISION — settle this first; it defines "done" for every row.**
The batch-order section at the foot of this file says the batch "ships untested
per your call", which has no merge criterion under Law 3. On the night of
2026-08-27 the user raised dropping formal verification (full gate 7 min and
growing; three fuzz bugs cost four hours) and did not decide. The honest
accounting from that night: the proofs cost about a minute (`cargo test` 0.75s,
refactor gate ~1 min); the four hours went to LOCALIZATION, not to proving.
Removing the squares would have saved none of it. The leak is that the FULL gate
is being run per-row when the recorded workflow is per-row `cargo test` +
`provenance.sh` + `localize.sh`, full gate every 2–3 rows.

**MISSING ROW — a complexity-fidelity gate.** `678e700` fixed four defects of one
class and NO gate in the repo can see that class: `provenance.sh` checks
citations, the refactor gate checks bytes, and every fuzz corpus member is below
the knee where a quadratic is invisible. The instrument is the dual of Article
E's resource-fidelity, which certifies the emitted code's cost but says nothing
about the compiler's own: compile a synthetic family at N, 2N, 4N (parameters,
blocks, slots as separate axes), read per-stage wall time from the `ZCC_TIME`
hooks that already exist, and assert growth stays under ~2.5× per doubling. This
is Tier-1-shaped and is not in the rows below.

**MEASUREMENT CONSTRAINT.** The `us-east-2` fuzz box was destroyed at end of
session (terraform state empty, zero instances, zero spot requests). No geo40,
exec, or sqlite number can be taken until one is relaunched:
`cd tests/tf && terraform apply -var "ssh_cidr=$(curl -s ifconfig.me)/32"`.
Implementation, squares and unit tests all work locally without it. **Do not
launch it without being asked.**

**WHAT R5.1 CANNOT BE JUDGED BY.** R5.1-B changes block order by design, so the
byte-identical gate FAILS on purpose for that row — it is not the proof. The
proof is `layout.rs:26`'s existing `SQUARE layout_preserves_every_edge`; the
JUDGEMENT is a paired INSN+EXEC re-measure per toggle, distribution not a single
number (CLAUDE.md Law 3c: 0.9× is the parity margin, and the claim is always
"on this suite, on this core").

---

## R5.1 — block weights → layout + spill-weighting  ★4 (Ball & Larus 1993)

**Goal.** Order blocks and weight spill decisions by real execution frequency,
not loop-nesting depth alone. This is the universal enabler and it also sharpens
R5.4 and R5.6.

**What already exists (do NOT rebuild):**
- `hir::freq::estimate(f, cfg, lf) -> Vec<u64>` — Ball & Larus, `ENTRY=10_000`,
  `TRIPS=10`. Done.
- `hir::Block.weight: u32` (advisory) and **`mir::MBlock.weight: u32`**, and isel
  **already copies it down**: `src/isel/lower.rs:2565  f.blocks[bi].weight = b.weight`.
- `src/regalloc/spill.rs` already loop-weights next-use via `lf.depth` (line 239).
- `src/mir/pass/layout.rs::run` currently orders by `cfg.rpo_num` then loop depth
  (lines 35–37), **ignoring weight**.

**Change A — populate the weights (one place).** Before isel, run
`hir::freq::estimate` once per function and write the result into `Block.weight`
(scaled/clamped to `u32`). Confirm nothing else already writes it; if `weight`
is still the `1` default everywhere, this is the missing line. Grep:
`rg 'weight *=' src/hir`.

**Change B — layout uses weight (`layout.rs`).** ⚠️ The "RPO+depth sort" this
describes does not exist: the depth sort at `layout.rs:36` is dead code, fully
overwritten by the `rpo_num` sort on the next line (§0 CORRECTION 3). Today's
order is plain RPO. Delete the dead sort and its misleading comment as part of
this change. Replace the RPO+depth sort with
a **fall-through-maximizing** order that prefers the heavier successor:
- keep RPO as the tie-breaker/seed (preserves the A6b square: order changes,
  edges do not — no semantic obligation),
- greedily chain blocks: from each block, lay its **highest-weight not-yet-placed
  successor** next so the hot edge becomes the fall-through; cold successors get
  the branch. (Pettis & Hansen bottom-up would be the full version; the greedy
  chain is the cheap 90%.)

**Change C — spiller uses weight (`spill.rs`).** ⚠️ **STALE — see §0 CORRECTION
2 before applying.** Written against an eviction ranking that `4de446c` replaced
hours later. The ranking now weighs a victim by what it COSTS to restore
(rematerializable values rank cheap), which is what closed `c6837`/`c04804`.
Weight is a NEW factor to combine with that cost term, **not** a replacement for
it, and `lf.depth` is no longer the only scale in play. Re-read the function
first. Keep `next_use` as the primary key; weight scales the distance.
**This is the general form of the proven pOp-residency win** — a value used in a
high-weight block is spill-resistant.

⚠️ **Interaction:** `regalloc/spill` is 61% of the pathological s7876 compile
(140s of 228s) and is super-linear there. It is NOT a regression — measured 127s
at `aab7da7`, before the eviction fix — so there is no commit to revert to. This
row makes the spiller do MORE work per eviction. Measure compile time, not only
output quality.

**Reuse:** `freq`, `MBlock.weight`, existing `lf.depth` plumbing.
**Proof:** layout keeps the A6b `SQUARE layout_preserves_every_edge` (already in
`layout.rs:26`) — weight has no semantic obligation. Spilling stays under the RA
verifier `§7.6` (correctness unchanged; only WHICH value spills moves).
**Toggle:** `ZCC_WEIGHTS=1` (off → depth-only, today's behaviour).

---

## R5.2 — TBAA → load-elim / DSE / LICM  ★1 (Diwan-McKinley-Moss 1998; C99 6.5p7)

**Goal.** Let the alias oracle actually disambiguate, so redundant loads and dead
stores across type-incompatible pointers are removed everywhere.

**What already exists:**
- `hir::AClass = u32`, `ACLASS_ANY = 0`; `Inst::Load{aclass}` / `Store{aclass}`
  carry it (`src/hir/mod.rs:190,229,237`).
- `src/hir/pass/mem.rs` — the load-elim/DSE pass — **already reads `aclass`** and
  treats two locations as DISJOINT by it (lines 11–17), with the
  `THEORY A7b SQUARE a_second_read_of_the_same_place_is_the_first` proof.

**The catch to verify first:** if every `Load/Store` is emitted with `ACLASS_ANY`,
the oracle is VACUOUS (everything may-alias) and `mem.rs` almost never fires.
Grep the emit sites: `rg 'aclass' src/hir/build.rs src/isel`. **R5.2's real work
is the FRONTEND assigning a real `aclass`,** not the oracle.

**Change — assign aclass from the C99 effective type (TBAA lattice).** In
`hir::build` (or `ast`→HIR lowering) where `Load`/`Store` are created, compute an
`AClass` from the access's effective type per C99 6.5p7:
- one class per *type-access* equivalence: compatible types share a class; a
  `char`/`unsigned char` access is `ACLASS_ANY` (may alias anything, 6.5p7 last
  bullet); distinct non-char scalar types get distinct classes; struct/union
  members alias per their own type.
- concretely: intern a small table `effective_type -> AClass` (e.g. hash of the
  canonical `Ty` + a struct-field tag), reserve `0 = ANY`, and stamp it at the
  access.
Then `mem.rs` (and LICM's load-hoist, `licm.rs`; DSE) start disambiguating with
no change to those passes.

**Reuse:** `mem.rs`, `licm.rs`, the existing `AClass` field and its DISJOINT test.
**Proof:** the oracle's soundness square is the obligation — **a class may only
be declared disjoint from another when C99 6.5p7 permits** (the aggressive
direction is the dangerous one). Add a battery: two pointers of incompatible type
to the same address, referee `cc -fstrict-aliasing`. `char*` must stay ANY.
**Toggle:** `ZCC_TBAA=1` (off → stamp `ACLASS_ANY` everywhere = today).

---

## R5.3 — SLP-SIMD (the simple, straight-line half)  #13 (Larsen & Amarasinghe 2000)

**Goal.** Pack adjacent isomorphic scalar ops in straight-line code into NEON
vector ops. **Only SLP — no loop vectorization** (no cross-iteration dependence
analysis). This is where the sub-1× headroom margin on kernels lives.

**What must be BUILT (this is the one real infra row):**
- `hir::Ty::V128` — add to the enum (`src/hir/mod.rs:31`); `bytes()=16`,
  `bits()=128`, `is_float()` per lane kind (carry a lane tag, e.g.
  `V128(LaneTy)` or a parallel field). Touch every exhaustive `match Ty`.
- MIR vector ops: `add/sub/mul/fadd/fmul/…` on the FPR class (which **already
  holds v-regs** — regalloc needs no new class), plus `ld1/st1` (contiguous
  vector load/store) and `ins/umov` (pack/unpack lanes). Add to `mir::mod.rs`
  op enums + `emit.rs` + `mir::cost.rs` latency (MEASURED).
- isel patterns for the packed ops.

**Change — the SLP pass (`src/hir/pass/slp.rs`, new):**
1. within a block, find **seed groups**: sets of ≥2 isomorphic scalar ops (same
   opcode, same `Ty`) whose operands are adjacent memory (`base+0`, `base+stride`)
   or already vectors.
2. build the SLP tree upward from seeds (Larsen-Amarasinghe): an op joins a pack
   if all its operands pack isomorphically; stop at non-isomorphic or aliasing.
3. cost-gate on `mir::cost` (a pack pays only if lane_count × scalar_cost >
   vector_cost + pack/unpack); **refuse packs that need a scatter/gather** (SLP
   keeps only unit-stride).
4. emit `V128` ops; leftover scalars stay scalar.

**Reuse:** FPR reg class, `mir::cost` latency framework, the block dataflow already
in `gvn.rs`/`mem.rs` for isomorphism checks.
**Proof:** per-pack commuting square `⟦scalars⟧ = ⟦pack⟧` on `hir::interp`
(extend the interpreter with V128 lanewise semantics — DDI 0487 lane rules).
Battery: pairwise sums/products over small arrays, referee `cc -O2`.
**Toggle:** `ZCC_SLP=1` (off → no `V128` ever emitted; the type can exist unused).
**Scope guard:** ship the type + a handful of packs FIRST (add/mul/load/store);
resist growing the pattern zoo until measured.

---

## R5.4 — BB list-scheduling  #9 (Gibbons & Muchnick 1986; Law 3c)

**Goal.** Reorder within a basic block by critical-path priority so long-latency
ops issue early and dependence chains shorten. Helps the exposed-critical-path
cases (mispredict recovery, div/mul chains).

**What already exists:**
- `src/mir/cost.rs::latency(inst, src) -> u32` and `div_latency` — **MEASURED**
  (`MEASURED M10`, `tests/bench/latency.sh`). The latency table is done.
- the critical-recurrence machinery in `cost.rs` (the loop-carried bound) already
  reasons about latency-weighted chains.

**Change — the scheduler (`src/mir/pass/sched.rs`, new; POST-RA, pre-emit):**
1. per block, build the dependence DAG: RAW/WAR/WAW over registers **and memory**
   (use `MInst::effects()` — verify it exists/covers mem; if not, add a
   conservative `effects()` that says "any store/call orders all mem").
2. list-schedule: ready-list by **priority = longest latency-weighted path to a
   block exit** (compute once, bottom-up with `cost::latency`). Tie-break by
   keeping def→use close (register pressure neutral — post-RA, so no new spills).
3. **Post-RA means fixed registers** → the schedule may not violate the existing
   anti-dependences; the DAG's WAR/WAW edges enforce that. No reg changes.

**Reuse:** `cost::latency`. **Do NOT** invent latencies — anything unmeasured
uses the `cost.rs` fallback and is flagged.
**Proof:** `⟦m⟧ = ⟦Pm⟧` via dependence preservation — the schedule is a
topological order of the DAG, so `mir::interp` gives identical results; a machine
TV over the corpus (`md5` of behaviour, not bytes). On an OoO core the win is
modest except on exposed chains — **measure before growing it.**
**Toggle:** `ZCC_SCHED=1` (off → source order).

---

## R5.5 — VRP + branch-fold + udiv/shift narrowing  ★2 (Patterson 1995)

**Goal.** Propagate integer ranges; fold branches whose condition is
range-decided; narrow `udiv`/`%` by constants and shifts when the value provably
fits.

**What already exists:**
- `src/hir/pass/sccp.rs` — conditional constant propagation, **already a
  sparse-conditional lattice** with the CFG-reachability machinery VRP needs.
- `src/hir/pass/fold.rs` — the peephole/const-fold sink.

**Change — extend the SCCP lattice to ranges (`sccp.rs` or new `vrp.rs`):**
1. lattice element = interval `[lo, hi]` per integer value (signed + unsigned
   views), meet = interval union, widen at loop back-edges to `±∞` after N steps
   (Cousot widening — bounded, terminating).
2. transfer functions for `add/sub/mul/and/shift/cmp/zext/sext` on intervals.
3. **branch folding**: a `cmp` whose interval makes the result constant → replace
   the conditional branch with an unconditional one (feeds DCE, already present).
4. **narrowing**: when `x ∈ [0, 2^k-1]`, lower `x / c`/`x % c` and shifts to the
   narrow-width form; `udiv` by a constant → the multiply-shift sequence gated on
   the proven range.

**Reuse:** `sccp.rs` lattice skeleton, `fold.rs`, existing DCE.
**Proof:** `⟦f⟧ = ⟦Pf⟧` — a standard monotone-lattice pass; the obligation is that
every interval is a SOUND over-approximation (never narrower than reality),
checked by a battery of boundary values (`INT_MIN`, wraparound, unsigned edge).
**Toggle:** `ZCC_VRP=1`.

---

## Batch order & the gate you owe afterward

Implement in the order **R5.1 → R5.2 → R5.5 → R5.4 → R5.3** (cheapest wiring
first; SLP last because it adds the `V128` type and is the only real infra).
⚠️ "The batch ships **untested per your call**" is the OPEN DECISION in §0 and
has no merge criterion under Law 3 as written. Settle it before starting. The
recommended tiering, which is also the workflow already recorded from a previous
session:

- **per row:** `cargo test --release` + `tests/provenance.sh` + `localize.sh`,
  and — for any row that should not change output — `tests/refactor_gate.sh`
  (record `baseline` on the PRE-change binary via `git stash`, then `check`).
  About a minute total.
- **every 2–3 rows:** `FUZZ_N=1000 sh tests/fullsuite.sh all` (7+ min and
  growing — this is the cost that made the night feel expensive, and it is a
  scheduling problem, not a reason to drop the squares).
- **end of batch:** the 10k seal on a relaunched box, plus a paired
  INSN+EXEC geo40/realprog re-measure **per toggle** (A/B each `ZCC_*` on/off)
  to bank the per-row Δ and quarantine any that does not clear the noise floor.

Report the measurement as a distribution, never a single geomean (Law 3c): the
suite is ~35 kernels on one microarchitecture, 0.9× is the parity margin, and the
claim is always "matches gcc -O1 **on this suite, on this core**".
