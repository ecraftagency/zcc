# THEORY.md — the theoretical foundations of zcc

> This document realizes the **MATHEMATIC FOUNDATION** principle of the project
> charter. It is the **complete, exhaustive catalog**: the answer to the question
> "on what theoretical basis does zcc rest" is this file. It is updated whenever a
> new theorem, constant, or table is added.

---

## §0. ROOT PRINCIPLE — the source-code decomposition theorem (if only one rule is kept, keep this one)

```
zcc source  =  ( math / theory     → control-flow + data-structure + algorithm )
            ⊕  ( iso / os / arch / gcc spec → constant + param + value-table )
```

**Every line of `src/` belongs to EXACTLY one of the two sides. There is no third side.**

- **Side I (theory → the METHOD of computing):** control flow, data structures,
  algorithms — each derived from a theorem or mathematical structure. If a fragment
  of code cannot be mapped back to any theorem in Part I, the architecture is suspect.
- **Side II (spec → the VALUE):** constants, parameters, and lookup tables copied
  from a normative document (ISO C99 / AAPCS64 / System V / ELF / AArch64 ARM ARM /
  GNU). No "magic number without provenance": every constant must be traceable to a
  line of specification.

Verification corollary: `grep 'EXT(' src/` covers 100% of the nonconforming surface
(Side II, the gcc/apple branches); every layout/ABI constant lives in TyTab plus the
target file (Side II); everything else is Side I.

---

## §0b. WHAT CORRECTNESS IS — real-software coverage is CHEAP evidence

- A compiler of **10–15k LOC** can compile **250+ real programs** with ease, because
  conservative C uses only a **narrow, shared subset** of the language. Covering many
  projects demonstrates **usability**, NOT **correctness**.
- Conversely, **dozens of compilers of the same size still FAIL csmith/yarpgen** —
  random differential torture probes exactly the semantic corners that real software
  never reaches (evaluation order, UAC boundaries, bitfield packing, sign/overflow,
  aliasing, rare ABI cases).
- Hence the **correctness-evidence ladder (weak → strong):**
  `compiles-an-app  <  runs-an-app-correctly  <  differential-vs-oracle over a corpus  <  structural-exhaustion (sci-gate)  <  random-differential (csmith/yarpgen)  <  IR-equivalence-by-theorem`.
- Hence the reason the **sci-gate tier** exists (structural exhaustion, ground truth)
  and the reason for the **IR→IR_ops proven-by-theorem** direction: they catch defects
  that 250 applications never expose. The application stack is PRACTICAL corroboration
  (lower tier); the theorems are ground truth (upper tier).

---

# PART I — THEORY → CONTROL-FLOW / DATA-STRUCTURE / ALGORITHM

> Side I of §0: *how zcc computes*. Indexed along four axes: **A** pipeline phase ·
> **B** pure mathematics · **C** computability/complexity · **D** sci-gate. Status:
> **[IN USE]** implemented and gated · **[PLANNED]** IR/opt tier · **[FOUNDATION]**
> implicit (every decision rests on it).

## A — BY PIPELINE PHASE

