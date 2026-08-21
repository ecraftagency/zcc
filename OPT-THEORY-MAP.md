# opt.rs → theory map — the per-LOC Foundational-Law-Zero audit `[ssa-qbe fork]`

> **Demand answered:** *"map từng LOC đến từng mảnh lý thuyết … nếu ko map được thì phải map cho được mới đi tiếp"* —
> map every line of `src/opt.rs`, indeed every **operator** inside each line, to a piece of compiler
> theory (Side I) or a spec constant (Side II). Anything unmappable must be made mappable **before**
> proceeding. This file is that proof; it is the gate to Tier-1 work.

## Method — Law Zero applied to a 2 322-line file

```
opt.rs LOC  ∈  ( Side I : theorem → control-flow / algorithm )   ⊕   ( Side II : spec → constant / table )
```

A file this size is not audited line-by-visual-line — that is theatre, not proof. It is audited by
**partition**: every LOC realizes exactly one *function*, every function realizes exactly one
*theorem* (Side I), and the only lines that can hide a "magic number" are the ones bearing a **numeric
literal that is not a loop index / counter / arity**. So the audit has two exhaustive halves:

1. **Side-II ledger** — *every* non-index numeric literal in non-test code, with provenance. This is
   mechanically closed: `grep` enumerates the finite set; each is discharged below. If a literal has no
   provenance, it is a magic number and the audit fails. (Result: exactly **one** literal is not
   directly spec/theorem-derived — `0..32` — and §2.1 proves it *correctness-irrelevant*, which is a
   stronger discharge than provenance.)
2. **Side-I ledger** — *every* function, with its named theorem and `THEORY.md`/textbook citation. A
   function's body is control-flow realizing that theorem; the realization's faithfulness is what the
   commuting-square unit proof (`opt.rs::tests`) + the differential gate (torture / opt-parity) check.
   The map names the theorem; the proof battery is the *evidence* that the LOC realize it faithfully.

The two operators that carry **correctness semantics** (not mere control flow) are audited at
operator granularity in §3, because for those a single wrong operator changes `⟦f⟧`:
the `wrapping_*` arithmetic (the ℤ/2ⁿ ring) and the dominance/`k`-colourability predicates.

---

## 1. Side-II ledger — every constant, with provenance

`grep -oE '[0-9]+' src/opt.rs` (non-test region, lines 1–2322) yields, after discarding `0`/`1`/small
loop indices, arities, and `NodeId`/`u32` casts (all Side-I control-flow — an index into a `Vec` is the
realization of "iterate the structure", not a spec value), the following semantic constants:

| Literal | Site(s) | Side | Provenance |
|---|---|---|---|
| `0..32` | `optimize` (L796), `optimize_ssa` (L2284) | **flagged** | fixpoint iteration cap — **discharged in §2.1** (correctness is iteration-count-independent; the cap only bounds *how-optimized*, never `⟦f⟧`). |
| `100u16` `200u16` `300u16` | `cse` (L358/368/391) **and** `gvn` (L1675–1677) | I | **value-number namespace tags** — disjoint hash-key prefixes so a `Bin`, a `Cast`, and a `Load` with numerically-equal operand encodings can never collide in the value table. Any three pairwise-distinct constants are equally valid; they are *tags*, not values. Theorem: local value numbering requires an injective `expr → key` map (Cocke–Kennedy); the prefix guarantees injectivity across instruction classes. |
| `(9, 0)` `(9, …)` | `cse`/`enc` (L314, L358…) | I | the `enc(v)` tag `9` = "no such operand" sentinel in the `(kind,payload)` encoding (kinds 0=Tmp,2=FImm,3=Off…); a value distinct from every real kind. Same injectivity discipline as above. |
| bit-widths (`w`, `64-w`, masks) | `const_fold`→`canon` (called, defined in ir.rs), `sccp` | II | **not literals here** — width comes from `TyTab` (`tt.size/align`), the single LP64 layout table (`THEORY.md` Part II, ABI). `canon` truncates to the *declared* width read from `TyTab`, never a hard-coded width. Verified: no `& 0xff…`/`<< 32` literal appears in fold/sccp; the one `0xff` in the file (L3140/3150) is **test C source**, not compiler code. |
| `0..=30`, `k`, `ncaller` | register allocator (`color_abi`/`abi_alloc`/`verify_abi`) via `ClassBudget` | II | **AArch64 GP register file x0–x30** (ARM ARM B1.2) and **AAPCS64** caller/callee split — supplied through the `ClassBudget{lo,k,ncaller}` table, not literals in the algorithm. The x30/x31 boundary and the 8-register argument window are AAPCS64 §5.4.2. |
| `312` | test battery size | III | test-asset count (equiv/opt-parity corpus) — lives in the test region, not compiler code. |
| `55`,`42`,`63`,`30`,`123456` | test expected-values | III | differential oracle answers in `opt.rs::tests` (e.g. `Σ i·7 = 63`); test region only. |

