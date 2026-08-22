# OPT.md — the single optimization working-doc `[ssa-qbe fork]`

> **One file. Transient.** This is the *only* optimization doc. When the fork's opt work is
> done it is **deleted** — the durable facts cook into `THEORY.md` (theorems/tables) and
> `SEMANTICS.md` (⟦·⟧). It replaces the three scattered files (`OPTIMIZATION-ROADMAP.md`,
> `OPT-THEORY-MAP.md`, `IR.md`) whose proliferation caused drift.
>
> **Decisions come from §1 (the scoreboard), never from §6 (the catalog).** The catalog is a
> reference shelf; the scoreboard is the one measured surface. If a technique is not on the
> path from §1's measured gap, it does not get built — no matter how good its textbook name.

---

## §1 — Scoreboard: the one number (measured, `zcc-box` docker, ELF aarch64-musl)

**The gap = inner-loop instruction count of hot kernels. Everything reduces to shrinking it.**

Solid, measured (bench geomean vs referee):

| kernel | vs gcc-**O0** | vs gcc-**O2** | where the O2 residual lives (measured category) |
|---|---|---|---|
| fib    | **1.05×** (par) | **3.44×** | call/return + O2 scheduling |
| loops  | 0.37× | **1.08× (≈par!)** | already O2-class |
| matmul | ~1.20× | **3.87×** | loop-nest memory + inner-loop address/index/move overhead |
| sieve  | 0.48× | **3.60×** | loop-nest memory + SIMD |
| **geomean** | **0.69×** | **2.68×** | — |

**Reading it:** vs O0 zcc is already ahead (0.69×). The real chase is the **2.68× vs O2**, and it
is **NOT classical scalar passes** (those are flat — see §3) — it is **loop-nest memory + the
backend's per-iteration overhead** (address recompute, index muls, x0-funnel moves).

**Leading hypothesis (NOT yet confirmed — the §4 diagnostic decides it):** matmul's inner k-loop
carries ~2 `adrp` + index `mul`s + x0-funnel `mov`s **on top of** the `ldrsw;ldrsw;madd` core,
where gcc-O2's inner loop is ~6 instructions. If those extras are hoistable/removable, backend
isel is a direct win; if the inner loop is genuinely pressure-bound, they are not. **Measure
before building** (§4).

---

## §2 — The one theorem: why proof-faster ≠ machine-faster (the reusable insight)

Two different categories. `⟦f⟧=⟦opt(f)⟧` lives in the category of **values** (inputs→outputs,
machine-independent) — it proves identical *output* and is **silent on cost**. Cost lives in a
different category: `C_M : IR → ℝ⁺`, the cost on machine `M` with **finite registers (k=10 GP)**,
a memory hierarchy, a pipeline. `C_M` is **not** a homomorphism of `⟦·⟧`, and crucially **the
*sign* of `C_M(opt(f)) − C_M(f)` is not decided by the IR rewrite** — it is decided by how the
rewrite collides with scarce registers. So a `⟦·⟧`-proof cannot, in principle, prove a speedup.

**Mechanism (LICM):** hoisting invariant `x=e` out of a trip-`N` loop saves `(N−1)` recomputes in
the infinite-register model, but forces `x` **live across the whole body**. If live-count exceeds
`k` at any point, the allocator **spills** → 1 store + `N` reloads → traded `(N−1)` 1-cycle ALU
recomputes for `N` 4-cycle reloads → **≈N·3 cycles slower**. `⟦·⟧` cannot see it because registers
are not in `⟦·⟧`. **Count ≠ cost.**

**The fix that stays inside CbC — the decidable pressure guard.** Let `P = max` GP-temps live at
any point in the loop (from `liveness()`, no tuned weight). Each hoist raises live-count by ≤1, so
cap `#hoists ≤ k − P` ⟹ pressure stays ≤ k ⟹ k-colouring survives ⟹ **no new spill** ⟹ each hoist
strictly deletes ops with zero added memory traffic ⟹ `C_M` strictly decreases. Speed-positivity
becomes a **theorem about a guarded transform**, not a gamble. `k` = the one Side-II ABI constant
`GP_BUDGET.k`, threaded from the backend.

**Residual honesty:** `P` is SSA-pressure, measured *before* `out_of_ssa`; φ-destruction inserts
edge copies that can bump real pressure. The guard is a sound-ish **proxy**, not airtight with the
backend — a model with its own residual (the very phenomenon, one level up). That residual is what
the box A/B closes → the guarded passes stay **default-OFF pending the box**, never flipped on
faith. Law 3 in its purest form: proven at the earliest decidable layer, *confirmed* in the box.