### A1. Lexing `[IN USE]`
| concept/theorem | description | zcc |
|---|---|---|
| Regular language | a token is a regular language | `lexer.rs`, `gate shape` |
| Finite automaton (DFA/NFA), Kleene | finite-state machine; RE↔DFA | `lexer.rs` |
| Maximal munch / longest-match | longest token (`>>`, `->`) | `lexer.rs` |
| Chomsky hierarchy (Type-3 ⊂ Type-2) | regular tokens ⊂ CFG parser | `lexer.rs`↔`parser.rs` |
| Translation phases (8 phases, 5.1.1.2) | line splicing `\`, comments, tokens, macros | `lexer.rs`+`preprocess.rs` |

### A2. Preprocessing `[IN USE]`
| concept/theorem | description | zcc |
|---|---|---|
| Term rewriting system (TRS) | macro expansion = term rewriting → normal form | `preprocess.rs`, `gate cpp` |
| Confluence (Church–Rosser) | the expansion result is deterministic | `preprocess.rs` |
| Termination / well-foundedness | expansion must terminate | `preprocess.rs` |
| Hideset / blue paint | prevents recursive macro expansion | `preprocess.rs` |
| Constant-expression evaluation (#if) | evaluates integer constants (sub-grammar + interpreter) | `gate cpp` |

### A3. Parsing `[IN USE]`
| concept/theorem | description | zcc |
|---|---|---|
| Context-free grammar (Type-2) | the C grammar is a CFG | `parser.rs`, `gate shape` |
| Recursive descent (LL, top-down) | recursion descending by production | `parser.rs` |
| Precedence climbing / Pratt | operator-precedence climbing for binary operators | `parser.rs` (`mkbin`+bp) |
| Lexer hack (typedef feedback) | `T*x` declaration vs. multiplication requires a typedef table | `parser.rs` (`is_type_word`) |
| Dangling-else resolution | `else` binds to the nearest `if` | `parser.rs` |
| Inductive datatype / term algebra | AST = arena + `NodeId(u32)`, no `Box` | `ast.rs` |

### A4. Type system & static semantics `[IN USE]`
| concept/theorem | description | zcc |
|---|---|---|
| Type-derivation lattice | pointer/array/function derivation; array→pointer decay | `gate decay` |
| UAC = join-semilattice | least upper bound over rank | `parser.rs` (`common_ty`), `gate alg` |
| Integer promotion / rank order (6.3.1.1) | ordering over rank | `parser.rs` (`promote`) |
| Typing judgment Γ⊢e:τ | type environment + scope/shadowing | `parser.rs` (`locals`, `typedefs`) |
| Record-layout automaton | struct/union/bitfield = a stateful cursor | `gate shape` |
| Constant folding = partial evaluation | evaluate constants at translation time | `parser.rs` (`fold`), `gate alg` |
| Commuting square fold↔runtime | fold(e)=run(e) | `gate alg` |

### A5. Codegen & ABI `[IN USE]`
| concept/theorem | description | zcc |
|---|---|---|
| Instruction selection = per-node simulation | simulate AST node → asm (maximal munch over the tree) | `codegen/arm64_elf.rs` |
| ABI = finite automaton | argument classification (NGRN/NSRN/NSAA) | `gate abi` |
| Cross-link cancellation | same-compiler ABI errors cancel → 4-way gate | `gate abi` |
| Activation record / frame layout | fp-relative, spill, variadic save | `codegen/arm64_elf.rs` |

### A6. IR — the intermediate form `[PLANNED, scaffolding built]`
| concept/theorem | description | zcc |
|---|---|---|
| Control-flow graph (CFG) | a function is a graph of blocks | `ir.rs` (`IrFunc.blocks`) |
| Basic block | a straight-line sequence + exactly one terminator | `ir.rs` (`Block`) |
| Terminator = automaton over BlockId | Jmp/Br/Ret/Switch/Unreachable | `ir.rs` (`Term`) |
| Virtual registers / temps (SSA-free) | a temporary carries a type from Γ | `ir.rs` (`temps`) |
| CORE vs. EXOTIC-typed two-tier | CORE (Bin/Un/Copy/Load/Lea/Cast) is reachable by passes; EXOTIC-typed (Call/Store/Overflow/Va*/Sync/Asm…) is impure, no DCE/CSE (Inst::Opaque has been REMOVED) | `ir.rs` (`Inst`) |
| Well-formedness verifier | reference integrity + def coverage + entry | `ir.rs` (`verify`) |
| SSA + φ-node `[ssa-qbe fork: repr+interp DONE (Stage 1)]` | single-assignment temps; a φ at a join carries the value of the predecessor edge actually taken | `ir.rs` (`Inst::Phi`, interp φ-select via `prev`, verifier φ-arm V1); proven by `phi_diamond`/`phi_loop` |

### A7. Optimization — proving each pass `[IN USE: IR→IR proven]` (each pass provable, in place of a LOC ceiling)

> Current status: 5 passes implemented and PROVEN at the IR→IR level (`src/opt.rs`,
> 29 tests): const-fold / DCE / copy-prop / CSE are gated by `ir::tests::equiv`
> (commuting square, ⟦A⟧≡⟦P(A)⟧ over the small-domain-exhaustive battery plus
> boundaries) plus `verify`; regalloc (liveness dataflow-fixpoint → interference →
> Chaitin coloring → `verify_coloring`) is gated by the INTERFERENCE INVARIANT
> (rename bisimulation). The orchestrator `optimize()` is a fixpoint over passes 1–4,
> run behind the `ZCC_OPT` flag on the default IR path (measured in-box: torture
> opt≡noopt end-to-end).

| concept/theorem | description |
|---|---|
| Denotational semantics ⟦·⟧:State→State | a pass is correct ⟺ ⟦f⟧=⟦f'⟧ — formalized in `SEMANTICS.md` §1-4 (LEVEL-1) |
| Operational semantics (small/big-step) | interp realizes ⟦·⟧ (Σ=⟨ρ,μ⟩); executable theorem, `SEMANTICS.md` §5 |
| Translation validation (Pnueli/Necula) | validate EACH execution of a pass |
| Bisimulation / simulation | match states edge-by-edge (regalloc) |
| Symbolic execution | symbolic variable → closed term; COMPLETE loop-free |
| Value numbering / congruence / e-graph | normalization + the basis of CSE |
| Term-rewriting soundness ⟦L⟧=⟦R⟧ | correctness BY CONSTRUCTION |
| Newman's lemma | terminating + locally confluent → confluent |
| Dataflow = monotone framework over a lattice | climb to fixpoint |
| Fixpoint Kleene / Knaster–Tarski | least/greatest fixpoint |
| Liveness / reaching-defs / available-expr | basis of DCE/copy-prop/CSE |
| Dominance / dom-tree (Lengauer–Tarjan) | A dom B; basis of copy-prop, SSA |
| Graph coloring / interference (Chaitin–Briggs) | regalloc = coloring |

**SSA pipeline `[ssa-qbe fork]` — theorem ladder (each stage a commuting square; QBE is a *projection*, CbC is supreme):**

| stage | theorem (proof obligation) | status |
|---|---|---|
| φ-node semantics | interp selects the arm of the taken predecessor edge (denotational ⟦φ⟧) | **DONE (Stage 1)** — `phi_diamond`/`phi_loop` |
| SSA construction (Braun 2013, on-the-fly, no dominance frontier) | `⟦f⟧ = ⟦to_ssa(f)⟧` (semantics-preserving promotion of non-address-taken locals). Preconditions: a promoted `float`(size 4) `Load` becomes a narrowing self-`Cast` (`Store∘Load` on a size-4 float cell = round-to-f32, not identity — C99 6.3.1.5); the transform bails on an incomplete CFG (computed goto `GotoPtr` — shared `cfg_complete` guard, also gating `gvn`/`sccp`); an undefined read (recursion to a predecessor-less block) yields `Imm(0)`, not a malformed entry-φ (UB, C99 6.3.2.1p2) | **DONE (Stage 2)** — `opt::to_ssa`; proven by `to_ssa_semantics_preserved` (312 exprs × equiv), `to_ssa_diamond_and_loop`, `to_ssa_respects_address_taken`, `to_ssa_gate_has_teeth`, `to_ssa_narrows_promoted_float`, `sccp_truncates_signed_bitfield`, `to_ssa_undefined_read_is_wellformed`, `to_ssa_bails_on_computed_goto` |
| out-of-SSA / φ-destruction (parallel-copy, swap/lost-copy handled) | `⟦to_ssa(f)⟧ = ⟦out_of_ssa(to_ssa(f))⟧` | **DONE (Stage 3)** — `opt::out_of_ssa`; φ→edge-copies with critical-edge splitting + `seq_pcopy` (Boissinot parallel-copy sequentialization); proven by `out_of_ssa_semantics_preserved` (312 exprs × equiv, φ-free result), `out_of_ssa_diamond_and_loop`, `out_of_ssa_swap` (fib), `out_of_ssa_critical_edge`, `seq_pcopy_swap_is_faithful` |
| SCCP (Wegman–Zadeck sparse conditional constant prop) | lattice ⊤/const/⊥ over CFG-reachability; `⟦f⟧=⟦sccp(f)⟧` | **DONE (Stage 4)** — `opt::sccp` (SSA pass); reachability × constant lattice in one monotone fixpoint (folds through φ where const-fold cannot), faithful via shared `eval_bin/eval_cast/canon`, div0→Bot; proven by `sccp_semantics_preserved` (312 exprs × equiv), `sccp_folds_through_reachability` (dead-branch φ collapse), `sccp_gate_has_teeth` |
| GVN (dominator-based global value numbering) | pure `(op,τ,operand-VNs)` equal + dominating def ⟹ redundant; `⟦f⟧=⟦gvn(f)⟧` | **DONE (Stage 4)** — `opt::gvn` (SSA pass); sound because SSA single-def makes same-operand-temp ⟹ same-value; `dominators` (Allen–Cocke iterative dom-sets); arith only (Loads stay block-local `cse`); proven by `gvn_semantics_preserved` (312 exprs × equiv), `gvn_eliminates_across_dominating_block`, `gvn_respects_dominance` |
| ABI-aware register allocation (consume Chaitin coloring in the backend) | interference-invariant rename-bisimulation ⊕ **call-clobber set-disjointness** — allocatable ∩ scratch = ∅ (no mid-instruction corruption) and, for a call-crossing temp, allocatable ∩ caller-saved = ∅ (survives every `bl`) — with `⟦·⟧` unchanged | **DONE (Stage 5b)** — `opt::abi_alloc`/`color_abi`; class-split Chaitin (GP callee x19–x28 ⊕ caller x14–x15, FP callee v8–v15 ⊕ caller v16–v31) with the crossing-temp select-range restricted to the callee-saved colors; homes wired into the backend `tmp_load`/`tmp_store` contract + a frame-bottom callee-saved save/restore slab; proven by `abi_alloc_valid` (per-class `verify_coloring`) + `abi_alloc_no_clobber` (no crossing temp lands in caller-saved) + `abi_alloc_spill`, gated by the fuzzer differential |
| CFG simplification (Phase A — block merge + unreachable elimination) | straight-line splice (a block spliced into its sole predecessor) + dead-block deletion + renumber ⟹ the executed instruction SEQUENCE is unchanged ⟹ `⟦f⟧=⟦cfg_simplify(f)⟧` | **DONE (Phase A)** — `opt::cfg_simplify` (in the `optimize_ssa` fixpoint); `cfg_complete`-guarded; collapses the straight lines / dead blocks SCCP exposes (φ-arms renamed/pruned to the new numbering); proven by `cfg_simplify_semantics_preserved` (312 exprs × equiv, ≥36 shrink), `cfg_simplify_prunes_dead_branch` (SCCP→prune synergy), `cfg_simplify_gate_has_teeth` |
| register COALESCING (Phase A — conservative biased coloring) | a non-interfering move pair (`Copy` dst/src) prefers the SAME color ⟹ the copy is a self-move (peephole-elidable); the bias picks only among already-free legal colors ⟹ the coloring stays valid ⟹ the SAME `verify_abi` interference invariant ⟹ `⟦·⟧` unchanged (no new proof obligation, no node-merge ⟹ k-colorability never worsened) | **DONE (Phase A)** — `opt::color_abi` `bias` arg fed by `abi_alloc`'s non-interfering `Copy` pairs; proven valid by `abi_alloc_valid` (now WITH bias) + non-vacuous by `coalesce_shares_register_for_moves` (a real edge-copy pair shares a register) |
| LICM (Phase B — loop-invariant code motion) | a PURE, TRAP-FREE, SINGLE-DEF instruction whose operands are all defined outside the loop is hoisted to the loop PREHEADER (a dominating block on the sole entry edge): computed once, not n times; speculation is safe (pure+trap-free), def-before-use holds (preheader dominates the loop) ⟹ `⟦f⟧=⟦licm(f)⟧`. Fences: hoist only Bin(¬Div,¬Rem)/Un/Copy/Cast/Lea, NEVER Load; SINGLE-DEF-gated (zcc IR is only partial-SSA — freezing one def of a multi-def loop-condition temp would turn a finite loop infinite, a bug `equiv` is BLIND to since interp of an infinite loop → Err → skipped); `cfg_complete`-guarded | **DONE, TOGGLEABLE, default-OFF** — `opt::licm` + loop infra (`back_edges`/`natural_loop`/`ensure_preheader`). Proven ⟦·⟧-preserving (`licm_semantics_preserved` 312×equiv, `licm_hoists_invariant`, `licm_respects_variance`, `licm_multidef_condition_stays_finite` direct-interp regression, `licm_gate_has_teeth`) — but MEASURED to REGRESS the memory-bound naive-slot backend (matmul 2.44→2.70×): hoisting trades a cheap recompute for a reload. Kept wired behind `Passes.licm` (`ZCC_OPT_ON=licm`) for a future register-resident backend; ships OFF (only a MEASURED win is default-on) |
| STRENGTH REDUCTION (Phase B.5 — induction-variable based) | a DERIVED IV `j = i·d` (d const) riding a BASIC IV `i₁=φ(i₀, i₂), i₂=i₁+c` (c const) is replaced by a parallel ACCUMULATOR φ `j₁=φ(i₀·d, j₂), j₂=j₁+c·d` and `j := j₁`. PROOF by induction on trip count: base j₁=i₀·d=i₁·d; step j₂=j₁+c·d=i₁·d+c·d=(i₁+c)·d=i₂·d ⟹ j₁=i₁·d always ⟹ `⟦f⟧=⟦sr(f)⟧`. Distribution exact in ℤ/2ⁿ (no overflow gap). Fences: INTEGER-only (float × non-distributive), CONST c,d (c·d folded), all SINGLE-DEF (partial-SSA — checked not assumed), REDUCIBLE single-latch (2-arm header φ), `cfg_complete`-guarded. ENABLING dep: needs `copy_prop` first (mem2reg leaves a copy between the φ and each IV use) — SR is NOT independent | **DONE, TOGGLEABLE, default-OFF** — `opt::strength_reduce`. Proven ⟦·⟧ (`strength_reduce_semantics_preserved` 312×equiv, `strength_reduce_fires` + accumulator-φ evidence, `strength_reduce_in_pipeline_terminates_correct`, `strength_reduce_gate_has_teeth`). The 312-space proof MISSED a nested-loop stale-defcnt panic (`loop-ivopts-1`) that the BOX torture caught — evidence that the commuting-square space ⊊ the real-program space; fixed (per-loop defcnt) + re-gated 0 FAIL / 0 DIVERGE. Same memory-bound backend ⟹ ships OFF behind `ZCC_OPT_ON=sr` |
| BACKEND PEEPHOLE (Phase C — redundant-move + dead-move elimination) | the emitter is an x0-accumulator machine (every value flows through x0 then to/from its home register), so values are stored to a home and immediately reloaded (`mov xH,x0 ; mov x0,xH`), and — because the coalescer gives many short-lived temps the SAME home — a home is often overwritten before it is read (dead stores). TWO region-local passes: (1) REDUNDANT — track a 64-bit value-equivalence, DROP `mov xD,xS` when D≡S already (a verified no-op); (2) DEAD — region-local backward liveness, DROP `mov xD,xS` when xD is rewritten before any read (its value is never observed). Soundness: recognized DEFs give a FRESH value-id / kill liveness; float-or-vector-dest instructions (`ldr q0,[x0]`, `fmov d0,x0`) write NO GP reg so their GP operands are READS (parsed POSITIONALLY); any branch/call/label/unknown/writeback FLUSHES (redundant) or sets live-out = FULL (dead). Machine-level translation-validation, not IR `equiv` | **DONE (Phase C), default-ON — the BIGGEST MEASURED WIN.** `arm64_elf::{drop_redundant_moves, drop_dead_moves}` (one-file-per-target law), toggle `ZCC_OPT_OFF=peephole`. Measured: matmul 398→306 insns (240→148 movs); **bench geomean 1.39×→0.98× vs gcc-O0 — zcc now BEATS gcc-O0 on average** (loops 0.66×, sieve 0.60×, matmul 2.44→1.68×, fib 1.39×). Gated: 16 machine unit tests (incl. teeth + the `dce_keeps_addr_of_float_load` regression) + torture 0 FAIL + opt-parity 0 DIVERGE. THE DCE-BUG LESSON: unit tests passed but BOX torture caught 32 FAIL (stdarg-1 SIGABRT) — a positional-parse bug mistook the GP *address* of `ldr q0,[x0]` for its *destination* and dropped the live `mov` feeding it; the differential gate, not the unit proof, is what caught it |
| COMPUTE-INTO-HOME instruction selection (Tier-1 #1 — kill the x0 funnel at the SOURCE) | per-node simulation with the target register = the allocator's HOME, not a fixed x0 accumulator. For an integer `Bin(d,op,a,b)`/`Un(d,u,a)`: read each operand from the register that already HOLDS it (`src_gp` → its GP home x19–x28, or x0/x1 scratch when spilled/immediate), emit the ALU op DIRECTLY into d's home (`ir_bin_r`/`ext_r` parametric over the dest reg), no `mov x0,·` in, no `mov ·,x0` out. Where the peephole DELETES funnel copies after the fact, this NEVER EMITS them. Soundness = ir_bin's, register-renamed: for the degenerate rd=ra=0,rb=1 (all-spilled = the `ZCC_O0` path) the emitted bytes are IDENTICAL to the old x0-funnel ⟹ the -O0 reference (already clang-parity) is untouched; the register-resident path is validated against it, exit-code for exit-code, by opt-parity. rd may alias ra/rb only where the allocator coalesced them (source dead here); rem's `x2` quotient scratch is never a home | **DONE (Tier-1 #1), default-ON.** `arm64_elf::{ir_bin_r, ext_r, src_gp, gp_home}` + rewritten `Inst::Bin`/`Inst::Un` (float keeps the fmov funnel). Measured (box, best-of-3): matmul inner product `mul xD,xA,xB`/`add xACC,xACC,x·` land in homes directly; **bench geomean 0.98×→0.81× vs gcc-O0** (loops 0.66→0.44×, matmul 1.69→1.38×, sieve 0.59→0.48×; fib 1.43× — call-bound, awaits inlining #5). Gated: opt-parity 1552/0 DIVERGE + torture 1378/0 FAIL. This is the register-resident backend the LICM/SR rows were waiting on — their re-measure is now unblocked |

**5 passes → theorem (all DECIDABLE; no loop restructuring → outside Rice):**
const-fold = rewrite-soundness · DCE = liveness · copy-prop = dominance + Leibniz ·
CSE = value-numbering · regalloc = rename-bisimulation.

**The SSA pipeline `opt::optimize_ssa` (the QBE-level projection under CbC) `[ssa-qbe fork]`:**
`to_ssa ▸ (sccp ∘ const_fold ∘ copy_prop ∘ gvn ∘ cse ∘ dce ∘ cfg_simplify [∘ licm ∘ strength_reduce])* ▸ out_of_ssa ▸ optimize`
(register coalescing runs later, inside `abi_alloc`, as a coloring bias; `licm` is
default-OFF — see below). The active pass set is not hard-coded: `optimize_ssa` reads an
`opt::Passes` record, so any element toggles via `ZCC_OPT_OFF=`/`ZCC_OPT_ON=` comma lists
(the gcc `-fno-<pass>` / LLVM `PassBuilder` idiom). Each toggle is `⟦·⟧`-neutral by
construction — every pass carries its own commuting-square proof, so any subset composes
to the same denotation; the toggle changes only PERFORMANCE (`passes_toggle_wiring`).
Each stage is an individually-proven ⟦·⟧-invariant rewrite ⟹ the composite is too;
re-MEASURED end-to-end by `optimize_ssa_preserves` (312 exprs × equiv, φ-free result)
+ `optimize_ssa_preserves_corpus_and_reduces` (value-correct + shrinks). The artifact
Stage 5 wires into the backend behind an optimization flag. **LICM and STRENGTH REDUCTION
are IMPLEMENTED and PROVEN but ship OFF** (measured-negative on the naive-slot backend —
see their rows); one env flag (`ZCC_OPT_ON=licm,sr`) from ON, latent for a register-resident
backend. Still OMITTED (QBE "most of the win, a fraction of the complexity"): cross-loop
GVN and other loop restructuring.

### A8. Testing & proof methodology `[IN USE]`
Differential testing · Metamorphic (commuting-square) · Property/boundary-value ·
Structural exhaustion · UB filtering · 2-fact (PASS|NOT-IMPL|FAIL, gate = 0 FAIL) ·
Translation-validation-as-gate (`ir.sh`, planned) · Evidence-trail (clean input).

## B — BY PURE-MATHEMATICS BRANCH (reverse index)

- **B1. Discrete & graph theory:** automata/formal languages (A1–A3), directed graphs
  (CFG, dom-tree, interference), trees (AST, expression tree), combinatorics/counting
  (exhaustive generator), equivalence relations (bisimulation, value-number classes).
  Algorithms: DFS/postorder, reverse-postorder, SCC.
- **B2. Algebra:** semilattice (UAC join, dataflow meet), lattice/complete-lattice
  (types, dataflow; the basis of Tarski), free term algebra (AST/IR),
  monoid/associativity (token/block/fold concatenation), Boolean algebra (`#if`,
  branching, bit operations), sparse linear algebra (multidimensional array
  offset/stride = an affine map index→address, the 2-D VLA `i·rowsz+j·esz`).
