# zcc optimization roadmap — the theory-derived catalog `[ssa-qbe fork]`

Every entry is a compiler-theory *theorem* projected into an algorithm (Foundational Law Zero:
`theory → control-flow/algorithm`). Each carries: its **space/theorem**, its **proof obligation**
under CbC (`⟦f⟧=⟦pass(f)⟧` — a commuting square, or a machine-level translation-validation for
backend passes), and its **measured value on THIS backend**. The last column is the honest part:
a technique is only worth its LOC if *measurement* says so — several proven-correct passes here
are default-OFF because they don't pay on the current codegen (see LICM/SR).

**Where we are (Tier-1 #1 + #2 + #3 + #5 done):** bench geomean **0.69× vs gcc-O0** (fib **1.05×**,
loops 0.37×, matmul 1.22×, sieve 0.48×). Cumulative: #1 compute-into-home 0.98→0.81×, #2 addr-fold
0.81→0.78×, #3 madd 0.78→0.74×, **#5 inlining 0.74→0.69× (fib 1.38→1.05×)**. The matmul inner loop
is now essentially optimal (`ldrsw;ldrsw;madd`) — its residual 1.22× is **memory/cache** (needs #14
scalar-replacement or tiling, Tier-3), not per-instruction. The fib call-overhead gap is now closed;
the remaining Tier-1 item is **#4 sxtw-elim** (trims loop-counter extension litter).

