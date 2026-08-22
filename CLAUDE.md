# zcc — Project Charter

zcc is a strict-C99 C compiler (C89 ⊂ C99), written in Rust, zero external crates. Terminology stays English. This file is the constitution: the three laws, then the supporting articles — each an amendment with detail offloaded to its own document. Only the first three carry the word **law**; the rest are mechanisms.

> **`[ssa-qbe fork]` override — this branch only; `main` (zcc-slim SEAL) keeps the body verbatim.** The two-fact model is literal: `[THEORY.md, SEMANTICS.md]` (Side I) ⊕ `[iso/os/arch/gnu specs]` (Side II) **is** the source of zcc; `src/*.rs` is its *compiled object*. This fork **is** the optimizer. **FINAL GOAL: match `gcc -O1` — and that is the STOPPING POINT (điểm dừng): not O2, not O3.** No toy compiler has reached O1 parity; that is the finish line, after which the optimizer is DONE. The fork suspends two clauses of Article B — nothing else: "-O0 / no optimization pass" → SUSPENDED (the fork's purpose is `opt.rs` + `OPT.md`; every pass ships under the CbC gate of Law 3); **"LOC ceiling" → REMOVED entirely for this fork** — the hard LOC number is dropped (it caused doc/context churn with zero correctness value); minimal-LOC survives only as design taste, **never a tracked budget or number to re-count**. Laws 1–3, architecture, driver, extension, and the test gate remain in full force. Scoreboard + O1 gap: `OPT.md §1`.

## Law 1 — the decomposition theorem (Law Zero; if only one is kept, keep this. Full text: `THEORY.md`)

```
zcc source = ( math/theory → control-flow + data-structure + algorithm )   [Side I: THEORY.md Part I]
           ⊕ ( iso/os/arch/gcc spec → constant + param + value-table )      [Side II: THEORY.md Part II]
```

Every line of `src/` lies on exactly one side — an algorithm derived from a theorem, or a constant transcribed from a spec line (no magic number without provenance). **Correctness-by-construction:** if no line lies outside {theory-fact ∪ spec-fact} and each faithfully realizes its side, zcc necessarily passes every suite — zcc and the referee are both shadows of the same specification. Gödel/Rice/Halting lie *outside* the compiler↔suite relation: the referee is an *independent* oracle, so zcc never proves itself (the same reason the AI is kept out of the trust path).

## Law 2 — bug-fix by decomposition (a *direct corollary* of Law 1, by inverse proof)

`src/*.rs` is produced by nothing but `compile(theorems) ⊕ inject(ultimate-fact values)` (Law 1). Contrapositive: a defect in `src` can therefore lie in **only two places** — there is provably no third door:
1. **Side I** — a *misinterpreted theorem*: an algorithm/control-flow outside its theorem.
2. **Side II** — a *wrongly-injected value*: a spec-constant (offset, ABI value, layout, section) applied wrongly.

The **one exception**, rare and rarer still, is that the **measurement itself is wrong** — the oracle / referee / test / generator lied, and zcc was *innocent all along*. This is not a co-equal Side III; it is the case that proves zcc was never guilty, so it may be claimed **only after multi-angle proof** (several independent formulations converging). Reflexive blame of the test is exactly what conceals a real Side-I/II bug.

**Measure before speaking:** locate the line mechanically *first*, classify into I / II (/ the measurement exception) *after* — guessing the wrong side first is normal, keep measuring. No classification is asserted before a script has printed a verdict; the AI may only build and run the oracle, then stay silent until it speaks. (Recorded evidence + full presumption-of-guilt text: `tests/README.md`.)

## Law 3 — early-catch: certify at the middle, not at the binary

Not a corollary of Law 1 but an *idea of the Correctness-by-Construction approach itself*: since `src/` is compiled theorem-by-theorem, each **intermediate artifact carries the very theorem that certifies it** — so the question is always *"can this be proven here, before the final suite (csmith + yarpgen)?"*, and where the answer is yes it is proven **at the earliest layer where it becomes decidable** (IR / emitted `.s`), never deferred down to the final binary + suite. A theorem is not only a *compiler* of `src/`; it is a *prover*.
- **Correctness** — a pass `IR→IR'` ships with its commuting square `⟦IR⟧=⟦IR'⟧` (IR passes) or a machine translation-validation (backend passes). csmith/yarpgen only *confirm*, never *discover*.
- **Optimization** — a transform is applied the moment a *static* cost-measure on the before/after `.s` (instruction / move / memory-op count) shows a win; a win not statically evident is measured *in isolation on the exact case*, never shipped-then-suite-tested (the LICM ship-then-regress trap).

---

## Article A — the two supreme requirements (every decision reduces to these)

1. **Strict C99 compliance** — semantics exactly per spec; extensions (C11/vendor) only when real software demands, marked `EXT(...)`. Status of the remaining C99 items: `MILESTONES.md`.
2. **Minimal LOC** — no feature before a real `.c` demands it, no anticipatory abstraction, zero external crates; the compiler is the theorem and must stay readable. Ceiling + budget: `MILESTONES.md`.

When they conflict, **compliance wins over LOC.**

## Article B — architecture (invariant)

```
main.rs (driver) → lexer → parser → AST (arena + NodeId(u32)) → codegen/<target> → .s text
```
- **Frontend/backend boundary = `src/ast.rs`** (AST + TyTab). Frontend builds, backend only reads; no cross-import. Layout size/align live in TyTab (LP64 locked — parameterize TyTab, don't scatter conditionals).
- **One file per target** under `src/codegen/`; `codegen/mod.rs` is the single door (`emit(&Ast)->String`). ABI / section / asm syntax live entirely in the target file. `src/codegen/` is **ELF-only** (AArch64 Linux; x86_64 deferred; macOS is the clang oracle only).
- Single crate, no workspace.

## Article C — driver drop-in

`CC=zcc` slots into a *real* build system (configure/make/cmake) **without editing one build file**. The driver coordinates the host toolchain directly (`as`→`ld`, not through a `cc` driver). Flag surface acquired test-first: implement a flag when a real build uses it and swallowing would be wrong; swallow the rest silently — but **never** mis-swallow a flag that carries an argument (one misalignment consumes an input file). Standard `file:line:` diagnostics + correct exit codes (configure greps stderr).

## Article D — extension-decoupling (the ISO-C / vendor boundary must be visible)

Extension logic lives in **`src/ext.rs`**; the core only calls `ext_*`. Unfactorable touch-points carry `// EXT(gcc|clang|apple|c99)` — `grep 'EXT(' src/` must cover 100% of the deviation surface. Verified by excision: removing `ext.rs` + marked branches leaves a remainder that still passes the full C99 suite. Extension tests live in `tests/ext/` (referee `cc`, no `-std`), never in `tests/cases/`.

## Article E — test & proof mechanisms (not laws — the gate that anchors Laws 1–3)

- **Differential referee** — every correctness verdict is differential: referee is `cc` or an independent oracle; a diff at a UB/unspecified point is meaningless (the generator must filter UB first, proven by spec + referee, never hand-waved).
- **Iteration-speed** — an iteration mechanism, however elegant, is discarded the moment measurement shows it *slower* than the direct loop (detect → fix → re-test exactly the failing case). Full suite runs once at the end, in the background.
- **Science-gate** (theorem-verification tier, above the corpus) — `abi.sh` / `alg.sh` / `cpp.sh` / `shape.sh` / `decay.sh` exhaust the *structural* space + boundary value-samples; to be *expanded, never contracted*. Runner `fullsuite.sh [TARGET] [SEEK]`, 100% in-box.
- **Clean-input** — a green verdict is valid only with a mechanical evidence trail (N binaries + bytes + exit codes), never a bare pass/fail number. Abnormal timing is measured, not guessed.
- **Numeric-provenance** — every number derives from a stated premise.
- **Resource-fidelity** `[fork]` (the dual of the commuting-square, for *performance*-theorems) — the commuting-square certifies a pass is *correctness*-faithful (`⟦f⟧=⟦f'⟧`); this gate certifies it is *realization*-faithful. A performance-pass (allocation, LICM, strength-reduction, scheduling) must declare (a) the hardware ultimate-fact it exploits, as a **spec citation** (e.g. AAPCS64 §6.1.1 register table), and (b) that it is instantiated over the **full** fact, not a convenient truncation. Every resource-constant (`k`, spill-threshold, issue-width) is either the **spec's number** or carries a dated justification for the gap. The mandatory question for each: **"is this the spec's number, or my convenience's number?"** — a truncation posing as a Side-II constant is a **Law-1 violation** (algorithm not faithfully realizing its side) catchable as a **Law-2 Side-II defect**, *not* a missing "improvement law": improvement stays inside Law 1's "faithfully realizes" clause, measured against the full ultimate-fact. (This is Law 3's "certify at the middle" extended from the correctness-theorem to the cost-theorem. Worked example — `GP_BUDGET.k=10` vs AAPCS64's ~18 leaf-usable GPRs — in `OPT.md`.)
- Full text of the correctness five + recorded traps: **`tests/README.md`**. Argument-offset lives in three byte-identical places (codegen call, codegen spill, parser va_off) — edit all three, then run `abi.sh`.

## Article F — ABI

AArch64-ELF (Linux) specifics — no leading `_`, `adrp`/`:got:`/TLS relocations, variadic-in-registers + 192B reg-save area, `char` **unsigned**, AAPCS64 register table, sections — are Side-II constants: **`THEORY.md` II-3 (AAPCS64) + II-4 (ELF/relocations/sections)**. Mistakes here produce cryptic crashes; read before touching codegen.

## Index

- **`THEORY.md`** — the two-side catalog (Part I theorems / Part II spec-tables); answers "what foundation does zcc rest on". Adding a theorem or constant updates it.
- **`SEMANTICS.md`** — the reference operational semantics (the executable meaning behind `⟦·⟧`).
- **`MILESTONES.md`** — milestone ladder, LOC budget, C99-remaining, debt ledger.
- **`tests/README.md`** — test-asset register, full test-mechanism text, baseline + traps.
- **`OPT.md`** `[fork]` — the single, transient optimization working-doc (scoreboard · one-theorem · done-ledger · next-gate · catalog · IR contract + opt.rs audit). Deleted at opt-end; durable facts cook into `THEORY.md`/`SEMANTICS.md`.
- **`src/ext.rs` + `grep 'EXT(' src/`** — the entire current deviation surface.