- **B3. Order theory:** poset (rank, dominance, lattice), monotone + fixpoint (Kleene
  chain, Knaster–Tarski), well-founded/termination (macros, rewriting), Galois
  connection [FOUNDATION, abstract interpretation].
- **B4. Logic & proof theory:** typing judgment / natural deduction (Γ⊢), Hoare
  logic/wp [PLANNED], FOL/SMT-style (symbolic path condition, decidable loop-free)
  [PLANNED], Leibniz equality (copy-prop), SAT [FOUNDATION].
- **B5. Analysis & machine arithmetic (a NARROW but genuine role):** IEEE-754
  floating-point semantics (rounding/NaN/Inf/signed-zero — codegen preserves the bit
  pattern), real analysis [FOUNDATION] (floating point is NOT associative → no
  reordering of float folds), monotone convergence (dataflow reaches a finite
  fixpoint), number theory/modular arithmetic (align = modulo 2^k, two's-complement =
  modulo 2^n, `%`/`/` truncation-toward-zero per C99).
- **B6. Probability (test methodology):** random differential / fuzzing (csmith/yarpgen,
  planned) — expected defect coverage ∝ sample count, below structural-exhaustion in
  certainty (see §0b).

## C — COMPUTABILITY & COMPLEXITY (architectural complexity)

