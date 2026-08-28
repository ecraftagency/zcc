# zcc — Project Charter

zcc is a strict-C99 C compiler (C89 ⊂ C99), written in Rust, zero external crates. Terminology stays English. This file holds the project's standing rules: the laws first, then the articles that support them, each with its detail offloaded to its own document. Only Laws 0–3 and **Law 3c** carry the word **law**; the rest are mechanisms.

**THIS FILE CARRIES NOTHING THAT CHANGES ACROSS A PHASE OR A MILESTONE.** That is the whole test, and it is a test about the line's LIFETIME, not its subject: if a milestone can make the line wrong, it belongs in a document the Index names, never here. Measurements, ratios, statuses, commits, tags, branch names, plan positions and date-stamped findings all fail it by construction — every one of them is true only on the day it was written, and a stale rule is worse than no rule because it is still obeyed. What stays is what remains true whatever the numbers do. A line that would need editing after a good session does not belong here.

> **⚠️ AWS OPERATIONAL SAFETY only operation on **`us-east-2` region

> **⚠️ EVERY SCRIPT RUNS DETACHED, NOTIFIED ABLE, PROGRESS CHECKABLE, ON EITHER BOX — and NOTHING WATCHES IT. The completion notice is the signal; a second process that sleeps and greps competes with the run it is watching and quietly corrupts every timing taken beside it. To look in mid-run, read the output file once.

## Law 0 — PURITY IS THE PRECONDITION (standing order, 2026-08-26)

THE ULTIMATUM names 1× against gcc-O1 on both axes as the STOPPING POINT.
This says what may not be spent to reach it:

```
purity  ≫  exec  >  size  >  compile speed
```

**No number is banked at the cost of a citation.** A row that would reach parity
by removing a proof does not ship, however large the number. Laws 1 and 3 are
claims ABOUT THE SOURCE — every line in theory ∪ fact, every pass carrying its
commuting square — and `tests/provenance.sh` checks both in the sci gate.
`PURITY.md` is the plan of record for that work; `MEASURED.md` holds the facts
that have no spec to cite, so `THEORY.md` II-* stays cited-spec and nothing else.

zcc is an EDUCATIONAL compiler and a community project. A citation is therefore
a READING PATH — a student who lands on any line should be able to read upward
to the theorem it realizes — not a lint marker. Write it for a person.

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
- **Optimization (cost-square)** — cost is proven the way correctness is: at the **theorem layer, in the theorem-programming-language**, not on the compiled artifact (Rust `.s`) via patch→build→suite. The instruction-count of a function is a *catamorphism over the IR*, `cost(f) = Σ_inst emit_len(inst, alloc)`, written **independently** from the lowering theorems (each `Inst` → its machine-insn expansion); the **cost-square** `cost(f) ≡ len(codegen(f))` — proven per-function over the whole corpus — certifies that the Rust in the backend *faithfully realizes* that cost-theorem. `[mir-rearch]`: this square is now EXACT BY CONSTRUCTION rather than a separate model — one `MInst` is one machine instruction after frame/layout, so `cost(f) = |MIR_final(f)|` and `emit.rs` expands nothing (the single exception, `MovImm`, reports its chain length via `isa::mov_chain().len()`). This is the exact dual of the correctness commuting square: there `⟦f⟧=⟦opt(f)⟧`, here `cost=len∘codegen`. A mismatch is a **Law-2 defect**, localized to the offending `Inst` — either code drifted from its lowering theorem (Side I) or the cost-theorem was mis-transcribed — *never* a mystery to be grepped out of `.s`. Once the square holds, a transform's Δinsn is computed **on the model, before any build** (predict → apply → the model confirms); patch-Rust-then-run-the-suite is the slow path of last resort, reserved for effects the cost-model provably cannot see. `.s` and the suite *confirm*, never *discover* (the LICM ship-then-regress trap).
- **Exhaustion (vắt kiệt — Law-4 folded in, a corollary not a fourth law)** — a theorem is not "done" at its first positive result, however large; **cấm dừng ở green đầu tiên**. For each shipped theorem T, its **residual** — the multiset of sites where T *could* fire but did not — is measured on the cost-model, and every residual case classified: (a) a *fundamental limit* (a real ISA/ABI encoding boundary, proven — e.g. an offset genuinely beyond imm12 range), or (b) a *convenience truncation* (an incomplete or gated realization). T is **exhausted** only when residual = (a) entirely. This is the **coverage-dual** of Article E's constant-fidelity question: there it is *"the spec's number, or my convenience's number?"* (the value of a constant); here it is **"have I PROVEN this theorem exhausted, or did I stop at the first green?"** (the coverage of a transform). Stopping at a small positive while (b)-cases remain is a **Law-1 violation** — the algorithm does not *faithfully realize* its side over the full ultimate-fact — catchable as a Law-2 Side-I defect. Improvement therefore stays *inside* Law 1's "faithfully realizes" clause, now made mechanical and deterministic by the cost-model, with no separate "improvement law" needed.