**Done:** const-fold · DCE · copy-prop · CSE · GVN · SCCP · CFG-simplify · register-coalescing
(biased) · **LICM (pressure-guarded, off—box-pending)** · **strength-reduction (pressure-guarded,
off—box-pending)** · backend peephole (redundant + dead move elim) · **compute-into-home (#1)** ·
**addressing-mode fold (#2)** · **madd fusion (#3)** · **inlining (#5)** · **rematerialization (#26,
off—box-pending)**.

**Batch 2026-08-22 (guards + remat).** Answered *why proof-faster meets reality-slower* (the
`⟦·⟧`-vs-`C_M` category split — see "measured-reality" above) and converted the LICM/SR **speed** claim
from a gamble into a **theorem**: a decidable register-pressure guard (`#hoists ≤ k − P`, `k=GP_BUDGET.k`
threaded in) makes each ship-eligible hoist provably spill-free ⟹ `C_M` strictly decreases. Added
**rematerialization** (#26) — the CbC-pure slice of register allocation. All three proven `⟦·⟧`-preserving
on mac (`cargo` 100/100, incl. new `licm_pressure_guard_caps` · `strength_reduce_pressure_guard_caps` ·
`remat_recomputes_pressured_address` · `remat_refuses_operand_bearing_defs`) and kept **default-OFF**:
the SSA-vs-backend pressure residual is closed by the box A/B, not asserted. Default build output is
**byte-unchanged** (all three OFF; only an inert `gp_k` param threaded). **NEXT (box):** `ZCC_OPT_ON=licm,
strength,remat` bench A/B + torture + opt-parity → flip the winners default-ON.

---

## QBE cross-check — the 70%/10% target (source-verified against `ref/qbe/`, gitignored)

QBE's stated goal *is* ours: **70% of an advanced compiler's performance in 10% of the code.** Its
ENTIRE optimizer pipeline (`main.c`): `memopt → ssa → alias → loadopt → copy → fold → isel →
spill → rega → simpljmp`. The decisive finding: **QBE reaches its target with NO GVN, NO CSE, NO
LICM, NO PRE, NO strength-reduction, NO unrolling, NO vectorization** — every one of which zcc
ALREADY has. So on the mid-level IR, **zcc is already past QBE.** QBE's whole edge reduces to THREE
source-verified theorems zcc lacks, all in **memory + register allocation**, not the IR:

| QBE pass | theorem (the source it is compiled from) | file / LOC | zcc |
|---|---|---|---|
| `alias` (`fillalias`) | aliasing = a **4-point base lattice** {Con, Sym, Loc, Unk} + integer **offset intervals**. Two accesses (p,sp),(q,sq): **NoAlias** iff bases provably-distinct (different stack slots, or one a non-escaped local vs. an unknown) ∨ (same base ∧ offsets disjoint); **MustAlias** iff same base ∧ overlap. One RPO pass, **no points-to, no fixpoint**. Escape analysis falls out free (a stack slot never stored-through/passed opaquely is `ALoc` = provably local) | `alias.c` **167** | **MISSING** (roadmap #9 wrongly assumed "a real alias analysis" = missing infra; QBE: 167 LOC) |
| `loadopt` | a `Load` from `a` dominated by a `Store` to a **must-alias** address, with **no intervening may-alias store**, equals the stored value ⟹ forward it; a fully-redundant load reuses the earlier value. Gated by the `alias` oracle | `load.c` **457** | **MISSING** (roadmap #9) |
| `spill`+`rega` | **Hack's theorem:** an SSA program's interference graph is **chordal** (live ranges = dominator-tree subtrees) ⟹ once **max register-pressure ≤ k** everywhere, it is **k-colorable with NO backtracking** — a single dominance-order scan assigns colors, register *hints* bias coalescing. So allocation **decouples** into (1) `spill` = **Belady MIN × loop-depth weight** (`limit(b,k)`: keep the k nearest-use / highest-cost temps, spill the rest) then (2) `rega` = guaranteed one-pass coloring | `spill.c` **485** + `rega.c` **687** | **BEHIND** — zcc runs `out_of_ssa` BEFORE `abi_alloc`, discarding chordality; uses **Chaitin–Briggs** (iterated simplify/spill, NP-hard heuristic) with no Belady/loop-weighted spill |

**Consequence for prioritization:** the QBE-class gap is **not more IR passes.** Roadmap Tier-2
(#6 PRE, #7 VRP, #8 reassoc, #10 ADCE, #11 IV-elim) and Tier-3 loop work are **past the 10% budget** —
QBE hits 70% without any of them; defer until a real `.c` demands one. Of QBE's three edges, **two are
CbC-pure and enter the batch (alias + load-elim); the third (Belady spill + chordal alloc) sits below
the purity line** — see the admission test and the "Below the CbC-purity line" note below.

### Batch queue — CbC-PURE items only (each a total theorem → Rust at single-LOC granularity)

**Admission test (the hard filter):** an item enters the queue only if it is *completely* representable
by a theorem and compiles to Rust line-by-line — every LOC is a theorem-step or a Side-II spec constant,
with **zero tuned/empirical constants without provenance**. A transform gated by a *static* cost measure
(instruction count) still qualifies (Law 3); a transform riding a *tuned* weight does not. Reward is
judged on this arm64 backend at geomean 0.69× (fib 1.05 / loops 0.37 / matmul **1.22** / sieve 0.48).

| # | item | theorem (Side-I) — total & decidable | reward here | cplx | ~LOC | ratio |
|---|------|------------------|-------------|------|------|-------|
| **B1 ✅DONE** | **Lightweight alias analysis** (`alias.c` re-derived) | aliasing = 4-point base lattice {Con,Sym,Loc,Unk} + integer offset intervals; NoAlias/MustAlias/MayAlias is a **decidable relation**, one RPO pass, no fixpoint. Escape (`AEsc`) falls out free | **enabler** (unlocks B2 + B4; roadmap #9/#21) | LOW | ~120 | ★★★★★ |
| **B2 ✅DONE** | **Load-elim / store→load forwarding** (`load.c`) | a `Load` from `a` dominated by a `Store` to a **must-alias** `a` with no intervening may-alias store = the stored value ⟹ forward. `⟦f⟧=⟦load-elim(f)⟧` validated by `equiv` (interp's per-frame `mem` already models intra-function store/load) | attacks reload-heavy shapes (flat on the 4 kernels — no hot store→reload) | MED | ~200 | ★★★★☆ |
| **B3 ⏸DEFERRED** | **#4 sxtw / extension elimination** | an extension of an already-canonical-width value is the identity; width is a decidable static property. `⟦f⟧=⟦elim(f)⟧` | LOW here — the backend already ext's at def (width≥4) so redundant `sxtw` is rare; HIGH miscompile risk (the lazy width<4 canonicalization the pr81913 load-elim bug already tripped). Reward < LOC/complexity tax ⟹ deferred (same basis as its Tier-1 #4 deferral) | LOW | ~100 | ★★★★☆ |
| **B4 ✅DONE** | **`csel` if-conversion (#24) + `ldp`/`stp` pairing (#23)** | a side-effect-free / speculatable diamond `Br(c)?a:b` = `Select(c,a,b)` (a `⟦·⟧`-identity, gated on B1's `fault_free` for arm loads); two accesses to `[base,#o]`,`[base,#o+sz]` (o a spec-range multiple) = one pair op. Both gated by a **structural/static** test, no heuristic | MED — branchy code reaches PAR with gcc-O0 (2 csel, cond-branch 3→1); pairs the callee-save slab | LOW-MED | ~230 | ★★★☆☆ |

**BATCH RESULT (2026-08-22): B1 · B2 · B4 shipped, B3 deferred.** Gates all green with the full
stack default-ON: cargo **96/96** · torture **1378 pass / 0 FAIL** · opt-parity **1552 PARITY / 0
DIVERGE**. Bench geomean **0.69× vs gcc-O0**, unchanged from the B1 baseline — the 4 compute kernels
(fib 1.06 / loops 0.37 / matmul 1.20 / sieve 0.49) exercise neither store→load forwarding nor
if-convertible diamonds in their hot loops, so B2/B4 are flat *there* by construction (Law-3 predicted).
B4-csel's win was measured **in isolation on its target shape** (a two-ternary hot loop): 2 `csel`,
cond-branches 3→1, zcc reaches PAR with gcc-O0. New CORE IR inst `Select` (pure/total) carries the
if-conversion; `AliasInfo::fault_free` (B1) licenses arm-load speculation. Two box-only regressions
were caught by torture that `equiv` was blind to — the recurring Law-3 lesson (the eager-canon interp
model cannot see backend lazy-canonicalization nor out_of_ssa's φ-arm bookkeeping); both were Side-I
fixes (`fwd_canonical` gate for B2; convert-M's-φs-too for B4), never a test edit.

**Kept because zcc already EXCEEDS QBE (free wins past the 70% line):** GVN · CSE · LICM(off) ·
strength-reduction(off) · **inlining** — QBE has none of these.

### Below the CbC-purity line — admitted-NOT (heuristic mass, not single-LOC-theorem-traceable)

These are where QBE beats zcc but the technique is **not a total theorem** — so by the admission test
they do **not** enter the batch. The stop-line is exactly here: the point where CbC purity breaks *is*
the complexity tax spiking.

- **Belady loop-weighted spilling** (`spill.c`) — Belady MIN is optimal only on a straight line; the CFG
  lifting rides a **tuned loop-weight (~10^depth)** with no theorem provenance. Optimal spilling on a
  general CFG is not decidably a theorem — it is inherently a heuristic. **Rejected** (empirical constant).
- **SSA-chordal register allocation** (`rega.c`, Hack) — its *kernel* IS a clean theorem (SSA interference
  graph chordal ⟹ pressure≤k ⟹ k-colorable via a dominance-order scan — strictly **more** pure than zcc's
  current Chaitin), but it is inseparable from (a) a spiller = the rejected heuristic above and (b)
  coalescing register-*hints* (heuristic). **Rejected as a whole**; its colorability theorem is noted for
  the day a pure spiller exists. NB: **register allocation is the ONE place zcc already sits below this
  line** (Chaitin, NP-hard by necessity) — the standing exception; we do not grow it.

---

## The measured-reality principle — why proof-faster meets reality-slower (read first)

This is the deepest question the fork answers, and the whole reason LICM/SR ship guarded, not raw.

**The two axes are two different categories.** The commuting square `⟦f⟧ = ⟦opt(f)⟧` is a statement
in the category of **values** — the denotation `⟦·⟧` is a map (inputs → outputs), machine-independent,
decidable. It establishes identical *output* and says **nothing about cost**. Performance lives in a
*different* category: the operational **cost** `C_M : IR → ℝ⁺` of realizing `⟦·⟧` on a concrete machine
`M` with **finite registers** (k=10 GP here), a **memory hierarchy** (L1/L2/DRAM latencies), and a
**pipeline** (issue width, latencies, ports). `C_M` is **not** a homomorphism of `⟦·⟧`: two programs
with `⟦f⟧=⟦g⟧` routinely have `C_M(f) ≠ C_M(g)` — that is the entire point of optimization. But the
crucial fact is the converse: **the *sign* of `C_M(opt(f)) − C_M(f)` is NOT determined by the IR-level
transform.** It is decided by how the transform collides with `M`'s scarce resources. A proof about
`⟦·⟧` therefore cannot, even in principle, prove a speedup — it is silent on the axis that carries cost.

**Why the theory says "faster" and the machine says "slower" (the exact mechanism).** Take LICM. It
hoists an invariant `x=e` out of a loop of trip `N`.
- *In the theory's cost model* (`C_count` = dynamic instruction count, the model with **infinite
  registers**), it saves `(N−1)` executions of `e` ⟹ strictly faster. LICM is *optimal* in that world.
- *On `M`*, `x` must now be **live across the whole loop body**. If, at any point inside, the count of
  simultaneously-live values exceeds `k`, the allocator must **spill** — a store at the def and a
  **reload per use, `N` times**. LICM has then traded `(N−1)` register-resident 1-cycle ALU recomputes
  for `N` 4-cycle memory reloads. Net **`≈ N·3` cycles slower**. This is the precise 2026 regression.

So the gap is not sloppiness in the theory — it is that **LICM trades recomputation (a claim on ALU
bandwidth, which is plentiful) for live-range extension (a claim on registers, which are scarce =
k).** When registers are the binding constraint, the trade is a loss, and `⟦·⟧` cannot see it because
registers do not appear in `⟦·⟧`. Count ≠ cost; fewer *static* instructions can be *more* real cycles.

**Closing the gap without leaving CbC — the decidable pressure guard (shipped 2026-08-22).** The
honest fix is not "measure and hope" but to make speed-positivity a **theorem about a guarded
transform**. Define loop register-pressure `P = max over points p in the loop of |{GP temps live at
p}|` (computed from `liveness()` — measured, no tuned weight). Each hoist raises the live-count at any
point by **at most 1**, so post-hoist max pressure `≤ P + (#hoists)`. **Cap #hoists at `k − P`** ⟹
pressure stays `≤ k` ⟹ the k-colouring survives ⟹ the allocator introduces **no new spill** ⟹ each
hoist strictly deletes `(N−1)` dynamic ops with **zero** added memory traffic ⟹ `C_M` strictly
decreases. Now BOTH obligations are discharged *before* ship: output identity (`⟦f⟧=⟦licm_guarded f⟧`,
a subset of the proven hoist set — `equiv`, mac) **and** speed-positivity (the pressure theorem). SR
carries the same guard for its accumulator φ (`P + 2 ≤ k`). `k` is the ONE Side-II ABI constant
(`GP_BUDGET.k`), threaded from the backend so it is never duplicated.

**The residual gap (honesty about the guard itself).** `P` is **SSA**-pressure, measured *before*
`out_of_ssa`; the real allocator runs *after* it, and φ-destruction inserts edge copies that can bump
pressure at latches. So the guard is a *sound-ish proxy*, not an airtight identity with the backend's
pressure — the guard is itself a **model**, and a model has a residual gap with the machine (the very
phenomenon this section is about, one level up). That residual is exactly what a box A/B closes, which
is why the guarded passes stay **default-OFF pending the box measurement**, not flipped on faith. This
is Law 3 in its purest form: proven at the earliest decidable layer, *confirmed* — never discovered —
in the suite.

**Corollary for prioritization.** The dominant backend cost here is **register-move traffic + spills**,
not raw instruction count (the peephole that deletes x0-funnel moves was the biggest single win,
1.39×→0.98×). So the highest-leverage remaining work is the part of **register allocation** that is
CbC-pure: **rematerialization** (#26, this batch) — recompute a pure operand-free value at its use
instead of spilling it — plus the now-guarded classical loop passes whose payoff unlocks once values
are register-resident.

---

## Tier 1 — highest measured leverage on THIS backend (do next)

| # | Technique | Theorem / space | Proof obligation | Attacks | ~LOC |
|---|-----------|-----------------|------------------|---------|------|
| ~~1~~ | **Compute-into-home instruction selection** ✅ **DONE** — geomean 0.98×→0.81× | per-node simulation with a *target register* = the allocator's home, not a fixed accumulator | machine translation-validation (opt-parity **1552/0** DIVERGE) + torture **1378/0** | done for integer `Bin`/`Un`; Load/index arith remains → #2/#3 | ~90 (`ir_bin_r`/`ext_r`/`src_gp`) |
| ~~2~~ | **Addressing-mode folding** `ldr xD,[base,idx]` ✅ **DONE** — matmul 1.38×→1.25× | tree-pattern matching (BURS / maximal munch): `Load(Add(b, i))`, `i` single-use → one addressed load | machine translation-validation (opt-parity **1552/0**) + torture **1378/0** | matmul `A[i][k]`,`B[k][j]` — drops the index `add`. `lsl #k`/`[base,#imm]` forms still open | ~75 (`try_fuse_addr`/`load_idx`) |
| ~~3~~ | **Multiply-add fusion** `madd xD,xA,xB,xC` ✅ **DONE** — geomean 0.78×→0.74× | pattern `Add(Mul(a,b),c)`, mul single-use → `madd` | machine translation-validation (opt-parity **1552/0**) + torture **1378/0** | matmul inner product `s += a*b`; loops. `msub` still open | ~45 (`try_fuse_madd`) |
| 4 | **Sign/zero-extension elimination** (the pervasive `sxtw x0,w0`) | a value already in canonical width need not be re-extended; range/def-width analysis | `⟦f⟧=⟦elim(f)⟧` (an extension of an already-canonical value is identity) | every loop counter / index (`sxtw` litters the output) | ~100 |
| ~~5~~ | **Function inlining** ✅ **DONE** — geomean 0.74→0.69× (**fib 1.38→1.05×**) | call-graph β-reduction; depth-1 (self-recursion → 1-level unroll); callee frame APPENDED below the caller's (one offset law shared by interp + `lea_local`); cost model (leaf ≤16 / self ≤40 insts) bounds growth | commuting square `⟦f⟧=⟦inline f⟧` (`inline_leaf/void/self_recursion` — `equiv` recurses through residual `Call(Sym)`) + torture **1378/0** + bench. Restricted to **scalar-only params + scalar/void return** (aggregate byval/sret needs Memcpy the scalar store-seed can't model); **variadic/VLA callers excluded** (frame append would clobber their reg-save/VLA region) | fib call overhead closed; opens IPA-CP (#18) | ~220 (`inline`/`splice`/`inline_ok`) |

## Tier 2 — classical IR passes, payoff unlocks once values are register-resident

| # | Technique | Theorem / space | Proof obligation | Notes | ~LOC |
|---|-----------|-----------------|------------------|-------|------|
| 6 | **PRE / lazy code motion** (Knoop–Rüthing–Steffen) | 4 data-flow lattices — availability, anticipability, partial-availability, latest — the "commuting square of analyses"; **subsumes CSE + LICM** | `⟦f⟧=⟦pre(f)⟧` over SSA; hardest classical proof | the theoretical crown; one pass replaces two | ~300 |
| 7 | **Value-range propagation** (extend SCCP to intervals) | abstract interpretation over the interval lattice `[lo,hi]` | `⟦f⟧=⟦vrp(f)⟧` + soundness of the abstract transfer functions | enables bounds/sign simplification, branch folding | ~200 |
| 8 | **Reassociation + algebraic simplification** (instcombine) | term-rewriting over the ℤ/2ⁿ ring: `x+0,x*1,x*2→x+x,(a+b)+c→a+(b+c)` to expose CSE | each rewrite rule is a ring identity → `⟦·⟧`-invariant | partially subsumed by fold+GVN; reassoc exposes more | ~150 |
| 9 | **Redundant-load / dead-store elimination (memory GVN)** → **now B1+B2** | memory SSA + alias analysis; a load after a store to a must-alias address reuses the value | `⟦f⟧=⟦mem(f)⟧` gated on the alias oracle | ~~needs a real alias analysis (missing infra)~~ **CORRECTED by QBE cross-check: the alias oracle is 167 LOC, not missing infra** — see B1/B2 | ~120+200 |
| 10 | **ADCE** (aggressive DCE) | control-dependence via the post-dominator frontier; instructions dead unless they feed a live effect | `⟦f⟧=⟦adce(f)⟧` | marginal over mark-sweep DCE; needs post-dominators | ~120 |
| 11 | **Induction-variable elimination / LFTR** | replace the loop test on `i` by a test on a derived IV, then delete `i` | IV commuting square (complements strength-reduction) | pairs with SR; removes the basic IV entirely | ~120 |

## Tier 3 — loop restructuring (Rice-boundary: need trip-count / dependence reasoning)

| # | Technique | Theorem / space | Attacks | ~LOC |
|---|-----------|-----------------|---------|------|
| 12 | **Loop unrolling** | trip-count analysis; unroll factor amortizes branch + exposes ILP | branch overhead (fib/loops), scheduling | ~150 |
| 13 | **Loop unswitching** | hoist a loop-invariant branch out, duplicating the loop body | invariant conditionals | ~120 |
| 14 | **Scalar replacement of array elements** | dependence analysis: promote `a[i]` to a register across iterations | matmul (keep `s` and rows in registers) | ~200 |
| 15 | **Loop interchange / tiling / fusion (polyhedral)** | the polyhedral model — iteration space as a ℤ-polytope, dependences as Farkas constraints | matmul **cache locality** (the real -O3 win); large | ~600+ |
| 16 | **Auto-vectorization (SLP + loop)** | dependence + NEON SIMD lowering | matmul/sieve throughput; high ceiling | ~500+ |

## Tier 4 — interprocedural / whole-program

| # | Technique | Theorem / space | Notes |
|---|-----------|-----------------|-------|
| 17 | **Tail-call optimization** | activation-record reuse: a tail call becomes a jump | recursion depth; niche on the bench |
| 18 | **Interprocedural constant propagation (IPA-CP)** | call-graph data-flow | unlocked by, and amplifies, inlining |
| 19 | **Function specialization / cloning** | partial evaluation on a hot argument | |
| 20 | **Dead-function / dead-global elimination** | call-graph + reference reachability | link-time size |
| 21 | **Escape analysis → stack allocation** | points-to escape lattice | not yet relevant (no heap modelling) |

## Tier 5 — backend / machine (the rest of the codegen quality gap)

| # | Technique | Theorem / space | Notes |
|---|-----------|-----------------|-------|
| 22 | **Instruction scheduling** (list scheduling) | dependence DAG + critical-path heuristic | in-order pipelines; OoO cores gain less |
| 23 | **Load/store pair formation** (`ldp`/`stp`) | adjacent-access pattern match | halves memory-op count in prologues/copies |
| 24 | **Branch → conditional-select (`csel`)** | if-conversion of a diamond with no side effects | removes unpredictable branches |
| 25 | **Full iterated register coalescing** (Briggs/George) | the interference graph stays k-colourable after a merge | ~~beyond biased-colouring~~ **made MOOT by B6** — QBE's SSA-chordal `rega` gets coalescing from register *hints* over a guaranteed-colorable graph, no iterated rebuild |
| 21′ | **Escape analysis → stack allocation** (now FREE with B1) | a stack slot never stored-through/passed opaquely is provably local | QBE's `alias.c` computes `AEsc` as a by-product of the alias pass — B1 delivers escape analysis at zero extra cost (see Tier-4 #21) |
| 26 | **Rematerialization** ✅ **DONE (guarded, off—box-pending)** | recompute a pure OPERAND-FREE value (`Copy(Imm)` / `Lea` / `FunAddr` / `LabelAddr`) at its use instead of spilling it. Theorem `⟦f⟧=⟦remat f⟧`: an operand-free pure def is a function of nothing ⟹ its value is identical at every point ⟹ cloning it before a use is a `⟦·⟧`-identity (`equiv`, mac). Fires ONLY on a temp live at a point of GP pressure ≥ k (a spill victim) ⟹ trades {store + N reloads} for {N cheap recomputes} — the CbC-pure slice of the register-allocation gap the QBE cross-check named (Belady spill stays below the purity line) | spill-heavy functions; the pressured `&local`/constant |
| 27 | **Block layout / branch alignment** | order blocks so the hot path falls through | I-cache + branch prediction |

## Tier 6 — research / beyond -O2 (out of current scope)

E-graph equality saturation (egg-style; profitable rewrite search) · superoptimization
(exhaustive/stochastic instruction search) · profile-guided optimization (PGO — a real
profile replaces static heuristics) · speculative/value-prediction passes · polyhedral
auto-parallelization. Each is a legitimate theorem projection but far past the QBE-class target.

---

## Priority verdict (measured, not guessed)

To close the two remaining gaps toward QBE-class:
- **matmul 1.68×** → Tier-1 #2 (addressing-mode fold) + #3 (madd) + #1 (home-register selection)
  attack the inner-product address arithmetic directly. #14 (scalar replacement of `s`/rows) is the
  cache-level follow-up. Tiling (#15) is the -O3 ceiling but a large, dependence-analysis project.
- **fib 1.38× → 1.05× (CLOSED)** → Tier-1 #5 (**inlining**) was the *only* lever; no per-instruction
  pass touches call/return overhead. Depth-1 self-unroll halves the `bl`/prologue traffic. TCO (#17)
  does not apply (fib is not tail-recursive). Residual 1.05× is at par with gcc-O0.
- **Global** → Tier-1 #1 (compute-into-home) removes the x0 funnel at its source rather than
  peepholing its symptoms — the single change with the widest blast radius, and the point at which
  the default-OFF IR passes (LICM, SR, PRE) would start to pay.

**CbC gate for every one of the above (non-negotiable):** ship with its commuting-square proof
(IR passes, via `equiv`) or its machine translation-validation (backend passes, via opt-parity
0 DIVERGE) + torture 0 FAIL, docs synced *before* commit, and the perf delta *measured* in the
box — never asserted.