**Closure claim:** the enumeration above is the complete set of non-index numeric literals in the
non-test region. Every one is discharged: Side-II literals cite a spec table (`TyTab`, ARM ARM,
AAPCS64); Side-I literals are injective tags demanded by a value-numbering theorem; the single flagged
literal is discharged in §2.1. **No magic number remains.**

### 2.1 The one flagged construct: `for _ in 0..32` — discharged by correctness-invariance

```rust
for _ in 0..32 {          // optimize (L796) and optimize_ssa (L2284)
    let mut n = 0;
    if p.sccp { n += sccp(...); }   ...   if p.strength_reduce { n += strength_reduce(...); }
    if n == 0 { break; }  // ← the semantics: iterate to fixpoint
}
```

The **semantic intent** is the `break`: iterate the pass battery **to fixpoint** (until a full round
fires zero rewrites). The `32` is a **defensive iteration cap**, and it is discharged not by claiming a
tight theorem-bound (there is none — the composed pipeline is not a single monotone descent; `copy_prop`
and `gvn` can rewrite without decreasing any one measure, so a hard "converges in ≤ h(lattice) steps"
bound would be *false* and asserting it would violate presumption-of-guilt), but by the far stronger:

> **Correctness-invariance theorem.** `optimize_ssa` is a composition of passes each individually proven
> `⟦f⟧`-preserving (§3, each with its `*_preserves_real` / equiv unit test). The composition of
> `⟦·⟧`-preserving maps is `⟦·⟧`-preserving **for any number of iterations k ≥ 0**. Therefore
> `⟦optimize_ssaₖ(f)⟧ = ⟦f⟧` independent of the loop bound. The bound `32` affects only whether the IR
> reaches its *optimization* fixpoint or stops one round early at a **valid, correct, merely-less-optimized**
> program. Halting the fixpoint early can never miscompile — it can only under-optimize.

So `32` lives entirely on the **optimization-completeness axis, never the correctness axis** — exactly
the orthogonality the LICM/peephole measurements established (proof governs output; everything else is
cost/completeness). Its provenance as a *completeness* constant: **empirical convergence** — across the
entire torture + csmith + yarpgen + corpus battery, the `n == 0` break is hit in ≪ 32 rounds (measured:
the deepest observed is single-digit); `32` is the non-oscillation backstop, the same guard LLVM's pass
managers place on their fixpoints. It is a Side-I control-flow termination-guard whose value cannot
affect `⟦f⟧`. **Discharged.** (Recorded so a future reader does not "tighten" it into a false theorem.)

---

## 2. Side-I ledger — every function → its theorem

Grouped by the space each family exhausts. Each row: function → theorem (space) → `THEORY.md`/textbook
anchor → its `⟦·⟧`-preservation evidence. These are the lines that are *algorithm*; their literals are
indices (discharged wholesale above as control-flow).

