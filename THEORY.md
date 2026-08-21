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
| SSA construction (Braun 2013, on-the-fly, no dominance frontier) | `⟦f⟧ = ⟦to_ssa(f)⟧` (semantics-preserving promotion of non-address-taken locals) | **DONE (Stage 2)** — `opt::to_ssa`; proven by `to_ssa_semantics_preserved` (312 exprs × equiv), `to_ssa_diamond_and_loop`, `to_ssa_respects_address_taken`, `to_ssa_gate_has_teeth` |
| out-of-SSA / φ-destruction (parallel-copy, swap/lost-copy handled) | `⟦to_ssa(f)⟧ = ⟦out_of_ssa(to_ssa(f))⟧` | PLANNED (Stage 3) |
| SCCP (Wegman–Zadeck sparse conditional constant prop) | lattice ⊤/const/⊥ over CFG-reachability; `⟦f⟧=⟦sccp(f)⟧` | PLANNED (Stage 4) |
| wired register allocation (consume Chaitin coloring in the backend) | interference-invariant bisimulation + `⟦·⟧` unchanged | PLANNED (Stage 5) |

**5 passes → theorem (all DECIDABLE; no loop restructuring → outside Rice):**
const-fold = rewrite-soundness · DCE = liveness · copy-prop = dominance + Leibniz ·
CSE = value-numbering · regalloc = rename-bisimulation.

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
