# zcc test-asset ledger

Principle: **every test asset is either a script runnable within the repo, or is
documented here together with instructions to reconstruct it**. No orphan assets.

**zcc is fundamentally an ACADEMIC artifact**: each line of code maps to a
compiler-theory theorem — `[math/compiler theorem] --compiled--> [rust source]
--compiled--> zcc`. Tests are divided into TWO LAYERS:

1. **THEOREM-VERIFICATION layer — sci-gate (ground truth, more important than
   the corpus)**: exhausts the STRUCTURAL SPACE to certify a theorem. Speed does
   NOT matter; completeness is paramount (running for a whole day is acceptable).
   It must be EXTENDED further.
2. **PRACTICAL-CORROBORATION layer — corpus + app**: torture/csmith/linux/libc
   are only real-world SAMPLES, the lower layer, confirming that the theorem does
   not diverge from real-world practice.

**Minimalism**: the old framework (probe/gate-overnight/SOP/ledger + the
nginx/redis/git/sqlite app-stack + m8..m14) has been REMOVED — over-automation
slowed iteration and generated too many files (measure-the-speed rule,
CLAUDE.md). The only application retained is musl libc (directly relevant to the
minimal-distro goal). **ELF is the AUTHORITATIVE target; Mach-O/mac is currently
kept only to let clang serve as an ORACLE** (Mach-O may be forked out later to
reclaim LOC).

```
tests/
├── shape.sh cpp.sh decay.sh alg.sh abi.sh   # SCI-GATE + gen_*.py (theorem layer)
├── gate.sh                # dispatcher: gate.sh <area>  (runs exactly the gate owning the area)
├── box.sh                 # run 1 file / shell inside the ELF box (iteration)
├── run.sh  cases/ ext/    # base differential + hand-written cases (+ cases.known-fail)
├── suites/*.sh            # external corpus (gate = FAIL ⊆ *.known-fail)
├── halfsuite.sh           # thin alias: = fullsuite.sh base (fast loop)
└── fullsuite.sh           # SOLE RUNNER, 100% BOX: [TARGET] [SEEK] — self-build+docker
```

## Runner — 100% BOX (the box is fast; the mac runner has been removed)

`fullsuite.sh` is the SOLE entry point: on the mac it self-builds zcc-ELF
(musl,release) + `docker run zcc-box` + re-invokes itself inside the box. The mac
remains only to let clang serve as an ad-hoc oracle; there is NO mac runner any
more (static-musl inside the box is nearly free, whereas the mac costs 2.7s/case
for codesign/dyld).

**`sh tests/fullsuite.sh [TARGET] [SEEK]`** — SEEK reaches an individual LAYER
without re-running the whole thing:

| TARGET | what it runs |
|---|---|
| `all` (default) | sci + corpus + app |
| `sci` `corpus` `app` `base` | group (`base` = run.sh cases+ext, fast loop) |
| `shape` `cpp` `decay` `alg` `abi` | 1 sci-gate |
| `cases` `ext` | 1 base differential |
| `torture` `cts` | 1 corpus suite |
| `musl` | app libc |

**`SEEK`** (2nd argument, optional) = a substring of a case name → seek deep into
an INDIVIDUAL UNIT within 1 suite. E.g.: `fullsuite.sh torture pr22061`,
`fullsuite.sh cases float`. Applies to cases/ext + every corpus suite (filtered
with `grep -F` over the file list). Sci-gates generate cases INTERNALLY via
gen_*.py and therefore do not yet accept SEEK (to be extended later if needed).

**`sh tests/halfsuite.sh [SEEK]`** = alias for `fullsuite.sh base [SEEK]` — fast
loop.

## Sci-gate — theorem-verification layer (run inside the box via fullsuite.sh sci)

| Gate | Mathematical foundation | What is exhausted |
|---|---|---|
| `shape.sh` | regular languages + grammar automata + recursive record-layout | integer-literal 3.1.3.2 × escape × maximal-munch; declarator depth ≤3 (including calls through a fn-ptr); struct/union/bitfield layout, member combinations ≤3 + per-member offset |
| `cpp.sh` | term rewriting (terminating due to blue-painting) | expansion matrix (prescan/paste/stringize/rescan) + exhaustive `#if` arithmetic (dual oracle zcc==cc==python) |
| `decay.sh` | type-derivation lattice (lvalue conversion 6.3.2.1) | 12 ways to produce an array expr × 11 contexts × 2 branches; differential oracle over observables |
| `alg.sh` | UAC semilattice (3.2.1.5) + commuting-square (isomorphic oracle) | op × type × type × corner²: ~43k runtime points + ~21k fold; 4 comparisons including fold↔runtime INTERNAL to zcc (two paths from the same AST must meet) |
| `abi.sh` | finite automaton (AAPCS64 = an NGRN/NSRN/NSAA state machine) | 292 cases × 4 directions CROSS-LINK zcc↔gcc — a same-compiler ABI error self-cancels; only cross-link exposes it |

