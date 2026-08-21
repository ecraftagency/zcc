# zcc optimization roadmap — the theory-derived catalog `[ssa-qbe fork]`

Every entry is a compiler-theory *theorem* projected into an algorithm (Foundational Law Zero:
`theory → control-flow/algorithm`). Each carries: its **space/theorem**, its **proof obligation**
under CbC (`⟦f⟧=⟦pass(f)⟧` — a commuting square, or a machine-level translation-validation for
backend passes), and its **measured value on THIS backend**. The last column is the honest part:
a technique is only worth its LOC if *measurement* says so — several proven-correct passes here
are default-OFF because they don't pay on the current codegen (see LICM/SR).

**Where we are (commit `8418dcb`):** bench geomean **0.98× vs gcc-O0** — zcc now beats gcc-O0 on
average (fib 1.39×, loops 0.66×, matmul 1.68×, sieve 0.60×). The two remaining gaps above 1.0
name the next two levers: **matmul 1.68×** (address arithmetic + the x0-funnel residue) and
**fib 1.39×** (pure function-call overhead).

**Done:** const-fold · DCE · copy-prop · CSE · GVN · SCCP · CFG-simplify · register-coalescing
(biased) · LICM (off) · strength-reduction (off) · backend peephole (redundant + dead move elim).

---

## The measured-reality principle (read first)

The commuting-square proof establishes identical **output**, and says **nothing about cost**.
Performance is an orthogonal axis governed by the *backend*. On the current x0-accumulator emitter
the dominant cost is **register-move traffic**, not instruction count or memory — which is why:
- The backend peephole (deletes moves) was the biggest win: 1.39×→0.98×.
- LICM and strength-reduction (which *add* loop-carried values + copies) are measured-neutral/negative.

**Corollary for prioritization:** until instruction *selection* stops funnelling every value through
x0, IR-level passes that add live values will keep under-paying. The highest-leverage remaining work
is therefore **backend/instruction-selection**, then **inlining**, then the classical IR passes whose
payoff unlocks once values are register-resident.

---

## Tier 1 — highest measured leverage on THIS backend (do next)

| # | Technique | Theorem / space | Proof obligation | Attacks | ~LOC |
|---|-----------|-----------------|------------------|---------|------|
| 1 | **Compute-into-home instruction selection** (kill the x0 funnel at the source) | per-node simulation with a *target register* = the allocator's home, not a fixed accumulator | machine translation-validation (opt-parity 0 DIVERGE) + the existing `verify_abi` | everything (the funnel is global); matmul, fib, all | ~200–400 (emitter rework) |
| 2 | **Addressing-mode folding** `ldr xD,[base,idx,lsl #k]` / `[base,#imm]` | tree-pattern matching (BURS / maximal munch): `Load(Add(b, Shl(i,k)))` → one addressed load | local pattern equivalence (the folded form computes the same effective address) | matmul `A[i][k]`,`B[k][j]` — collapses ~4 insns/access | ~120 |
| 3 | **Multiply-add / multiply-sub fusion** `madd/msub xD,xA,xB,xC` | pattern `Add(Mul(a,b),c)` → `madd` | local equivalence | matmul inner product `s += a*b`; any `x*y+z` | ~60 |
| 4 | **Sign/zero-extension elimination** (the pervasive `sxtw x0,w0`) | a value already in canonical width need not be re-extended; range/def-width analysis | `⟦f⟧=⟦elim(f)⟧` (an extension of an already-canonical value is identity) | every loop counter / index (`sxtw` litters the output) | ~100 |
| 5 | **Function inlining** | call-graph substitution + β-reduction; a cost model bounds growth | `⟦call f(args)⟧ = ⟦inline body[args/params]⟧` (the meta-enabler; unlocks const-prop across calls) | **fib 1.39× (pure call overhead) — the only fix** | ~200 |

## Tier 2 — classical IR passes, payoff unlocks once values are register-resident

| # | Technique | Theorem / space | Proof obligation | Notes | ~LOC |
|---|-----------|-----------------|------------------|-------|------|
| 6 | **PRE / lazy code motion** (Knoop–Rüthing–Steffen) | 4 data-flow lattices — availability, anticipability, partial-availability, latest — the "commuting square of analyses"; **subsumes CSE + LICM** | `⟦f⟧=⟦pre(f)⟧` over SSA; hardest classical proof | the theoretical crown; one pass replaces two | ~300 |
| 7 | **Value-range propagation** (extend SCCP to intervals) | abstract interpretation over the interval lattice `[lo,hi]` | `⟦f⟧=⟦vrp(f)⟧` + soundness of the abstract transfer functions | enables bounds/sign simplification, branch folding | ~200 |
| 8 | **Reassociation + algebraic simplification** (instcombine) | term-rewriting over the ℤ/2ⁿ ring: `x+0,x*1,x*2→x+x,(a+b)+c→a+(b+c)` to expose CSE | each rewrite rule is a ring identity → `⟦·⟧`-invariant | partially subsumed by fold+GVN; reassoc exposes more | ~150 |
| 9 | **Redundant-load / dead-store elimination (memory GVN)** | memory SSA + alias analysis; a load after a store to a must-alias address reuses the value | `⟦f⟧=⟦mem(f)⟧` gated on the alias oracle | needs a real alias analysis (the missing infrastructure) | ~250 |
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
| 25 | **Full iterated register coalescing** (Briggs/George) | the interference graph stays k-colourable after a merge | beyond the current biased-colouring heuristic |
| 26 | **Rematerialization** | recompute a cheap value instead of spilling it | spill-heavy functions |
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
- **fib 1.39×** → Tier-1 #5 (**inlining**) is the *only* lever; no per-instruction pass touches
  call/return overhead. TCO (#17) does not apply (fib is not tail-recursive).
- **Global** → Tier-1 #1 (compute-into-home) removes the x0 funnel at its source rather than
  peepholing its symptoms — the single change with the widest blast radius, and the point at which
  the default-OFF IR passes (LICM, SR, PRE) would start to pay.

**CbC gate for every one of the above (non-negotiable):** ship with its commuting-square proof
(IR passes, via `equiv`) or its machine translation-validation (backend passes, via opt-parity
0 DIVERGE) + torture 0 FAIL, docs synced *before* commit, and the perf delta *measured* in the
box — never asserted.
