# zcc IR — the design contract (stage 3)

> Design note. This document is an architectural proposal that precedes
> implementation. Stage 1 (removing Mach-O → ELF-only) and stage 2 (sealing the
> educational `u"`/`U"` surface) are closed and green (14/14). Because this is a
> major architectural decision, it is deliberated in written form before code is
> written rather than approached directly through code.

## 0. Why the IR exists (two rationales, NOT speed)

1. **A hard frontend/backend contract.** The current boundary is `ast.rs` (AST +
   TyTab), but the AST still carries C SOURCE semantics (declarators, lvalue-ness,
   scope) → the backend is forced to *understand C* in order to generate code.
   Adding a target then means rewriting a per-node walk: work of
   **O(target × construct)**. An IR cuts this into **O(construct)** [frontend→IR] +
   **O(target)** [IR→asm]. This is the precondition for a community member to add a
   target as a single file.
2. **Unlocking the space of theorems** that AST→asm conceals: dataflow analysis,
   fixpoint over a lattice, liveness, "optimization = semantics-preserving
   transformation". zcc claims that "each LOC maps to a theorem" → without an IR an
   entire region is missing.

**Optimization is DEFERRED.** The baseline is IR ONLY (contract + code organization +
extensibility), with NO passes. The rationale: the IR is "more academic + more about
organization + more about extensibility than optimization". The pass layer (§5) is
future work, outside the baseline. Correctness lives in lowering, NOT in the passes.

**The 10k ceiling is RETAINED.** Every LOC — every token — maps to a theorem; the whole
of `src/` is a few hundred theorems compiled into 10k of Rust. The LOC reality: one
target means the IR **adds** code (two lowering stages); `src/` at 8484 leaves ~1516 of
budget. Therefore **interp + verifier + ir.sh are TEST-SIDE** (proof-checkers, not
transformation logic ⇒ they live in `tests/`, off-ceiling). `src/ir.rs` carries only
**IR types + AST→IR lowering**; the IR→asm backend replaces the old AST→asm one (little
net addition — the backend no longer has to "understand C").

## 1. Position & new architectural invariants

```
lexer → parser → AST (ast.rs) ──lower──▶ IR (ir.rs) ──lower──▶ codegen/<target> → .s
                     │                      │                        │
                  shared TyTab ─────────────┴────────────────────────┘  (layout size/align)
```

- **New boundary = `src/ir.rs`.** The backend reads only IR + TyTab; it must NEVER read
  the AST/parser. The mechanical contract test: *a new backend can be written from the IR
  spec ALONE, without reading a single line of the frontend*.
- **TyTab is reused verbatim** (the type system is not duplicated). An IR value carries a
  `TypeId`.
- The invariant that replaces the 10k ceiling: **each pass is provable** (the 4-part
  statement in §6).
- `codegen/mod.rs` remains the single entry point; its signature changes to
  `emit(&Ir) -> String`.

## 2. IR shape — typed linear 3-address, NON-SSA (settled)

Rationale for non-SSA (see discussion): local const-fold/peephole plus linear-scan
regalloc do NOT require SSA; liveness over 3-address is a compact classical theorem. SSA
construction (dominance frontier, phi) + destruction (out-of-SSA: lost-copy/swap) is a
source of miscompiles and costly in LOC — reserved, opened only when a genuine global
optimization demands it (at -O0, educational, almost never).

```
Ir            = { funcs: Vec<IrFunc>, globals: …(borrowed from AST), strs, … }
IrFunc        = { name, params: Vec<(Vloc, TypeId)>, ret: TypeId,
                  temps: Vec<TypeId>,        // type table for all temps t0..tN
                  blocks: Vec<Block>,        // block[0] = entry
                  frame:  …(local offsets, kept from the parser) }
Block         = { id: BlockId, insts: Vec<Inst>, term: Term }   // term REQUIRED at the end
Val           = Temp(u32) | Imm(i64) | FImm(f64)                // 3-address value
Inst          = Bin(dst, Op, a, b)          // dst = a op b   (op carries type via dst)
              | Un (dst, Op, a)
              | Load(dst, addr, TypeId)      // read memory by the type's width
              | Store(addr, val, TypeId)
              | Addr(dst, Vloc)              // &local / &global / &param
              | Cast(dst, a, from→to)        // usual-arith / trunc / ext / f↔i
              | Call(dst?, callee, args, nfix)  // nfix = number of fixed args (variadic ABI)
              | Copy(dst, a)
Term          = Jmp(BlockId)
              | Br(cond, BlockId, BlockId)   // if cond
              | Ret(val?)
              | Switch(val, Vec<(i64,BlockId)>, default)
```