**"Exhaustion"** = exhausting the STRUCTURAL space + boundary samples of the
value space (not the full 2^64) — any claim of "proof" must be qualified with
this sentence (honest bounds).

**Theoretical weak points to be reinforced**: the ABI/register/memory-layout/ELF
layer has NO ISO standard guarding it (only AAPCS64 + the ELF psABI). The ground
truth here = the ABI spec + the **gcc/ld reference implementation** (exploited
through abi.sh's cross-link automaton). abi.sh is the thinnest guardian → priority
for extension: return HFA/composite, C.11 split reg↔stack, variadic edges,
over-alignment, and the full 9×9 product when the counters are mixed.

## Corpus — practical-corroboration layer (fullsuite.sh corpus)

Common template: referee-filter (`cc` rejects = out of scope → skip), differential
on exit+stdout, **gate = FAIL ⊆ baseline `*.known-fail`** (triaged line by line).
Run SEQUENTIALLY (each suite consumes all cores).

| Suite | Source (clone --depth 1) | Baseline notes |
|---|---|---|
| `torture.sh` | gcc-mirror/gcc (`gcc.c-torture/execute`) | **2-fact, 3-way** (see below): PASS \| NOT-IMPL (`torture.not-impl`, named) \| FAIL. Gate = **0 FAIL**. Has caught real bugs (pr60017, pr33631, va_arg HFA, pr92904 aligned) |
| `cts.sh` | c-testsuite/c-testsuite | oracle `.expected` (stdout-byte, no referee → cheap/deterministic). 00162 `[const 5]`, 00219 `_Generic` (construct UNIQUE to torture=0, pinned in Phase-3), 00204 LD=fp128 (ELF debt) |

**REMOVED (mechanical coverage-diff):** `kr` (UB in the reference answer —
diff-invalid), `nora` (1630 cases, 0 unique constructs — fingerprint dominated by
torture in every column), `chibicc` (41 cases, duplicate constructs; `_Generic`
already covered by cts), `tcc` (Darwin-locked `xcrun` → zombie dies inside the ELF
box). Evidence: torture dominates every construct by 1-2 orders of magnitude; the
only construct UNIQUE across all four = `_Generic`, retained via cts. The
bootstrap-compiler idea (from tcc) is reborn at the **third-party build** layer
(counterweight slimcc), not at the construct-corpus layer.

### torture 2-fact — classification contract (against silent skipping)

`torture.sh` no longer uses known-fail/skip. Referee = `cc -std=c99 -w -O0` (gcc
inside the box, an independent referee native to this suite). Each case receives
exactly 1 label:

- **PASS** — zcc compiles a valid C99 program → binary exits 0 (self-check
  abort()).
- **NOT-IMPL** — not a bug; recorded in `torture.not-impl`, NAMING the specific
  reason:
  - `oracle-invalid` — the c99 referee itself rejects it / does not run cleanly
    (gcc-ext, target-specific, UB). Reason = gcc's `error:` line.
  - `zcc-reject` — the referee is OK but zcc prints `zcc:…` then exits 1, produces
    NO binary and does NOT crash (honestly not yet implemented). Reason = the zcc
    diagnostic (`<case>:<ln>: msg`).
- **FAIL** — zcc SWALLOWS a valid C99 program then miscompiles/crashes (the count
  must be = 0): `runtime` (produces a binary but wrong/abort), `backend` (exit 1
  WITHOUT a `zcc:` line → as/ld chokes on junk asm), `crash` (panic/signal). This
  is what a reviewer fears: swallow-then-crash instead of reject-at-compile.

**CONSERVATION LAW** (enforced, not a promise): `pass+not-impl+fail` must = the
number of cases loaded; each case appears with exactly 1 verdict. A vanished case
(worker died/hung) or a duplicate → the harness TURNS RED immediately, allowing no
false green. → a green verdict is valid only when the equation is closed; a
reviewer verifies one line, without having to trust any narration.

## App — musl libc (fullsuite.sh app)

`musl-box.sh` / `musl.sh`: build musl 1.2.5 + libc-test, differential
`F_zcc \ F_ref` (referee musl-gcc). LDBL64 port; outstanding debt `-shared`/.so,
wide/mbc. It is the ONLY real software retained (foundation of the minimal-distro)
— test it thoroughly.

## float_h — a DOCUMENTED standard deviation (base differential)

zcc chooses `long double = double` on ELF (VALID per C99 §5.2.4.2.2 "LD ≥ double";
MSVC makes the same choice), and `float.h` declares `LDBL_MANT_DIG=53` for
SELF-CONSISTENCY (memory layout stays binary128 for ABI interop). Linux `cc` uses
binary128 (113) → `cases/float_h.c` differs, the ONLY objective failure inside the
box, already recorded in `cases.known-fail`. On the mac (Darwin LD=double) this
case passes.

Baseline principle: every known-fail line must carry an explanation (this table /
the head of the `*.known-fail` file / the commit). **The baseline is NOT a trash
can for hiding bugs** — a new, unexplained failure = a zcc bug until proven
otherwise (presumption-of-guilt rule, CLAUDE.md).

## Traps already paid for (read before debugging "ghosts")

- **A same-compiler ABI error self-cancels** — an integration test that runs fine
  may still have a wrong ABI; only cross-linking zcc↔gcc (abi.sh) exposes it.
- **The arg offset lives in 3 places that must match byte-for-byte**: codegen
  `call()`, the codegen spill prologue, and the parser's `va_off`. Change 1 =
  change 3 + run abi.sh.
- **implicit-int truncates pointer/double**: a libc missing a prototype → returns
  int → (1) a pointer sxtw truncated to 64-bit → segfault dependent on ASLR
  (heisenbug); (2) a returned double in d0 while the caller reads junk from x0 →
  wrong SILENTLY. Suspect: `nm -u` + cross-check `src/headers/*.h`.
- **A stale old-generation .o** → a "ghost" link error. Run `make distclean`
  before debugging a link.
- **A diff at a point of UB is meaningless** — a program reading stdin/argv:
  feeding the source as stdin manufactures UB (uninitialized var) = exactly the
  reason the kr suite (UB answers) was DROPPED.
- **"missing image zcc-box" while `docker images` STILL lists it** — docker's
  name index is stale: `docker image inspect zcc-box` fails but inspect-by-ID
  (716e3cce…) is OK. Fix: `docker tag <ID> zcc-box:latest` (a local, non-
  destructive operation).

## The test & proof laws — full text (offloaded from `CLAUDE.md`)

`CLAUDE.md` keeps these as terse articles and points here for the full text + recorded lessons. Nothing below is optional; it is the evidence layer those laws rest on.

- **Iteration-speed law (stands above every other test law)**: an iteration mechanism, however academically or scientifically elegant, is discarded immediately if *measurement* yields the opposite number — that is, if it makes iteration *slower* than the direct approach (detect bug → fix → re-test exactly the failing case) — regardless of how much code it represents. Recorded lesson: a four-tier SOP with harvest/regress staging was eliminated on the same day it was created, because it made the actual redis test queue behind bureaucracy. Ritchie wrote a C compiler on the PDP-11 without any of it.
- **Mathematical foundation (root law)**: every compiler feature must connect to, or be derived from, a principle — compiler theory, discrete mathematics, set theory, automata (lexer = regular language, preprocessor = term-rewriting system, parser = context-free grammar, UAC = semilattice, ABI = finite automaton, codegen = per-node simulation). Internal tests must cover the mathematical proof as far as possible: a new feature is first asked "which space does it belong to, can that space be exhausted, which gate guards it?"
- **Test-first forces LOC**: compile real programs *first*; implement a construct only when a program breaks on it.
- **Every correctness verdict is differential**: the referee is `cc` (the specification made flesh) or an independent oracle; a diff at a point of undefined behavior is meaningless — the generator must filter UB first.
- **Presumption-of-guilt law (recorded lesson)**: the compiler is *guilty until proven innocent*. Every accusation of "oracle/generator/test defect, not zcc" requires *multi-angle proof before it is asserted* — several independent formulations / viewpoints converging on the *same* result. The instinct to blame the test is precisely what *conceals* compiler bugs. Evidence: four "fall-off-end-of-main" cases were once declared "oracle-invalid, diff at UB" *without proof*; a double-check showed `clang -std=c89` also returned 0, revealing the real cause to be a zcc-ELF codegen bug (failing to emit `return 0` on falling off the end of `main`). Two errors compounded: (1) being lazy and not proving, (2) even the supposed "proof" being wrong. **Meta-conclusion: within a single session an AI assistant produced two contradictory judgments, so correctness-by-assertion is impossible; only correctness-by-mechanical-differential-verdict is viable. The assistant is an unreliable narrator and must be removed from the trust path: it may only *build* and *run* the oracle, and stay silent until the oracle speaks. Measure-before-speaking: no classification (bug / oracle / ext) may be asserted before a script has printed a verdict.** The fall-off paradox is evidence that the *mechanism* is correct: measurement crushed faulty reasoning — the error was in speaking before measuring. Consequences: (a) "diff at UB" may be *invoked* only after that point is proven to *genuinely* be UB / unspecified, by specification plus referee, never hand-waved; (b) "clang/gcc also fail, so we are allowed to fail" is *absolutely forbidden* as an excuse — the root cause of the referee's rejection must be dug out, as it may itself expose an edge case; (c) a case is excluded only when proven to lie outside the implementation scope (IR + Optimization / vendor dialect); mistakenly dropping a case that represents a semantic edge case is a disaster.
- **The science-gate is the theorem-verification tier (ground truth, more important than the corpus)**: zcc is academic in nature — each line maps to a compiler-theory theorem, and the science-gate *exhausts the structural space* to verify that theorem (corpus / csmith / linux are only *practical* verification, a lower tier). The relevant space is exhausted on contact: `abi.sh` (ABI automaton, *cross*-linked — same-compiler ABI errors cancel), `alg.sh` (UAC semilattice + fold↔runtime commuting square = isomorphic oracle), `cpp.sh` (term-rewriting system), `shape.sh` (lexer / declarator / layout — grammar automata), `decay.sh` (type-derivation lattice). "Exhaustion" means exhausting the *structural* space plus boundary samples of the *value* space — any claim of "proof" must carry this qualifier. Dispatcher: `gate.sh <area>`; run inside the ELF box via `box.sh`. The single runner is `fullsuite.sh [TARGET] [SEEK]`, entirely *inside the box* — TARGET seeks to a tier (sci | corpus | app | all | one gate | one suite | base), SEEK seeks an individual case; `halfsuite.sh` = `fullsuite.sh base`. **The science-gate is to be *expanded*, never contracted.** The application stack (nginx/redis/git/sqlite) has been removed from the runner (run manually when needed).
- **External suites**: a new failure outside the triaged baseline is a zcc bug until proven otherwise; the baseline is not a dumping ground for hidden bugs.
- **Clean-input law: the ultimate source of error is bad/garbage input collected while running the suite.** A PASS/FAIL verdict is worthless if the measurement itself rests on garbage data (a referee-filter skipping wrongly, `2>/dev/null` swallowing errors, mislabeled counts, a suite that is "green" without running anything). A green verdict is valid *only* when accompanied by a *mechanical evidence trail* proving real work occurred: number of artifacts produced + checksums + observed exit codes, *not* merely a pass/fail number. Publication standard: a "torture pass" claim must carry evidence of N real ELF binaries + total codegen bytes + a deterministic re-run sample (e.g. a torture-box run of 16s producing 1377 real ELF binaries / 21MB / 1694 cases fully covered — the suspicion "16s means no-op" is refuted by the manifest). An abnormal timing (fast *or* slow) is *measured*, not guessed (macOS clang compile+run is 2.7s per invocation due to codesign/dyld; Linux static-musl is nearly free — the same suite is 19 minutes on macOS versus 16s in the box).
- **Test-loop optimization**: during triage/fix, re-run *exactly* the case/unit that failed last time, *not* the full suite; the full suite runs only once at the end to close the books (in the background, not blocking). Heavy suites run *sequentially*, not contending for cores.
- **Numeric-provenance rule**: every number / decision must be derivable from a stated premise — no magic number without provenance.
- **Byte-identical gate** — proves a pure-code-motion refactor changed nothing: identical `md5(.s)` over a fixed corpus *is* the commuting-square `⟦f⟧=⟦refactor f⟧` (Article G). Mechanism + usage in the script header: `tests/refactor_gate.sh`.
- Compare against the reference answer at any time: `clang -S -O0 -std=c99 foo.c`.
</content>
</invoke>