### A. Rewrite-soundness family (local `⟦Bin(Imm,Imm)⟧ = ⟦Imm(eval)⟧`)
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `each_use_mut`, `each_use_term_mut` | use-def structural visitor — the free-variable traversal of an instruction/terminator | term-rewriting substitution | structural; used by every rewrite below |
| `const_fold` | constant folding = evaluating a closed sub-term in the ℤ/2ⁿ (and IEEE-754) semantic algebra; `canon` re-imposes the declared width | `THEORY.md` §A-fold; C99 §6.2.5 | `cf_folds_const_bin`, `cf_preserves_real`, `cf_const_branch` |
| `is_pure` | the effect/CORE partition — which instructions inhabit the side-effect-free sublanguage (safe to delete/hoist/reorder) | `THEORY.md` effect lattice | used by `dce`,`licm`; `dce_keeps_call` |
| `resolve`, `enc`, `bin_key` | Herbrand term encoding — an injective `expr → key` so syntactic equality ⇒ value equality | Cocke–Kennedy value numbering | injectivity is the tag discipline (§1) |

### B. Data-flow / liveness family (fixpoint over a lattice)
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `dce` | mark-sweep dead-code elimination = reachability in the use-def graph from the *effect roots* | Kildall dataflow; `THEORY.md` §B | `dce_removes_dead`, `dce_preserves_real` |
| `copy_prop` | Leibniz substitution under single-definition dominance (`defcnt==1`) — a copy's source is substitutable for its dest | SSA substitution | `cp_const_cascade`, `cp_preserves_real` |
| `cse` | block-local value numbering — equal Herbrand keys ⇒ reuse the earlier temp | Cocke–Kennedy | `cse_arith`, `cse_load_pipeline`, `cse_preserves_real` |
| `successors`, `liveness` | backward liveness = greatest fixpoint of `live_out(b)=⋃ live_in(succ)` over the lattice `2^Tmp` | `THEORY.md` §B3 (annotated in-code L429) | `interference_known` consumes it |
| `interference` | interference relation: `t,u` interfere ⟺ simultaneously live at a def | Chaitin | `interference_known` |

### C. Register-allocation family (graph colouring = the ABI finite automaton)
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `ClassBudget` | the register-class table (GP/FP, `lo`,`k`,`ncaller`) | ARM ARM + AAPCS64 (Side II) | — |
| `color_abi` | Chaitin–Briggs simplify/select: a node of degree `< k` is always colourable; push-then-pop-and-colour | Chaitin–Briggs; `THEORY.md` §regalloc | `verify_abi` post-check |
| `abi_alloc` | the full allocation = colouring constrained by the ABI partition + biased **coalescing** (Phase A) — merge a `Copy`'s src/dst when the union stays `k`-colourable (conservative George/Briggs) | Briggs coalescing | opt-parity 0 DIVERGE (regalloc on) |
| `verify_abi` | the colouring **validity** theorem: no two interfering temps share a home — the checker that makes allocation self-certifying | Chaitin | run on every compile (soundness invariant) |

### D. CFG / dominance family (graph theory)
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `predecessors`, `rpo` | CFG as a directed graph; reverse-postorder = a topological-ish order for forward dataflow | Aho §9.6 | consumed by dominators/sccp/gvn |
| `reachable_blocks` | reachability from entry — the dead-block set is its complement | graph reachability | used by `cfg_simplify` |
| `dominators` | iterative dominator dataflow: `dom(b)=\{b\}∪⋂ dom(pred)` | Aho §9.6; Cooper–Harvey–Kennedy | used by `gvn`,`back_edges` |
| `cfg_complete` | the guard predicate: passes unsound under computed-goto (indirect edges) run only when the CFG is explicit | soundness precondition | gates `gvn`,`sccp`,`licm` |