---

## §3 — Done ledger (one line each; measured effect; on/off)

Always-on IR: const-fold · DCE · copy-prop · CSE · GVN · SCCP · CFG-simplify · register-coalescing
(biased) · backend peephole (redundant + dead-move elim).

| pass | measured effect | state |
|---|---|---|
| #1 compute-into-home isel | geomean 0.98→0.81× (killed the x0-funnel at source) | ON |
| #2 addressing-mode fold | matmul 1.38→1.25× | ON |
| #3 madd fusion | geomean 0.78→0.74× | ON |
| #5 inlining (β-reduction, depth-1) | geomean 0.74→0.69×, **fib 1.38→1.05×** | ON |
| B1 lightweight alias (4-pt lattice, 1 RPO pass) | enabler for B2/B4; escape falls out free | ON |
| B2 load-elim / store→load forwarding | flat on the 4 kernels (no hot store→reload) | ON |
| B4 csel if-conversion + ldp/stp pairing | isolated win on branchy shape (csel×2, cond-branch 3→1, PAR w/ O0) | ON |
| LICM (pressure-guarded) | **flat** — kernels register-resident, nothing to relocate | **OFF** |
| strength-reduction (pressure-guarded) | **flat** — same reason | **OFF** |
| #26 rematerialization (operand-free pure defs) | **flat** — nothing spills to relieve | **OFF** |

**Gates green, full stack default-ON:** cargo **96/96** · torture **1378/0** · opt-parity **1552/0
PARITY** · csmith 300 = **254/0** (rest = skip). Default build output **byte-unchanged** by the
guarded-OFF passes (only an inert `gp_k` param threaded).

**Why the OFF three are not waste (the WIN-or-FOUNDATION gate):** each is proven `⟦·⟧`-preserving +
speed-safe and shipped OFF; their lasting value is the **pressure-guard infrastructure** (measured
`P`, `k−P` headroom) that any pressure-aware backend work reuses. That is the *foundation* leg of
the gate. As standalone wins they are flat — so **no further investment in IR scalar opt**.

---

## §4 — Next: the gate (WIN, or FOUNDATION for a big win — never flat-and-cutoff)

**The only remaining lever is backend instruction-selection** (x0-funnel moves + index muls +
inner-loop address recompute) — a **different category** from the flat IR scalar passes above, not
a continuation of them. It is the one thing on the path from §1's measured 2.68× gap.

**But it is unproven, and there is an open anomaly:** LICM ON does **not** hoist matmul's
invariant inner-loop `adrp` (verified: `adrp` still inside the `.Lir_mm_7` region with LICM ON).
Three undisambiguated causes: (a) pressure guard blocks it (inner-loop P≈10 ≥ k=10 → headroom 0);
(b) genuine high pressure makes the hoist unprofitable; (c) the `Lea` isn't in hoistable single-def
form. **This is exactly the gate:** run one cheap box diagnostic to decide *whether the inner-loop
overhead is removable at all* — if yes, backend isel is a confirmed WIN; if no (pressure-bound), it
is flat and must NOT be built. **Measure before writing a single pass.** Nothing enters the batch
that cannot show, on the exact target shape, that it deletes inner-loop instructions.

---

## §5 — QBE cross-check verdict (why the IR is already done)

QBE's stated goal is ours (70% perf, 10% code). Source-verified against `ref/qbe/`: QBE hits its
target with **no GVN, CSE, LICM, PRE, strength-reduction, unrolling, vectorization** — all of which
zcc already has. So **on the mid-level IR, zcc is already past QBE.** QBE's entire edge is three
theorems in **memory + register allocation**, not the IR:

- **alias** (`alias.c`, 167 LOC) → **B1 DONE** (4-point base lattice + offset intervals, 1 RPO pass).
- **loadopt** (`load.c`) → **B2 DONE** (store→load forwarding gated on the alias oracle).
- **spill+rega** (Hack chordal alloc + Belady spill) → **BELOW the CbC-purity line, NOT built:** the
  colorability kernel is a clean theorem, but it is inseparable from a **tuned loop-weight spiller**
  (empirical constant, no provenance) → fails the admission test. Register allocation is the one
  place zcc already sits below the purity line (Chaitin, NP-hard by necessity); we do not grow it.

