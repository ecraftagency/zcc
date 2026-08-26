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

### A5. Instruction selection & ABI `[IN USE — R0.6 base cover / R3.1 munch]`
| concept/theorem | description | zcc |
|---|---|---|
| Instruction selection = tree pattern matching (BURS / maximal munch, Aho-Ganapathi-Tjiang 1989) | an HIR expression tree is covered by machine-instruction patterns; a value with ONE use may be folded into its user, a multi-use value is materialized once. Each pattern row is a theorem `⟦hir-tree⟧ = ⟦mir-seq⟧` with its own battery entry | `isel/lower.rs`. **R0 shipped the BASE COVER** (one HIR instruction → one canonical machine sequence, the identity cover, proven correct); **R3.1 added the MUNCH TABLE on top of it** — `isel/lower.rs::munch`, ONE pre-pass over the use-def graph deciding, before any instruction is emitted, which producers each consumer absorbs (it must be a pass, not an emission-time choice, because the producer is emitted first). There is no `isel/pattern.rs` — the rows live in `munch` + `lower`. **Two licences, not interchangeable**: an ADDRESS folds only when EVERY use of it is a memory operand (folding into some while still computing it for others duplicates work); an ALU operand folds on a SINGLE use (the shift/extension happens inside the consumer). A producer that has itself absorbed something may not be absorbed again. Rows: `[base,#off]`, `[slot,#off]`, `[base,idx,ext #k]`, `add/sub …, sxtw`, `op …, lsl #k`, `madd`/`msub` (the multiply's operands are read as OPERANDS, so a LITERAL multiplier does not refuse the row — the literal must reach a register for `mul` regardless, and `madd` then absorbs the `add` for free), `cmp`+`b.cc`, `cbz`/`cbnz`, `tbz`/`tbnz` (sign/single-bit tests), `cmp`+`csel`, `ubfx`/`sbfx`. `mul(x,2^k)→shl` is an HIR canonicalization (`fold::canon`), because only the shift form folds into an address |
| ABI = finite automaton over the C signature | AAPCS64 §6.4–6.8 C.1–C.15: the state is (NGRN, NSRN, NSAA) and each parameter advances it. Composites ≤16B in registers, HFA/HVA, >16B by reference, sret in x8, C.11 lock, variadics | `isel/abi.rs` (R0: the scalar subset; composites/varargs R1.2). Side-II table: II-3 |
| Constrained instruction = local parallel copy (Hack 2007 §4) | rather than teach the allocator about "argument registers", ONE `ParallelCopy` before a call moves every operand where the ABI wants it, and one after moves the result out. The allocator then sees no fixed constraint anywhere, and an argument permutation (`f(b,a)`) is resolved by the same windmill sequentialization every block edge uses | `isel/lower.rs::call`, entry prologue |
| Immediate legalization | a constant either fits an instruction's immediate field or must be materialized; which is a pure encoding question | `isel/imm.rs` over the `mir/isa.rs` predicates (II-5) |
| Cross-link cancellation | same-compiler ABI errors cancel → 4-way gate | `gate abi` |

### A6. HIR — the target-independent SSA layer `[IN USE — R0.2/R0.3]`
| concept/theorem | description | zcc |
|---|---|---|
| Control-flow graph (CFG) | a function is a graph of blocks | `hir/mod.rs` (`Func.blocks`) |
| Basic block | a straight-line sequence + exactly one terminator | `hir/mod.rs` (`Block`) |
| Terminator = automaton over BlockId | `jmp` / `br` / `switch` / `ret` / `unreachable` / `goto_ptr` | `hir/mod.rs` (`Term`) |
| **SSA with BLOCK PARAMETERS, not φ** (Cranelift/MLIR/Swift model) | `br %c, bb1(%a,%b), bb2(%c)`. The edge transfer is EXPLICIT, so: the verifier needs no φ-arm rule, the interpreter needs no "which predecessor did we come from" state, and SSA destruction is literally one parallel copy per edge. The lost-copy and swap problems do not arise — they are artifacts of φ, whose reads happen "on the edge" only by convention | `hir/mod.rs` (`Block.params`, `Target.args`); the same model continues into MIR |
| Closed scalar type domain | `Ty = I8\|I16\|I32\|I64\|F32\|F64`; pointers are I64. Signedness and width live in the OPCODE (`sdiv`/`udiv`, `icmp.slt`/`icmp.ult`, `sext`/`zext`), never in a TyTab lookup — which is what makes `⟦·⟧` a CLOSED definition independent of the frontend | `hir/mod.rs` (`Ty`), `SEMANTICS.md` §1/§3 |
| Effect classification | `Pure \| Read \| Write \| Call`: DCE, CSE, GVN, LICM and sinking legality are ONE table lookup, never a per-pass hand-written opcode list | `hir/mod.rs` (`Inst::effect`) |
| Alias class on every access (TBAA hook, C99 6.5p7) | the effective-type tag a type-based alias oracle will read. Carried from day one because retrofitting it through every load/store later is expensive | `hir/mod.rs` (`AClass`); consumed at R2.2 / §16 ★1 |
| Well-formedness verifier | single assignment · every use DOMINATED by its definition · block-argument arity and type against EVERY incoming edge · opcode/operand typing · exactly one terminator · entry takes no parameters | `hir/verify.rs` |
| Narrow arithmetic never appears (C99 6.3.1.1) | integer promotion means HIR performs no `Bin`/`Cmp` at I8/I16; narrow types occur only in `load`/`store`/`cvt`, which is exactly where A64 has a dedicated form. Making the invariant explicit is what lets isel compare in a `w` register with no re-extension | `hir/build.rs::promote` |
| Dominators (Cooper, Harvey & Kennedy 2001) | iterative meet over predecessors in reverse postorder until stable; simpler than Lengauer-Tarjan and adequate at our scale. Preorder + subtree extent makes `dominates` an O(1) range test | `cfg.rs` (`DomTree`), shared by HIR and MIR |
| Natural loops / loop forest | a back edge b→h with h dom b defines the loop of h; body = every node reaching b without leaving h. Nesting by body containment | `cfg.rs` (`LoopForest`) |
| Critical-edge splitting | an edge from a multi-successor block to a multi-predecessor block has nowhere to put edge code. Its square is the identity: the inserted block is empty and forwards exactly the arguments the edge carried | `hir/dom.rs`, `regalloc/destruct.rs` |

### A6b. MIR — the machine layer `[IN USE — R0.5]`
| concept/theorem | description | zcc |
|---|---|---|
| ONE type, two lifecycle states | VIRTUAL (SSA over virtual registers, block parameters) and PHYSICAL (after allocation: no vreg, every constraint met, parameters destructed). Exactly LLVM's "MIR" | `mir/mod.rs` (`MFunc.physical`) |
| Machine operands are FIRST CLASS | addressing modes (`[base,#imm]`, `[base,idx,ext #k]`, pre/post-index), shifted and extended register operands, condition codes. **This is the correction of rc3's architectural defect**: none of these were expressible in the old IR, so every machine optimization had to be a string peephole on `.s`, where nothing can be verified | `mir/mod.rs` (`AddrMode`, `Rhs`, `CC`) |
| NZCV as a register class of size k=1 | flags are a value with a definition and uses. Compare-elimination becomes a value-numbering over flag definitions; "two live compares" becomes an ordinary interference the allocator resolves by rematerializing the compare (always legal — a compare is pure) | `mir/mod.rs` (`Class::Flags`) |
| The operand visitor is the ONLY access path | liveness, the allocator, the verifier and the interpreter reach registers only through `visit`/`visit_mut` and memory only through `effect()`. No component outside `isa.rs` matches on an opcode — which is what keeps a new instruction from needing edits in five places | `mir/mod.rs` |
| Cost = instruction count, exactly (REARCH §10) | one `MInst` is one machine instruction after frame/layout, so `cost(f) = \|MIR_final(f)\|` needs no separate cost model and Δinsn of any transform is computed BEFORE emitting anything. The one expansion, `MovImm`, reports its length via `isa::mov_chain().len()` | `mir/mod.rs`, `emit.rs` |
| Well-formedness verifier, two modes | VIRTUAL: SSA + dominance + edge arity + register-class agreement per instruction form. PHYSICAL: no vreg survives, every ABI-fixed operand satisfied | `mir/verify.rs` |
| Emission determinism | identical MIR ⟹ identical bytes. Sealed by a gate that compiles each program in several FRESH processes, so a per-process hash seed leaking into the output is caught | `tests/determinism.sh` |

### A7. Register allocation ON SSA — the load-bearing theorem `[IN USE — R0.7]`

> Why this is the centre of the architecture. rc3 ran `to_ssa ▸ passes ▸ out_of_ssa ▸ abi_alloc`:
> allocation AFTER SSA was destroyed, on a graph that is NOT chordal, where colouring is
> NP-hard, so a heuristic gave every value one home for its whole life and live-range
> splitting was structurally impossible — 27,403 frame-slot memory operations on sqlite.
> Allocating while the program is still in SSA changes the complexity class of the problem.

| concept/theorem | description | zcc |
|---|---|---|
| **SSA interference graphs are CHORDAL** (Hack 2007, PhD) | and a preorder of the dominator tree is a PERFECT ELIMINATION ORDER for them. Greedy colouring along that order is therefore OPTIMAL — it uses exactly ω(G) = the maximum register pressure — and it CANNOT get stuck once pressure ≤ k. No graph is built, no node is merged, there is no simplify/spill iteration | `regalloc/color.rs` |
| Liveness on SSA with block parameters | `live_out(b) = ⋃_{s∈succ} (live_in(s) ∪ args(b→s))`, `live_in(b) = uses(b) ∪ (live_out(b) ∖ defs(b))` with parameters counted as definitions. PHYSICAL registers are tracked in the SAME set — an argument register is genuinely live from the copy that sets it up until the call that reads it, and a virtual register overlapping it must not take that colour | `regalloc/live.rs` |
| Spilling = Belady MIN on a working set (Braun & Hack 2009) | per register class, walk each block forward carrying a WORKING SET `W` of ≤ k values; at the head, live-in non-memory values ordered by next-use distance, truncated to the budget (what does not fit becomes memory-resident — Belady's provably-optimal rule for a fixed-size cache, which is what a register class is); a use of a value absent from `W` gets a RELOAD into a fresh vreg serving every later use in the block; a call's clobber set counts as registers already spoken for, so what survives a call is ≤ the class's callee-saved count (this SUBSUMES all "crosses a call" reasoning; two values crossing DIFFERENT calls are bound by whichever call comes first). Post-condition: pressure ≤ k at every point — exactly colouring's precondition | `regalloc/spill.rs`. **R0/R1 shipped the sound base case** (spill-at-def, reload-per-use); **R2.2 shipped Braun-Hack proper**, the blocking prerequisite mem2reg forced (the base case collapsed exactly as predicted: sqlite 12,253 → 275,665 frame mem-ops before it). **Two deviations, theory not implementation notes**: (1) **NO SSA reconstruction** — a reload's fresh register is used only inside the block that created it, so its live range is dominated by its definition and SSA holds by construction; a value staying in `W` across an edge keeps its original name. The price is one reload per block-residency rather than one per program region (the §13b spill-traffic excess). (2) **A spilled BLOCK PARAMETER cannot be stored at its definition** — its definition is the block head — so the parameter is removed from the IR and each predecessor writes the slot; one slot per SSA WEB (parameter ∪ arguments), merged only where members do not interfere |
| Rematerialization (Briggs 1992) | a value whose producer reads no register (`MovImm`, `Adrp`, `SlotAddr`, an extend of a live value) is RECOMPUTED instead of stored-and-reloaded — the recomputation is one instruction and the store disappears entirely. Flags are the extreme case: never spilled, always rematerialized | `regalloc/spill.rs` (shipped R2.2 with the spiller) |
| Coalescing = BIASED COLOURING | at a definition that is a copy, prefer the partner's colour when free. It never merges nodes, so it can never break the pressure guarantee — the property Chaitin-Briggs coalescing must be careful about. Upgrade to Boissinot merging only if the MEASURED residual copy count justifies it (Law-4) | `regalloc/color.rs` |
| Calls need no special rule | the caller-saved set is a property of the value, not of the instruction: a value live across a call may not take a caller-saved colour (AAPCS64 §6.1.1). Everything else falls out of ordinary constraint-respecting greedy colouring | `regalloc/live.rs::crosses_call` |
| SSA destruction = one parallel copy per edge (Boissinot et al. 2009) | with block parameters this is the whole of it. Sequentialization is the windmill: emit any copy whose destination is nobody's source; when only cycles remain, break one through the reserved scratch register (x16 = AAPCS64 IP0, v31) — which is what those registers are reserved FOR | `regalloc/destruct.rs` |
| Correctness = renaming bisimulation | allocation renames values and may route some through memory; it must not change what the function computes. Proven as `⟦mir_v⟧ = ⟦mir_p⟧` under one interpreter, plus the structural checks: no vreg survives, every fixed operand satisfied, every reload DOMINATED by a spill of its slot, no parallel copy left | `regalloc/verify.rs`, `regalloc/tests.rs` |
| Frame lowering realizes the ABI CONTRACT | before frame lowering, AAPCS64's promise that x19–x28 / v8–v15 / x30 survive a call is an ASSUMPTION the allocator already relied on; after, real `Spill`/`Reload` instructions keep it. So `⟦mir_p⟧ = ⟦mir_final⟧` is precisely the statement that frame lowering realizes that assumption in instructions — and the prologue is ordinary MIR, not a printed string, so the interpreter executes it | `mir/pass/frame.rs`, `mir/pass/tests.rs` |

### A7b. Optimization — proving each pass `[IN USE — R2 (HIR) / R3 (MIR); two rows ⬜]`

> Status after R3: **the ladder is shipped**, each pass carrying its commuting square (Law 3)
> under `hir::interp`/`mir::interp`, gated by `opt-parity` (passes off vs on, 0 DIVERGE) plus
> torture/csmith300/yarpgen300. The R0 pipeline was `AST → HIR → isel → regalloc → frame →
> layout → .s`; the passes below now sit between lowering and isel (HIR ladder) and between isel
> and regalloc / after frame (MIR ladder). The MIR ladder is COMPLETE (`auto_inc` R3.2 and
> `shrink_wrap` R3.3 shipped 2026-08-25). Still `⬜`, all in the HIR loop tail: iv /
> pointer-iv / LFTR, and rotate / final-value. Scalar strength-reduction (`mul`→`add` on an
> induction variable) is CLOSED as Law-4 category-(a) rather than pending — the rewrite is
> 1:1 static and an out-of-order core pipelines `mul` at ≈`add` cost, so it is null on every
> target (REARCH §13c). The general theorems in the first table are
> architecture-independent and survived the re-architecture unchanged — they are about how a
> pass is PROVEN, not about what the IR is.

| concept/theorem | description |
|---|---|
| Denotational semantics ⟦·⟧:Σ→Σ | a pass is correct ⟺ ⟦f⟧=⟦P f⟧ — formalized in `SEMANTICS.md` |
| Operational semantics (big-step) | the interpreter REALIZES ⟦·⟧; an executable theorem. Bounded by a GLOBAL step budget shared across the whole recursion — a per-frame depth guard bounds only a deep spine, never a shallow-but-exponential call tree (fib(255) is depth ~253 yet ~10⁵³ nodes); past the budget is "outside the modelled space" → ⊥, exactly like UB |
| Translation validation (Pnueli/Necula) | validate EACH execution of a pass — the shape used for isel and for the allocator |
| Bisimulation / simulation | match states edge by edge (allocation = renaming bisimulation) |
| Value numbering / congruence / e-graph | normalization, and the basis of CSE/GVN |
| Term-rewriting soundness ⟦L⟧=⟦R⟧ | correctness BY CONSTRUCTION |
| Newman's lemma | terminating + locally confluent ⟹ confluent |
| Dataflow = monotone framework over a lattice | climb to fixpoint (liveness, SCCP, the extend lattice) |
| Fixpoint Kleene / Knaster–Tarski | least/greatest fixpoint |
| Liveness / reaching-defs / available-expressions | the basis of DCE, copy propagation, CSE |
| Dominance / dominator tree | A dom B; the basis of GVN, LICM, and of the colouring order itself |
| **Chordal graphs / perfect elimination order** | the SSA colouring theorem (A7); replaces "graph colouring is NP-hard" as the operative fact |

**The HIR pass ladder (REARCH §4, gcc `-ftree-*` order):** ✅ shipped —
`cfg_simplify` (`pass/cfg.rs`) · `sroa+mem2reg` (Braun 2013, `pass/sroa.rs`) · `sccp`
(Wegman-Zadeck, `pass/sccp.rs`) · `gvn` (absorbing CSE, copy-prop, constant folding, algebraic
normalization, `pass/gvn.rs`) · `load_elim/dse` (gated by the alias oracle, `pass/mem.rs`) · `dce`
(Effect table, `pass/dce.rs`) · `inline` (β-reduction + interprocedural purity, `pass/inline.rs`) ·
`licm` (unconditional at O1 — the ALLOCATOR owns pressure, not the pass, `pass/licm.rs`) · `if_convert`
(diamond → select, `pass/ifconv.rs`) · `sink` (licm's dual — a pure trap-free instruction with one
using block moves down to it; added at R3 because §13b ranked register pressure the largest residual,
`pass/sink.rs`). `purity` (the INTERPROCEDURAL read-only predicate — gcc's `pure`, not `const`; an optimistic
fixpoint over the call graph, which is what makes a recursive read-only callee read-only, since
"performs a write" is existential over the body, `pass/purity.rs`) · `invariant pure-call hoist`
(licm with a CALL as the hoisted term: same preheader, same invariance rule, plus two fences a
scalar does not need — the loop must be MEMORY-CLEAN, because a read-only callee is a function OF
the memory state, and the call must be proven to run on the first iteration anyway, because purity
does not imply termination, `licm::hoist_call`).
⬜ remaining — `iv/pointer-iv/LFTR`, `rotate / final-value` (`pass/iv.rs`).

**The MIR pass ladder (REARCH §8):** pre-allocation on SSA — ✅ `cmp_elim` (value numbering over flag
definitions: `subs`/`ands` for a following `cmp #0`, fusing only where the condition code survives —
`cmp d,#0` sets C=1,V=0 by definition, so only codes reading N/Z alone carry, and `lt`/`ge` are
rewritten to `mi`/`pl`; `mir/pass/cmpelim.rs`), ✅ `ext_lattice` (forward known-width dataflow — ONE
pass replacing rc3's five text `sxtw` levers, `mir/pass/ext.rs`), ✅ `ldst_pair` (`ldp`/`stp`, runs
LAST after frame/legalize so slots have numbers and displacements are final, `mir/pass/ldstp.rs`);
✅ `auto_inc` (pre/post-index writeback, with the register-allocator TIE that makes the base and
its writeback one value, `mir/pass/autoinc.rs`). Post-allocation — ✅ block `layout` + branch
relaxation, ✅ jump tables, ✅ STRUCTURED peephole on MIR (never on text), ✅ `shrink_wrap`
(callee-saved save/restore off the fast path, `mir/pass/shrink_wrap.rs`).
gcc -O1 has no instruction scheduler and neither does this list.

**5 classic passes → theorem (all DECIDABLE):**
const-fold = rewrite-soundness · DCE = liveness · copy-prop = dominance + Leibniz ·
CSE = value-numbering · regalloc = rename-bisimulation over a CHORDAL graph.
(The loop passes DO restructure loops, yet stay decidable via explicit fences —
single-definition gates and induction on trip count, never a Rice-undecidable whole-loop
equivalence.)

### A8. Testing & proof methodology `[IN USE]`
Differential testing · Metamorphic (commuting-square) · Property/boundary-value ·
Structural exhaustion · UB filtering · 2-fact (PASS|NOT-IMPL|FAIL, gate = 0 FAIL) ·
Translation-validation-as-gate (`ir.sh`, planned) · Evidence-trail (clean input).

## B — BY PURE-MATHEMATICS BRANCH (reverse index)

- **B1. Discrete & graph theory:** automata/formal languages (A1–A3), directed graphs
  (CFG, dom-tree, interference), **chordal graphs and perfect elimination orders** (the SSA
  colouring theorem, A7 — the single graph-theoretic fact the whole backend rests on),
  trees (AST, expression tree, dominator tree), combinatorics/counting
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
| **variadic anon args go in registers** x0–x7/v0–v7 (standard AAPCS64, NOT darwinpcs stack-only), saved to a 192B reg-save area (128B VR + 64B GP) below the frame | — | §6.4 (R1.2) |
| **plain `char` is UNSIGNED** (explicit `signed char` = signed) | inverse of Darwin | AArch64 Linux / `parser.rs` |

Where it lives: the TABLE in `mir/isa.rs` (register file, classes, allocation order,
caller-saved set) and the AUTOMATON in `isel/abi.rs` (NGRN/NSRN/NSAA over a call's C
signature). The argument-offset algorithm still lives in **two places that must agree
byte-for-byte** — `isel/abi.rs` and the parser's `va_off` — so changing one means changing
both, plus running `gate abi`. (rc3 had a third, the codegen spill path; the parallel-copy
model of A5 removed it.)

### II-4. Object format — ELF / sections (AArch64 Linux)
| constant | value | source |
|---|---|---|
| sections | `.text`/`.rodata`/`.data`/`.bss` | System V ABI |
| symbol: **NO** underscore (unlike Darwin) | — | ELF |
| local relocation | `adrp`+`:lo12:` (PAGE/PAGEOFF) | AArch64 ELF |
| extern/GOT | `:got:`+`:got_lo12:` | ELF |
| TLS | `:tprel_*` / TLS model | ELF TLS |

Where it lives: **`emit.rs`** (sections, symbols, relocations — one file, because after the
re-architecture emission makes no decisions). (The former Darwin idiosyncrasies —
`_`, `@PAGE`, `@TLVPPAGE`, variadic-args-on-stack — were removed when Mach-O was
dropped; they are recorded in CLAUDE.md to avoid confusion.)

### II-5. Arch — AArch64 instruction/encoding constants
Register file (x0–x30, sp, v0–v31), immediate ranges (add/sub 12-bit, logical bitmask,
branch offset ±128MB), condition codes (eq/ne/lt…), addressing modes (`[base,#off]`,
`[base,index,lsl]`). Source: **ARM ARM (DDI 0487)**. Where it lives: **`mir/isa.rs`** — the register files, the
allocation orders, and the ENCODABILITY PREDICATES (`add_imm` imm12/imm12<<12, `logical_imm`
bitmask, `mov_chain` movz/movn/movk, `mem_off_ok` scaled-unsigned vs signed-9, `pair_off_ok`,
`fp_imm8`), each with a battery row in `mir/tests.rs` checking it against the manual case by
case. No decoder exists any more: rc3 needed one because its optimizations were text
peepholes that had to re-parse their own output; MIR passes read structure directly.
Two encoding facts worth naming, both found by that battery or by running real code:
a bitmask immediate is a ROTATED RUN OF ONES replicated across the register (0 and all-ones
are NOT encodable), and in the ADD/SUB **immediate** form register 31 is **SP, not ZR** —
`add w0, wzr, #5` is not an instruction, while the shifted-register and logical-immediate
forms do read 31 as ZR.

**Side-II arch constants the R2/R3 machine passes rest on** (each transcribed from the manual,
consumed at the named site; the register-width rule is the load-bearing one — three of the
R2.4/R3 wrong-code defects, §15c, ARE that one line):

| constant | value | source | consumed at |
|---|---|---|---|
| **every 32-bit (`w`-form) write ZEROES bits 63:32** | the upper half is cleared, not preserved — so `sxtb w0, w1` is sign-extended within the low 32 bits and zero above, and `mov w0, w0` is a TRUNCATION, not a no-op | **DDI 0487 B1.2.1** | `mir/pass/ext.rs` (the extension lattice's third field), `regalloc/destruct.rs` (self-move-as-truncation fixpoint), `isel/lower.rs` |
| `ldp`/`stp` displacement | SIGNED 7-bit field, SCALED by the element width (`imm7 · size`), naming the FIRST of two consecutive registers | DDI 0487 C6.2.130 | `mir/isa.rs::pair_off_ok`, `mir/pass/ldstp.rs` |
| conditional-branch reaches | `b.cc`/`cbz`/`cbnz` 19-bit signed = ±1 MB; `tbz`/`tbnz` 14-bit = ±32 KB; unconditional `b` 26-bit = ±128 MB. The assembler does NOT relax these — a far target needs a trampoline | DDI 0487 C6.2.26/C6.2.42/C6.2.375 | `mir/pass/layout.rs::relax_branches` |
| bit-field extract | `ubfx`/`sbfx dst, src, #lsb, #width` — a shifted mask (`and(lshr(a,s),mask)`, `shl+ashr`) in one instruction | DDI 0487 C6.2 | `isel/lower.rs` munch, `MInst::Bfx` |
| jump-table density rule | a `switch` becomes a table (`adrp`/`ldrsw`/`br` over signed 32-bit offsets) when it has ≥ 4 cases AND occupies ≥ half its span (span ≤ 4096); below that, a balanced compare tree | gcc default policy (dated constant) | `isel/lower.rs::jump_table` |

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
