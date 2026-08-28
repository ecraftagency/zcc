# zcc — Project Charter

A strict-C99 compiler (C89 ⊂ C99) in Rust, no external crates, AArch64 ELF.

**Only what stays true across every milestone belongs here.** Measurements,
ratios, statuses, commits, tags, branch names and dated findings do not — a
stale rule is worse than no rule, because it is still obeyed. **Every rule below
is complete as written**: act on it without opening anything. The citations say
where a derivation lives, for a reader who wants one.

**Operating:** run one thing, wait, report it — detach only on request, and say
so. Parallel work makes it cheap to start things and expensive to stay aimed.
**AWS: `us-east-2` only.**

## Law 0 — purity is the precondition

    purity ≫ exec > size > compile speed

**No number is banked at the cost of a citation.** A row that reaches parity by
removing a proof does not ship, however large the number. zcc is educational, so
a citation is a reading path — a student landing on any line can read upward to
the theorem it realizes — not a lint marker. `tests/provenance.sh` checks it.

## Law 1 — the decomposition theorem

    zcc source = ( theory    → control flow + data structure + algorithm )
               ⊕ ( spec text → constant + parameter + table )

Every line of `src/` lies on exactly one side. **No magic number without
provenance.** If no line lies outside {theorem ∪ spec}, zcc passes every suite by
construction: zcc and the referee are both shadows of the same specification. zcc
never proves itself — the referee is independent, which is also why the AI stays
out of the trust path.

## Law 2 — a defect has exactly two doors

1. **Side I** — a misread theorem: an algorithm outside the theorem it claims.
2. **Side II** — a wrongly-injected value: an offset, ABI constant, layout or
   section applied wrongly.

**The exception, and it is not a third door:** the measurement lied. Claim it only
after several independent formulations converge. Reflexive blame of the test is
how a real defect hides.

**Measure before speaking.** Locate the line mechanically first, classify after.
Guessing the wrong side is normal; keep measuring. Build the oracle, run it, stay
silent until it prints a verdict.

## Law 3 — certify at the middle, not at the binary

Every intermediate artifact carries the theorem that certifies it, so always ask
*"can this be proven here, before the final suite?"* A theorem is not only a
compiler of `src/`; it is a prover.

- **Correctness** — a pass ships its commuting square `⟦IR⟧ = ⟦IR'⟧` (translation
  validation for backend passes). **On this axis** a generator only confirms; it
  never discovers, because the square already decided the question.
- **Cost** — instruction count is a fold over the IR, written independently of
  the lowering; the square `cost(f) ≡ len(codegen(f))` certifies the backend
  realizes it. A mismatch is a Law-2 defect at one instruction, never a mystery
  to grep out of `.s`. Compute a transform's delta on the model **before any
  build**; patch-and-run-the-suite is the last resort.
- **Exhaustion** — a theorem is not done at its first green. Measure its
  **residual**, every site where it could have fired and did not, and classify
  each: (a) a real ISA/ABI boundary, proven, or (b) an incomplete realization. It
  is exhausted only when the residual is entirely (a). Stopping while (b) remains
  is a Law-1 violation.

## Law 3c — count is not cost

    size:  cost(f)   = |MIR(f)|                 proven per function
    time:  cycles(L) = critical recurrence(L)   proven per loop

> **Fewest instructions is not fastest code.** A codegen row is judged by the
> longest dependence chain it leaves. Where the two models disagree, time wins.

**Operative rule:** never leave a multi-cycle operation in front of an address or
a loop-carried value when a one-cycle operation computes the same thing.

**Third blindness:** a static count weighs an instruction in a latch run millions
of times the same as one in a cold arm. Rank a row by executions.

**What may be claimed.** A suite is always narrower than the language and runs on
one microarchitecture at one input size. **Name the suite and the core, or say
nothing.** Claim parity only with a margin — how far the sampled fraction must
win before the unsampled remainder is unlikely to flip the sign. Parity announced
from a narrow suite is a Law-0 failure. Widen the surface, and note that a
generator's role INVERTS here: on the correctness axis it only confirms, on this
one it discovers — it finds the cliffs, while real programs carry the geomean.

## Articles

**A** — (1) strict C99; extensions only when real software demands one, marked
`EXT(...)`. (2) **Nothing is built before a real `.c` demands it** — no
anticipatory abstraction, no feature acquired from a checklist, zero external
crates.

