# zcc IR — Reference Semantics ⟦·⟧

**Status.** This document is a *mechanized reference semantics*: a formal
denotational semantics for the CORE intermediate representation, realized by the
interpreter `src/ir.rs::tests::interp` and validated by structural exhaustion in
`src/opt.rs::commuting_square_structural_exhaustion`. It is **not** a
machine-checked proof. By Rice's theorem, semantic equivalence of programs is
undecidable in general, so every theorem stated below is quantified over a
*finite class of program shapes* (a decidable fragment) and is *checked
mechanically*, not proved universally. This artifact is the foundation on which
translation validation and per-pass machine-checked proofs are intended to build.

This document is the *mathematical definition of every `Inst`*. It is a
*specification of the code*, not an aspiration: each rule maps one-to-one onto an
arm of `ir.rs::tests::interp`, or onto one of the atomic semantic functions
(`eval_bin` / `eval_cast` / `canon`) that live in the **non-test** part of
`ir.rs` and are shared with the constant folder — establishing *faithfulness*
(the folder and the interpreter are one and the same denotation function).

See also: `IR.md` §3b/§3c (the IR contract), `THEORY.md` §A7 (denotational
semantics), and `tests/alg.sh` (the source-level fold-vs-runtime commuting
square that this document lifts to the IR level).

---

## 1. Value domain

A *machine value* `Val` is a canonical 64-bit word, matching the "canonical
register" contract of `ast.rs`:

```
𝕍 = { canon_τ(z) : z ∈ ℤ }        for an integer type τ (width w = size(τ)·8, signedness s)
   ∪ { bits(x)   : x ∈ 𝔽₆₄ }      for a floating type (stored as the f64 BIT PATTERN;
                                     32-bit float is widened to f64 in registers)
```