**C1. Computability:** the Halting problem / undecidability (the root of every limit) ·
**Rice's theorem** (⟦f⟧=⟦f'⟧ is undecidable in general → a pass must constrain shape
into a decidable class) · decidable fragment (loop-free/bounded → symbolic equivalence
is COMPLETE) · recursively enumerable (the set of valid programs).

**C2. Complexity per phase:** lexing **O(n)** · preprocess **O(n)** amortized (hideset
bounds blow-up) · recursive-descent parsing **O(n)** (no exponential backtracking) ·
type/layout **O(n)** · codegen **O(n)** · dataflow **O(n·h·|lattice|)** · dom-tree
**O(n·α(n))** Lengauer–Tarjan · value-numbering **O(n)–O(n log n)** · **register
allocation NP-complete** (Chaitin) → simplify/spill heuristic · SSA construction
**O(n·α(n))**.

**C3. Complexity classes:** P (frontend + most analyses) · NP-complete (regalloc —
hence the heuristic rather than "absolute optimum"; but *valid-coloring* is verifiable
in P) · undecidable (equivalence in general → only structural + per-run translation
validation) · **the complexity of the compiler ITSELF** (invariant: `src/` ≤ ceiling —
a compiler is a theorem and must remain readable).

## D — SCI-GATE ↔ THEOREM (ground-truth tier)
| gate | space exhausted | theorem |
|---|---|---|
| `shape` | lexer/declarator/layout | grammar automata + record-layout automaton |
| `cpp` | preprocessor | term rewriting system + #if const-eval |
| `decay` | type derivation | type-derivation lattice |
| `alg` | UAC + fold | join-semilattice + commuting-square fold↔runtime |
| `abi` | ABI classify + link | finite automaton + cross-link cancellation |
| `ir` *(`cargo test`)* | IR + 5 passes + reference semantics | reference semantics ⟦·⟧ (`SEMANTICS.md`, LEVEL-1) + executable THEOREM: commuting-square exhaustion of 𝔼_struct (312 expr × 5 passes = 1560 squares) + interference invariant (regalloc) |