Characteristics: **flat** (no nested expressions — the parser has already lowered the
tree), **typed** (every Temp has a `TypeId`), **explicit control flow** (blocks +
terminators), **explicit memory** (Load/Store — no implicit lvalues). The backend is left
with only *per-inst instruction selection* + *regalloc* + *ABI* — it no longer understands
C.

The Op set (test-first): acquired when lowering an AST construct requires it; the starting
list = { add sub mul div mod, and or xor shl shr, cmp{eq ne lt le gt ge} (signed/unsigned),
neg not, fadd… fcmp… } — one op per match arm in interp and in each target.

### 2b. CORE vs. OPAQUE (settled after reading ast.rs — 40+ Nodes, a long exotic tail)

The AST has a tail that does NOT fit 3-address: `Sync` (atomics LL/SC), `Overflow`, `Asm`
(inline), `VaStart/VaArg/VaArea` (va_list AAPCS), `Alloca`/VLA, `SRet` (struct return),
`Tramp/Upvar/NlGoto` (nested functions), TLS. Forcing these into 3-address means bloat and
needless risk.

→ **The IR splits instructions into two classes:**
- **CORE** (Bin/Un/Load/Store/Lea/Cast/Call/Copy + terminators): typed 3-address, the
  interpreter can evaluate them, the verifier covers them, and **passes touch ONLY this
  class**.
- **OPAQUE** (`Inst::Op(...)` wrapping an exotic AST construct verbatim): lowered 1-to-1 to
  the backend EXACTLY as the current codegen handles it — passes do NOT touch it, and the
  interpreter dispatches to a dedicated handler (or marks it "impure, do not fold across").
  The correctness of the tail is preserved AS-IS, without refactoring.

This yields: (a) a minimal IR core, (b) optimization playing only on the core (safe), and
(c) an AST→IR migration that does not rewrite the exotic logic — only re-wraps it.

## 3. The contract = 3 invariant documents (this is the "bug-resistant IR standard")

No external format (QBE/LLVM) is taken as a dependency — the project is zero-crate and
built from theorems. It borrows the QBE PHILOSOPHY (typed, minimal, a short spec). The
"bug-resistance" of the standard comes from three formal artifacts:

### 3a. Verifier — a well-formedness automaton (run AFTER each pass)
Invariants (reject if violated, do not let garbage flow down to asm):
- **Typed**: every Temp is assigned the correct type; each op's type signature matches its
  operands.
- **Def-before-use** over the whole CFG (every path to a use passes through a def).
- **CFG well-formed**: every block ends in exactly one terminator; the target block exists;
  the entry has no predecessor branching into its middle.
- **No dangling**: Temp/Block/global references are valid.

### 3b. Interp — a reference evaluator (semantic ground truth)
Runs the IR directly (not via asm) → an observable result. Used as an INTERNAL oracle. The
**fully formalized semantics (LEVEL-1) is in `SEMANTICS.md`** — the state Σ=⟨ρ,μ⟩ plus the
mathematical definition ⟦·⟧ of every Inst/Term, mapped 1-to-1 onto `ir.rs::tests::interp`:

### 3c. Commuting square — every pass must COMMUTE with interp
```
   ir_before ──interp──▶ result
      │                    ‖
    pass                   ‖   (MUST be equal)
      ▼                    ‖
   ir_after  ──interp──▶ result
```
A pass is correct ⟺ it commutes with interp. This is the fold↔runtime commuting square
(already present in `alg.sh`) lifted to the IR level — it catches a bug RIGHT at the pass
that produced it, without waiting for end-to-end testing. **Lifted to an EXECUTABLE THEOREM
(LEVEL-1, done):** `opt.rs::commuting_square_structural_exhaustion` exhausts `𝔼_struct`
(312 expressions (5 shape families) × 5 passes = 1560 squares) proving ∀e commutation, plus
`commuting_square_selfproof` (anti-blindness). Statement: SEMANTICS §5.

