# zcc

> ### AI-authored
> **Not a single line of code in this repository was written by a human.** The
> compiler, its tests, and its documentation were produced entirely by AI, under
> the direction of the project author. The author designed, reviewed, and steered
> the work; the AI wrote all of it. This disclaimer applies to every file here.

A C compiler for strict C99, written in Rust — targeting AArch64 (ELF/Linux and
Mach-O/macOS). Single crate, zero external dependencies.

## Why another compiler

The world has enough compilers. zcc is not meant to be one more — it is an
experiment in a single question:

> *How do you know a compiler is correct?*

It began as a study of how to **test** a compiler: differential testing against
a reference compiler, random-program fuzzing (csmith/yarpgen), and structural
exhaustion over the grammar of each phase. Pushed far enough, testing runs into
the wall Dijkstra named:

> *Program testing can be used to show the presence of bugs, but never to show
> their absence.*

So zcc turns the question around. Instead of only *checking* correctness after
the fact, it tries to *construct* it. Every line of `src/` maps either to a
theorem (control flow, data structure, algorithm) or to a line of specification
(a constant or lookup table) — nothing else.

The pressure point is the optimizer. Differential-fuzzing studies of production
compilers (csmith, EMI) consistently locate the majority of *miscompilation*
bugs not in the parser but in the intermediate representation and its
transformation passes — the part that separates a translator from a *compiler*.
zcc carries that part (a typed IR and five optimization passes) precisely
because it is the hard part,
and then makes it the object of proof: each pass carries an **executable
equivalence check** against a **reference semantics** of the IR. A pass is
correct if and only if it commutes with the meaning of the program, and that is
*measured*, not asserted. Small enough to read in full; hard enough to be worth
verifying.

The long-term aim is to retire the test suite by proof.

## Capabilities

Everything below is what a compiler of roughly **11k lines** does. A ✅ is
measured and reproducible; a ⏳ is a gate that must close before release.

| | Capability |
|---|---|
| ✅ | **Language:** C99 (C89 is a subset); preprocessor, parser, full type system, code generator |
| ✅ | **Targets:** AArch64 — ELF (Linux) and Mach-O (macOS) |
| ✅ | **Dependencies:** none — a single Rust crate, zero external crates |
| ✅ | **Drop-in driver:** `CC=zcc` slots into real build systems (configure/make/cmake) unmodified; drives `as`/`ld` directly |
| ✅ | **Real software:** compiles and differentially validates real C projects (redis, sqlite, git, nginx, …) |
| ✅ | **IR + optimization:** a typed intermediate representation with five optimization passes — constant folding, dead-code elimination, copy propagation, common-subexpression elimination (alias-aware), and register allocation (Chaitin–Briggs) |
| ✅ | **Structural-exhaustion gates:** five science-gates exhaust the grammar of each phase (lexer/layout, preprocessor, type derivation, usual-arithmetic-conversions, ABI) |
| ✅ | **Random differential — csmith:** passing |
| ✅ | **Mechanized reference semantics:** a formal semantics ⟦·⟧ of the IR (`SEMANTICS.md`) with an executable commuting-square theorem over the optimization passes (312 expressions × 5 passes = 1560 checks) |
| ⏳ | **Random differential — yarpgen:** *release gate* |
| ⏳ | **End-to-end — Linux chroot boot:** *release gate* |

**Stated honestly:** the correctness evidence above is *mechanized and
structurally exhaustive*, **not** a machine-checked proof. The project does
**not** claim to be "verified." Formal proof (translation validation, then
per-pass machine-checked proofs) is on the roadmap — see `SEMANTICS.md` §6.

## Design documents

- `THEORY.md` — the theoretical catalog: every feature mapped to a theorem.
- `SEMANTICS.md` — the formal reference semantics ⟦·⟧ of the IR.
- `MILESTONES.md` — development history and roadmap.
- `OPT.md` `[ssa-qbe fork]` — the single optimization working-doc (IR contract · scoreboard · pipeline plan); transient, folds into the above at opt-end.

## Build

```sh
cargo build --release      # the compiler: target/release/zcc
cargo test                 # unit tests, including the commuting-square theorem
```

`zcc` drives the host assembler and linker directly and slots into a real build
system as `CC=zcc`.

## License

MIT — see [`LICENSE`](LICENSE). Use it for any purpose, without restriction.