---

# PART II — SPEC → CONSTANT / PARAM / VALUE-TABLE

> Side II of §0: *the values zcc copies from the standards*. Every constant must be
> traceable to a line of specification. Where they live: **TyTab in `ast.rs`** (layout,
> LP64) + **the target file** (ABI/section/asm) + **`ext.rs` plus the `EXT(...)` marker**
> (vendor surface). Target: AArch64 ELF Linux.

### II-1. ISO C99 — language constants
| table/constant | spec source | zcc |
|---|---|---|
| integer conversion rank | 6.3.1.1 | `parser.rs` promote/common_ty |
| `<limits.h>` (INT_MAX, CHAR_BIT=8…) | 5.2.4.2.1 | header + TyTab size |
| UAC conversion table | 6.3.1.8 | `common_ty` |
| escape/trigraph, numeric literal suffixes | 6.4.4 | `lexer.rs` |
| source/exec char set = UTF-8 multibyte (decode table RFC 3629: masks `0x1f/0x0f/0x07/0x3f`, shift 6) | 5.1.1.2 + 6.4.5 | `lexer.rs` `utf8_cp` |
| `%`, `/` truncation-toward-zero; signed overflow = UB | 6.5.5 | codegen + UB-filter |
| char = **unsigned** (AAPCS64 aarch64 default, locked) | 6.2.5 + AAPCS64 | TyTab (`char`→UCHAR) |