*There is no line-count ceiling and there never really was one: the constraint
is DEMAND, not size.* A cap on lines cannot tell a proof from bloat, and this
project spends most of its lines on the proofs Law 0 ranks above every number —
so a ceiling would have to be paid for by deleting exactly what makes the
compiler worth reading. The rule that does the work is the demand rule, and it
is unchanged.

**B** — `main.rs → lexer → parser → AST → compile.rs → HIR (target-independent
SSA) → isel → MIR (machine SSA) → regalloc → frame/layout → emit.rs → .s`. The
frontend/backend boundary is `src/ast.rs`: frontend builds, backend only reads.
`compile.rs` is the single door. **All target knowledge lives in the ISA tables,
the ABI automaton and the emitter**; nothing above `isel` may name a machine
register. A second target adds a second MIR and isel, never a conditional. **A
pass is a pass, never a text peephole** — the emitter decides nothing and
re-parses nothing. Single crate.

**C** — `CC=zcc` slots into a real build system without editing one build file.
Acquire flags test-first; swallow the rest silently, but **never mis-swallow a
flag carrying an argument** — one misalignment eats an input file. Standard
`file:line:` diagnostics, correct exit codes.

**D** — extension logic lives in `src/ext.rs`; the core only calls `ext_*`.
Unfactorable sites carry `// EXT(...)` and `grep 'EXT(' src/` must cover the whole
deviation surface. Verified by excision. Extension tests live apart from the C99
cases.

**E — how anything gets believed.**
- **Differential** against an independent oracle. A diff at an undefined point is
  meaningless: filter by spec, never by hand-waving.
- **Numeric provenance** — every number derives from a stated premise, and every
  constant answers: *the spec's number, or my convenience's?* A truncation posing
  as a spec constant is a Law-1 violation.
- **Clean input** — a green verdict needs an evidence trail, never a bare count.
- **Byte-identical** — pure code motion holds `md5(.s)` unchanged; identical bytes
  *are* the square. Two witnesses, because small programs cannot see a pass that
  scales with function size. A green is scoped to what was compiled, and a
  baseline is an artefact of the compiler being reproduced, never of an earlier
  candidate.
- **Determinism** — identical IR gives identical bytes, across fresh processes.
- **Iteration speed** — a mechanism measured slower than the direct loop is
  discarded, however elegant.
- **Science gates** — structural exhaustion above the corpus, expanded, never
  contracted.

**F** — the ABI specifics are Side-II constants and mistakes there produce
cryptic crashes. Read the spec tables before touching the ISA tables, the ABI
automaton or the emitter.

**G** — refactor, optimize and extend all obey Laws 1–3 and none trades
verification for a number. A **refactor** ships a byte-identical proof and is
ranked *better ground for optimization ∧ easier proof* — never merge two
proof-carrying passes or blur a theorem seam. An **optimization** ships both
squares. An **extension** stays strict C99 with the deviation visible.

## Index — five documents, and `src/` may point at no others

A document per campaign is how a repository acquires contradictions faster than
facts. **A new one is not created without deleting one.**

- **`THEORY.md`** — the two-side catalog: Part I theorems, Part II spec tables.
- **`SEMANTICS.md`** — the reference semantics behind `⟦·⟧`.
- **`MECHANISM.md`** — how the compiler is built and every fact measured about
  it, each dated and pinned to a commit. **Part G §G0 is the field guide to where
  defects live — start there when something is wrong and the reason is not
  obvious.** Facts with no spec to cite are in Part F, cited as `MEASURED M<n>`.
- **`ARM64.md`** — the target's facts and the isel exhaustion checklist.
- **`README.md`** — what zcc is, how to build it, the milestone ladder and debt.

**`PLAN.md` is not one of the five.** One grind, not a list; 100 lines; never
cited from `src/`; emptied when the grind closes. Every row leaves by one of two
doors: into `MECHANISM.md` because it won, or into its Part F as a refutation
because it lost.

**A citation is a name, not a fetch.** A comment says enough *why* to fix the line
it sits on without opening anything; the citation names where the derivation
lives. `tests/provenance.sh` can only check this direction — a document pointing
back at `file.rs:412` is stale at the next refactor and nothing catches it.
