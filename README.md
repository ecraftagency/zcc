# zcc

> **AI-authored.** Not a single line in this repository was written by a human.
> The compiler, its tests and its documents were produced entirely by AI under
> the direction of the project author, who designed, reviewed and steered.

A compiler for strict C99, written in Rust, targeting AArch64 ELF (Linux).
Single crate, zero external dependencies. `CC=zcc` slots into a real build
system unmodified.

## Correctness by construction

zcc exists to ask one question: *how do you know a compiler is correct?* Testing
answers it only halfway — Dijkstra's wall, that testing shows the presence of
bugs and never their absence — so zcc tries to **construct** correctness instead
of only checking it afterwards. Two rules do the work:

**Every line of `src/` lies on exactly one side** — it realizes a *theorem*
(control flow, data structure, algorithm) or transcribes a line of
*specification* (a constant, an ABI offset, a table). Nothing else is allowed, no
constant appears without provenance, and `tests/provenance.sh` fails the build if
a line escapes both. If nothing lies outside {theorem ∪ spec}, zcc and its
referee are two shadows of the same document.

**Every optimization pass ships its commuting square.** `SEMANTICS.md` gives an
executable reference semantics ⟦·⟧ of the IR; a pass is correct exactly when
⟦IR⟧ = ⟦pass(IR)⟧, and that equality is *run*, per pass, on every `cargo test` —
and checked **non-vacuous**, because a theorem nobody exercises is not a theorem.

The pressure point is deliberate: differential studies locate most
*miscompilation* bugs in the IR and its passes rather than the parser, so zcc
carries that part — typed SSA and about thirty passes — because it is the hard
part, and makes it the object of proof.

**Stated honestly:** this is *mechanized and structurally exhaustive*, **not** a
machine-checked proof, and zcc does not claim to be "verified". Formal proof —
translation validation, then per-pass machine-checked proofs — is on the roadmap
in `SEMANTICS.md` §6.

## Capabilities

A ✅ is measured and reproducible; a ⏳ is a gate that must close before release.

| | |
|---|---|
| ✅ | **Language** — C99 (C89 is a subset): preprocessor, parser, full type system, code generator |
| ✅ | **Target** — AArch64 ELF (Linux). macOS/clang is a reference oracle only |
| ✅ | **Drop-in driver** — `CC=zcc` through configure/make/cmake; drives `as` and `ld` directly |
| ✅ | **Real software** — builds and differentially validates sqlite, lua, zlib, libpng, bzip2, oniguruma, git, redis, nginx and musl libc |
| ✅ | **IR + optimization** — typed SSA, 17 HIR passes and 13 MIR passes, each carrying its square |
| ✅ | **Structural-exhaustion gates** — five science-gates exhaust the grammar of each phase: lexer/layout, preprocessor, type derivation, arithmetic conversions, ABI |
| ✅ | **Random differential** — csmith and yarpgen, 0 divergence; a 10,000-seed seal is owed before release |
| ⏳ | **End-to-end Linux chroot boot** — *release gate* |

The gate is 16 stages, native on AWS Graviton4: torture 1694, c-testsuite 220,
opt-parity 1552, csmith, yarpgen, musl 479 test binaries, determinism 188×8,
provenance, UBSan over every benchmark. It prints which binary it graded — see
below for why that matters.

## Measurements

Referee is **`gcc -O2`** — the level real software is built at — on a
**Graviton4 / Neoverse V2** box, native Debian 13, gcc 14.2.0, 2026-08-29. Both
compilers build and RUN every program, output checked before any clock is read,
and each comparison pins the WORK rather than only the answer: libpng's encoded
size and bzip2's compressed length fix every filter and Huffman decision.