### II-2. Memory model — size & alignment (LP64, locked)
| type | size | align | source |
|---|---|---|---|
| char/short/int/long/long long | 1/2/4/8/8 | =size | LP64 (System V AArch64) |
| pointer | 8 | 8 | LP64 |
| float/double | 4/8 | =size | LP64 |
| long double | **16** | **16** | binary128 memory/ABI (AAPCS64); *arithmetic* performed as double (float.h `LDBL_MANT_DIG=53`), libgcc `__extenddftf2`/`__trunctfdf2` at the boundary — a documented design choice |
| struct/union | Σ with padding | max field, aggregate ≥ **8** for `data_align` | AAPCS64 §5.1 |
| bitfield | packing by storage unit | — | 6.7.2.1 + ABI |

Where they live: **`ast.rs` TyTab** (`size/align/data_align`). Changing the model =
**parameterizing TyTab**, NOT scattering conditionals (architectural rule).

### II-3. Calling convention — AAPCS64 (register table + classification)
| parameter | value | source |
|---|---|---|
| integer/pointer arg regs | x0–x7 (NGRN 0–7) | AAPCS64 §6.4 |
| FP/SIMD arg regs | v0–v7 (NSRN 0–7) | §6.4 |
| return | x0 (+x1 for 16B), v0 | §6.4 |
| stack arg (NSAA) | overflow after x7/v7, align 8 | §6.4 |
| sp before `bl` | aligned to 16 bytes | §6.2.2 |
| callee-saved | x19–x28, x29(fp), x30(lr) | §6.1.1 |
| composite overflow locks NGRN=8 (C.11); HFA overflow does NOT lock | — | §6.8 rule C.11 |
| prologue | `stp x29,x30,[sp,#-16]!` | §6.2.2 |