## Law 3c — COUNT IS NOT COST (the TIME dual; performance stands immediately behind correctness)

A law, not a mechanism: correctness is first, and **performance is the thing
directly behind it**. Law 3 certifies a pass at the middle on two axes — meaning
(`⟦f⟧ = ⟦opt f⟧`) and SIZE (`cost(f) = |MIR(f)|`, exact by construction on this
branch). That second square is exact for size **and blind to time by the same
construction**, because one `MInst` is one machine instruction. So it needs its
dual:

```
size:   cost(f)   = |MIR(f)|                    proven per function
time:   cycles(L) = critical-recurrence(L)      proven per loop
```

> **Fewest instructions is not fastest code. A code-generation row is judged by
> the longest dependence chain it leaves, not by how many instructions it emits.**
> Where the two models disagree, TIME wins (Law 0: `exec > size`).

**The operative rule.** Never leave a multi-cycle operation in front of an
address or a loop-carried value when a one-cycle operation computes the same
thing: `madd` on a strided address → `add`; `add …, w, sxtw` → `ldrsw` + `add`;
`mul` by a constant → shift-and-add.

**Measured, not asserted.** The law was not reasoned into this file; it
was forced by a kernel that reached parity at an instruction count that did not
change at all, because a multiply had stood at the head of a chain ending in a
strided load. `MEASURED.md` holds that case and the latency table it rests on,
and `REARCH.md` holds the derivation and the row that builds the model.

**Law-4 dual (exhaustion).** A row is exhausted only when no remaining site
trades chain length for instruction count. A residual measured on `cost = |MIR|`
alone cannot see this class, so it does not discharge Law 4 for a codegen row.

**WHAT MAY BE CLAIMED, and it is not what the number says.** Parity against
gcc -O1 is the stopping point, and a measured parity on the suite of the day
does NOT establish it. The suite is always narrower than the language: whole
classes go unsampled — heavy floating point, working sets past cache, deep call
graphs and indirect dispatch, varargs and bitfields — and it runs on ONE
microarchitecture at one input size per program. A geomean over it is evidence
about ITS members first.

Therefore parity is claimed only with a **margin**, and even then the claim
names the suite and the core it was taken on — never "matches gcc -O1" plain.
The margin is not padding; it is coverage insurance, the amount by which the
sampled fraction must win before the unsampled remainder is unlikely to flip the
sign. Compiler comparison is not a single number, and a compiler that announces
parity from a narrow suite has mis-stated its own result, which is a Law-0
failure (a claim bought at the cost of its provenance) rather than a small one.

The surface is to be WIDENED, and the two instruments do
different jobs: **csmith finds CLIFFS** (run many, report only the tail where
zcc/gcc exceeds a threshold; it is a discovery engine pointed at time instead of
correctness, and it is already in the gate), while **real programs carry the
geomean** (csmith's control flow and global-memory traffic are nothing like real
software, so a geomean over it would be a number about csmith).

---

## Article A — the two supreme requirements (every decision reduces to these)

1. **Strict C99 compliance** — semantics exactly per spec; extensions (C11/vendor) only when real software demands, marked `EXT(...)`. Status of the remaining C99 items: `MILESTONES.md`.
2. **Minimal LOC** — no feature before a real `.c` demands it, no anticipatory abstraction, zero external crates; the compiler is the theorem and must stay readable. Ceiling + budget: `MILESTONES.md`.

When they conflict, **compliance wins over LOC.**

## Article B — architecture (invariant)