## 4. Lowering (where correctness lives)

- **AST → IR** (`ir.rs`): translate the flattened AST tree per-node into blocks + insts.
  This, plus IR→asm, are the ONLY two places holding correctness. UAC/cast/va/HFA… are
  lowered here (the parser has already inserted `Node::Cast` → 1-to-1). Differential:
  interp(IR) agrees with running the current asm.
- **IR → asm** (per target): per-inst instruction selection + regalloc + ABI. The ABI
  automaton (the three-place-agreeing argument offset) moves ENTIRELY here — the frontend is
  no longer involved.

## 5. Pass layer (FUTURE — OUTSIDE the baseline; optimization is deferred)

> The baseline does not implement this section. It is retained as a design for when
> optimization is opened. The §5 invariant below is the safety condition for later enabling
> passes one at a time.

(A COVER LAYER that can be switched off — 70–90% of the risk is confined here.)

Safety invariant: **the IR lowers straight to CORRECT asm without any pass**. Turn all
passes off → the suite is still green (only slower / more stack). Proof-by-deletion, as with
ext.rs.

Order = INCREASING risk; each pass is closed (verifier + commuting square) before the next:
1. **regalloc** (nearly mandatory — naive stack-slot is correct but disastrous): linear-scan
   over live intervals. Theorem: interference graph / liveness dataflow.
2. **const-fold / peephole** (local, easy to verify): abstract interpretation over the
   constant lattice.
3. **DCE** (needs GLOBAL liveness, higher risk): the reachability theorem.

No pass is added "for elegance". A pass that cannot state §6 is not written.

## 6. The "each LOC a theorem" rule — made concrete

A pass may be written only when all 4 parts are stated:
1. **Input invariant** (IR well-formed before the pass).
2. **Rewrite rule** (what the transformation does).
3. **Preservation theorem** (why output ≡ input in observable behavior).
4. **Output invariant** (the verifier still passes afterward).

The UB filter is a root rule: an optimizer EXPLOITS UB → the generator filters UB FIRST; a
differential at a UB point is doubly meaningless once optimization is present.

## 7. Implementation plan (each step closed green before the next)

1. **ir.rs skeleton**: IR types + verifier + interp (interp/verifier under `#[cfg(test)]` or
   behind a debug flag ⇒ test-side, off-ceiling). Not yet wired to codegen. Test: interp runs
   a few hand-written IR programs (factorial, branch, load/store) matching expectations.
   ← CURRENT POSITION.
2. **AST → IR lowering** covering the current corpus. The ELF codegen reads IR instead of the
   AST. GATE: 14/14 suite green through the IR path (differential against the old asm baseline).
3. **Delete the old AST→asm path** (arm64_elf.rs becomes pure IR→asm). Measure LOC — must be
   ≤10k.
4. **STOP the baseline here** (optimization deferred). The §5 pass layer is opened only when
   explicitly enabled.

## 8. Risk / gate register

- Suspecting a pass is "correct" → the commuting-square oracle must print a verdict BEFORE any
  claim is made (the measure-before-speaking rule).
- Sci-gate extension: add `ir.sh` (verifier exhausting well-formedness + interp↔asm
  differential over the machine-generated IR space).
- Every green verdict carries a mechanical evidence trail (number of IR-funcs verified + number
  of commuting points matched), not merely pass/fail.

---
Open questions to settle before implementing §7.1: (a) does the IR get its own file `ir.rs` or
merge into `ast.rs`? (proposal: a separate file, a clean boundary). (b) what does interp return
as the "observable" — exit code + memory trace, or only the return value? (proposal: exit +
minimal syscall trace, to compare I/O). (c) regalloc linear-scan or graph-coloring? (proposal:
linear-scan, fewer LOC + sufficient for -O0).