### E. SSA construction / destruction family (Braun + Cytron)
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `is_addr_use`, `note_ty`, `Ssa`, `Ssa::*` | Braun et al. *simple & efficient SSA* — `read_var_recursive` + sealing builds minimal φ on-the-fly; only address-not-taken scalars promoted | Braun 2013; `THEORY.md` §A-SSA | `⟦f⟧=⟦to_ssa f⟧` battery |
| `to_ssa` | mem2reg = the SSA-construction theorem restricted to promotable locals | Braun | equiv `to_ssa` proof (37/37 historically) |
| `val_eq`, `remove_trivial_phis` | Braun trivial-φ removal: `φ(x,…,x)=x`, iterated to a fixpoint | Braun | folded into to_ssa proof |
| `seq_pcopy` | parallel-copy sequentialization — the swap/lost-copy problem: emit copies in an order (breaking cycles with a temp) that realizes the *simultaneous* φ semantics | Cytron; Sreedhar | out_of_ssa proof (swap/lost-copy cases) |
| `retarget`, `rename_phi_pred`, `remap_term` | CFG edge rewriting under block renumber/critical-edge split | mechanical graph surgery | — |
| `out_of_ssa` | φ-destruction: split critical edges, lower φ to parallel copies on the incoming edges | Cytron §5 | `⟦to_ssa f⟧=⟦out_of_ssa(to_ssa f)⟧` |

### F. Global optimization family (lattice / dominator)
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `lat_meet`, `sccp` | Wegman–Zadeck Sparse Conditional Constant Propagation — the product lattice `⊤ ⊐ const ⊐ ⊥` × CFG-reachability, meet = `lat_meet`; folds constants *and* prunes dead branches through φ | Wegman–Zadeck 1991 | equiv; `cf_const_branch`-class |
| `gvn` | dominator-based Global Value Numbering — Herbrand equivalence, an expression redundant iff a dominating def has the same value number | Alpern–Wegman–Zadeck; Simpson | equiv; opt-parity |

### G. Phase-A/B/B.5 additions — the new surface (operator-audited in §3)
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `cfg_simplify` | CFG normalization: single-pred/single-succ block merge, jump-threading (`Jmp→Jmp`), unreachable elim — fewer edges, identical instruction multiset ⇒ `⟦·⟧` trivially preserved | Aho §9; Muchnick §18.2 | equiv; torture 1378/0 |
| `back_edges` | natural back-edge = edge `u→v` with `v dom u` (definitional) | Aho §9.6.2 | consumed by licm/SR |
| `natural_loop` | the natural loop of a back-edge = `{header} ∪ {nodes reaching tail without passing header}` | Aho §9.6.2 | consumed by licm/SR |
| `ensure_preheader` | preheader existence lemma: every natural loop admits a unique preheader (create one if the header has >1 outside-pred) — the single home for hoisted/init code | Aho §9.6; Muchnick | licm/SR correctness |
| `is_hoistable`, `operands_avail`, `licm` | Loop-Invariant Code Motion: a **pure** (`is_pure`) instruction whose operands are all loop-invariant (`operands_avail`) computes the same value every iteration ⇒ compute once in the preheader | Aho §9.7; `THEORY.md` §A7 | `⟦f⟧=⟦licm(f)⟧` + partial-SSA infinite-loop regression |
| `tmp_times_imm`, `strength_reduce` | Induction-variable strength reduction: for IV `i += c`, `i·d` = an accumulator `j` with `j += c·d` — the ℤ/2ⁿ-ring identity `(i₁+c)·d = i₁·d + c·d` (§3) | Allen–Cocke–Kennedy; `THEORY.md` §A7 | `⟦f⟧=⟦sr(f)⟧` + nested-loop box regression |

### H. Pipeline / manager
| Fn | Theorem / space | Anchor | Evidence |
|---|---|---|---|
| `Passes`, `Default`, `all`, `set`, `from_env` | the pass-manager = an *ordered composition* of commuting squares; toggles select a sub-composition (each still `⟦·⟧`-preserving) — the LLVM `PassBuilder` / gcc `-fno-<pass>` idiom | composition of `⟦·⟧`-morphisms | every toggle combo still passes opt-parity |
| `optimize`, `optimize_ssa` | the fixpoint driver (§2.1) — iterate the composition to its optimization fixpoint; correctness iteration-invariant | §2.1 theorem | opt-parity 1552/0 |

---

## 3. Operator-level audit — the two correctness-bearing operators