Where it lives: **`codegen/arm64_elf.rs`**. The argument-offset algorithm lives in
**three places that must agree byte-for-byte** (codegen call / codegen spill / parser
va_off) — changing one means changing all three, plus running `gate abi`.

### II-4. Object format — ELF / sections (AArch64 Linux)
| constant | value | source |
|---|---|---|
| sections | `.text`/`.rodata`/`.data`/`.bss` | System V ABI |
| symbol: **NO** underscore (unlike Darwin) | — | ELF |
| local relocation | `adrp`+`:lo12:` (PAGE/PAGEOFF) | AArch64 ELF |
| extern/GOT | `:got:`+`:got_lo12:` | ELF |
| TLS | `:tprel_*` / TLS model | ELF TLS |

Where it lives: **`codegen/arm64_elf.rs`**. (The former Darwin idiosyncrasies —
`_`, `@PAGE`, `@TLVPPAGE`, variadic-args-on-stack — were removed when Mach-O was
dropped; they are recorded in CLAUDE.md to avoid confusion.)

### II-5. Arch — AArch64 instruction/encoding constants
Register file (x0–x30, sp, v0–v31), immediate ranges (add/sub 12-bit, logical bitmask,
branch offset ±128MB), condition codes (eq/ne/lt…), addressing modes (`[base,#off]`,
`[base,index,lsl]`). Source: **ARM ARM (DDI 0487)**. Where it lives:
`codegen/arm64_elf.rs` (asm text).

### II-6. GCC/vendor spec — the nonconforming surface (`EXT(...)`)
| feature | status | marker |
|---|---|---|
| stmt-expr `({...})`, `__extension__` | IN USE | `EXT(gcc)` |
| `__attribute__((aligned/packed/weak/alias/transparent_union))` | IN USE | `EXT(gcc)` |
| `__attribute__((mode(QI/HI/SI/DI/word/SF/DF/TF)))` → width remap | IN USE (Side-II machmode table; TI/XF rejected) | `EXT(gcc)` `parser.rs apply_mode` |
| `__builtin_*` (whitelist), `typeof`, `__GNUC__=4`, `types_compatible_p` | IN USE, selectively | `EXT(gcc)` |
| labels-as-values (`&&label`, `goto *e`), stmt-expr, range `case lo…hi`/`[lo…hi]`, elvis `?:` | IN USE | `EXT(gcc)` |
| extended asm (template + narrow constraints, musl-critical) | IN USE, subset | `EXT(gcc)` |
| `vector_size`, `scalar_storage_order`, nested functions, `mode(TI/XF)` | **cleanly REJECTED** → NOT-IMPL | `EXT(gcc)` |