**B3 (sxtw-elim) deferred:** low reward here (backend already ext's at def, width≥4) + high
miscompile risk (the lazy width<4 canonicalization that tripped the pr81913 load-elim bug). Reward <
complexity tax.

**Prioritization consequence:** more IR passes (Tier-2 PRE/VRP/reassoc/ADCE/IV-elim, Tier-3 loop
work) are **past the 10% budget** — QBE reaches 70% without any of them. Defer each until a real
`.c` demands it.

---

## §6 — Catalog (REFERENCE SHELF — not a plan; decisions come from §1)

Kept only so a future item has its theorem + proof-obligation on hand. Building any of these
requires a §1-measured gap pointing at it **and** the §4 gate (win or foundation, measured first).

**Tier-2 (classical IR, payoff needs values register-resident):** #6 PRE/lazy-code-motion
(Knoop–Rüthing–Steffen; subsumes CSE+LICM) · #7 value-range propagation (interval lattice) · #8
reassociation/instcombine (ℤ/2ⁿ ring identities) · #10 ADCE (post-dominator frontier) · #11
IV-elim/LFTR.

**Tier-3 (loop restructuring, Rice-boundary — trip-count/dependence):** #12 unrolling · #13
unswitching · #14 scalar-replacement of array elements · #15 interchange/tiling/fusion (polyhedral;
the real -O3 matmul cache win, large) · #16 auto-vectorization (SLP+loop NEON).

**Tier-4/5 (IPA / machine):** #17 TCO · #18 IPA-CP (amplifies inlining) · #20 dead-fn/global elim ·
#22 instruction scheduling · #23 ldp/stp (**done in B4**) · #24 csel (**done in B4**) · #27 block
layout/branch alignment.

Each carries the same CbC obligation: ship with the commuting square `⟦f⟧=⟦pass f⟧` (IR passes, via
`equiv`) **or** machine translation-validation (backend passes, via opt-parity 0 DIVERGE), + torture
0 FAIL, + the perf delta measured in the box — never asserted.

---

## §7 — IR contract + opt.rs↔theory audit (compact; the durable bits cook into THEORY/SEMANTICS)

**IR shape (settled, shipped, SSA):** typed linear 3-address, explicit CFG (blocks + one terminator
each), explicit memory (Load/Store, no implicit lvalues). **CORE vs OPAQUE split:** passes touch
only CORE (Bin/Un/Load/Store/Lea/Cast/Call/Copy/Select + terminators — interp-evaluable,
verifier-covered); the exotic tail (atomics, Overflow, Asm, Va*, Alloca/VLA, SRet, nested-fn tramp,
TLS) is wrapped OPAQUE and lowered 1-to-1, untouched by passes. *(NB: the old `IR.md` "NON-SSA
settled" is superseded — the fork went SSA via Braun mem2reg + Cytron out-of-ssa.)*

**The 3-artifact contract (bug-resistance of the standard):** (1) **verifier** — well-formedness
automaton run after each pass (typed, def-before-use, one terminator/block, no dangling refs); (2)
**interp** — reference evaluator = semantic ground truth (`SEMANTICS.md` LEVEL-1, state Σ=⟨ρ,μ⟩);
(3) **commuting square** — every pass must commute with interp, lifted to an **executable theorem**:
`commuting_square_structural_exhaustion` (312 exprs × 5 passes = 1560 squares) + `_selfproof`
(anti-blindness). A pass may be written only when all 4 parts are stated: input invariant · rewrite
rule · preservation theorem · output invariant. UB filter is a root rule.

**opt.rs audit verdict (Law Zero, reproducible by `grep`):** every non-index numeric literal is
discharged to a spec table (`TyTab` LP64 · ARM ARM x0–x30 · AAPCS64) or a value-numbering
injectivity tag; the one flagged construct `for _ in 0..32` (fixpoint cap) is discharged by the
**correctness-invariance theorem** (composition of ⟦·⟧-preserving passes is ⟦·⟧-preserving for any
iteration count ⟹ the cap affects only *how-optimized*, never `⟦f⟧`). Every function maps to a named
theorem (Braun SSA · Cytron out-of-ssa · Wegman–Zadeck SCCP · Alpern–Wegman–Zadeck GVN ·
Cocke–Kennedy CSE · Chaitin–Briggs regalloc · Aho dominance/loops · Allen–Cocke–Kennedy SR). The two
correctness-bearing operator families (`wrapping_*` = the ℤ/2ⁿ ring; `dom`/`degree<k`/`defcnt==1`
predicates) are exactly the operators their theorems require. **No line lies outside {theory ∪
spec}.** A future edit adding a constant/operator updates this section (then, at opt-end, THEORY.md).