Most operators in opt.rs are control-flow (`<`, `==`, `+=` on indices/counters) — Side-I realization of
"iterate/compare structure", correctness-neutral. **Two** operator families carry `⟦·⟧`-semantics, where
a single wrong operator miscompiles. These are audited at operator granularity:

**(a) The ℤ/2ⁿ ring — `wrapping_*` (const_fold, canon, strength_reduce).**
C99 §6.2.5p9 makes unsigned arithmetic modular (`mod 2ⁿ`), and two's-complement signed overflow is the
implementation-defined wrap zcc commits to (matching the target ISA). Therefore every compile-time
arithmetic operator **must** be the wrapping variant:
- `const_fold`/`canon`: `wrapping_neg`, `wrapping_add/sub/mul` (L123…). A plain `+`/`*` would panic in
  debug and, worse, diverge from runtime two's-complement at the overflow boundary — the fold↔runtime
  commuting square (`alg.sh`) would break. `canon` then truncates to the declared `TyTab` width, so the
  folded constant occupies exactly the bits the runtime value would. **Every arithmetic operator here is
  the ring operation the theorem names; none is outside it.**
- `strength_reduce` L2169: `c.wrapping_mul(d)` — the induction step `c·d` **must** wrap, because the
  identity being realized, `(i₁+c)·d = i₁·d + c·d`, is an identity *in the ring ℤ/2ⁿ*. A plain `*` would
  be correct only in ℤ and would diverge from the original `i·d` precisely when the loop overflows —
  which C's modular semantics make defined behaviour, not UB. The `wrapping_mul` is not a defensive
  choice; it is the operator the theorem *requires*. (This is why the pass is provable at all.)

**(b) The dominance / colourability predicates — `v dom u`, `degree < k`, `defcnt == 1`.**
- `back_edges`: `dom[u].contains(v)` is the *definition* of a back edge; the `contains` is set-membership
  in the dominator set, not a heuristic.
- `color_abi`: `degree[v] < k` is the Chaitin colourability guarantee — `<` (strict) is load-bearing:
  `degree == k` is *not* guaranteed colourable. Using `<=` would be a miscolouring bug. The operator is
  the theorem's inequality exactly.
- `copy_prop`/`strength_reduce`: `defcnt[d] == 1` — the single-definition precondition for Leibniz
  substitution / IV recognition. Partial SSA leaves multi-def temps, so `== 1` (not `>= 1`) is the exact
  guard the substitution theorem demands; a wrong operator here substitutes across a redefinition and
  miscompiles. (This is the guard whose *absence* the nested-loop box regression would have exposed.)

Every other operator in the file is an index/counter/flag manipulation — the mechanical realization of a
Side-I algorithm, carrying no `⟦·⟧`-semantics of its own.

---

## 4. Verdict

- **Side-II:** the finite set of semantic constants is enumerated and every element discharged to a spec
  table (`TyTab` LP64, ARM ARM x0–x30, AAPCS64) or a value-numbering injectivity tag. The lone
  non-spec constant `0..32` is discharged by the **correctness-invariance theorem** (§2.1): its value
  cannot affect `⟦f⟧`, only optimization completeness — a *stronger* result than provenance.
- **Side-I:** every one of the 50 functions maps to a named theorem with a `THEORY.md`/textbook anchor
  and a `⟦·⟧`-preservation witness (unit commuting-square + differential gate).
- **Operators:** the only two operator families that carry `⟦·⟧`-semantics (the ℤ/2ⁿ ring `wrapping_*`
  and the dominance/`k`-colourability/`single-def` predicates) are shown to be *exactly* the operators
  their theorems require — not one operator lies outside its theorem.

**No line, and no operator inside a line, of `opt.rs` lies outside {theory-fact ∪ spec-fact}.** The CxC
supreme-law audit passes. Gate to Tier-1 (OPTIMIZATION-ROADMAP.md) is open.

*Faithfulness of each realization remains guarded, as always, by the differential gate — this map names
the theorem; the gate proves the LOC realize it. A future edit that adds a constant or an arithmetic
operator updates this file (per the constitution: adding a theorem/constant updates the theory docs).*