| program | what it exercises | exec | insn |
|---|---|---|---|
| sqlite 3 CLI | query engine, cold branches, real spilling | **1.123** | — |
| lua 5.4.7 | ~100-arm dispatch loop, FP through the VM, GC | **1.160** | — |
| zlib 1.3.1 | one hot loop, 32 KB sliding window | **1.130** | 1.155 |
| libpng 1.6.43 | byte-wise row filters, narrow types, constant stride | **1.010** | 1.073 |
| bzip2 1.0.8 — compress | suffix sort, genuinely unpredictable branches | **1.102** | 0.548 |
| bzip2 1.0.8 — decompress | inverse BWT, one dependent load per byte | **1.074** | 1.611 |
| oniguruma 6.9.9 | backtracking regex engine | *1,516 / 1,516 pass, = gcc* | — |

bzip2's two INSN columns run opposite ways and cancel to 1.005 in the total —
which is why the arms are reported, and the geomean is not.

**Kernel suite** — 96 programs, one shape each, deliberately adversarial:

    EXEC geomean 1.209   ·   INSN geomean 1.015   ·   0 divergence

49 of the 96 sit above 1.1× and the worst is 2.75×. The gap is measured, not
guessed: `perf` says the twelve worst are all **count-driven**, never
chain-driven — zcc's IPC is 4.1–6.7 and it retires 1.4×–4.6× the instructions,
because gcc -O2 vectorizes seven of those twelve and zcc vectorizes none.

**Read the instrument's floor before any of these.** Six runs of ONE unchanged
binary over the same 96 programs spread 1.1990 to 1.2107 — 0.012. The suite
geomean cannot resolve a change below ~1.5%, and two rows measured the day this
was written were buried by exactly that; `perf`'s dynamic instruction count is
the deterministic instrument for anything smaller. A 100 ms real program resolves
to ±0.3%, which is why the surface grows by programs, not by kernels.

**No parity and no superlative is claimed.** Only cproc+qbe has been run against
zcc here — 2.141×, and it cannot compile the sqlite amalgamation at all. tcc,
PCC, lacc and chibicc have not; tcc and chibicc are one-pass compilers with no
optimizer *by design*, which is a statement about their architecture, not a
measurement. CompCert sits in range of `gcc -O1`. Nor does a number transfer
across cores: these binaries read 1.069× on an Apple M1 Pro where Neoverse reads
1.172× against `-O1`.

## What the surface found

Widening from benchmarks to real applications found three defects in one
afternoon that 20,000 generated programs never produced. All three are fixed.

- **C99 6.2.3** — tags are their own name space, so `enum SaveType {...}` may sit
  beside `typedef int SaveType;`. zcc refused the tag.
- **C99 6.4.3** — zcc had *no `\u` arm in its escape handler at all*: a universal
  character name fell through to the undefined-escape rule and the escape for OHM
  SIGN became the five characters `u2126`. **Silent** wrong answers.
- **The gate itself** — it hard-set the compiler path to a symlink into a build
  tree it did not own, so fifteen stages reported PASS about a binary four hours
  older than the fix under test. It now prints what it graded.

Neither compiler defect was reachable from a benchmark: libpng and bzip2 agree
with gcc byte for byte because no workload here writes a `\u`. **A differential
comparison covers only what its workload executes; an application's own test
suite sweeps the application deliberately** — the whole argument for the
real-program surface.

## Documents

Five, and the source may point at no others — a document per campaign is how a
repository acquires contradictions faster than facts.

- `CLAUDE.md` — the charter: the laws, above all five.
- `THEORY.md` — every line mapped to a theorem or a spec constant.
- `SEMANTICS.md` — the reference operational semantics ⟦·⟧ of the IR.
- `MECHANISM.md` — how it is built and every fact measured about it; Part G §G0
  is the field guide to where defects live.
- `ARM64.md` — the target's facts and the isel exhaustion checklist.

`PLAN.md` is not one of them: one grind, 100 lines, never cited from `src/`,
emptied when the grind closes. And the history is in the tags rather than here —
`git tag -n50 rc3` through `rc11` is the chronicle, each carrying the
measurements and the refutations of its milestone.

## Build and license

```sh
cargo build --release      # the compiler: target/release/zcc
cargo test                 # unit tests, including the commuting-square theorems
```

MIT — see [`LICENSE`](LICENSE). Use it for any purpose, without restriction.