- **Integers.** A value lives in ℤ/2^w with signedness s. `canon_τ` (see
  `ir.rs::canon`) normalizes any `z ∈ ℤ` to its canonical representative: mask
  the low w bits, then sign-extend when s is signed. This is exactly the
  backend's register normalization — "integer arithmetic wraps at w bits"
  (two's-complement is arithmetic modulo 2^w).
- **Floats.** A value is the IEEE-754 f64 bit pattern. A 32-bit C `float` is
  widened to f64 when loaded into a register (`Load` with size 4 performs
  `f32→f64`) and narrowed when stored (`Store` with size 4). The bit pattern is
  preserved verbatim; floating-point addition is not associative, so folds must
  not reassociate float operands.

A `TypeId τ` carries the *algebraic structure*: an `Op` is a pure symbol, while
the interpretation — ℤ/2^w (integer, with signedness) versus ℝ (approximated by
f64, floating) — is determined by τ. This separates "operation" from
"structure".

---

## 2. Machine state Σ

```
Σ  =  ⟨ ρ , μ ⟩
ρ  :  Tmp → 𝕍              register file (ρ[t] is the canonical value of temporary t)
μ  :  [0, frame) → Byte    flat local memory (a byte array the size of the stack frame)
```

**Memory model** (see `interp`). Only the local frame is modeled. A local
address is `x29 − off`; flat-memory index 0 corresponds to `x29 − frame`, hence
`index(off) = frame − off` (`Lea Local`). `load_mem` / `store_mem` perform
little-endian byte serialization (LP64, AArch64 little-endian). Global and
string addresses are **not** modeled and evaluate to ⊥ (§4): a function that
touches them lies outside the CORE space.

**Parameter seeding.** The i-th parameter `(off, τ)` is seeded as
`canon_τ(argᵢ)`, written into `μ` at `index(off)` with width `size(τ)`. There are
no parameter temporaries: the body reads every variable (parameters included)
through `Var→Load`, giving a uniform unoptimized (-O0) model.

**Observable.** The observable is the return value — `⟦Func⟧(args) ∈ 𝕍`. (I/O
traces are omitted: CORE is pure computation; a function with external
side effects, such as a call to an external symbol or inline assembly, is
exotic and evaluates to ⊥.)

---

## 3. Atomic denotations — the faithfulness keystone

The three functions below live in the **non-test** part of `ir.rs` and are
called by *both* the interpreter (semantics side) and the constant folder
(`opt.rs`, release side). A single definition guarantees that the folder and the
interpreter cannot diverge: `⟦fold(e)⟧ = ⟦e⟧` holds *by construction*
(term-rewriting soundness).

### 3.1 `canon_τ : ℤ → 𝕍`  (`ir.rs::canon`)
```
canon_τ(v) = v                          if float(τ) ∨ size(τ) ≥ 8
           = sext_w( v mod 2^w )         if τ is a signed integer,   w = size(τ)·8
           = ( v mod 2^w )               if τ is an unsigned integer
```

### 3.2 `⟦op⟧_τ : 𝕍 × 𝕍 → 𝕍 ∪ {⊥}`  (`ir.rs::eval_bin`)
- **float(τ):** decode bits to f64, apply op ∈ {+, −, ×, ÷} in 𝔽₆₄ (IEEE-754);
  comparisons yield {0, 1}. (Float ÷ is not ⊥: it follows IEEE-754 for
  ±∞ / NaN.)
- **int(τ):** arithmetic in ℤ/2^w with signedness s, canonicalized by `canon_τ`:
  - `+, −, ×` are wrapping (modulo 2^w).
  - `÷, %` : **`y = 0 ⟹ ⊥`** (undefined behavior — the folder must decline to
    fold, leaving the instruction for the target). Otherwise division truncates
    toward zero (signed `wrapping_div`; unsigned via `u64`).
  - `& | ^` are bitwise; `<<` is `wrapping_shl`; `>>` is arithmetic (signed) or
    logical (unsigned).
  - `== != < ≤ > ≥` yield {0, 1}, compared according to s.

### 3.3 `⟦cast⟧_{σ→τ} : 𝕍 → 𝕍`  (`ir.rs::eval_cast`), per C99 6.3.1.2 / 6.3.1.4
```
int→int    : _Bool ⟹ (v ≠ 0);   otherwise canon_τ(v)        (truncate / extend)
int→float  : (float)v            (unsigned uses u64→f64)
float→int  : _Bool ⟹ (f ≠ 0);   otherwise canon_τ(⌊f⌋)      (truncate toward zero)
float→float: v                   (f64 is canonical for both)
```

---

## 4. Instruction semantics ⟦Inst⟧ : Σ → Σ  (CORE, big-step)

This mirrors the `match inst` in `interp`. Write `⟨v⟩ρ` for the fetch
`Tmp t ↦ ρ[t]`, `Imm x ↦ x`, `FImm b ↦ b`, and `ρ[d ↦ u]` for register update.

| Inst | ⟦·⟧ : Σ → Σ |
|---|---|
| `Bin(d,op,τ,a,b)` | `ρ' = ρ[d ↦ ⟦op⟧_τ(⟨a⟩ρ, ⟨b⟩ρ)]`   (⊥ if op is ⊥) |
| `Un(d,⊝,τ,a)` | `ρ[d ↦ canon_τ(−⟨a⟩)]` (Neg int) / `bits(−f)` (Neg float) / `canon_τ(¬⟨a⟩)` (BNot) |
| `Copy(d,τ,a)` | `ρ[d ↦ canon_τ(⟨a⟩ρ)]` |
| `Load(d,τ,a)` | `ρ[d ↦ decode_τ(μ, ⟨a⟩ρ)]`   (read size(τ) bytes; f32→f64 when float, size 4) |
| `Store(τ,a,v)` | `μ' = μ[⟨a⟩ρ ↦ encode_τ(⟨v⟩ρ)]`   (write size(τ) bytes; f64→f32 when size 4) |
| `Memcpy(d,s,n)` | `μ' = μ[⟨d⟩ρ ..+n ↦ μ(⟨s⟩ρ ..+n)]`   (copy n bytes forward; struct assignment, C99 6.5.16) |
| `Lea(d, Local off)` | `ρ[d ↦ frame − off]`   (`Global` / `Str` ↦ ⊥) |
| `Cast(d,σ,τ,a)` | `ρ[d ↦ ⟦cast⟧_{σ→τ}(⟨a⟩ρ)]` |
| `Call(Some d, Sym g, ā, _)` | `ρ[d ↦ canon_{τd}( ⟦g⟧(⟨ā⟩ρ) )]`   (recursive big-step; `Ptr` / depth > 500 ↦ ⊥) |
| `Phi(d,τ,[(bᵢ,vᵢ)])` `[ssa-qbe]` | `ρ[d ↦ canon_τ(⟨v_k⟩ρ)]` where `b_k = π` (the predecessor edge just taken); no arm for `π`, or `π` undefined at entry ↦ ⊥ |

> **φ and the predecessor π.** The φ-node (SSA form, present only between `to_ssa`
> and `out_of_ssa`) extends Σ with an auxiliary `π ∈ BlockId ⊎ {⊥}` — the block
> the current edge came from — threaded by §4b (each `goto` sets `π :=` the block
> being left). φ-nodes are *parallel* at a join, but SSA freshness (a φ dst is a
> new temp never named by a sibling φ arm) makes left-to-right evaluation over `ρ`
> faithful. This is the `Inst::Phi` arm of `interp` and the `prev` variable.
>
> **`to_ssa` (Stage 2, `opt::to_ssa`, Braun 2013).** These φ are now *produced* by
> mem2reg: a local is PROMOTABLE ⟺ scalar (int/float/pointer, LP64) ∧ type-consistent
> ∧ not a parameter (params live in ABI-seeded frame slots) ∧ not address-taken (every
> `Lea` of it feeds only Load/Store addresses — no escape). A promotable local's
> `Store` becomes `writeVariable`, its `Load` a `Copy` of `readVariable`, its `Lea`
> is dropped, and joins get φ; everything else stays in memory. The transform carries
> no new denotation — its whole content is the theorem **`⟦f⟧ = ⟦to_ssa(f)⟧`** (§4
> semantics unchanged), gated mechanically by `equiv`, never trusted.
>
> **`out_of_ssa` (Stage 3, `opt::out_of_ssa`, φ-destruction).** The inverse: a φ has
> no machine form, so before the backend runs each `Phi(d,τ,[(bᵢ,vᵢ)])` becomes an
> explicit `Copy(d,τ,vᵢ)` on the control edge from `bᵢ`. This makes the auxiliary `π`
> unnecessary — `interp` reads `d` straight from `ρ`, the value the taken edge deposited.
> Two classic miscompiles are handled by construction: (1) a *critical edge* (the
> predecessor `bᵢ` has ≥2 successors and the φ-block has ≥2 preds) is SPLIT — a fresh
> block on the edge holds the copies, so they never leak onto `bᵢ`'s other edge;
> (2) the *swap / lost-copy* problem — φ at a join are PARALLEL, so on one edge the
> copy set `{d ← v}` may be mutually referential (`{a←b, b←a}`); `seq_pcopy`
> (Boissinot sequentialization) emits a leaf whose dst no pending copy reads, and
> breaks a residual cycle by saving one value to a fresh temp. Its whole content is
> the theorem **`⟦to_ssa(f)⟧ = ⟦out_of_ssa(to_ssa(f))⟧`**, gated by `equiv`. The
> result is φ-free (backend-consumable) but no longer single-assignment.
>
> **`sccp` (Stage 4, `opt::sccp`, Wegman–Zadeck).** An SSA pass (between `to_ssa` and
> `out_of_ssa`). A per-temp lattice `⊤ ⊒ Const(c) ⊒ ⊥` and a CFG-reachability set are
> raised together in one monotone fixpoint: a temp is `Const(c)` only if it is `c` on
> every REACHABLE path, and a `Br` on a proven constant marks only the taken edge
> reachable — so a φ merging (reachable-const, dead-arm) collapses to the constant,
> which plain const-folding cannot see. The transfer function reuses interp's own
> `eval_bin/eval_cast/canon` (faithfulness) and declines div/rem-by-0 (→ `⊥`, keeping
> the instruction). It carries no new denotation — its content is **`⟦f⟧ = ⟦sccp(f)⟧`**,
> gated by `equiv`. Uses of a `Const` temp become `Imm`; a constant `Br` becomes `Jmp`
> (a later DCE reclaims the pruned block).
>
> **`gvn` (Stage 4, `opt::gvn`, dominator-based value numbering).** The SSA-global
> generalization of block-local `cse` (§ Pass 4). A pure `(op, τ, operand-value-numbers)`
> is a value number; in SSA a temp has ONE definition, so its value is invariant along
> any path — hence two instructions with the same value number compute the same value.
> A redundant one is replaced by a `Copy` of the earlier temp ONLY when that temp's
> defining block DOMINATES the use (`dominators`, the Allen–Cocke iterative fixpoint),
> so the value is available on every path reaching here. Restricted to arithmetic
> (Bin/Un/Cast/Lea-Local); Loads keep block-local `cse` (cross-block load reuse needs
> memory-availability analysis, omitted). Content: **`⟦f⟧ = ⟦gvn(f)⟧`** for f in SSA
> form, gated by `equiv`.

**Exotic instructions (⊥ — impure, outside the CORE space):** `FunAddr`,
`LabelAddr`, `Zero`, `VaStart`, `VaArg`, `Overflow`, `VaArea`, `GotoPtr`,
`Alloca`, `CallX`, `Sync`, `Asm`. The interpreter returns an error, meaning the
input has reached a function containing an exotic instruction — an impure
function — so the commuting square skips it (as it does for undefined behavior).
This is the CORE / EXOTIC-typed partition of the IR (IR.md §2b): passes touch
only CORE, so only ⟦·⟧ over CORE is needed to establish that a pass commutes.

## 4b. Terminator semantics ⟦Term⟧ : Σ → (BlockId ⊎ Halt)  (mirrors `match term`)

```
⟦Jmp b⟧          =  goto b   (π := this block)
⟦Br c b_t b_e⟧   =  goto (⟨c⟩ρ ≠ 0 ? b_t : b_e)   (π := this block)
⟦Ret v?⟧         =  Halt(⟨v⟩ρ)    (Halt(0) when None)
⟦Unreachable⟧    =  ⊥             (reaching it means malformed IR, or genuinely unreachable dead code)
```
`π` (the predecessor block) is set by every taken edge and read only by `Phi` (§4).

## 4c. Function big-step ⟦Func⟧ : 𝕍* → 𝕍 ∪ {⊥}

```
⟦f⟧(ā) evaluates from block 0 with Σ₀ = ⟨ρ = 0̄, μ = seed(ā)⟩; it runs each ⟦inst⟧
       in order within a block, then ⟦term⟧ selects the next block; it halts at
       Halt(v), yielding v. Two safety bounds: a step budget (non-termination ↦ ⊥)
       and a call depth ≤ 500 (host-stack recursion ↦ ⊥).
```

Here ⊥ (an interpreter error) means "the input lies outside the modeled space":
undefined behavior (division by zero), an exotic instruction, a global address,
recursion deeper than the bound, or exceeding the step budget. A difference at ⊥
is meaningless, so the commuting square skips it.

---

## 5. The commuting-square theorem (executable)

`alg.sh` establishes the fold-vs-runtime commuting square at the **source**
level (it diffs two binaries, produced by the system compiler and by zcc, over
an exhaustively enumerated algebraic space). This document lifts that square to
the **IR + reference semantics** level (in-process, dependency-free, without the
system compiler):

> **Theorem (metamorphic / structurally-exhaustive translation validation).**
> For every pass `P ∈ {const_fold, copy_prop, cse, dce, optimize}`, every
> expression `e` in the generated structural space `𝔼_struct`, and every input
> `i` in the battery:
> $$ ⟦lower(e)⟧(i) \ne ⊥ \;\Longrightarrow\; ⟦P(lower(e))⟧(i) = ⟦lower(e)⟧(i). $$

Equivalently, the following square commutes for every `e ∈ 𝔼_struct`:

```
      lower(e) ───────⟦·⟧──────▶  v
         │                        ‖
         P                        ‖   (equal for every i with ⟦·⟧ ≠ ⊥)
         ▼                        ‖
     P(lower(e)) ────⟦·⟧──────▶  v
```

**Mechanical check:** `opt.rs::tests::commuting_square_structural_exhaustion`.
`𝔼_struct` is the union of **five shape families**, each exhausting the operator
set over a distinct structure, so together they cover every kind of `Inst`
(Bin/Un/Copy/Load/Store/Lea/Cast) and both terminators (Jmp/Br):

| family | shape | size | passes / Inst exercised |
|---|---|---|---|
| A | straight-line arithmetic (`POOL³`) | 216 | fold + CSE + copy-prop + DCE, Bin |
| B | div / mod (`POOL × {/,%}`) | 12 | symmetric UB skip; folder declines div-by-zero |
| C | shift (`POOL × {<<,>>}`) | 12 | Shl / Shr (arithmetic `>>`) |
| D | pointer / memory (`POOL²`) | 36 | Lea / Load / Store, **CSE memory-kill** (GCC PR84169) |
| E | loop / CFG (`POOL²`) | 36 | Br / Jmp back-edge, copy-prop / DCE across blocks |

Total: **312 expressions × 5 passes = 1560 commuting squares**, all green. Here
⟦·⟧ is `interp`, and equivalence is checked over the `battery` (small-domain
exhaustion [−6, 6]ⁿ plus the INT_MAX / INT_MIN boundaries; see `ir.rs::battery`).
Family E bounds its trip count with `b & 7`, so the interpreter always
terminates (the check is non-vacuous). A mechanical evidence trail (the
expression count and the square count) is asserted exactly, forbidding a vacuous
"passing" run.

**Anti-blindness.** `commuting_square_selfproof` injects a mutation (deleting a
`Store`, removing a memory write) and requires the commuting square to catch it;
if the equivalence check were blind, every verdict would be worthless.

---

## 6. Limitations and the path forward

- ⟦·⟧ models local memory and the return value only; it does not model globals,
  the heap, I/O, or concurrency, so it can establish preservation only for pure
  CORE functions. This suffices for the five current passes (all CORE) but not
  for interprocedural optimization or global alias analysis.
- Exhausting `𝔼_struct` is *finite structural coverage*, not universal: it
  catches defects on the generated shapes but does not prove correctness for all
  programs (Rice's theorem). The reference semantics of §1–§4 is the object to
  be formalized for the next stages — translation validation (a per-compilation
  certificate and checker) and per-pass machine-checked proofs — which is why it
  is stated explicitly and mapped one-to-one onto the code here.