Where it lives: **`src/ext.rs`** plus touch points marked `EXT(...)`. Verified by
excision: remove ext.rs plus the marked branches → the remainder still passes the full
C89 suite (`grep 'EXT(' src/` covers 100%).

---

# PART III — KEYSTONE: correctness-by-construction & why Gödel lies outside

**Proposition:** if NO line of `src/` lies outside the space {theory-fact ∪ spec-fact}
— each line being a **faithful** realization of a theorem (Side I) or a spec-constant
(Side II) — then zcc **necessarily passes every suite**. This cannot be negated.

**Why it holds (the tight condition — "faithfulness" is the hinge):** a suite is a
differential test against the referee (`cc`); both zcc and the referee are **shadows of
the SAME specification** (ISO C99 + AAPCS64 + ELF + AArch64) over the same mathematical
ground. Two faithful shadows of one object coincide — a mismatch ⟹ one side reads the
spec wrong ⟹ the **bug lies WITHIN the space** (a faithless realization, not "outside
the space"), and it is caught by a gate. The three conditions of "faithfulness":
1. **Faithfulness** — the code genuinely realizes the theorem CORRECTLY; the constant
   genuinely matches the correct line of spec. A bug does not hide "outside the space"
   but hides in a "WRONG realization inside the space".
2. **Completeness** — theory + spec cover the entire fragment of the language the suite
   touches; a gap is a **NOT-IMPL** (an honest rejection), NOT a miscompile. This is the
   2-fact discipline: **0 FAIL**, without requiring 0 NOT-IMPL.
3. **Shared ground truth** — zcc and the referee share a spec origin ⟹ agreement is
   necessary, not accidental.

Hence the entire engineering apparatus (sci-gate for Side I, differential-vs-referee for
Side II, evidence trail) IS the **mechanical audit of faithfulness**. The philosophy and
the test suite are ONE, seen from two sides.

**Gödel's incompleteness, though true, lies OUTSIDE the compiler↔suite relation.**
Incompleteness states that a sufficiently strong formal system cannot prove its OWN
consistency / every true arithmetic proposition. The compiler↔suite relation is not that
problem:
- **Per-case decidable** — run zcc and the referee on a concrete input and compare: this
  is finite and terminating.
- **Correctness-by-construction** is proved at the level of rewrite rules / a finite
  structural space — each piece decidable (the reason the 5 passes are CHOSEN to lie in
  the decidable fragment, §C1). Rice/Halting/Gödel bite only if one demands an algorithm
  deciding equivalence for ALL programs, or forces the system to prove ITSELF — which is
  not done here.
- **The escape from self-reference is an EXTERNAL oracle.** Differential testing uses an
  independent referee: zcc never has to prove its own consistency; it only has to AGREE
  with an independent witness on a concrete input. Gödel forbids a system from proving its
  own consistency; it does NOT forbid two independent systems from agreeing on a decidable
  predicate. This is the same reason the charter removes any unreliable narrator from the
  trust path (only mechanical verdicts are valid): moving the referee OUTSIDE the system is
  how one evades both Gödel and the self-trust paradox at once.

**DEBUG corollary — fix BY DECOMPOSITION, no ad-hoc patching.** When zcc fails a suite
(especially csmith/yarpgen), and the theory for that feature is ASSUMED sound, the failure
can only be one of three (or a combination), in this order of investigation: **(1)** the
decomposition from theorem produced the WRONG control-flow/algorithm → there is ≥1 LOC
**outside the theorem** (Side I); **(2)** an ISO/OS/arch **spec-constant** is applied
WRONG (Side II); **(3)** the test/oracle/referee/generator is faulty or collected garbage
input (LOW probability, but ≠0) — CONSTRAINED by the presumption-of-guilt rule: the
compiler is guilty until proven innocent, so cause 3 is the LAST resort, asserted only
after MECHANICAL multi-angle proof plus an independent referee; it may not be used as a
reflexive excuse, and "clang/gcc also fail" is not a valid excuse. We code by
decomposition ⟹ we fix by decomposition: LOCALIZE the fault by mechanical measurement
(bisect pass/module, diff asm, seek the case) FIRST, classify Side-I/II/III SECOND, then
fix precisely there. If a fix requires adding a line that maps to no theorem, the
direction is wrong. Measurement overrides every hypothesis — the first hypothesis-fix
being wrong is normal; keep measuring (illustrative case pr43220: guessed CSE-Side-I →
measurement refuted it → the true cause was a Side-II frame-layout constant in the
backend).

---

*Founding statements: "1 (theory → control-flow/data-structure/algorithm) ⊕ 2
(iso/os/abi/arch/gcc spec → constant/param/value-table) = zcc source code — if only one
rule is kept in CLAUDE.md, keep this one." And: "covering 250+ applications is easy;
passing csmith/yarpgen is the hard part — dozens of compilers of the same size still
fail." Further entries are merged into the appropriate Part/Branch/Table as they arise.*