```
main.rs (driver) → lexer → parser → AST (arena + NodeId(u32)) → compile.rs (the single door)
                 → HIR (target-independent SSA) → isel → MIR (machine, SSA) → regalloc
                 → MIR (physical) → frame/layout → emit.rs → .s text
```
- **Frontend/backend boundary = `src/ast.rs`** (AST + TyTab). Frontend builds, backend only reads; no cross-import. Layout size/align live in TyTab (LP64 locked — parameterize TyTab, don't scatter conditionals).
- **`src/compile.rs` is the single door** (`compile(&Ast) -> String`); every layer below it is private to the pipeline. Layer map and the rationale for each seam: `REARCH.md` §2.
- **Layered, not one module per target** `[mir-rearch]`. The old rule ("one module per target under `src/codegen/`") was written when the backend was a single AST→asm emitter; it does not survive a pipeline whose seams are LAYERS, and `src/codegen/` no longer exists. The invariant it protected does survive, restated: **all target knowledge — ABI, register file, encodability, sections, asm syntax — lives in `src/mir/isa.rs` (Side-II tables), `src/isel/abi.rs` (the AAPCS64 automaton) and `src/emit.rs` (sections/relocations)**, and nothing above `isel` may name a machine register. HIR is target-independent by construction (closed scalar `Ty`, no TyTab lookup); MIR is AArch64-specific by design, and a second target adds a second MIR + isel, not a conditional. **ELF-only** (AArch64 Linux; x86_64 deferred; macOS is the clang oracle only).
- **A pass is a pass, never a text peephole.** A machine optimization is an `MIR→MIR` pass shipping its commuting square; `emit.rs` makes no decisions and re-parses nothing. This is the rc3 defect written into the architecture so it cannot recur.
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
- **Byte-identical gate** — pure code motion (file splits, helper extraction, renames) is proven by `md5(.s)` held unchanged: identical bytes *are* the commuting-square `⟦f⟧=⟦refactor f⟧`, confirming not discovering. It takes TWO witnesses, because a corpus of small programs cannot see a pass whose behaviour scales with the size of a function — one large translation unit is compiled alongside the corpus, and when it cannot be, the gate says so and fails rather than reporting a pass it did not establish. A green result is scoped to what was actually compiled; a baseline is the artefact of the compiler being reproduced, never of an earlier candidate. `tests/refactor_gate.sh`; corpus + rationale in `tests/README.md`.
- **Resource-fidelity** (the dual of the commuting-square, for *performance*-theorems) — the commuting-square certifies a pass is *correctness*-faithful (`⟦f⟧=⟦f'⟧`); this gate certifies it is *realization*-faithful. A performance-pass must declare (a) the hardware ultimate-fact it exploits, as a **spec citation**, and (b) that it is instantiated over the **full** fact, not a convenient truncation. Every resource-constant is either the **spec's number** or carries a dated justification for the gap. The mandatory question for each: **"is this the spec's number, or my convenience's number?"** — a truncation posing as a Side-II constant is a **Law-1 violation** (algorithm not faithfully realizing its side) catchable as a **Law-2 Side-II defect**, *not* a missing "improvement law": improvement stays inside Law 1's "faithfully realizes" clause, measured against the full ultimate-fact. (This is Law 3's "certify at the middle" extended from the correctness-theorem to the cost-theorem.)
- **Determinism seal** — identical IR ⟹ identical bytes, checked by compiling each corpus program in several FRESH processes so a per-process hash seed cannot leak into the output. Distinct from the byte-identical refactor gate, which only compares across a refactor.
- Full text of the correctness five + recorded traps: **`tests/README.md`**, which also names every place a spec constant is duplicated and the gate that proves the copies agree.

## Article F — ABI

AArch64-ELF (Linux) specifics — no leading `_`, `adrp`/`:got:`/TLS relocations, variadic-in-registers + 192B reg-save area, `char` **unsigned**, AAPCS64 register table, sections — are Side-II constants: **`THEORY.md` II-3 (AAPCS64) + II-4 (ELF/relocations/sections)**. Mistakes here produce cryptic crashes; read before touching `mir/isa.rs`, `isel/abi.rs` or `emit.rs`.

## Article G — operation supremacy (refactor · optimize · extend obey the Laws)

Every operation is subordinate to Laws 1–3 + THE ULTIMATUM and must leave CbC intact — none trades correctness/verification for a number (speed·size·LOC). **Refactor** ships a byte-identical proof (Article E), ranked *better-ground-for-optimization ∧ easier-proof ≫ fewer-LOC* — never merge two proof-carrying passes or blur a theorem seam to save lines. **Optimize** ships its commuting-square + cost-square (Law 3); **extend** stays strict-C99 with the deviation visible (Article D).

## Index

- **`THEORY.md`** — the two-side catalog (Part I theorems / Part II spec-tables); answers "what foundation does zcc rest on". Adding a theorem or constant updates it.
- **`SEMANTICS.md`** — the reference operational semantics (the executable meaning behind `⟦·⟧`).
- **`MILESTONES.md`** — milestone ladder, LOC budget, C99-remaining, debt ledger.
- **`tests/README.md`** — test-asset register, full test-mechanism text, baseline + traps.
- **`PURITY.md`** — the ONE goal: every LOC provably in theory ∪ fact, every pass
  squared and non-vacuous. What `tests/provenance.sh` checks, what the audit
  found, and what is open. Purity outranks every number (Law 0).
- **`MEASURED.md`** — target facts with no spec to cite (no vendor optimization
  guide exists for this core): value, method, date, machine, and what reads it.
  Cited from code as `MEASURED M<n>`, exactly as spec is cited as `THEORY II-<n>`.
- **`REARCH.md`** — the plan of record: the layer map, the layers themselves, the
  proof map and cost model, the milestone ladder whose status is edited in place,
  the baselines and the decision log. Boot here and resume at its first open row.
- **`src/ext.rs` + `grep 'EXT(' src/`** — the entire current deviation surface.
