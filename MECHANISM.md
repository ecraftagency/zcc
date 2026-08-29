# MECHANISM — how zcc is built, and what has been measured

**THE CONTRACT OF THIS FILE.** Everything here is a FACT about the compiler that
is currently green. A plan is not a fact: an unproven row lives in `PLAN.md`,
which is capped and allowed to be wrong, and leaves it only by being baked into
this file (it won) or into Part F as a refutation (it lost). Nothing arrives here
without a date and the commit it was measured on — a line without one is
suspect, and that is the mechanism against a claim that quietly expires.

This is one of five documents the source is allowed to point at: `THEORY.md`
(the theorems and the spec tables), `SEMANTICS.md` (the meaning behind `⟦·⟧`),
this file (the machinery and the measurements), `ARM64.md` (the target's own
facts) and `README.md`. `CLAUDE.md` stands above all five as the charter.

## Contents

- **Part A — the gate**, and what a green verdict is allowed to mean.
- **Part B — purity**, the precondition Law 0 names, and what checks it.
- **Part C — compile speed**, the profile and the fixes that were banked.
- **Part D — the spiller**, its ladder, and the two rows its own ceiling refused.
- **Part E — the copy census**, the family worth seventy percent of the gap
  (⚠ superseded — read `M26-correction`).
- **Part G §G0 — WHERE THE DEFECTS LIVE**, a layer-by-layer field guide: the
  shape of defect each layer produces, the measurement that exposed it, and the
  method, in order. Start here when a program is slow and the reason is not
  obvious.
- **Part F — MEASURED**, the facts that have no spec to cite. This is the
  citation namespace: code says `MEASURED M<n>` and `tests/provenance.sh`
  resolves it here.

Section numbers repeat across parts (each was its own document until
2026-08-28); read a `§n` as belonging to the part it sits in.

---

# Part A — the gate

## zcc test-asset ledger

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

### Runner — 100% BOX (the box is fast; the mac runner has been removed)

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

### Sci-gate — theorem-verification layer (run inside the box via fullsuite.sh sci)

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

### Corpus — practical-corroboration layer (fullsuite.sh corpus)

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

#### torture 2-fact — classification contract (against silent skipping)

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

### App — musl libc (fullsuite.sh app)

`musl-box.sh` / `musl.sh`: build musl 1.2.5 + libc-test, differential
`F_zcc \ F_ref` (referee musl-gcc). LDBL64 port; outstanding debt `-shared`/.so,
wide/mbc. It is the ONLY real software retained (foundation of the minimal-distro)
— test it thoroughly.

### float_h — a DOCUMENTED standard deviation (base differential)

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

### Traps already paid for (read before debugging "ghosts")

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

### The test & proof laws — full text (offloaded from `CLAUDE.md`)

`CLAUDE.md` keeps these as terse articles and points here for the full text + recorded lessons. Nothing below is optional; it is the evidence layer those laws rest on.

- **Iteration-speed law (stands above every other test law)**: an iteration mechanism, however academically or scientifically elegant, is discarded immediately if *measurement* yields the opposite number — that is, if it makes iteration *slower* than the direct approach (detect bug → fix → re-test exactly the failing case) — regardless of how much code it represents. Recorded lesson: a four-tier SOP with harvest/regress staging was eliminated on the same day it was created, because it made the actual redis test queue behind bureaucracy. Ritchie wrote a C compiler on the PDP-11 without any of it.
- **Mathematical foundation (root law)**: every compiler feature must connect to, or be derived from, a principle — compiler theory, discrete mathematics, set theory, automata (lexer = regular language, preprocessor = term-rewriting system, parser = context-free grammar, UAC = semilattice, ABI = finite automaton, codegen = per-node simulation). Internal tests must cover the mathematical proof as far as possible: a new feature is first asked "which space does it belong to, can that space be exhausted, which gate guards it?"
- **Test-first forces LOC**: compile real programs *first*; implement a construct only when a program breaks on it.
- **Every correctness verdict is differential**: the referee is `cc` (the specification made flesh) or an independent oracle; a diff at a point of undefined behavior is meaningless — the generator must filter UB first.
- **Presumption-of-guilt law (recorded lesson)**: the compiler is *guilty until proven innocent*. Every accusation of "oracle/generator/test defect, not zcc" requires *multi-angle proof before it is asserted* — several independent formulations / viewpoints converging on the *same* result. The instinct to blame the test is precisely what *conceals* compiler bugs. Evidence: four "fall-off-end-of-main" cases were once declared "oracle-invalid, diff at UB" *without proof*; a double-check showed `clang -std=c89` also returned 0, revealing the real cause to be a zcc-ELF codegen bug (failing to emit `return 0` on falling off the end of `main`). Two errors compounded: (1) being lazy and not proving, (2) even the supposed "proof" being wrong. **Meta-conclusion: within a single session an AI assistant produced two contradictory judgments, so correctness-by-assertion is impossible; only correctness-by-mechanical-differential-verdict is viable. The assistant is an unreliable narrator and must be removed from the trust path: it may only *build* and *run* the oracle, and stay silent until the oracle speaks. Measure-before-speaking: no classification (bug / oracle / ext) may be asserted before a script has printed a verdict.** The fall-off paradox is evidence that the *mechanism* is correct: measurement crushed faulty reasoning — the error was in speaking before measuring. Consequences: (a) "diff at UB" may be *invoked* only after that point is proven to *genuinely* be UB / unspecified, by specification plus referee, never hand-waved; (b) "clang/gcc also fail, so we are allowed to fail" is *absolutely forbidden* as an excuse — the root cause of the referee's rejection must be dug out, as it may itself expose an edge case; (c) a case is excluded only when proven to lie outside the implementation scope (IR + Optimization / vendor dialect); mistakenly dropping a case that represents a semantic edge case is a disaster.
- **The science-gate is the theorem-verification tier (ground truth, more important than the corpus)**: zcc is academic in nature — each line maps to a compiler-theory theorem, and the science-gate *exhausts the structural space* to verify that theorem (corpus / csmith / linux are only *practical* verification, a lower tier). The relevant space is exhausted on contact: `abi.sh` (ABI automaton, *cross*-linked — same-compiler ABI errors cancel), `alg.sh` (UAC semilattice + fold↔runtime commuting square = isomorphic oracle), `cpp.sh` (term-rewriting system), `shape.sh` (lexer / declarator / layout — grammar automata), `decay.sh` (type-derivation lattice). "Exhaustion" means exhausting the *structural* space plus boundary samples of the *value* space — any claim of "proof" must carry this qualifier. Dispatcher: `gate.sh <area>`; run inside the ELF box via `box.sh`. The single runner is `fullsuite.sh [TARGET] [SEEK]`, entirely *inside the box* — TARGET seeks to a tier (sci | corpus | app | all | one gate | one suite | base), SEEK seeks an individual case; `halfsuite.sh` = `fullsuite.sh base`. **The science-gate is to be *expanded*, never contracted.** The application stack (nginx/redis/git/sqlite) has been removed from the runner (run manually when needed).
- **External suites**: a new failure outside the triaged baseline is a zcc bug until proven otherwise; the baseline is not a dumping ground for hidden bugs.
- **Clean-input law: the ultimate source of error is bad/garbage input collected while running the suite.** A PASS/FAIL verdict is worthless if the measurement itself rests on garbage data (a referee-filter skipping wrongly, `2>/dev/null` swallowing errors, mislabeled counts, a suite that is "green" without running anything). A green verdict is valid *only* when accompanied by a *mechanical evidence trail* proving real work occurred: number of artifacts produced + checksums + observed exit codes, *not* merely a pass/fail number. Publication standard: a "torture pass" claim must carry evidence of N real ELF binaries + total codegen bytes + a deterministic re-run sample (e.g. a torture-box run of 16s producing 1377 real ELF binaries / 21MB / 1694 cases fully covered — the suspicion "16s means no-op" is refuted by the manifest). An abnormal timing (fast *or* slow) is *measured*, not guessed (macOS clang compile+run is 2.7s per invocation due to codesign/dyld; Linux static-musl is nearly free — the same suite is 19 minutes on macOS versus 16s in the box).
- **Profit-first law (a PERFORMANCE row is gated only after its profit is stated).** A gate costs 7+ minutes and proves exactly one thing: that nothing broke. It cannot say whether the row is worth keeping — the A/B measurement already answered that, and it is in hand the moment the interleaved pair returns. So the order is: measure the row, PRINT the before/after table including the losses (compile time, any regressed program), and only then gate. A row whose profit is ~0 does not enter a gate without the decision being taken first, because seven minutes of verification spent on a row nobody has agreed to keep is seven minutes of the iteration-speed law being broken. Recorded lesson (2026-08-29, M36): the `delabel` row was measured at x1 5.014 → 1.469 and suite 1.0871 → 1.0715, then put through cargo + torture + the 90-program suite + two sqlite runs before either number was ever said out loud; the session after it repeated the pattern with a licm row whose measured profit was exactly zero. This law is the profit-side twin of the test-loop rule below: that one says re-run the failing case, not the suite; this one says state the number, not the gate.
- **Test-loop optimization**: during triage/fix, re-run *exactly* the case/unit that failed last time, *not* the full suite; the full suite runs only once at the end to close the books (in the background, not blocking). Heavy suites run *sequentially*, not contending for cores.
- **Numeric-provenance rule**: every number / decision must be derivable from a stated premise — no magic number without provenance.
- **Byte-identical gate** — proves a pure-code-motion refactor changed nothing: identical `md5(.s)` *is* the commuting-square `⟦f⟧=⟦refactor f⟧` (Article G). Mechanism + usage in the script header: `tests/refactor_gate.sh`.

  **It takes two witnesses, and the second one is why.** The corpus is 58 small
  programs, and on 2026-08-28 six inlining rows passed it — green every time —
  while changing the assembly of the sqlite amalgamation. Nothing in the corpus
  is large enough to make an inliner, a spiller, or anything else that reruns an
  analysis per rewrite do the work where its behaviour differs. So the gate also
  compiles ONE large translation unit and compares its `md5` against
  `sums.large.txt`, which costs a single compile. When the box is unavailable the
  gate reports that and FAILS: a gate that quietly proves less than it claims is
  precisely what went wrong. `ZCC_GATE_LARGE=0` runs the corpus alone and prints
  the waiver in place of the result.

  **The baseline is the compiler being reproduced, never an earlier candidate.**
  The same episode ran six rows against a hash produced by the first of them, so
  the chain agreed with itself while walking away from the tree it was supposed
  to match. Record `baseline` on a tree you trust, then leave it alone.
- Compare against the reference answer at any time: `clang -S -O0 -std=c99 foo.c`.
</content>
</invoke>

---

# Part B — purity

## PURITY — the one goal, and how it is checked

**Purity is the precondition, not a phase.** The ULTIMATUM names 1× against
gcc-O1 on both axes as the stopping point; purity is what may not be spent to
get there. Standing order, 2026-08-26:

```
purity  ≫  exec  >  size  >  compile speed
```

No number is banked at the cost of a citation. A row that would reach parity by
removing a proof does not ship.

---

### What purity means, exactly

Two of the three Laws are claims **about the source**, and until 2026-08-26
neither was checked by anything:

| | the claim | the check |
|---|---|---|
| **Law 1** | every line of `src/` is a theorem (Side I) or a constant transcribed from a spec line (Side II) | every module, constant and pass carries a citation |
| **Law 3** | every pass ships the commuting square that certifies it | every pass names its square, and the square is not vacuous |
| — | compilation is a FUNCTION: identical input, identical bytes | `tests/determinism.sh` |

`tests/provenance.sh` is the first two. It runs in the sci gate.

#### The three documents

* **`THEORY.md`** — Side I theorems and Side II **citations**: a section number
  in ISO 9899, AAPCS64, DDI 0487 or the ELF ABI that a reader can look up.
* **`MECHANISM.md` Part F** — facts with **no spec to cite**. Apple publishes no
  Software Optimization Guide for the M1, so a latency, or whether a transform
  pays here, can only be measured. Keeping these out of `THEORY.md` is what lets
  Law 1's two-side claim stay literally true.
* **`SEMANTICS.md`** — the ⟦·⟧ every square is stated against.

`MECHANISM.md` is **not** one of these. It is the execution plan — R4 is unfinished
(R4.15, R4.12, and the tracked residuals) and it stays until that ladder closes.
What comes OUT of it as purity work proceeds is the durable half: theorems into
`THEORY.md`, spec constants into II-*, measured facts into `MECHANISM.md` Part F, ⟦·⟧
definitions into `SEMANTICS.md`. It gets thinner, not deleted.

#### A citation is a reading path, not a lint

zcc is a teaching compiler. A student who lands on any line should be able to
read upward to the theorem it realizes, so a citation is written for a person:

```rust
// src/mir/pass/ldstp.rs
// THEORY A6b — MIR, the machine layer
// THEORY II-5 — DDI 0487 C6.2.130, the paired forms
// SQUARE      — a_pair_replaces_two_adjacent_accesses
```

The script only checks that what a person reads is true.

#### Why the vacuity check is the one with teeth

A commuting square holds **vacuously** for a pass that never fires. A test that
checks only `⟦f⟧ = ⟦P f⟧` therefore stays green when the pass does nothing at
all — which is how §17 came to carry eight ✔ marks that were measurably false,
and how `scev::compute_trips` was wrong for weeks under a green gate.

So a square must assert **both** halves: the equivalence (a battery helper, or
`⟦·⟧` named in its own failure message) **and an effect** — at least one
assertion of its own about what the pass DID. A body that calls `same_all([…])`
and asserts nothing else has checked exactly one thing: that the pass did not
break the program.

---

### State — 2026-08-26

```
provenance: 55 modules, 58 constants, 21 passes; 25 distinct citations
PROVENANCE PASS (every LOC in theory ∪ fact; every pass squared, none vacuous)
```

#### What the audit found

Six real defects, none of which the full gate could see:

1. **`mir/pass/cmpelim.rs` shipped with NO commuting square.** A pass in the
   default pipeline since R3, with no proof at all. Written:
   `an_arithmetic_result_needs_no_second_compare`, which also pins the condition
   rewrite (`lt` → `mi` once `subs` sets V from the arithmetic).
2. **Four vacuous squares** — `frame`, `layout`, `ldstp`, `legalize` — each
   calling a battery helper and asserting nothing else. All four would have
   stayed green for a pass that did nothing. Each now asserts its effect.
3. **`ladder_is_idempotent_at_the_fixpoint` had the effect half and not the
   equivalence half.** It proved the ladder reaches a fixpoint without ever
   proving the fixpoint means what the source meant — a ladder that miscompiled
   identically on both runs satisfied it.
4. **`layout_preserves_every_edge` is misnamed**, and the effect half is what
   exposed it: layout THREADS empty blocks, so a predecessor's successor changes
   (bb2: `[11,12]` → `[2,12]`). The edge set is not the invariant; the run is.
5. **Constants with no provenance**, now each carrying one — and four that have
   no spec to cite are labelled honestly in `MECHANISM.md` Part F rather than given
   invented citations: `MIN_CASES` (M4, measured INCONCLUSIVE), `WINDOW` (M5),
   `MAX_HEADER_INSTS` (M7, gcc's `--param`, not a spec), `ARM_LIMIT` (M8,
   reasoned, never swept).
6. **A missed pairing, found while writing the square.** `p->a + p->b` emits two
   adjacent loads off one base and does not pair: `ldstp::fuse` refuses when a
   destination equals the base register. DDI 0487 C6.2.130 constrains that only
   for the **writeback** forms — plain `ldp x1, x0, [x0]` reads the base once to
   form the address and is well defined. Recorded, not fixed: a correctness-
   sensitive ISA change does not belong at the end of a long session.


---

# Part C — compile speed

## `MECHANISM.md` Part C — the compile-speed campaign (transient)

> **Lifecycle (anti-bloat).** This is a TRANSIENT working doc, the compile-speed twin of `OPT.md`'s
> role for the optimizer. It holds the plan + scoreboard for the §CP campaign only. **Delete it when
> the campaign closes.** Before deleting, cook the durable results into the permanent record: any new
> algorithm that becomes load-bearing (bitset liveness, worklist dataflow, memoized SCEV) is a
> Side-I theorem and its provenance belongs in `THEORY.md`; the measured phase profile and final
> baseline belong in `MECHANISM.md` §13/§CP-closeout. `MECHANISM.md` keeps only a one-line pointer here
> while this runs, and that pointer is removed at deletion. This doc introduces NO new plan
> numbering that forks the R-ladder — it is the CP2.x detail of one side-campaign, edited in
> place here (anti-fragmentation law still binds).

### §CP — THE COMPILE-SPEED CAMPAIGN (opened 2026-08-25; a side campaign, orthogonal to R4)

**Why.** Gating R4.2 surfaced **6 yarpgen CTIMEOUT** (compile > 300 s: s0007, s0025, s0035, s0075,
s0231, s0228) where the fuzz suites are meant to be ~constant. Isolated to the OPTIMIZER + BACKEND,
not R4.2 (that change is in `destruct`, after the optimizer; `ZCC_O0=1` compiles s0007 in **12 s**
vs **259 s** opt-on). The trigger is a class of yarpgen function that is pathologically large — `init`
in s0007 is **7,266 blocks / 27,999 values / 1,643 loops / 1,643 SROA pieces** (sqlite's largest is
6,231 blocks / 59 loops / 328 pieces) — and several passes plus the register allocator are
super-linear in one of those dimensions.

**Goal (user, 2026-08-25):** MAINTAIN MAXIMUM OPTIMIZATION — no output change, no de-optimizing size
cap. Replace every super-linear site with the right algorithm (N² → N log N → N), trading MEMORY for
speed where it helps (bitsets, hash indices, incremental maps). The per-fix gate is **byte-identical
`.s`** over a corpus: identical bytes ARE the proof that output speed/size are untouched (the dual of
the refactor gate). A size cap that skips a pass is NOT allowed here — that is a different tool and it
loses optimization.

**FIRST, THE BUILD FACT (Law-2 measurement exception).** Every alarming compile number this session
was a **debug** zcc. `tests/box.sh` / `tests/fullsuite.sh` build the musl ELF debug; debug Rust is
**~9× slower**. Measured in-box (aarch64 musl), sqlite `-O1 -S`, byte-identical output (217,160 insns):
**debug 112 s → RELEASE 12 s**; old-main (rc3) debug was ALSO 112 s (no branch regression). The 6
yarpgen "CTIMEOUT" seeds: debug 259–300 s → **release 36–56 s, 0 CTIMEOUT**. gcc-O1 in-box = 7 s, so
release zcc is **1.7× gcc**. **Rule: TIME with a release zcc.** So §CP is a POLISH (12 s → ~7 s), not
a fire — but the quadratics below are real and DO scale the 12 s.

**MEASURED phase profile (RELEASE, `ZCC_TIME=1`, phase totals over the whole module):**

| phase | sqlite -O1 (~12 s) | s0025 -O1 (~29 s) | share |
|---|---|---|---|
| **`regalloc` (of which `spill`)** | **6.7 s (spill 6.1 s)** | **18.6 s (spill 18.5 s)** | **51 % / 64 %** |
| `hir::pass` (the HIR optimizer) | 3.2 s | 7.4 s | 27 % / 25 % |
| `mir::pass` | 0.7 s | 3.3 s | 6 % / 11 % |
| isel · emit · frame · verify · cfg · domtree | each < 0.2 s | each < 0.03 s | negligible |

**The spiller is HALF the compile.** `regalloc::spill::spill_with` (`src/regalloc/spill.rs`, 1495 lines)
is #1 by a wide margin on BOTH real code and the fuzzer monster — its `for _ in 0..bound` fixpoint
re-runs an O(function) decision over `BTreeSet`s (log-factor everywhere), so it is at least
O(bound × n log n). That is the campaign's first target, ahead of everything HIR.

**Root-cause anatomy of the spiller (`spill_with`, measured this session).** The fixpoint reruns the
whole pipeline from scratch every round:

```
for _ in 0..bound {              // bound = f.vregs.len() + 2   (s0007: ~28k)
    cfg = crate::mir::verify::cfg(f);   // CFG rebuilt from scratch every round
    lv  = live::compute(f, &cfg);       // full BTreeSet dataflow, clone/block/round
    simulate(f, &lv, &cfg, &spilled, …) // full-function pass, BTreeSet residency
}
```

Three compounding costs: (1) CFG + liveness are recomputed over the whole function EVERY round
though block structure is loop-invariant until `apply`; (2) `live::compute` is a `while changed`
round-robin over all blocks with `BTreeSet<usize>` cloned per block per round (log factor +
pointer-chase + allocation); (3) `spilled` / `physlive` / the simulate residency sets are all
`BTreeSet`. Liveness is the KEYSTONE — it runs inside this fixpoint AND is used again by `color.rs`,
so fixing it once pays in two places.

**Measured catalog (worst wall-time first; each fix must be byte-identical):**
| site | cost class | fix (memory-for-speed) | output |
|---|---|---|---|
| **`regalloc::spill::spill_with`** | **#1 — 51 % (sqlite) / 64 % (s0025)**; `for _ in 0..bound` fixpoint × O(n) BTreeSet work | bound the fixpoint / dirty-worklist; BTreeSet→bitset/Vec where order is not needed | identical |
| `hir::pass` (the optimizer, all rounds) | #2 — 27 %; the sroa/rotate/licm/scev O(n²) sites below live here | the rows below | identical |
| `mir::pass` | #3 — 6–11 % | profile which MIR pass | identical |
| `sroa` mem2reg DF construction | O(preds × domdepth × `df.contains`-Vec) | bitset frontier + Cytron IDF | identical |
| `LoopForest::new` nesting | O(loops² × body) + per-header `vec![false;n]` | near-linear parent (of[]-based), reused scratch | identical |
| `rotate::force` | O(iters × full CFG/dom/loop rebuild) | batch, or incremental invalidation | identical |
| `licm` hoist scan | O(hoists × body) restart-scan | worklist, not restart | identical |
| `scev::eval_fuel` | unmemoized, up to 2^16 per `eval` on DAGs | memoize `(ValueId, fuel)` | identical |
| `sroa` `ever.contains` | **✅ SHIPPED 3894fb5** — Vec→bitset, reused across pieces | — | identical |
| `licm` `refresh_defs` | **✅ SHIPPED 3894fb5** — full-`Func` per hoist → scoped `refresh_block_defs` | — | identical |

**Baseline table (RELEASE, in-box, sqlite `-O1 -S`):** gcc 7 s · **zcc 12 s (1.7×)** · target **≤ 7–10 s**
(user's sufficiency bar). The two shipped fixes are IN this 12 s; the spiller is where the next ~5 s is.

**Shipped with R4.2 (byte-identical, "minor compile-speed" per the bank):** the two ✅ rows — `sroa`'s
IDF `ever`/`seen` bitmaps and `licm`'s scoped `refresh_block_defs`. Verified output-neutral: sqlite
**217,160 insns unchanged**, opt-parity 1552/0, torture 0 FAIL, determinism 86×8. On the suite they
cut licm on yarpgen `test` from **10.7 s → 2.4 s** and dropped the CTIMEOUT count under a session's
worth of guard experiments from **6 → 1** (s0025, backend-bound). NOT shipped: the large-function
size guards trialed this session — they de-optimize and violate the campaign goal; the algorithm
fixes below replace them.

### Status

- **Phase 0 (profiler) — DONE.** No new instrument; the pipeline's existing `ZCC_TIME=1` phase timers
  gave the profile above. Reproduce with any release zcc: `ZCC_TIME=1 <compile>` then group the
  `[time]` lines per phase.
- **Phase 1 (rank by measured wall-time) — DONE.** The catalog table is the ranking.
- **Phase 2 (the algorithm fixes) — the CP2.x ladder below. NOT STARTED.**
- **Phase 3 (re-measure) — after each bank + at close:** release sqlite (target ≤ 7–10 s) and the 6
  yarpgen seeds, output byte-identical.

### Phase 2 — the CP2.x ladder (worst-first; each step byte-identical gated)

Ordered by measured share. The spiller is > half the compile, so CP2.1–2.4 come first; within them the
**bitset + worklist liveness (CP2.2) is the keystone** — the single biggest lever, reused by `color.rs`.
Then the HIR sites by their 27 % share, cheapest-high-value first (the exponential SCEV memo).

| # | site | current cost | industrial fix (trade = memory) | class | status |
|---|---|---|---|---|---|
| **CP2.1** | spiller fixpoint invariants | rebuild CFG + liveness every round | build CFG once above the loop (topology invariant across rounds); liveness stays per-round | bound× → 1× | **✅ banked** |
| **CP2.2** ⭐ | `live::compute` (keystone) | `while changed` round-robin + `BTreeSet` clone/block/round | **predecessor worklist** (re-queue preds only when `live_in` changes — Kildall), seeded reverse-RPO | fewer visits | **✅ banked** |
| **CP2.3** | spiller `spilled` set | `BTreeSet<VReg>` contains on the per-operand hot path | dense `Vec<bool>` over the (fixed) vreg index; `physlive` left as-is (order-iterated) | log→O(1) | **✅ banked (small)** |
| **CP2.4** | spiller `simulate` per-call cost | s0025 spill 16.5 s = 3 rounds × `simulate(6555 blk, 10039 spilled)` — the cost is INSIDE `simulate`, NOT round count (measured, `ZCC_ROUNDS`) | profile `simulate`; cheapen the per-point work; a dirty-worklist only caps at 3→1 | needs profile | ⬜ **NEXT — profile-first** |
| **CP2.5** | `scev::eval_fuel` | unmemoized, up to 2^16 per `eval` on DAGs | per-call memo `(ValueId, fuel)` | **exp→linear** | **✅ banked** |
| **CP2.6** | `LoopForest::new` | per-header `vec![false;n]` | one `mark` scratch reused, cleared by body | O(loops×n)→O(Σbody) | **✅ banked (scratch)** |
| **CP2.6b** | `LoopForest::new` parent nesting | O(loops²×body) `body.contains(header)` | per-loop membership bitset → O(1) contains (O(loops²)) | N²×body→N² | ⬜ |
| **CP2.7** | `rotate::force` | full CFG/dom/loop rebuild per rotation | batch rotations, or incremental invalidation | iters×N→N | ⬜ |
| **CP2.8** | `licm` hoist scan | restart-scan per hoist | worklist, no restart | hoists×body→N | ⬜ |
| **CP2.9** | `sroa` DF construction | `contains`-bitmap shipped; DF build still O(preds×domdepth) | Cytron IDF + bitset frontier | N²→~N | ⬜ |
| **CP2.10** | `destruct` parallel-copy seq | `.position().any()` (spill.rs/destruct.rs ~498, 312) | Boissinot in-degree worklist sequencing | N²→N | ⬜ |
#### The witness this campaign needed, and did not have (2026-08-28)

The byte-identical gate runs 58 small programs. Six `inline` rows passed it and
sqlite's assembly still moved: the gate-passing compiler emits
`c655fe3e83f79da3a1ddfa83c50e2c06` (289,478 lines) and the rows produced
`4f1b49325f69ce5efcf0abe68d7da714`. Nothing in the corpus inlines the way a
250,000-line translation unit does, so nothing in it could see the difference.

Two lessons, both paid for:

  * **A corpus proof is scoped to the corpus.** For a pass whose behaviour scales
    with function size, a green gate on small programs is not evidence. The
    reference `.s` for sqlite costs one slow run (5,332 s with the defect in
    place) and then every candidate is a one-second `cmp`. Take it FIRST.
  * **Never chain the baseline.** The hash the rows were compared against was set
    by the first candidate, not by the reference — so the chain agreed with
    itself while drifting away from the compiler it was supposed to reproduce.

`cfg`'s branch-threading carries the same warning from the other direction: it
was batched twice, and both times all 58 programs stayed identical while sqlite
changed. The second attempt rebuilt the use-count table from scratch after every
rewrite, which proves the cause is the interleaving of `run`'s six identities and
not stale bookkeeping — that fixpoint is not confluent.

| **CP2.11** ⭐ | `inline::run_module` — THE WALL, found 2026-08-28 | sqlite did not finish in 20 min against gcc -O1's 6 s. Five whole-program costs, all inside the per-splice `loop`: `live_across` (a whole-function dataflow) every splice; `has_loop` (CFG+domtree+loops of the CALLEE) and `body_size` and `inlinable` per CANDIDATE; `loop_blocks` (CFG+domtree+loops of the caller) every splice; and the site scan restarted from block 0 after every splice | (a) liveness asked LAST and only where it can win, at most once per splice; (b) per-callee facts memoised, invalidated only for the caller a splice rewrote; (c) `inloop` carried across splices — a splice appends, so the new blocks lie in exactly the loops `b` lies in; (d) the set is a **sparse set** (Briggs–Torczon, generation-stamped clear) not a `HashSet` and not a bitset — both of those are sized by how many values EXIST while the live set is a few dozen; (e) the scan RESUMES at the last site taken, since a refusal is a property of the callee and does not change | S×(N+V) → ~N | 🔬 **in flight — each step byte-identical (58 programs)** |

**Overlap guard (already shipped, do NOT redo):** the `sroa` `ever/seen`-`contains` bitmap and the
`licm` scoped `refresh_block_defs` landed in 3894fb5. CP2.9 is the REMAINING sroa work (the IDF/DF
construction algorithm), and CP2.8 is the REMAINING licm work (the hoist restart-scan) — neither
touches the shipped code.

### Scoreboard — first batch banked (2026-08-25)

CP2.1, CP2.2, CP2.5, CP2.6 shipped together (`src/regalloc/spill.rs`, `src/regalloc/live.rs`,
`src/hir/pass/scev.rs`, `src/cfg.rs`). All four are pure algorithm swaps, no output change.

**Measured, in-box (aarch64 musl, RELEASE, `-O1 -S`):**
| target | baseline | after batch 1 | Δ |
|---|---|---|---|
| sqlite wall | 12 s (1.7× gcc) | **9.99 s (1.43×)** | **−17 %** |
| sqlite `spill` | 6.1 s | **4.54 s** | **−26 %** |
| sqlite `regalloc` | 6.7 s | 5.08 s | −24 % |
| sqlite `hir::pass` | 3.2 s | 3.22 s | flat (few loops) |
| s0025 wall | 29 s | **23.14 s** | **−20 %** |
| s0025 `hir::pass` | 7.4 s | **2.69 s** | **−64 %** (scev+loopforest) |

Spill wins come from CP2.1+2.2 (real code); the −64 % HIR win on the yarpgen loop monster comes from
CP2.5+2.6, invisible on sqlite by design. Output identical: sqlite **217,160** insns, s0025 **31,651**.

**Correctness gate (batch 1):** byte-identical `.s` proven over — 57 host corpus (default opt),
7 freestanding stress at `-O1` (loops → scev/loopforest exercised), **1000 csmith at `-O1` patched
vs pristine (0 differ)**, in-box sqlite + s0025 (identical output). torture **1378 pass, 0 FAIL**.
(Full yarpgen-seed sweep skipped in this session — the pathological seeds are ~40 s each and the pure
byte-identical proof already covers the loop path; run it at campaign close.)

**Batch 2 (CP2.3) banked:** `spilled` `BTreeSet<VReg>` → dense `Vec<bool>` (contains on the
per-operand hot path is now O(1); `apply` still mints slots in ascending-vreg order, byte-identical).
Marginal by design: s0025 spill 16,954 → 16,493 ms (−3 %), sqlite neutral (low pressure). The bitset
removed the log factor, but the spiller's dominant cost on the high-pressure yarpgen function is the
NUMBER OF FIXPOINT ROUNDS (each re-simulates the whole function), not the per-lookup constant.

**Measurement correction (Law-2, `ZCC_ROUNDS`):** the spiller fixpoint runs only **3 rounds** on
s0025 (6555 blocks, 10039 spilled) and ≤5 on sqlite's biggest functions — it is NOT round-count
bound. The 16.5 s is `3 × simulate(...)`; the cost lives INSIDE one `simulate` call, superlinear in
the pressure (10039 memory-resident values), not in the number of rounds. So a dirty-worklist that
cut 3→1 caps at −66 % and carries real byte-identical risk (the per-block plan depends on cross-block
entry sets) — it is NOT the first move.

**Next ⬜ = profile `simulate` itself** (`ZCC_TIME`-style coarse timers around its setup vs its RPO
per-point loop, then within the loop) to find the construct that scales with the resident-value count,
and cheapen THAT (same memory-for-speed pattern as CP2.3 — a bitset/index where a set/scan sits on the
per-point path). `physlive` is bounded by the ~32 physical registers, so it is unlikely to be the
sink; the suspect is value-level residency tracking (`w` / `held` / `room`) over the up-to-nsp working
set. Measure before converting. Then CP2.6b / CP2.7–2.10 for the HIR tail.

### The per-fix loop (constitution's iteration process; unchanged)

For each CP2.x, worst-first, one at a time:

1. **Predict** the Δ on the complexity model (state the class change, e.g. `bound×n log n → n·E`).
2. **Baseline first** (TDD-shaped): record the RELEASE `ZCC_TIME=1` time for the target phase on
   sqlite + s0025, and snapshot the md5 corpus.
3. **Implement** the algorithm swap (memory-for-speed; no output change, no size cap).
4. **Gate — byte-identical `.s`** via `tests/refactor_gate.sh` over the fixed corpus (the proof output
   is untouched, Article G refactor dual) **PLUS the full correctness gate** (cargo + torture +
   opt-parity + csmith300 + yarpgen300 + determinism). Byte-identical alone is necessary, not
   sufficient — a correctness regression can still be byte-identical by luck, so the full gate stays
   mandatory.
5. **Re-measure** RELEASE sqlite (target ≤ 7–10 s) + s0025; record the number.
6. **Bank** (commit, number recorded) or, on a wall, quarantine that CP2.x with a `BLOCKED:` note and
   advance — never fork the plan.

**Standing caution (from §CP + §13n):** the allocator is where the nastiest defects live. Any CP2.x
that weakens an allocator invariant ships its verifier check (`mir::verify` virtual mode) in the same
commit. CP2.4's convergence argument is the one at risk of getting hairy — bounded Law-2 attempt; if
the dirty-worklist termination proof does not close, ship the CP2.1–2.3 gains and mark CP2.4 residual.

### Running the campaign

Each CP2.x is near-independent and has an objective acceptance gate (byte-identical + full gate), so
it maps cleanly onto `superpowers:subagent-driven-development`: one fix per subagent, the gate as the
acceptance criterion, `superpowers:verification-before-completion` as the proof-before-bank step.
Keep the scoreboard here (mark each row `✅ banked <sha>` / `BLOCKED: …`), edited in place.

---

# Part D — the spiller

## `MECHANISM.md` Part D — the spill-placement campaign

The plan of record for closing zcc's real-program performance gap. Opened
2026-08-27, after the sqlite-segfault night. Read §0, then start at §3.

---

### §0 BOOT — the one paragraph that matters

zcc's spiller ranks eviction by **raw static next-use distance**. Every
instruction counts 1, whether it runs once or four million times, so the loop
index, the loop pointer and the accumulator get spilled **inside** hot loops
while cold values sit in registers. gcc weights each use by `10^loop_depth` and
therefore never does this. `LoopForest` depth is already computed in
`spill.rs` — it just never reaches the eviction decision.

**This is one missing term in one sort key, not a broken architecture.** Do not
rewrite the allocator (§2).

---

### §1 THE MEASUREMENTS — all taken 2026-08-27, all reproducible

#### The ceiling, proven by hand (`scratchpad/nestjoin.c`, 25 lines)

A nested-loop join with 24 unfoldable values live across the inner loop:

| build | time | output |
|---|---|---|
| gcc -O1 | **1 ms** | 4087392 |
| zcc -O1 | **8 ms** | 4087392 |
| zcc, inner loop hand-edited | **1 ms** | 4087392 |

The hand-edit removes **five instructions** and closes **the entire 8× gap**.
That is the whole campaign in one number: the shape is worth everything.

Before (zcc, 4,000,000 iterations, 6 of 11 instructions are frame traffic):

```
.Ljoinit_6:
    ldr x2, [sp, #80]      <- reload pb       the POINTER
    ldr x3, [sp, #240]     <- reload j        the LOOP INDEX
    ldr w2, [x2, x3, lsl #2]
    cmp w2, w0
    ldr x2, [sp, #144]     <- reload hits     the ACCUMULATOR
    csinc x2, x2, x2, ne
    str x2, [sp, #144]     <- spill hits
    add x2, x3, #1
    str x2, [sp, #240]     <- spill j
    cmp x2, x1
    b.lt .Ljoinit_6
```

After (hoist the three into x4/x5/x7 before the loop, sink after):

```
    ldr x4, [sp, #80]      / mov x5, xzr / ldr x7, [sp, #144]
.Ljoinit_6:
    ldr w2, [x4, x5, lsl #2]
    cmp w2, w0
    csinc x7, x7, x7, ne
    add x5, x5, #1
    cmp x5, x1
    b.lt .Ljoinit_6
    str x7, [sp, #144]     / str x5, [sp, #240]
```

Note what the allocator did: it kept the COLD `c0..c23` in x6/x8/x10/x12/x14/
x15/x20/x22/x24 across the loop and spilled the hot three. Exactly inverted.

#### The same defect at scale — `sqlite3VdbeExec`

| | zcc | gcc -O1 |
|---|---|---|
| instructions | 10,766 | 6,040 (**1.78×**) |
| **distinct frame slots** | **235** | **43** |
| frame accesses | 1,862 | 515 (**3.6×**) |
| reg-reg mov | 1,736 | 484 (3.6×) |
| callee-saved used | x19–x28 (all) | x19–x28 (all) |

+4,726 instructions — **25% of the whole 19,079-instruction sqlite gap in one
function**, and it is the function every query runs. Across functions present in
both compilers zcc is only **1.045×**; the file-wide 1.1238× is mostly gcc
inlining small statics away, which is a different lever entirely.

⚠️ **This corrects a recorded belief.** "zcc spills less than gcc file-wide" is
true *on average* and hid the opposite where it counts. Never judge spilling by
a file-wide average again.

#### The code

`spill.rs::next_use` returns a position from `linear_positions`, which numbers
instructions in reverse-postorder, unweighted. The eviction key is

```rust
cand.sort_by_key(|r| (droppable(r), next_use(&uses, r.v as usize, head)))
```

`lf.depth` appears three times in the file: the fixpoint round budget, a
cold-edge reload placement test, and a reporting histogram (`inloop`, ~line 526).
**Never in the decision.**

#### ⚠️ WHAT THE DEFECT ACTUALLY WAS — measured 2026-08-27, and it is not §0's story

§0 above says "one missing weight in one sort key". That diagnosis was made by
reading. Instrumenting every eviction site (a temporary `eprintln!` at each
`newsp.push`) said something sharper, and a session that trusts §0's wording
will build the wrong mechanism:

```
SPILL joinit site=TERMARG bb29 depth2 v484 nextuse-1 from89   <- inner-loop latch
SPILL joinit site=TERMARG bb31 depth1 v439..v452 nextuse-1    <- outer-loop latch
```

`nextuse-1` is `usize::MAX`. **A back edge runs backwards in reverse postorder**,
so a value carried around a loop is read at a LOWER position than the latch that
passes it on; `partition_point(|&p| p <= from)` finds nothing and `next_use`
answers *never used again* — the strongest possible reason to evict, handed to
precisely the values that are used most. It was not that hot values were
under-weighted. **They were ranked as dead.**

A second blindness sat behind it. mem2reg splits one C variable into a chain of
SSA values joined by block parameters, and every link of that chain has exactly
ONE use: being passed to the next link. Asked of the vreg, "how far to the next
use of `c0`?" and "of `j`?" both answer 1 — twenty-four cold values and three
hot ones become indistinguishable at the exact edge where the choice is made.
Measured: every candidate at the preheader's terminator reported distance 3.

Both are fixed by measuring the distance the way Belady's theorem defines it —
along the TRACE, over the WEB (`spill.rs::Trace`). Neither is a weight.

---

### §2 WHY NOT A REWRITE

The user asked. The answer is no, and the reason is evidence, not conservatism.

CCC (the AI compiler benchmarked at 737×–158,000× on sqlite) needs a rewrite: it
has no allocator, uses "a single shuttle register", and produces 11,000-byte
frames for 32 variables. zcc is 1.4–2.0× on the same program, with:

* SSA-form allocation on a **chordal** interference graph, where greedy colouring
  along a dominator preorder is **optimal in k by construction** (THEORY A7);
* Braun–Hack spilling, live-range splitting, rematerialization, biased colouring;
* a commuting square `⟦mir_v⟧ = ⟦mir_p⟧` and structural obligations checked on
  every compile.

This is the modern design — the same family LLVM uses. A rewrite would spend
weeks re-deriving what A7 already proves and would put every correctness square
back in play; both bugs fixed on 2026-08-27 lived **at allocator seams**. The
measured defect is a cost model, and cost models are replaceable in isolation.

---

### §3 THE METHOD — this is the part that decides success

**Five previous attempts at this area all failed.** Every one of them edited
`color.rs` directly and was reverted: hint-set without re-check; rollback;
excluding the ParallelCopy path; a separate post-colouring pass (fired 0 times —
ABI args are `ParallelCopy` pairs, not `UseFixed`); retargeting (collides on
simultaneity). **Do not retry any of them.**

What worked instead — the method that took geo40 below 1×:

> **Never patch the compiler to test a codegen theory. Hand-edit the `.s`, link
> it, run it, time it. Prove the shape wins first; only then build the mechanism
> that produces that shape.**

So every phase below is: **(a) hand-edit to the target shape and measure the
ceiling → (b) only if the ceiling is worth it, build the minimal mechanism →
(c) prove it with a non-vacuous square → (d) full gate + seal.**

Phase 1's ceiling is already measured (§1): 8 ms → 1 ms. That is why it is
Phase 1.

**Non-vacuity is mandatory.** On 2026-08-27 two fixtures passed with their fix
disabled and were withdrawn. A test that passes without the change is not a
proof; `tests/provenance.sh` exists to refuse exactly that.

---

### §4 THE LADDER

Status lives HERE, edited in place. Do not open a new numbering elsewhere.

| # | row | gate | status |
|---|---|---|---|
| S0 | **A shape-matched kernel in the exec suite** — and, it turned out, an INSTRUMENT that could see it. Two things were hiding this defect from geo40, not one: no kernel in the suite spills, AND the harness timed with `date +%s%N` and then divided by 1,000,000, throwing the nanoseconds away before declaring everything under 5 ms unmeasurable. See §4a. | kernel `k1_vdbe_dispatch` reads **exec 1.939× / insn 1.561×**; timed programs 18 → 25 | ✅ |
| S1 | **The trace-distance model.** ~~Loop-weighted eviction~~ — the measurement (§1) refuted that framing: the defect was the `usize::MAX` a back edge produces, not a missing weight. Shipped `spill.rs::Trace`: Belady's distance measured along the execution trace (a use behind, inside this loop, is one wrap away; a use outside costs the remaining trips) and over the SSA WEB (the granularity at which eviction is paid, since `Sim::More` retires a whole web). | `nestjoin.c` **8 ms → 1 ms = gcc**; inner loop 11 insns → 8, frame ops 6 → 2 | ✅ |
| S2a | **The invariant reload.** ✅ The mechanism that carries a memory-resident value through a loop in a register — the cold-edge phi — was already built and was being REFUSED by its own pruning gate, which asked for a read strictly AFTER the block head when the read is AT the head, and answered `usize::MAX` for a value read only across the back edge. The same trace query S1 installed fixes it. | `nestjoin` inner loop 8 insns → **7**, zero reloads; at 36M iterations **12 ms → 11 ms = gcc's 11** | ✅ |
| S2b | **The accumulator's store.** ⛔ REFUSED BY ITS OWN CEILING, and no code was written — the §3 method working as intended. Hand-edited the store out of `nestjoin`'s inner loop and timed it at microsecond resolution: gcc 11,599 µs, zcc 11,634 µs, zcc-with-the-store-sunk **11,594 µs**. A 0.34% difference, because the store is off the dependence chain and retires into the write buffer (Law 3c: count is not cost, in the direction that says DON'T build it). A store-sinking dataflow pass is not worth 0.34% of one program. | ceiling measured at **0.34%**; not built | ⛔ |
| S3 | **`sqlite3VdbeExec`.** Re-measured after S1+S2a. **Gate NOT met**, and the reason was already on the record: slots fell 244 → **199** (−18%) while the function's ratio moved only 1.823× → **1.786×**, because `excess.sh` had already shown the gap in that function is COPIES, not spill traffic (+10,464 reg-reg `mov` file-wide against +1,741 frame accesses). Spill ranking was never going to close it. | wanted slots <80 (got 199) and ratio <1.2× (got 1.786×) | ⛔ |
| S4a | **The argument registers go last.** ✅ `assign` picks `hint.or_else(alloc_order.find(free))` and `alloc_order` began at x0, so every unhinted value in the function took an argument register before anything else and the argument that wanted it paid a `mov`. Reordering the caller-saved half to x8–x15 then x0–x7 (`MEASURED M13`) — no set, mask or ABI changes. | sqlite 175,407 → **174,677** (1.1167× → **1.1120×**); movs into x0–x7 22,829 → 19,985; geo40 INSN 1.0432 → **1.0301** | ✅ |
| S4b | **Re-colour the occupant.** ⛔ BLOCKED — attempted, refuted by the verifier, and the ceiling it was aimed at turns out not to exist. See §4b. | attempted; **7 recolours in all of sqlite**, −37 instructions | ⛔ |
| S4 | **The copy residual.** +1,252 reg-reg mov in that function. Only after S1–S3, because eviction pressure changes once hot values stop moving. | reg-reg mov in `VdbeExec` < 800 | ⬜ |
| S6 | **A small copy is not a libcall.** ✅ Added IN PLACE, not as a new numbering: S0's instrument made `e3_struct_byval` visible at **2.630× exec**, the worst program in the suite on both axes, and the cause was `isel/lower.rs` lowering EVERY `Inst::MemCpy` to `bl memcpy` — including the 16-byte home of a by-value struct parameter, which made a leaf function build a frame and call libc four million times. Now open-coded up to 32 bytes (`MEASURED M14`, the measured minimum of a nine-point sweep), emitted as two loads then two stores so `ldstp.rs` fuses them. | `e3_struct_byval` 2.630× → **1.953×** (insn 1.724 → 1.621); sqlite 174,677 → **174,572**; suite EXEC 1.0403 → **1.0304** | ✅ |
| S5 | **`ldp`/`stp` pairing.** ⛔ RE-CLOSED, and for a different reason than the first time — the row's premise was arithmetic that did not hold. "gcc emits 12,637 pairs to zcc's 7,616" counts gcc's PAIRS as if each one zcc lacks were an instruction zcc could delete, but a pair only saves an instruction when the two accesses exist. Counted properly, zcc emits **22,070 frame instructions to gcc's 24,720** — zcc is **2,650 AHEAD**; gcc has more pairs because it has 7,009 more frame accesses, i.e. it spills more. The real quantity is efficiency (0.757 instructions per access against 0.683), of which the census says ~1,009 are reachable. The first closure blamed gcc's lead on SCHEDULING; that was asserted, not measured, and it is false: at `-O1` gcc has `-fschedule-insns2` disabled, and forcing it on moves sqlite by 2 instructions and 0 in total count. | true ceiling ~1,009 pairs, not 5,130 | ⛔ |

#### §4a S0 — what was actually wrong with the instrument

**The suite could not see the defect for two reasons, and only one was planned
for.** The first is the one this row was written about: every geo40 kernel fits
in the register file and spills nothing. That is now proven rather than assumed —
the whole 35-program corpus is byte-identical across a 1000× sweep of the
spiller's one cost constant (`MEASURED M12`), which is only possible if no
allocation decision in any of them is pressure-bound.

**The second was the harness.** `exectime.sh` timed with `date +%s%N` — a
nanosecond clock — and then wrote `(t1-t0)/1000000`, truncating to whole
milliseconds, with a shell `fork` for `date` sitting between the two readings.
On the strength of that truncation it declared everything under 5 ms
"startup-dominated" and skipped it. **Fifteen of the thirty-five programs never
produced an exec number at all.** The resolution was never missing from the
machine: `clock_gettime(CLOCK_MONOTONIC)` is a vDSO read here, the counter
behind it runs at 24 MHz (41.7 ns/tick, 0.5% run-to-run over ten million
iterations), and the real floor — `fork`+`execve` of `/bin/true`, best of 20 —
is **189 µs**, reproducible to the microsecond. `tests/bench/timeit.c` measures
that floor on every run and prints it, so the cutoff is a measured number times
a margin rather than a constant someone chose.

What that changed, at the SAME tree:

| | old instrument | µs instrument |
|---|---|---|
| programs timed | 18 | **25** |
| EXEC geomean | 0.9500 | **1.0165** |
| worst exec | `d2_nested_loops` 1.111 | `e3_struct_byval` **2.642** |

⚠️ **The sub-1× reading was substantially an artifact of the skipping.**
`e3_struct_byval` was reported as `fast` and dropped; it is 2.6× slower than
gcc. `a2_udiv_mod`, `a3_sdiv_mod` and `a4_shift_mask` were dropped; they are
1.11–1.14×. A geomean over the 18 programs that survived a 5 ms floor was not a
statement about the suite. This is Law 3c's own warning arriving from an
unexpected direction: the narrow surface was narrower than anyone had counted.

**The kernel.** `tests/bench/suite/k1_vdbe_dispatch.c`, generated to the spec
measured from `sqlite3VdbeExec` itself (8,363 lines at amalgamation line 93,917;
**196 arms in one switch**; **42 for/while loops inside them**; brace depth 9;
a VM-state set live across every arm; per-arm locals with mutually exclusive
live ranges). Arms are heterogeneous by construction — integer chain,
struct-field chasing, byte/short work, double arithmetic, compare-and-select —
because a uniform body measures one lowering row 196 times instead of a
dispatch.

**Admission, and the honest shortfall.** Step 3 asked for 1.7–1.8×. On the
arbiter axis it exceeds that: **exec 1.939×**. On instructions it reaches
**1.561×** against the real function's 1.794×, with 86 zcc frame slots to gcc's
37 (the real pair is 199/43). Seven parameter settings were swept; the
instruction ratio plateaus at 1.5–1.6, and adding calls to the arms — VdbeExec
is the most call-dense function in sqlite — LOWERED it, because argument
marshalling costs gcc as much as zcc per call. The residual is heterogeneous
hand-written code over a large frame, which a generator does not reproduce. The
program carries the shape and the exec ratio; the last 0.23× of the instruction
ratio lives in sqlite, where `realprog.sh` measures it.

**Step 4 — the suite is re-baselined and the old numbers do not compare.**
geo40 becomes geo41. At HEAD, 36 programs: **EXEC 1.0403 over 25 timed** (median
1.004, worst `e3_struct_byval` 2.630, 6 above 1.1×) and **INSN 1.0421 over all
36** (median 0.991, worst 1.724, 12 above 1.1×). Never compare either against
0.9494×, 0.9565× or 0.9500×: those are a different program set read through a
different instrument.

#### §4b S4b — why the 8,784 was never a ceiling

The row was aimed at a number the colourer prints itself: of sqlite's 14,764
hints refused because the wanted register was OCCUPIED, **8,696 have an occupant
that "dies in this block"**, and the statistics replay says a register is free
across that occupant's whole range in 100% of them. The plan read that as 8,696
removable copies.

**It is not, and the instrument's wording is what misled it.** `HINT_OCC_LOCAL`
tests ONE condition — the occupant's LAST USE is in this block — and labels the
result "locally evictable". A value can die in this block and still have been
LIVE-IN, with its range reaching back through dominating blocks the colourer
walked earlier and keeps no occupancy record of. Recolouring one of those
changes its register in those blocks too, where the new register is very likely
taken.

That is not a deduction; it is what happened. The mechanism was built —
a per-point occupancy history so a refusal could ask what was busy in the part
of the occupant's range already walked — and on the first real program
`regalloc::verify` stopped the compile:

```
unixShmSystemLock: V(4) and V(25) are both live at bb0[3] and both hold Gpr9
```

Restricted to the genuinely local case — occupant DEFINED in this block, dying
in this block, not live-out — it is correct, the full corpus passes, and it
fires **7 times in the whole of sqlite** for **−37 instructions**. Seven, against
a claimed eight thousand.

**So the lever needs global interference**, which this allocator deliberately
does not carry: chordal colouring in dominance order is optimal in k precisely
because it never revisits (THEORY A7). Getting it would mean an interference
graph or live-range splitting at colouring time — a different allocator, not a
row. Reverted; the ~150 lines are not worth 37 instructions and they carry an
edge the verifier had to catch.

**What to fix instead of retrying this.** The instrument should say what it
measures. `HINT_OCC_LOCAL` should require defined-here AND dying-here before it
calls anything "locally evictable", so the next reader is not handed an 8,696
that means something else. Until then, treat that column as an upper bound on an
upper bound.

⚠️ §3 said five previous attempts in this area were refuted. This is the sixth,
and it is the first that says WHY in a form the next session can check: the
number in the report is not the number of removable copies.

---

### §4c THE NEXT SESSION STARTS HERE — pointer-residency, NOT copy-coalescing

> **VERDICT 2026-08-27 (supersedes the copy-coalescing framing below; full
> derivation `MEASURED M21`).**
> 1. **Full gate GREEN on `slotmerge.rs`** — `FUZZ_N=300 fullsuite.sh all` =
>    15 PASS / 0 RED (determinism ✅, csmith 254/0, yarpgen 300/0, musl ✅).
>    §4c item 1 discharged.
> 2. **The copy-coalescing campaign is CANCELLED.** The 283 cs←cs `mov`s are
>    COLD (100% branch to `abort_due_to_error`/`no_mem`) — a size cost, ~0
>    speed. M20's "these execute" was the Law-2 measurement exception.
>    libFIRM co-heur would buy SIZE only; not built (toggle-off if ever authored).
> 3. **The hot lever is pointer RESIDENCY.** gcc keeps p/pOp/pC register-
>    resident across the dispatch; zcc reloads them, and pOp's reload gates the
>    mispredicting jump-table branch. Proven: keeping pOp resident moves the
>    canonical `realprog.sh` geomean **1.1661× → 1.1553×** (+0.9%), size-neutral.
> 4. **No smash-and-grab remains.** `OP_Column`/`OP_Next` carry no structural
>    defect — only systemic spilling. Path to lower = a gated residency pass
>    (keep p+pOp+pC resident at the dispatch join), est. +2–4% → sqlite ~1.12×.
>    1× is not reachable by one trick on this surface.

**State at hand-off.** sqlite exec **1.159×** gcc -O1 (was 1.651 at the start of
2026-08-27). Size 1.1052×. The 42-program suite 1.0206. Everything in `§6` is
measured; do not re-take it.

**BEFORE ANYTHING ELSE.** `mir/pass/slotmerge.rs` is committed but the FULL GATE
WAS NOT RUN on it — the session ended first. It has: cargo 186/0, provenance
PASS, `localize.sh`'s output check green on sqlite (which is what caught its
predecessor's miscompile), and `determinism` NOT run. **Run
`FUZZ_N=300 sh tests/fullsuite.sh all` first, before adding anything.**

**THE TARGET, measured (`MEASURED M20`).** In `sqlite3VdbeExec`:

```
mov <callee-saved>, <callee-saved>      zcc 325   gcc 8     <- the gap
mov <callee-saved>, x0..x7 after a call zcc  38   gcc 26    <- ABI-forced, near-equal
mov -> x0..x7 (argument marshalling)    zcc 645   gcc 379
```

The excess is NOT the ABI. A result live across a later call must move to a
callee-saved register and gcc does that too. What zcc does 325 times and gcc 8 is
shuffle a value from one callee-saved register to another — coalescing failure,
and unlike the frame rows these copies EXECUTE.

**THE ORDER OF WORK, and step 1 is not code.**

1. **MEASURE THE CEILING BY HAND.** Take one hot arm of the dispatch, delete its
   callee-saved shuffles in the `.s` by renaming registers, link, check the
   output, time it. That number decides whether the campaign is worth 7 points or
   1. Everything on 2026-08-27 that skipped this step was refuted; everything
   that did it shipped. `MEASURED M20` says 325 is an upper bound on what the ABI
   does not force, NOT on what a colouring could avoid.
2. **Diagnose ONE shuffle.** Why did the colourer put the value somewhere its
   copy partner is not? `ZCC_HINT=1` already reports the refusals; the answer for
   the block-local case is in `§4b` and it is that recolouring the occupant needs
   interference the allocator does not carry.
3. **The mechanism, if the ceiling justifies it.** Post-colouring recolouring
   with WHOLE-FUNCTION occupancy, which is what `§4b`'s attempt lacked: build,
   per physical register, the set of program points where it is held, then for a
   copy `D = S` recolour `D` to `S`'s register when that register is free across
   `D`'s entire live range and the caller/callee partition allows it. The copy
   becomes a self-move and `destruct::sequentialize` already deletes those.
4. **Verify with `localize.sh` before timing anything.** It compares program
   output against the gcc build and refuses to report a number otherwise. It is
   the only instrument in the tree that caught the slot-merge miscompile — 185
   unit tests and all 42 suite programs passed it.

**WHAT IS NOT THE PATH TO 1×, measured on 2026-08-27 so nobody re-tries it:**

* frame-size work. Slot coalescing took `VdbeExec` from 203 slots to 116 and cut
  6,832 bytes of stack; the clock moved 1.279 → 1.276. Fewer ADDRESSES is not
  fewer ACCESSES, and the access count (1,629 against gcc's 598) is what costs.
* cold-path work. The rotation gate removed 662 instructions from loops that by
  definition do not execute.
* `madd`→shifts, `cset`/`cmp` folding, dispatch reordering, dispatch trees,
  small-struct SROA, invariant-constant hoisting — each priced by hand-edit and
  each worth ~1% or less. `arm64_elf.md` §6.1 records why.

**THE HONEST SIZING.** `VdbeExec` is ~47% of the remaining 15.9 points ≈ 7.5.
The tail (`MemShallowCopy` ~8%) is ~1.3. The rest is below the attribution
instrument's noise floor, which is what a systemic allocation problem looks like
from a distance. **1× is not reachable without this campaign**, and it may end at
"chordal colouring in dominance order cannot revisit, so this needs a different
allocator" — which is an architectural decision, not a row.

---

### §5 HOW TO JUDGE

* **Speed on real programs, not instruction count** (Law 3c). `realprog.sh` per
  phase, and `bench/quickapp.sh` for the SQL statements.
* **Both microarchitectures.** Apple silicon and Graviton disagreed by 40% on the
  same binary (geomean 1.45× vs 2.03×). A win on one is not a win.
* **geo40 must not regress.** It stands at 0.9494× (tag `rc5`). Loop-weighted
  eviction moves pressure *out* of loops and therefore *into* straight-line code;
  the kernels are where that shows up first.
* **A full seal, not the 300-seed gate.** S1 changes what every function spills.
  `c04804` (over-k panic at a `pcopy`) was a one-in-ten-thousand event that the
  300-seed gate never saw. Budget a 10k csmith + 10k yarpgen run on us-east-2 —
  and **tear the box down and verify** (0 instances, 0 volumes, 0 spot requests).

**Abandon criteria.** If S1's real yield is under 20% of the measured ceiling
after one bounded attempt, mark it `BLOCKED: <reason>`, revert to green, bank
anything positive, and advance. A blocker never authorizes a new direction.

---

### §6 THE NUMBERS ARE ALREADY TAKEN — DO NOT RE-TAKE THEM

Everything in §1 was measured on 2026-08-27 against `2d6461a`. **A later session
must not re-measure any of it to "confirm".** Re-measuring a recorded fact costs
an hour, produces the same number, and is the single most common way a session
spends itself without moving the ladder. The facts:

| fact | value | source |
|---|---|---|
| `nestjoin.c` gcc -O1 | 1 ms | §1 |
| `nestjoin.c` zcc -O1 | 8 ms | §1 |
| `nestjoin.c` zcc, hand-edited loop | 1 ms | §1 — **the ceiling** |
| `VdbeExec` distinct frame slots | zcc 235 / gcc 43 | §1 |
| `VdbeExec` instructions | zcc 10,766 / gcc 6,040 | §1 |
| sqlite file ratio | 1.1238× (173,176 / 154,097) | §1 |
| functions in both compilers | 1.045× | §1 |
| `ldp`/`stp` file-wide | zcc 7,266 / gcc 12,305 — ⚠️ **NOT a 5,039 opportunity**, see `MEASURED M15`: counted as instructions rather than pairs, zcc emits 22,070 frame instructions to gcc's 24,720 and is 2,650 AHEAD | S5 |
| geo40 EXEC geomean | **0.9565×** — SUPERSEDED, see §4a: 18 timed under a 5 ms floor | 2026-08-27 |
| **geo41 EXEC geomean** | **1.0403×** (25 timed at a 189 µs floor, median 1.004, worst `e3_struct_byval` 2.630, 6 above 1.1×) | 2026-08-27 |
| **geo41 INSN geomean** | **1.0421×** (all 36, median 0.991, worst `e3_struct_byval` 1.724, 12 above 1.1×) | 2026-08-27 |
| geo40 INSN geomean | **1.0432×** (deterministic, all 35, worst `e3_struct_byval` 1.759×) | 2026-08-27 |
| geo40 worst exec | `d1_switch` 1.111× | 2026-08-27 |
| realprog total | 1.410× Apple / 2.03× Graviton | report |

#### ⭐ THE HEADLINE — sqlite exec 1.651 → 1.159, and what actually did it

Three interleaved runs of each binary, `realprog.sh` at microsecond resolution,
session start (`d85aac9`) against `5ed5648`:

| | session start | after the jump-table row |
|---|---|---|
| **SQL geomean, 11 phases** | 1.6282 / 1.6743 → **1.651** | 1.1524 / 1.1646 → **1.159** |
| TOTAL (sum-weighted) | 1.490 | **1.164** |
| worst phase `p01_insert` | 2.818 / 2.593 | **1.313 / 1.301** |
| median phase | 1.551 / 1.578 | **1.154 / 1.147** |
| phases above 1.1× | 10 of 11 | 9 of 11 |

**65% slower than gcc -O1 became 16% slower, from one condition in `isel`.**
`sqlite3VdbeExec` dispatches 196 opcodes and every arm carries edge arguments,
so the jump-table row refused it and the hottest dispatch in the program was a
183-deep linear compare chain, walked ~1.4 million times per 100,000-row INSERT.

**AND HERE IS THE LESSON, which cost a day to learn.** The seven rows shipped
before it — trace-distance eviction, the phi gate, argument registers last,
inline of composite parameters, the parameter-copy elision, if-conversion —
were all real, all gated, all measured on their own programs, and together they
moved sqlite **by nothing** (1.679 → 1.649, ranges overlapping). Every one had
been aimed at a KERNEL, because kernels are the only programs small enough to
diff by hand. The row that moved sqlite was aimed at sqlite.

The chain that found it, in order, and none of the steps is skippable:

1. `localize.sh` — attribution by linker: **85% of the gap in one function**
   (`MEASURED M16`). Static instruction counts had said `VdbeExec` was 25% of
   the *size* excess; they could not say it was 85% of the *time*.
2. `xray.sh` — that function's mnemonic histogram against gcc. **Necessary but
   not sufficient**: a histogram names CLASSES, not sites. Three hypotheses
   drawn from it (`madd`→shifts, `cset`/`cmp` folding, dispatch reordering) were
   built or hand-edited and each refuted at ~1%.
3. **Narrowing the window.** `EXPLAIN` gave the opcodes the workload actually
   runs; counting `br` in the two assemblies gave the answer in one line —
   gcc 1, zcc 0.

Step 3 is the one that mattered, and it is the cheapest of the three.

#### The pre-jump-table state, kept because it is what the lesson is about

Three interleaved runs of each binary, `realprog.sh` at microsecond resolution,
session start (`d85aac9`) against HEAD (`47f8e77`) — seven shipped rows apart:

| | session start | HEAD |
|---|---|---|
| **SQL geomean over 11 phases** | 1.7179 / 1.6558 / 1.6620 → **1.679** | 1.6493 / 1.6363 / 1.6626 → **1.649** |
| TOTAL (sum-weighted) | 1.498 / 1.477 / 1.548 → 1.508 | 1.474 / 1.424 / 1.500 → 1.466 |
| worst phase | `p01_insert` 2.67–3.01× | `p01_insert` 2.79–2.92× |
| phases above 1.1× | 10–11 of 11 | 10–11 of 11 |

**The ranges overlap** (old 1.656–1.718, new 1.636–1.663), so a 1.8% shift
against a 3.7% spread is not a result. Say it plainly: the session moved the
taxonomy suite from 1.0400 to 1.0190 and sqlite's SIZE from 1.1216× to 1.1085×,
and did not measurably move real sqlite EXECUTION.

⚠️ **THE STANDING LESSON, and it is the one to read first.** Every row shipped
today was aimed at a shape found in a KERNEL — a by-value struct parameter, a
parser's dispatch arm, a nested-loop join. Each was real and each paid on its own
program. None of them was aimed at sqlite, and sqlite did not move. The 1.11×
size against 1.65× exec split said this in advance: **the remaining real-program
gap is not instruction count**, so rows found by counting instructions cannot
close it.

**What that makes necessary.** A localizer — WHICH FUNCTIONS carry the 1.65×.
`-DSQLITE_PRIVATE=` already exposes all 1,260 internal functions as symbols in
both compilers, and `objcopy --weaken-symbols=<list>` allows a hybrid link:
weaken every global in gcc's object except a chosen set, weaken exactly that set
in zcc's, link the two, and the chosen functions come from gcc while everything
else comes from zcc. One link and one run per experiment, no recompiles, so
binary-searching 1,260 functions is about eleven cycles. (An earlier attempt to
split the amalgamation into its original translation units failed — 47 of 102
units do not compile because the headers interleave — and `objcopy
--only-section` destroys the symbol table. The weaken-list route avoids both.)

#### After S1 — taken 2026-08-27 with ONE harness across both binaries

The baseline column is not a recorded number: `d85aac9` was rebuilt and run
through the same script in the same box session, because a ratio taken by two
different scripts is not a comparison.

| | before S1 | after S1 | gcc -O1 |
|---|---|---|---|
| `nestjoin.c` best-of-5 | 8 ms | **1 ms** | 1 ms |
| sqlite file instructions | 176,186 | **175,394** | 157,074 (1.1216× → **1.1166×**) |
| `VdbeExec` instructions | 11,014 | **10,841** | 6,041 (1.823× → **1.794×**) |
| `VdbeExec` distinct frame slots | 244 | **200** | 43 |
| `VdbeExec` frame accesses | 1,928 | **1,704** | 598 |
| geo40 EXEC / INSN | 0.9565 / 1.0432 | **0.9474 / 1.0432** | — |

The taxonomy suite's INSN geomean is unchanged **to four decimal places**, and
the whole 35-kernel corpus is byte-identical across a 1000× sweep of the model's
one constant (`MEASURED M12`). That is S0's thesis stated as a measurement: no
kernel in the suite is under enough pressure to spill, so the suite cannot see
this row at all — it can only certify that the row broke nothing.

⚠️ **`realprog.sh`'s ratio is not stable enough to read from one run.** Three
runs of the SAME tree gave totals of 1.390×, 1.467× and (before S1) 1.415×,
while gcc's own total moved 1,181 → 773 ms between them — the box's load
compresses the ratio toward 1. A realprog A/B must interleave the two binaries
in one sequence and be read across runs, never as a single pair. The gate that
DOES resolve S1 is `nestjoin` (8× effect) and the deterministic instruction and
slot counts above.

⚠️ **`exectime.sh` NEEDS `SUITE=`.** It defaults to `/work/tests/bench/suite`
while the repo mounts at `/work/zcc`, and with the wrong path it prints
"EXEC: no timed programs" instead of failing. Always:

```
docker run --rm -e ZCC=/usr/local/bin/zcc -e SUITE=/work/zcc/tests/bench/suite \
  -v "$PWD/target/aarch64-unknown-linux-musl/release/zcc":/usr/local/bin/zcc:ro \
  -v ~/.cache/zcc-suites:/suites -v "$PWD":/work/zcc:ro zcc-box \
  sh /work/zcc/tests/bench/exectime.sh
```

**Re-measure only when the tree has changed in a way that could move the number**
— i.e. AFTER shipping a row, as that row's gate. Never before, and never "to be
sure".

#### The first hour

1. Read `spill.rs` around `next_use`, `linear_positions`, and the
   `cand.sort_by_key` at ~line 1317. That is the whole surface of S1.
2. Decide the weighting form on the model *before* editing: what does `10^depth`
   do to a position scale that `next_use` binary-searches with
   `partition_point`? The ordering must stay monotone or the search breaks.
3. Then, and only then, write code — and measure once, at the gate.

---

# Part E — the copy census

## COALESCE — the register copy that is half the gap

The plan of record for one campaign. Boot here, read §0 for the number that
justifies it, §1 for what has already been refuted, and start at §3 — which is
measurement, not code.

---

> **⚠ §0 AND §2 BELOW ARE SUPERSEDED.** `MEASURED M26-correction` and
> `MEASURED M27` (Part F) re-took both on the same day and both headline numbers
> moved: the copy family is 44% of the gap rather than 70%, and the reachable
> coalescing ceiling on the suite is **203 instructions of 953**, because 289 of
> the 518 "block-edge copies" are `mov wN, wzr` — a constant zero, which gcc
> materializes with one instruction of its own. C0 is DISCHARGED; the attribution
> table it asked for is in `M26-correction`, and the refusal census that retires
> the eviction row is in `M27`. What is left of this campaign is FREE = 203 on the
> suite / 4,965 on sqlite, minus the 72 / 1,554 the shipped `ZCC_CSBIAS` row
> addresses.

### §0 THE FINDING (`MEASURED M26`, 2026-08-28, commit `5e03858`) — SUPERSEDED

The 49-program taxonomy suite, compiled by both compilers, every mnemonic
counted and the spellings combined:

```
zcc 7,551 instructions   gcc -O1 6,598      +953   (+14.4%)

mov reg,reg          1006   339   +667   ← 70% of the whole excess
  · at a block edge   519    56   +463   ← HALF the entire gap
  · in the body       312   206   +106
  · placing an argument 175   77    +98
load/store slots     1107   967   +140
cmp + subs            484   359   +125
mul + madd + msub     187    70   +117
sxtw + sbfiz          161    93    +68
mov reg,#imm          608   790   −182   ← zcc materializes FEWER constants
```

The same shape was measured independently on the sqlite amalgamation:
register-to-register moves are +10,464 of a 20,264-instruction gap, 52%. Two
different corpora, one answer.

**A block-edge copy is what SSA destruction leaves behind and coalescing does not
remove.** It is not a missing optimization row, which is why three rows built on
the strength of reading one inner loop (`MEASURED M25` and the refutations beside
it) each addressed a family worth 12%, 0% and 12% and each measured a loss.

### §1 WHAT IS ALREADY KNOWN, AND WHAT IS ALREADY REFUTED

Read this before proposing anything; the obvious moves have been made.

- **The hints are asked for and REFUSED.** On sqlite the coalescing hint hit rate
  is 56.5%, and **14,615 hints were refused because the register was already
  OCCUPIED** — not because the hint was absent or badly ordered. The conclusion
  recorded there is that this needs EVICTION or priority colouring, and that
  three ordering fixes were tried and refuted.
- **ABI argument placement is 40% of sqlite's size gap** on its own (x0–x7
  traffic 22,813 against gcc's 14,626), and §0's census puts +98 of the suite's
  copies there too. It is a second front, not the same one.
- **`evict_params` strips `has_def`**, so a loop-header phi can never carry an
  accumulator; the recorded next lever there was to split the PARAMETER at the
  terminator rather than the whole web.
- **Reconstruction is Braun-2013 at joins and headers**, and eviction is already
  a regional split rather than a whole-web one — the whole-web model was wrong
  for 96% of spilled values.

The surface: `regalloc/color.rs` (952 lines, where a hint is honoured or
refused), `regalloc/destruct.rs` (715, where the edge copy is created),
`regalloc/reconstruct.rs` (124), `regalloc/spill.rs` (2,862),
`regalloc/promote.rs` (341).

### §2 WHERE A COPY COMES FROM IN ZCC — the three sources, and they need separating

The census counts what reaches the assembler; it does not say which mechanism
minted each one. Nothing should be built until each of the 519 edge copies is
attributed to one of:

1. **SSA destruction** — a phi whose argument and result were coloured
   differently, so `destruct` places a copy on the edge.
2. **A parallel copy that is genuinely a permutation** — a swap or a cycle, which
   costs copies no matter how it is coloured, and is NOT a coalescing failure.
3. **A `Copy` minted by an earlier pass and never removed** — `mir/pass/ext.rs`
   turns a redundant extension into a `Copy` and expects colouring to erase it.
   67 of these survive with the SAME register at both ends (gcc: none);
   `k1_dispatch` ends every switch arm with the identical `mov w10, w10` behind
   an `and` that already zeroed the top half.

These want opposite fixes, and the ratio between them decides the whole campaign.

### §5 TRAPS, all of them paid for once already

- **Combine the spellings or the table lies.** gcc writes `mov w7, 18725` where
  zcc writes `movz`, and `bne` where zcc writes `b.ne`. Raw counts read as +432
  and +195 against a gcc that never emits either mnemonic.
- **The EXEC geomean has a ±0.007 spread across sessions.** Only interleaved
  pairs inside one box session compare; a single reading has already dismissed
  one row wrongly and promoted another wrongly.
- **INSN geomean is deterministic** and is the axis to trust for a size claim.
- **Never chain a baseline to an earlier candidate.** Six rows drifted unnoticed
  that way.
- **The byte-identical gate has no oracle** (it is zcc against zcc) and is scoped
  to what it compiles. A row that fires on none of the corpus is invisible to it;
  measure the row's coverage before trusting a green.
- **A permutation is not a coalescing failure.** Counting it as one will make a
  fix look like it did nothing.

### §6 HOW TO MEASURE

**The census** (one box command, the source of §0): for each `.c`, emit both
`.s`; `grep -oE '^[[:space:]]+[a-z][a-z0-9._]*'` for mnemonics; `uniq -c`; `join`
the two tables; sort by difference. Classify a `mov` by scanning forward up to
seven instructions — a `bl` first means argument placement, a branch or a label
first means a block edge, anything else means body. Split `mov` by whether its
second operand begins with `#` or a digit (constant) or not (register), and by
whether its two register operands are equal.

**The scoreboard**: `SUITE=/work/zcc/tests/bench/suite sh tests/bench/exectime.sh`
inside the box (its default `SUITE` path is wrong and it then reports "no timed
programs" in silence). Interleaved pairs only.

**The gate**: `sh tests/fullsuite.sh all` — 15 stages, about six minutes with
inlining on. Batch two or three rows per full gate; per row use `cargo test`,
`tests/fullsuite.sh provenance` and `tests/bench/localize.sh`.

---

# Part F — MEASURED: the facts with no spec to cite

## MEASURED — target facts with no spec to cite

Law 1 says every line of `src/` is either a theorem (Side I) or a constant
transcribed from a spec line (Side II). `THEORY.md` holds both, and Side II's
entries are **citations** — a section number in ISO 9899, AAPCS64, DDI 0487 or
the ELF ABI that a reader can look up.

Some facts have no such line to cite. **Apple publishes no Software Optimization
Guide for the M1**, so an instruction's latency, or whether a transform pays on
this machine, cannot be referenced — it can only be MEASURED. Those facts live
here rather than in `THEORY.md`, so that Law 1's two-side claim stays literally
true: `THEORY.md` II-* is cited spec and nothing else.

> ## READ EVERY RATIO BELOW AGAINST ITS DATE — the referee and the core both moved
>
> **THE REFEREE.** Entries dated **before 2026-08-29** compare against **`gcc
> -O1`**. From `M49` the harnesses default to **`gcc -O2`**, because that is what
> real software is built with; `GCC_OPT=-O1` restores the old column. The two are
> far apart — the same compiler reads **1.06 against -O1 and 1.31 against -O2** on
> the same 96 programs — so a ratio quoted without its level says nothing.
>
> **THE CORE.** Entries dated **before 2026-08-29** were measured on an **Apple M1
> Pro under Docker**. From `M46` the authoritative core is a **Graviton4 (Neoverse
> V2)**, native, because that is the machine zcc's declared target actually runs
> on. These are also far apart — the same binaries read **1.0686 on the M1 and
> 1.1716 on Neoverse**, and two rows worth 4.5x each were INVISIBLE on the M1.
>
> Nothing below is deleted for this: an entry is a dated measurement and stays
> one. But a number carried forward without its date and its two axes is a stale
> rule, and this file's own charter says a stale rule is worse than none.

An entry is not an opinion. Each carries:

* **VALUE** — the number or verdict the compiler acts on;
* **METHOD** — the instrument and the command, so it can be re-taken;
* **WHEN / WHERE** — the date and the machine, because a measured fact is only
  true of the machine that produced it;
* **WHAT USES IT** — the site in `src/` that reads it, so a change here has a
  visible blast radius.

**THE STANDING CAUTION.** Every number below was taken on **Apple M1 Pro cores
under Docker**, while the notional target is generic AArch64-Linux. A measured
fact is evidence about the measuring machine first and about the target second.
Where the two could differ, say so in the entry.

Cite an entry from code as `MEASURED M<n>`, exactly as a spec fact is cited as
`THEORY II-<n>`. `tests/provenance.sh` checks that every citation names an entry
that exists.

---

### M1. Extended-register ALU latency — 2 cycles against 1

**VALUE.** On this machine `add xN, xN, wM, sxtw` has a 2-cycle latency where
`add xN, xN, xM` has 1. The two are the same instruction COUNT, so `cost = |MIR|`
scores them identically and always will.

**METHOD.** j3_prefix_sum's loop-carried recurrence is `acc += ext(load)`. With
the extension in the ALU the recurrence bound is 2.0; with `ldrsw` doing the
extension in the load it is 1.0. Predicted from that table alone, with no build:
**2.0**. Measured: **1.940** — a 3% error. After the transform: **1.000**.

**WHEN / WHERE.** 2026-08-25, M1 Pro under Docker, `tests/bench/exectime.sh`.

**WHAT USES IT.** `isel/lower.rs`'s extending-load row prefers the extension in
the LOAD over the ALU operand; `mir/pass/ext.rs::plain_operand` drops an operand
extension the lattice proves is a no-op. Neither is justified by instruction
count — both are justified by this entry.

**CAUTION.** A core with a different extended-register path would not show this.
The transform is never WRONG there, only unmotivated.

---

### M2. The UNIT-STRIDE pointer / 64-bit induction variable is NEGATIVE on this target

**VALUE.** Rewriting a recomputed `[base, w, sxtw #k]` address into a pointer
walked by a post-index writeback makes zcc measurably WORSE. `hir/pass/iv.rs`
ships that half default-OFF because of this entry.

**SCOPE, narrowed 2026-08-26.** This entry is about a step EQUAL to the access
size, and only that. It is what the A/B below varied, and it is the only case
A64's scaled index reaches: `ldr Xt,[Xn,Xm,lsl #3]` scales by the access size
and by nothing else (DDI 0487 C6.2.130). An address whose step the mode cannot
express — `B[k][j]` walking a 240 x 8-byte ROW, step 1920 — is rebuilt with a
MULTIPLY on every iteration, so replacing it with an `add` costs the same
instruction count and removes a multiply from in front of a load. That half
ships ON and has its own fact, M9; nothing here measured it.

**METHOD.** `ZCC_IV=1` A/B over the 35-program suite, twice, on two different
compilers. §13k (pre-R4.7): EXEC ≥30 ms 1.3789 → 1.4087, INSN 1.2419 → 1.2454,
sqlite +1,276. Re-taken post-R4.7 (2026-08-25): INSN **1.1493 → 1.1538**, EXEC
**1.2044 → 1.2140**, programs above 1.1× 8 → 9.

**WHY, and this is the part that generalizes.** A64's scaled-index addressing
form makes rebuilding an address from a counter FREE — there is nothing to
strength-reduce. R4.7 then removed the one thing that was not free about it (the
`sxtw` feeding the loop-carried chain, M1), which is why the re-measurement is
worse than the first.

**WHEN / WHERE.** 2026-08-25, M1 Pro under Docker.

**WHAT USES IT.** `hir/pass/iv.rs::ENABLED = false` — which now gates the
unit-stride half alone (`strengthen`'s `unit` parameter), not the whole pass.

**RE-ENTRY TRIGGER.** §13k's own gate: a cost model that can say WHEN a
writeback pays. Until one exists this stays off. j5_insertion_sort is the one
program where it would pay, which is a statement about j5, not about the target.

---

### M3. The copy-partner graph saturates at depth 3

**VALUE.** `regalloc/color.rs` follows the copy-partner graph three hops looking
for a coloured member to bias toward. Three, not one and not eight.

**METHOD.** Swept on sqlite, `ZCC_CODEPTH=<n>`, whole-module instruction count:

| depth | 1 | 2 | **3** | 5 | 8 | 16 |
|---|---|---|---|---|---|---|
| insns | 188,659 | 187,260 | **187,097** | 187,081 | 187,104 | 187,104 |

Depth 1 is the old one-hop behaviour. It is flat from 3 on; 5 buys 16
instructions and 8 gives them back.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker.

**WHAT USES IT.** `regalloc/color.rs`, the `depth` bound in `assign`.

**CAUTION.** This is a property of sqlite's copy graph, not of the ISA. A corpus
with longer copy chains would move it.

---

### M4. The jump-table crossover is ~24 arms, and a BALANCED TREE never wins

**RE-TAKEN 2026-08-26, and the earlier entry is superseded below.** The first
attempt could not separate the forms because it swept a synthetic whose arms did
too little work. This one sweeps 4…64 arms with a pseudorandom index AND with a
repeating one, and the two agree.

**VALUE.** `isel/lower.rs::MIN_CASES = 24`, was 4 (chosen by taste at R3.3).

**METHOD.** Three dispatch forms, same program, ms best-of-7, outputs compared
first. Unpredictable index:

| arms | gcc | chain | tree | table |
|---|---|---|---|---|
| 4 | 21 | **46** | 53 | 54 |
| 8 | 36 | **54** | 69 | 62 |
| 16 | 49 | **62** | 84 | 65 |
| 32 | 50 | 71 | 98 | **67** |
| 64 | 53 | 87 | 111 | **68** |

Crossover, both index kinds (chain / table): 16 → 62/65 and 11/12 · 20 → 66/67
and 12/12 · 24 → 68/**67** and 14/**12** · 28 → 70/**67** and 15/**12**. The
chain is better or equal to 20 arms and the table wins from 24, whether the
index repeats or not. 21…23 were not measured and the constant does not pretend
otherwise — 24 is the first size where the table actually wins.

**THE BALANCED SEARCH TREE IS REFUTED.** It was built, proven and measured, and
it loses at EVERY size from 4 to 64 — at 16 arms, chain 62 ms, table 65, tree 84.
It asks strictly fewer questions (4 against 7 on d1_switch) and takes more time,
because the chain's tests FALL THROUGH while the tree spends a taken branch per
level and scatters the arms. Law 3c pointing the other way: fewer questions is
not less time either. The code was removed rather than kept behind a flag,
because no measured size wants it.

**RESULT.** d1_switch **1.500 → 1.200**; geo40 EXEC 1.0240 → **1.0180**; sqlite
173,344 → 173,519 (+175, +0.1%), which is the Law 0 ordering — `exec > size`.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, gcc 14.2.0.

**WHAT USES IT.** `isel/lower.rs::MIN_CASES`, and `jump_table`'s density test.

**OPEN, and FOUR HYPOTHESES REFUTED.** d1 sits at 1.200, and none of the obvious
explanations survives a controlled hand-edit (same file, one change, outputs
compared, best-of-11):

| d1 variant | ms |
|---|---|
| gcc -O1 | 10 |
| **zcc, compare chain (what ships)** | **12** |
| gcc's dispatch shape transcribed verbatim into zcc | 13 |
| `csel` on the last arm (if-converted arm body) | 12 |
| `tbnz` range split | 15 |
| counter widened to 64 bits, arms read `x1` (no `sxtw`) | 13 |

Transcribing gcc's own shape makes zcc SLOWER. The branchless arms buy nothing.
The range split hurts. Widening the counter — which removes three
extended-register operands (`MEASURED M1`) from the loop-carried accumulator —
also loses. Whatever the last 2 ms is, it is not the switch and not the arms, and
four experiments did not find it.

QUARANTINED at 1.200 rather than guessed at further. The re-entry is **R4.18**,
the time-dual cost model: this is precisely the case it exists for — a program at
INSN 1.077 whose remaining time gap no instruction-level reasoning has located.

---

### M4-superseded. A jump table and a compare tree are indistinguishable by case count

**VALUE.** `isel/lower.rs::MIN_CASES = 4` is UNSETTLED. The measurement does not
support any constant derived from the case count, so the R3.3 value stands
unchanged rather than being replaced by a fitted one.

**METHOD, and it is the disagreement that is the finding.**

* d1_switch (8 cases), repeatedly and directly: jump table **15 ms**, compare
  tree **12 ms** — the tree wins by 20% while emitting **12 MORE instructions**
  (95 against 83). The table's indirect branch is unpredictable.
* A synthetic sweep at 4, 6, 8, 12, 16, 24 and 32 cases, with a pseudorandom
  (unpredictable) index: table and tree within **1 ms of each other at every
  case count**.
* Whole-suite A/B: `ZCC_JT=9` moved the EXEC geomean 1.0899 → 1.0639, but d1
  alone moves 13% and the geomean would need 35% from it — the rest is
  cross-program noise.

**THE CONCLUSION.** The case count is not the variable. Something about d1's
switch — not how many arms it has — decides it, and no constant over arm-count
would be honest. `ZCC_JT` is left in place as the instrument.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker.

**WHAT USES IT.** `isel/lower.rs::MIN_CASES`, and `jump_table`'s density test.

---

### M5. The `ldp`/`stp` pairing window saturates within ten instructions

**VALUE.** `mir/pass/ldstp.rs::WINDOW = 10`.

**METHOD.** Distance distribution of pairable frame accesses on sqlite, after
the spills-first frame layout: 433 adjacent, then 302, 299, 144, 117, 116, 107,
102, 97, 90 at distances 2…10 — 1,807 in total, of which 761 are refused by the
paired form's imm7 range regardless of distance. The tail beyond ten is flat.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, `tests/bench/excess.sh`.

**WHAT USES IT.** `mir/pass/ldstp.rs::WINDOW`.

---

### M6. `alg.sh` is bound by zcc's compile time, not by the harness

**VALUE.** The expression-algebra gate does not scale past two workers, and the
reason is zcc, not the script.

**METHOD.** `ALG_JOBS` sweep: 1 → 98s, 2 → 57s, 4 → 54s, 8 → 54s, 16 → 57s.
Profiled: generation 147ms, the eleven `run` cases compile in **zcc 73.0s
against cc 4.2s** (17×) on 3.4k-line files, the runs take 8ms, concatenation and
diff 0ms.

**WHY.** These are exhaustively generated op × type × corner files — one huge
function each — which is the same superlinear compile-time shape that produced
the yarpgen CTIMEOUTs before the release-build fix.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker.

**WHAT USES IT.** `tests/alg.sh`'s comment, and §CP's target list. Nothing in
`src/` reads this; it is here so the next person to look at gate speed does not
re-derive it.

---

### M7. `MAX_HEADER_INSTS = 20` is gcc's number, not a spec's

**VALUE.** `hir/pass/rotate.rs::MAX_HEADER_INSTS = 20` — the largest loop header
worth copying.

**METHOD.** NOT measured here. It is gcc's own -O1 value for the same transform
(`--param max-loop-header-insns`, default 20, read by `-ftree-ch`), taken
because rotation trades a STATIC copy of the header for a DYNAMIC branch per
iteration — an exchange rate, so no bound falls out of the theorem.

**WHY IT IS HERE AND NOT IN THEORY.md.** gcc's default is not a specification.
Recording it as a Side-II citation would be inventing provenance, which is the
Article E failure this file exists to prevent.

**WHAT USES IT.** `hir/pass/rotate.rs`.

**OPEN.** Never swept on this corpus. A sweep would move it from "gcc's number"
to a measured one; until then it is honestly labelled.

---

### M8. The if-conversion arm bound is 2, and it is REASONED, not measured

**VALUE.** `hir/pass/ifconv.rs::ARM_LIMIT = 2` — the most instructions an arm may
hold and still be if-converted into a `select`.

**METHOD.** NOT measured. It is a reading of the trade: converting replaces a
compare, a taken branch and the pipeline bubble a misprediction costs with
unconditional work on both arms, so the bound is "fewer instructions than a
mispredict costs". Two is the conservative reading, and the shape this pass
exists for — a join parameter and nothing else — needs none at all.

**WHY IT IS HERE AND NOT IN THEORY.md.** There is no spec line for the cost of a
branch misprediction on this core, and none was measured. Recording it as a
Side-II citation would be inventing provenance. Labelled honestly instead.

**WHAT USES IT.** `hir/pass/ifconv.rs`.

**OPEN.** Never swept. A sweep over the suite would move it from "the
conservative reading" to a measured entry — and `csel` sits at 599 against gcc's
542, so the bound is not currently costing much either way.

### M9. A ROW-STRIDED pointer IV is POSITIVE on this target

**VALUE.** When a loop's load address advances by a step the addressing mode
cannot express, walking a pointer removes a MULTIPLY from in front of the load
at the same instruction count. `hir/pass/iv.rs` ships this half ON.

**METHOD.** `tests/bench/matmul.c` — `s += A[i][k] * B[k][j]`, where `B[k][j]`
walks a 240 x 8-byte row, step 1920. The k-loop is seven instructions either
way; the difference is one `madd x12,x11,x4,x1` computing the address against
one `add x14,x14,#1920` advancing a pointer. Both forms were HAND-ASSEMBLED from
the same zcc output and linked and run side by side, so nothing but that one
instruction differs, and both print `414714994`:

| k-loop form | ms, best of 5 | vs gcc -O1 |
|---|---|---|
| gcc -O1 (same shape as the pointer walk) | 69 | 1.000 |
| zcc, address rebuilt with `madd` | 113 | 1.638 |
| zcc, pointer walked by `add #1920` | 69 | **1.000** |

Adding gcc's other two tricks on top — post-index writeback for the `A` load and
a pointer-limit exit test instead of a counter, six instructions — changed
nothing: also 69 ms. The whole gap is the multiply.

**WHY.** The multiply sits at the head of a dependence chain that ends in a
strided load, and a strided load is where the machine most needs its address
early. `cost = |MIR|` cannot see this: the instruction COUNT is identical. It is
the same kind of fact as M1, and it is judged the same way — on the clock.

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, `zcc-box:latest`, gcc 14.2.0.

**WHAT USES IT.** `hir/pass/iv.rs::strengthen` — the `scaled` test that separates
this half from M2's.

**OPEN.** Stores are still refused (M2's j2_histogram argument, which is about
the unit-stride case). `ZCC_IVDBG=1` prints the residual per refusal reason;
matmul still reports 17 `scev-refused` and 3 `unit-stride-gated`.

---

### M10. Instruction latency on this core, in units of a dependent `add`

**VALUE.** The Side-II table the time model reads (`mir/cost.rs::latency`).

| latency | forms |
|---|---|
| **1** | `add`/`sub`/`and`/`orr`/`eor` (reg or imm), `lsl` (imm or reg), `csel`, `sxtw`, `uxtb`, `ubfx`, `mvn`, `rev`, and **`madd` reached through its ACCUMULATOR** |
| **2** | `add x,x,x,lsl #n` and `add x,x,w,sxtw` — a shifted or extended register operand |
| **3** | `mul`, `madd` reached through a MULTIPLICAND, `ldr` L1 hit (plain or register-offset) |
| **7** | `sdiv`, `udiv` |

**METHOD.** `tests/bench/latency.sh`. Time a loop whose body is 32 copies of one
instruction, each reading the register the previous wrote. The chain cannot
overlap, so wall time is `K x latency x iterations` whatever the core does about
width or reordering — and dividing by the same measurement for `add x0,x0,#1`
cancels the clock, which is why no frequency is needed and the answer is a ratio.
Measured ratios: 1.00 / 2.02 / 3.02 / 7.05, with a `nop` control at **0.12**
confirming the harness is not measuring itself.

**THE ONE THAT CHANGES DESIGN DECISIONS.** `madd` is TWO latencies in one
instruction: 3.02 through a multiplicand, **1.00 through the accumulator**. So
`s += a*b` accumulation is not multiply-bound, and a loop that looks
multiply-heavy may have a one-cycle recurrence. matmul is exactly that, which is
why a recurrence-only model could not see its gap and `Bound` grew a second axis.

**IT RE-DERIVES WHAT WAS ALREADY MEASURED**, which is R4.18's ship condition:

| case | from the table alone | measured on the clock | error |
|---|---|---|---|
| `loops.c`, `mul`+`add` (3+1) becomes `madd` (3) | 4/3 = **1.333x** | 771/565 = **1.365x** | 2.3% |
| j3, extended operand (2) becomes `ldrsw`+`add` (1) | **2.00x** | **1.940x** | 3% |
| matmul, `madd` address vs pointer walk | addr **3 -> 0** | 113/69 = 1.638x | direction |

**WHEN / WHERE.** 2026-08-26, M1 Pro under Docker, `zcc-box`, gcc 14.2.0.

**WHAT USES IT.** `src/mir/cost.rs`. `ZCC_CYCLES=1` prints the per-loop bounds.

**OPEN.** The FP forms, `Call`, and the `ldp`/`stp` pair are unmeasured and take
the ALU default of 1; a loop containing a call is reported UNSCORED rather than
guessed at. Issue width, ports, the reorder window, cache misses and branch
misprediction are not modelled at all — the recurrence is a LOWER bound, and
programs it scores at 1 while they run slower (j5, g1, d1) are bounded by
something else, which is itself a useful verdict.

---

### M11. Tail-duplicating a loop latch pays only at a MULTI-WAY dispatch

**VALUE.** `mir/pass/layout.rs::duplicate_latch` copies a loop tail into its
predecessors only when **three or more** of them reach it by an unconditional
branch.

**METHOD.** d1_switch's switch arms each end `b .Lwork_3`, and that block is the
whole loop tail — bump the counter, test it, branch back. Every iteration paid
TWO taken branches to reach the top. Hand-validated in zcc's own `.s` before the
pass was written (three passes, output identical at 8000006000000):

| d1_switch | ms |
|---|---|
| gcc -O1 | 10 |
| zcc, arms jump to a shared tail | 12 |
| zcc, tail copied into each arm | **10** |

**THE THRESHOLD, and what it cost to find.** Firing on TWO or more predecessors
— which describes any if-else join — fired on nearly every loop in the suite:

| predecessors required | geo40 EXEC | geo40 INSN | sqlite |
|---|---|---|---|
| ≥ 2 | 0.9430 | **1.3668** (32 of 35 above 1.1×) | +3,906 |
| **≥ 3** | **0.9494** | **1.0432** | **+840** |

33% of size for 2% of time is the trade R4.14 refused at 16-for-7. Three is the
count that distinguishes a multi-way dispatch from a two-armed join, which is
where a second branch per iteration actually repeats.

**AN EARLIER FENCE, AND THE VERIFIER THAT FOUND IT MISSING.** The first cut
tested only "conditional terminator, ≥2 unconditional predecessors" — describing
any join — and duplicating a join that reloads a spilled value moved the reload
above its store on one path. `regalloc::verify` said so at once: "reload of
unstored slot 31". A loop TAIL is a join whose terminator branches BACK to a
block that dominates it, and that is what the pass tests now.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, gcc 14.2.0.

**WHAT USES IT.** `mir/pass/layout.rs::duplicate_latch`.

**OPEN — THE THRESHOLD IS UNSWEPT.** 2 and 3 were measured; 4, 5 and beyond
were not. Three is where the measurement stopped, not where it was shown to be
best, and this entry says so rather than dressing a plausible story as a fact —
`MIN_CASES = 4` sat unswept in `isel/lower.rs` for a milestone and cost d1 50%
when someone finally measured it (`MEASURED M4`). Sweeping 4/5/6 on INSN and
sqlite is deterministic and needs no quiet box.

---

### M11-correction. "Locally evictable" counts ONE of three conditions

**WHAT THE REPORT SAYS.** `ZCC_HINT=1` prints, of the hints refused because the
wanted register was occupied, how many have an occupant that "dies in this block
(locally evictable)". On sqlite that is 8,696 of 14,764, and the FULL-RANGE line
then says a register is free across the occupant's whole range in 100% of them.

**WHY THAT IS NOT A CEILING.** `HINT_OCC_LOCAL` tests only that the occupant's
LAST USE is in this block. A value can die here and still be LIVE-IN, its range
reaching back through dominating blocks the colourer walked earlier and keeps no
occupancy record of. Recolouring one of those changes its register in those
blocks too. Measured, by building the mechanism and running it:
`regalloc::verify` stopped the compile at
`unixShmSystemLock: V(4) and V(25) are both live at bb0[3] and both hold Gpr9`.

**THE REAL NUMBER.** Restricted to occupants DEFINED in this block, dying in it,
and not live-out — the case a block-local history can actually justify — the
recolour fires **7 times in the whole of sqlite**, for −37 instructions. Seven,
against a reported eight thousand six hundred.

**WHAT USES IT.** Nothing, now: the mechanism was reverted (`MECHANISM.md` Part D §4b).
The entry exists so the next reader of that column knows it is an upper bound on
an upper bound, and so the row is not attempted a seventh time on the strength
of the same number.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

---

### M12. Assumed trips per loop level is TEN, and the choice is not load-bearing

**VALUE.** `TRIPS = 10` in `regalloc/spill.rs`. The spiller's next-use distance
is measured along the execution trace, and a value whose next use lies OUTSIDE
the current loop is reached only after the iterations still to run; that count
is unknowable statically, so the model assumes ten per nesting level — the same
convention as gcc's `10^depth` block frequency.

**METHOD.** The number is a cost-model parameter, so the honest question is not
"is ten right?" (no static analysis can know) but Article E's: *is this the
spec's number or my convenience's number?* Answered by sweeping it and showing
the decisions barely move. `ZCC_TRIPS` was made to override the constant and
sqlite plus all 35 taxonomy kernels were compiled at 1, 2, 3, 4, 5, 10, 20, 100
and 1000:

| TRIPS | sqlite instructions | taxonomy suite |
|---|---|---|
| 1 | 175,452 | byte-identical throughout |
| 2 | 175,438 | ″ |
| 5 | 175,405 | ″ |
| **10** | **175,394** | ″ |
| 20 | 175,390 | ″ |
| 100 | 175,380 | ″ |
| 1000 | 175,380 (identical bytes to 100) | ″ |

The whole three-orders-of-magnitude sweep moves sqlite by **72 instructions,
0.04%**, monotonically, and saturates at 100 — beyond which no ranking changes
at all. The taxonomy suite does not move by one byte at any value, which is a
second reading of the same fact recorded in `MECHANISM.md` Part D §4a: none of its kernels
is under enough register pressure to spill, so nothing there can see this
constant. Ten sits on the flat part of a flat curve.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `regalloc/spill.rs::Trace::next_use` — the step that leaves a
loop the value is not wanted in, and only that step.

**WHAT WOULD MAKE IT MATTER.** A body long enough that one loop's remaining
instructions outweigh a factor of ten across a level — deep nests over long
bodies. Nothing in the current corpus is that shape; a program that is would
show up as a sqlite-scale gap between TRIPS=10 and TRIPS=100, which today is
14 instructions.

---

### M13. The argument registers go LAST in the caller-saved half

**VALUE.** `GPR_ORDER` offers x8–x15 before x0–x7. The allocatable SET is
AAPCS64 §6.1.1 and does not change; only the order `assign` walks when a value
has no coalescing hint.

**WHY IT COULD MATTER.** x0–x7 are the only registers a call can demand by
name. `assign` picks `hint.or_else(|| alloc_order.find(free))`, so with x0
first every unhinted value in the function takes an argument register before
anything else — and the argument that later wants x0 finds it occupied, which
is one `mov` per refusal. The instrument (`ZCC_HINT=1`) had already measured
the refusals: **34,569 hints wanted, 55.4% taken, 15,348 refused because the
register was OCCUPIED**, never for want of a free register (0 refusals had no
spare).

**METHOD.** sqlite compiled with both orders, same binary otherwise:

| | x0-first | x8-first |
|---|---|---|
| reg-reg `mov` | 31,352 | **30,669** |
| of those, writing x0–x7 | 22,829 | **19,985** |
| file instructions | 175,407 | **174,677** |
| hint hit rate | 55.4% | 56.7% |

−730 instructions, 1.1167× → 1.1120× against gcc -O1.

**WHAT IT DOES NOT FIX, and the number that says so.** The hit rate moves by
1.3 points. 14,879 hints are still refused because the wanted register is
occupied, and for **8,784** of them the occupant dies inside the same block
with a register free across its WHOLE range — the colourer computes that in
its statistics replay and acts on none of it. Reordering cannot reach those:
they need the occupant RE-COLOURED, which greedy colouring in dominance order
does not do. That is the open lever, and it is larger than this one.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `regalloc/color.rs::assign`'s fallback scan — and nothing
else: masks, `k`, and the callee-saved set are all order-independent.

---

### M14. A copy of 32 bytes or less is cheaper open-coded than called — the SIZE answer, re-derived on time by `M40`

> **STATUS: the constant is now 128** (`M40`). The sweep below is correct and it
> answers the SIZE question; `M40` asks the TIME question of the same bound and
> gets a different number, which is what `M38`'s corr(INSN, EXEC) = 0.196 predicts.

**VALUE.** `INLINE_COPY_MAX`, **32 here and 128 since `M40`**, in `isel/lower.rs`. An `Inst::MemCpy` of this
many bytes or fewer becomes loads and stores; anything larger stays a call to
`memcpy`.

**WHY THERE IS A DECISION AT ALL.** C says a by-value parameter IS a local
object, so the frontend homes one by copying the incoming registers into the
local's storage. For a four-`int` struct that is a sixteen-byte `MemCpy`, and
lowering it to `bl memcpy` costs far more than the copy: the call itself, a
frame and an x30 save in what would otherwise be a LEAF function, and a
clobbered caller-saved half at the point where the argument registers are still
live. `e3_struct_byval` was **2.630× gcc -O1 on the clock — the worst program in
the taxonomy suite on both axes** — for a copy gcc does not perform at all.

**METHOD.** The threshold trades size against that cost, so it was swept rather
than chosen. sqlite compiled at nine settings, everything else identical:

| bound (bytes) | sqlite instructions |
|---|---|
| 0 (always call) | 174,677 |
| 8 | 174,659 |
| 16 | 174,604 |
| **32** | **174,572** |
| 48 | 174,584 |
| 64 | 174,604 |
| 128 | 174,703 |
| 256 | 174,703 |

A clean minimum at 32, and past 64 the open-coded form is worse than not
inlining at all — which is the shape the trade predicts, since a call is four
instructions whatever the length while the expansion grows with it.

**WHAT IT BOUGHT ON THE CLOCK.** `e3_struct_byval` 2.630× → **1.953×**, and its
instruction ratio 1.724 → 1.621. The taxonomy suite's EXEC geomean 1.0403 →
**1.0304** over 25 timed programs.

**WHAT IS STILL WRONG THERE, because the row is not exhausted (Law 4).** zcc
still round-trips the struct through memory twice: the incoming registers go to
the argument home, the home is copied to the local, and the fields are then
loaded back. gcc keeps the whole struct in x0/x1 and extracts the four `int`s
with `sxtw` and `asr #32`, touching memory not at all. Closing that needs the
local copy to be recognised as redundant when the parameter is never modified,
and small aggregates to live in registers (SROA) — neither is this row.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `isel/lower.rs::copy_inline`, reached from `Inst::MemCpy`.
The expansion emits two loads then two stores per sixteen bytes so that
`mir/pass/ldstp.rs` sees adjacent same-kind accesses and fuses them.

---

### M15. The `ldp`/`stp` residual, and why the layout cannot collect it

**VALUE.** On sqlite, `ZCC_LDSTP=1`:

```
paired=7616 | unpaired: no-partner=44137 (of which 3020 sit NEXT TO another
frame access of the same shape — a LAYOUT could pair them)
out-of-window=123  motion-blocked=886  partner-is-BEHIND=126
```

gcc -O1 emits 12,637 pairs to zcc's 7,616. This is the Law-4 residual of the
pairing theorem, classified: 41,117 accesses have no partner at any distance and
are a FUNDAMENTAL limit; 1,135 are convenience truncations of how this pass
looks (window, motion rule, direction); and 3,020 are refused only because the
two slots are not neighbours in the frame.

**THE 3,020 IS AN UPPER BOUND ON AN UPPER BOUND, and it was tested.** Two
orderings of the spill group were built and measured against the creation order
that §13o leaves in place:

| spill-slot order | pairs | sqlite instructions |
|---|---|---|
| creation order (shipped) | **7,616** | **174,572** |
| heaviest disjoint affinity pairs | 7,516 | 174,730 |
| first-access position | 7,435 | 174,882 |

Both alternatives are WORSE. The count says "these two could be adjacent" one
pair at a time and cannot say that making them adjacent separates two others —
`ldp`/`stp` consume RUNS, and a disjoint matching cuts a four-slot run into two
pairs where creation order had three. The allocator mints spill slots in an
order already correlated with the order they are accessed in, which is why the
inherited order is hard to beat.

**THE PREMISE OF THE WHOLE ROW WAS WRONG, and here is the arithmetic.** "gcc
emits 12,637 pairs to zcc's 7,616, so 5,130 instructions are being left on the
table" counts gcc's PAIRS as if each one zcc lacks were an instruction zcc could
delete. A pair only saves an instruction when the two accesses exist. Counted
properly, on sqlite:

| frame traffic | zcc | gcc -O1 |
|---|---|---|
| paired instructions (`ldp`/`stp` on sp/x29) | 7,097 | 11,456 |
| single `ldr` | 8,862 | 7,976 |
| single `str` | 6,111 | 5,288 |
| **total frame instructions** | **22,070** | **24,720** |
| accesses those instructions cover | 29,167 | 36,176 |

**zcc emits 2,650 FEWER frame instructions than gcc -O1.** gcc has more pairs
because it has 7,009 more frame accesses to pair — it spills more file-wide,
which is a fact already on the record. There was never a 5,130-instruction
opportunity here.

What is real is pairing EFFICIENCY: 0.757 instructions per frame access against
gcc's 0.683. Matching that on zcc's own accesses would be ~2,100 instructions,
and the census above says ~1,009 of those are reachable (886 motion-blocked, 123
out of window).

**AND IT IS NOT SCHEDULING.** An earlier version of this entry blamed gcc's lead
on instruction scheduling. Measured instead of asserted: at `-O1` gcc reports
`-fschedule-insns [disabled]` and `-fschedule-insns2 [disabled]`, and forcing
`-fschedule-insns2` on at `-O1` moves sqlite's pair count by **2 instructions**
and its instruction count by **zero** (157,074 either way). 91% of gcc's pairs
are sp/x29-based — prologue, epilogue and spill runs, emitted adjacent by the
frame expander, with no scheduler involved. Scheduling is an `-O2` transform and
is out of scope against an `-O1` reference.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** Nothing in the compiler: these are counters behind
`ZCC_LDSTP`. The entry exists so the next reader of the 3,020 knows it was
tried, and so the row is not attempted again on the strength of the number
alone.


---

### M16. `sqlite3VdbeExec` IS the sqlite gap — 85% of it, in one function

**VALUE.** On sqlite's worst workload (`p01_insert`, 100,000 rows through a
recursive CTE, in-memory), compiling **one function** with gcc and the other
1,259 with zcc takes the program from **1.98× to 1.15×**. That one function is
`sqlite3VdbeExec`, the VDBE interpreter loop every statement runs.

| function taken from gcc | ratio | closes |
|---|---|---|
| **`sqlite3VdbeExec`** | 1.145–1.165 | **83.2 / 83.9 / 85.2 / 88.5%** (four runs) |
| `sqlite3BtreeInsert` | 1.87–1.95 | 1.8–2.1% |
| `balance_nonroot` | 1.96 | 1.7% |
| `sqlite3VdbeRecordCompare` | 1.91 | 0.4% |
| `sqlite3VdbeMemGrow` | 2.00 | −0.5% |
| `sqlite3BtreeMovetoUnpacked` | 2.04 | −4.4% |

Everything that is not the interpreter is at or below the noise floor.

**METHOD.** `tests/bench/localize.sh` — attribution by LINKER, because this box
exposes no PMU (`/sys/bus/event_source/devices` carries software events only, and
forcing gcc's own scheduler on at -O1 moves nothing, so there is no profiler to
borrow). The same source is compiled by both compilers; every global in the gcc
object is weakened except the chosen names; those names are weakened in the zcc
object; the zcc object is linked first. A strong definition beats a weak one, so
the chosen functions come from gcc and every other name from zcc. The output is
compared against the pure-gcc build before any time is reported.

**WHAT IT COST TO LEARN, and why it is worth an entry.** Seven optimization rows
shipped on 2026-08-27 moved the 42-program taxonomy suite from 1.0400 to 1.0190
and moved real sqlite execution by **nothing** (1.679 → 1.649, ranges
overlapping). Every one of those rows was aimed at a shape found in a KERNEL,
because kernels are the only programs small enough to diff by hand. This entry
is the first fact about WHERE sqlite's time actually goes, and it says the
kernels were never going to reach it.

**⚠️ WHAT THE NUMBER IS NOT.** `-DSQLITE_PRIVATE=` externalizes sqlite's 1,260
internal functions so they have symbols to select by, and that costs BOTH
compilers their static-function inlining. The hybrid is therefore a slightly
different program from the shipping build — read these ratios against the
baselines the script prints under the same flag (gcc 43,070 µs / zcc 85,634 µs),
never against `realprog.sh`'s.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** Nothing in the compiler — it is an instrument. What it
DIRECTED, within the hour it was built: `sqlite3VdbeExec`'s 196-case dispatch
was found to be a 183-deep linear compare chain (gcc: one indirect branch),
because the jump-table row refused any switch whose arms carry edge arguments.
Fixing that took **sqlite's SQL geomean from 1.651 to 1.159** and this workload
from 1.988× to 1.279×. The instrument paid for itself the same day.

**A CAUTION FOR THE NEXT READER.** Attribution to a function is not attribution
to a defect. The histogram of that function (`xray.sh`) named classes — `mov`
+1148, `mov #imm` +485, `str` +353 — and three rows built from those classes
were each refuted at ~1%. What worked was narrowing the window to what the
workload actually executes (`EXPLAIN`) and then counting ONE mnemonic (`br`) in
both assemblies.


---

### M17. The pass audit — which passes pay, which refuse, which are dead weight

**METHOD.** `ZCC_NOPASS=<name>` disables one pass. Compile sqlite with each
disabled in turn and compare: a pass whose removal costs nothing is a pass that
is refusing everything, and a pass whose removal SHRINKS the program is buying
its size with something else — or with nothing. No instrumentation is needed;
the bisection tool already in the tree answers it.

**SIZE, sqlite (baseline 173,611 instructions):**

| pass | instructions if removed |
|---|---|
| `sroa` | **+18,635** |
| `gvn` | +9,728 |
| `cfg` | +6,706 |
| `mem` | +3,122 |
| `ifconv` | +1,424 |
| `sccp` | +645 |
| `purecall` | **0 — inert on this program** |
| `iv` | −944 |
| `inline` | −1,980 |
| `licm` | −1,998 |
| `rotate` | **−4,786** |

**SPEED, `p01_insert`.** Four passes cost size, so the question is what they buy:

| disabled | speed |
|---|---|
| `inline` | **+7.7% slower** — it earns its size |
| `rotate`, `licm`, `iv` together | **−0.2% to −1.1%** — noise |

**THE FINDING.** `rotate`, `licm` and `iv` add **7,728 instructions to sqlite —
4.5% of it — for no measurable speed.** And they are not optional: disabling them
on the 42-program taxonomy suite takes EXEC from **1.0206 to 1.4236**, with
`l2_nested_join` at **10.889×** and 26 of 42 programs above 1.1×. They are worth
40% of execution on loop code.

So this is not a deletion, it is a **missing profitability gate**: three loop
passes that pay enormously on loops and inflate everything else. The row is to
make them decline a transform that cannot pay, not to switch them off.

**A SECOND FINDING, smaller.** `purecall` changes sqlite by zero instructions —
it fires nowhere in 173,611 instructions of real C. Either its precondition is
too narrow or the shape does not occur outside the suite; it should be measured
before it is trusted.

**⚠️ WHAT THIS ENTRY IS NOT.** Removal cost is not the same as value: a pass can
be worth nothing on its own and load-bearing in combination (its output feeding
another's precondition). These numbers rank suspicion, not merit.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation,
`p01_insert` in memory, best-of-7.

**WHY IT WAS RUN.** Three of the four rows shipped on 2026-08-27 were an
EXISTING pass refusing a common shape — `args_match` refusing composite
parameters (2.2× on one program), `ifconv` requiring exactly two join
predecessors (12.5%), and the jump table refusing arms with edge arguments (30%
of sqlite). None was a missing optimization. The audit exists to find the next
one by measurement instead of by accident.


---

### M18. The peer landscape — zcc against cproc+qbe, and what cproc cannot build

**VALUE.** Over the 42 programs of `tests/bench/suite`, exec geomean against
`gcc -O1` on the same machine, all three compilers producing byte-identical
program output:

| compiler | exec geomean |
|---|---|
| **zcc** | **1.0229** |
| cproc + qbe | **1.5555** |
| | worst: `i1_global_acc` **4.13×** |

zcc is ~1.52× faster than cproc+qbe on this surface. cproc compiled all 42 with
zero failures, so the comparison is over the whole set rather than a subset.

**AND THE PART THAT IS NOT A RATIO.** The comparison could not be run on sqlite,
because **cproc cannot compile the amalgamation.** Two separate walls:

* the GCC atomic builtins sqlite selects when the preprocessor advertises
  `__GNUC__` (`__atomic_load_n`/`__atomic_store_n`). This one is fair to patch —
  sqlite's OWN non-GCC branch is `*(PTR)`, which is what any non-GCC compiler
  takes — and past it lies the second;
* `volatile store is not yet supported`. Patching around THAT would change the
  program's semantics, so the run stops there rather than reporting a number for
  different code.

**AND IT IS UPSTREAM-DOCUMENTED, not a quirk of this setup.** cproc's own
`README.md`, under *What's missing*: "`volatile`-qualified types ([#7], requires
qbe support)" and "`long double` type ([#3], requires qbe support)". Its
`doc/software.md` records that building binutils required patching out "subtle
`volatile` usage" — the same wall. And `grep -ri sqlite` over the whole cproc
repository returns nothing: it does not claim sqlite among the software it
builds. (The small compilers known for compiling sqlite are chibicc and tcc,
both of which implement `volatile`.)

**AND THE OBVIOUS OBJECTION, ANSWERED.** cproc builds Oasis Linux, so how can it
fail on sqlite? Three facts, and they are consistent:

* `cproc/qbe.c:458` refuses UNCONDITIONALLY —
  `if (tq & QUALVOLATILE) error("volatile store is not yet supported")`;
* cproc's `doc/software.md` says of Oasis: *"One of the main goals of cproc is to
  compile the entire oasis linux system (excluding kernel and libc). This is a
  WORK IN PROGRESS, but many packages have PATCHES to fix various ISO C
  conformance issues, enabling them to be built."*;
* Oasis's package tree holds **153 packages and sqlite is not one of them**
  (`api.github.com/repos/oasislinux/oasis/contents/pkg`, checked 2026-08-27).

So Oasis is cproc-built on patched sources, by design, and never had to compile
sqlite. That is the difference the comparison is about: `Article C` asks zcc to
be a DROP-IN, and the amalgamation is compiled here unmodified.

zcc compiles the amalgamation unmodified, which is Article C's whole premise.

**WHAT THIS ENTRY IS FOR, AND WHAT IT IS NOT.** THE ULTIMATUM names `gcc -O1` as
the finish line, and nothing here changes that. cproc+qbe is a PEER — the
nearest comparable project, a small C compiler with a real SSA backend — so this
answers "is zcc actually good, or only good against a toy?" It must never become
a gate: beating a weaker reference is flattering, and a number quoted against it
would be exactly the Law 3c failure of announcing parity from a favourable
surface.

**METHOD.** qbe and cproc built in-box with gcc (clang is not installed there;
the compiler used to BUILD a compiler does not affect the code it GENERATES).
Each program compiled by all three, outputs compared before any timing, then
best-of-5 wall time through `tests/bench/timeit.c`.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, qbe and cproc at their
repository tips.


---

### M19. Static block execution frequency, and the 23% of loops that are cold

**VALUE.** `hir::freq::estimate` gives every block a relative execution
frequency, the entry block scaled to `ENTRY = 10_000`. Wu & Larus, *Static
Branch Frequency and Program Profile Analysis*, MICRO-27 (1994) — the same paper
`M12` cites for the trip-count convention.

`ENTRY` and `CEIL` are a SCALE and a saturation bound, not thresholds: `ENTRY`
is the fixed-point denominator that lets integer division carry a fraction, and
`CEIL` stops a deep nest from overflowing. Neither was tuned and neither can be:
every consumer reads a RATIO against `ENTRY`.

**WHY IT WAS BUILT.** Three decisions in one day could not be made for want of
it, and one of them was refused twice:

* the profitability gate for `rotate`/`licm`/`iv`, which add 7,728 instructions
  to sqlite for no measurable speed and are worth 40% of exec on the taxonomy
  suite (`M17`);
* the spiller's `TRIPS = 10`, a stand-in for exactly this analysis (`M12`);
* "we have no profile", offered three times as a reason not to decide.

**THE MODEL.** Reverse postorder, one pass, no linear system: a loop header takes
its non-back-edge predecessors' sum times `TRIPS`; every other block sums its
predecessors weighted by edge probability. Two structural heuristics ship — an
edge into an `Unreachable` terminator is weighted 1 against 1,000, and a
successor that returns immediately 250 against 1,000. **No statistical heuristic
from the paper is included**, because each is a claim about C programs that this
compiler has not measured.

**WHAT IT FOUND, and it is the point.** Of the 1,387 loops rotation touches in
sqlite:

| loop frequency | count |
|---|---|
| **below entry — runs less than once per call** | **325 (23%)** |
| 1–10× entry | 708 |
| 10–100× | 244 |
| above 100× | 110 |

Loop DEPTH cannot see this: 1,066 of those 1,387 are outermost loops, and so are
most of the taxonomy suite's hot loops. What separates the cold 23% is the GUARD
in front of them, which is what a frequency estimate measures and a depth does
not.

**WHAT IT BOUGHT, first consumer.** `rotate` now declines a loop below entry
frequency: sqlite **173,611 → 172,949** instructions (−662), and the taxonomy
suite's INSN geomean is **unchanged to four decimals** (1.0721) — the hot loops
were never touched, which is the whole claim.

**WHY THE GATE IS AT `ENTRY` AND NO TIGHTER.** The model gives every loop the
same `TRIPS` multiplier per level, so it cannot rank two loops by trip count —
only by the guards above them. A threshold above the entry frequency would start
refusing loops whose only property is being at depth 0, which is what most of the
suite's hot loops are.

**DETERMINISM.** Integers, `Vec` by block id, reverse postorder. No hash
iteration, no floating point. `tests/determinism.sh` checks it end to end.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

**WHAT USES IT.** `hir/pass/rotate.rs`. The gates for `licm` and `iv`, and the
spiller's `TRIPS`, are the obvious next consumers and are NOT yet wired.


---

### M20. The copies that are NOT the ABI's fault — 325 against gcc's 8

**VALUE.** `sqlite3VdbeExec`, register-to-register `mov`, zcc against gcc -O1:

| kind | zcc | gcc | reading |
|---|---|---|---|
| total reg-reg `mov` | 1,757 | 482 | +1,275 |
| writing x0–x7 (argument marshalling) | 645 | 379 | +266 |
| into a callee-saved reg from x0–x7, right after a call | 38 | 26 | **near-equal — ABI-FORCED** |
| **callee-saved ← callee-saved** | **325** | **8** | **the gap** |
| into a caller-saved temp x8–x15 | 25 | 0 | 25 |

**WHAT IT SETTLES.** The copy excess is not argument marshalling and not the
call-result convention. A result that is live across a later call MUST move to a
callee-saved register — gcc obeys that rule too, and does it 26 times to zcc's
38. What zcc does 325 times and gcc 8 is move a value from one callee-saved
register to ANOTHER: pure allocator shuffling, forced by nothing.

**WHY IT MATTERS MORE THAN THE COUNT SUGGESTS.** These execute. Unlike the
frame-size rows measured the same day — slot coalescing (203 → 116 slots,
−6,832 bytes of stack) and the cold-loop rotation gate (−662 instructions) —
both of which moved the clock by nothing, a copy in the dispatch path is
retired on every pass through it.

**WHAT IT DOES NOT SAY.** That 325 is an upper bound on what coalescing can
remove, in the same sense `M11-correction` and `M15` were upper bounds: it counts
copies that the ABI does not force, not copies that a colouring could avoid. Two
callee-saved values that genuinely interfere still need a move between them at a
join. The number to beat is 8; the number reachable is unmeasured, and the first
job of the campaign is to measure it — by hand-editing the copies out of one hot
arm and timing it, before any allocator code is written.

**WHEN / WHERE.** 2026-08-27, M1 Pro under Docker, sqlite 3 amalgamation.

### M21. M20 CORRECTED — the 283 copies are COLD; the hot lever is pointer residency

**M20 CLAIMED** the 325 callee-saved←callee-saved `mov`s "execute, unlike the
frame rows" and named a copy-coalescing campaign. **THAT HOTNESS CLAIM IS THE
LAW-2 MEASUREMENT EXCEPTION** — it counted static `mov`s and never checked their
branch targets.

**WHAT THE BRANCH TARGETS SAY.** Of the 138 `mov x22, x19` (the dominant pair,
90% of the 283 with `mov x21, x20`), **107 branch to `_220` (`abort_due_to_error`)
and 32 to `_8` (`no_mem`)** — 100% land on error/OOM/return handlers. They are
SSA phi-args for the error-state join, retired ~never in any workload. The count
is real (a SIZE cost, ~283 insns); the "these execute" is false. A copy-coalescer
(libFIRM co-heur, the §4c-planned recolour) would remove SIZE only, ~0 speed.
Per Law 0 (`exec > size`) and the 1× *speed* goal, it is not built (keep any such
pass behind a default-off toggle if authored, per user 2026-08-27).

**WHERE THE HOT COST ACTUALLY IS — pointer RESIDENCY, proven.** The hot dispatch
reloads the interpreter's core pointers that gcc keeps register-resident:
`p`(Vdbe*, slot #96, 114 loads), `pOp`(#456/#472, ~68), `aOp`(#104, 67),
`pC`(per-Column spill+reload). Only `pOp`'s reload is on the *branch-resolution
critical path* (reload pOp → read opcode → jump-table `br`).

**THE MICROBENCH (isolates one reload+respill of a loop-carried dispatch index
on this core):**
* perfectly-predicted dispatch: ratio mem/reg = **0.966** — the reload is FREE
  (store-forwarding + spare LSU bandwidth; the Law-3c load-analogue of move
  elimination).
* mispredicted dispatch (random opcode stream, like real VdbeExec): ratio
  **1.129** — the L1 load lands on the misprediction-recovery critical path.
  The cost is not the instruction; it is that the reload GATES the mispredicting
  branch.

**THE HAND-EDIT (actual sqlite `.s`, keep pOp resident across the dispatch:
carry it in x20 from both preds of `_15`, drop the reload — a store-to-load of
the pOp slot, provably equivalent; verified byte-for-byte identical output):**
on the CANONICAL `realprog.sh` 11-phase geomean, box-exclusive best-of-5,
**base 1.1661× → patch 1.1553× gcc -O1** — **+0.9%, ~6.5% of the remaining gap,
from removing ONE reload/opcode.** Size-neutral (entry +1 `mov`, `_15` −1 `ldr`).
Gains concentrate in the dispatch-bound phases (p03 index 1.214→1.143, p05 scan
1.222→1.185, p08 subquery 1.192→1.178); no regression.

**SMASH-AND-GRAB VERDICT.** Hunted the two hottest arms (`OP_Column` _81 run
2×/row, `OP_Next` _132 1×/row) for a jump-table-class structural defect: none.
No bad `madd`, no redundant `sxtw`, no wrong addressing — every arm just reloads
p/pOp/pC. The jump-table row was the last big grab; what remains is systemic
spilling worth single-digit % (full p+pOp+pC residency est. +2–4%, sqlite
~1.15×→~1.12×), a gated allocator campaign, not one trick. The stopping point
is honest: 1× is not reachable by a grab on this surface.

**REPRODUCE.** microbench `scratchpad/disp*.s` + `bench*.c`; hand-edit
`scratchpad/patch.pl` against a fresh `zcc -S sqlite3.c` (slot number is
build-specific — the pass will not be). **WHEN / WHERE.** 2026-08-27, M1 Pro
under Docker, sqlite 3 amalgamation, gcc -O1 referee.

### M22. The unroll budgets — how many copies of a loop are worth making

**WHAT THIS IS.** `hir/pass/unroll.rs` fully unrolls a loop whose trip count is
a small literal. Two numbers decide when: `max_trips` (how many copies at most)
and `max_body` (how large the body may be, in HIR instructions). Neither the ISA
nor the ABI has anything to say about either — no spec sentence sets them — so
Article E's question ("the spec's number, or my convenience's number?") has only
one honest answer available: measure. This is that measurement, and the row is
cited from the code as `MEASURED M22`.

**METHOD.** The 49-program taxonomy suite, each program compiled by both
compilers and its output compared before any time was taken, best-of-3 twice per
program with the fork+exec floor subtracted, gcc -O1 re-timed in the same rounds.
The reported figure is the EXEC geomean over all 49 — no program excluded, no
bucket. The budgets are read from the environment so a sweep costs no rebuild
(`ZCC_UNROLL_TRIPS`, `ZCC_UNROLL_BODY`).

| max_trips (body 24) | EXEC geomean | | max_body (trips 4) | EXEC geomean |
|---|---|---|---|---|
| 0 — pass off | 1.0261 | | 12 | 1.0212 |
| 2 | 1.0210 | | **24** | **1.0181** |
| **4** | **1.0181** | | 48 | 1.0184 |
| 8 | 1.0196 | | | |

**WHAT IT SAYS.** Trips has a real optimum at 4 and turns over after it: eight
copies is measurably worse than four (1.0196 vs 1.0181), which is the point
where the duplicated code stops paying for the branch and the register it saves.
Body is flat above 24 — 48 buys nothing (1.0184 vs 1.0181) and 12 costs 0.3% —
so 24 is the smallest value that gives up nothing, which is the one to hold.

**WHY IT IS NOT A TUNING KNOB.** Both numbers move a geomean over 49 programs on
one microarchitecture, which is the same narrow surface Law 3c warns about: they
are the best available answer for THIS suite on THIS core, and a wider suite may
move them. What the sweep does establish is the shape — a maximum near 4, and
saturation in body — and that shape is what a re-measurement should be checked
against.

**WHEN / WHERE.** 2026-08-28, M1 Pro under Docker, aarch64-linux musl release
zcc, gcc -O1 referee, 49-program suite (42 taxonomy + the 7 database kernels).

### M23. What the interprocedural row costs, and what it buys

**SUPERSEDED BY M24 (2026-08-28, the same day).** Every number below is a correct
reading of the row as it stood, and the conclusion drawn from them — park the row
— was wrong, because the measurement was taken of the ROW when the cost belonged
to ONE OF ITS THREE RULES. M24 attributes it. The row is on by default again.

**WHAT THIS IS.** `hir/pass/inline.rs` was off by default. That is a POLICY, and a
policy needs a number rather than a preference, so this is the measurement it was
decided on.

**THE ROW IS NOT SLOW ITSELF.** Its own time on the sqlite amalgamation is about
two seconds — splicing 1.1 s over 7,968 splices, plus 0.5 s of liveness rebuilds.
What it does is GROW every function it touches, and the passes below it are
superlinear in function size, so it multiplies their cost. sqlite goes from
237,846 to 289,478 lines of assembly, +22%, and the growth concentrates in the
functions that were already largest.

| | with the row | without it |
|---|---|---|
| sqlite compile (gcc -O1: 6.4 s) | 22.2 s | 4.7 s |
| 60 cached yarpgen tests, vs gcc | 5.19× | 3.84× |
| taxonomy suite EXEC geomean | 1.0184 | 1.0784 |
| taxonomy suite INSN geomean | — | 1.0768 |

**WHAT IT COSTS TO TURN OFF.** About six points of exec geomean over the 49
programs, and the damage is concentrated rather than spread: `e3_struct_byval`
is 1.985, a by-value-struct callee the row used to erase entirely.

**WHY IT IS OFF ANYWAY.** The fuzzing campaigns are how miscompiles are found,
and they are gated on compile time: at 5.19× a 300-seed local gate does not fit
in the ten minutes it is meant to, and a 10,000-seed campaign is about six hours
of compiling. A correctness gate that cannot be run often is not a gate. The row
waits for the passes it feeds, not the other way round.

**WHAT TURNS IT BACK ON.** `spill` (7.7 s of the 22) and `cfg` (~3 s) growing
faster than linearly in function size. Make those proportional and the row's cost
falls to roughly its code growth — about +22%, not +370% — and the six points
come back with it.

**WHEN / WHERE.** 2026-08-28, M1 Pro under Docker, aarch64-linux musl release
zcc, gcc -O1 referee, sqlite 3 amalgamation and the 49-program taxonomy suite.


### M24. Which of the three inlining rules was the growth, and what it bought

**WHAT THIS IS.** M23 measured the interprocedural row and parked it. This
attributes that measurement to the individual rule responsible, which reverses
the decision. Cited from `hir/pass/inline.rs` and `hir/pass/mod.rs`.

**THE THREE RULES.** A site was admitted if the callee is called once (and either
has internal linkage, or the site is in a loop and the callee has none of its
own); or if the body is no larger than the call sequence it replaces; or — the
third rule — if the site is in a loop, the callee is loop-free, and the body is no
larger than the call sequence PLUS the number of values live across the site.

The first two cannot make the program bigger. The first moves a body that is then
deleted; the second substitutes something smaller than what it removes. Only the
third can grow code, by up to the live-across count per site, and it is the only
one that fires at many sites for the same callee.

**ATTRIBUTION, sqlite amalgamation**, counted at the chosen site:

| rule | splices | instructions added |
|---|---|---|
| called-once | 523 | +24,376 (bodies then deleted) |
| body ≤ call sequence | 3,592 | +9,288 (each ≤ what it replaced) |
| live-across | 4,555 | **+62,520** |

**WHAT THE THIRD RULE COST**, with the row on both times:

| | all three rules | first two only |
|---|---|---|
| sqlite compile | 24,235 ms | 7,006 ms |
| sqlite assembly | 219,461 insn | 174,730 insn |
| `cfg` row | 6,285 ms | 675 ms |
| `spill` | 8,155 ms | 1,809 ms |
| the inline row itself | 2,708 ms | 198 ms |

Code grew 28.1% while the function COUNT fell 30%, so the average function grew
83% — that, not the 28%, is what the superlinear passes below were charging for.

**WHAT THE THIRD RULE BOUGHT.** Three interleaved pairs of the 49-program
taxonomy suite, alternating within one session because the geomean's spread
across sessions is ±0.007:

| | all three | first two |
|---|---|---|
| EXEC geomean | 1.0236 / 1.0227 / 1.0218 | 1.0254 / 1.0252 / 1.0247 |

0.24%, consistent in sign across all three pairs, and it is one program:
`n1_btree_page` 1.314 → 1.244, the byte-pair reader inside a binary search that
the rule was written for.

**WHY IT WAS REMOVED, and it is not a compile-speed argument.** The file already
carries the criterion: dropping `is_static` was refused at 16% size for 7% speed
as failing THE ULTIMATUM's both-axes clause. This rule is 26% size for 0.24%
speed. And its compile cost is what parked the WHOLE row, so it was spending 4.9
points of exec geomean to buy 0.24 — an exec-for-exec trade, decided on the axis
Law 0 ranks first among the two, not on compile speed.

**WHAT THE ROW IS WORTH WITHOUT IT**, measured on the shipped default:

| | row off | row on |
|---|---|---|
| taxonomy suite EXEC geomean | 1.0741 | 1.0220 |
| sqlite compile (gcc -O1: 6,544–6,814 ms) | 4,829 ms | 6,687–6,808 ms |
| sqlite assembly (gcc -O1: 157,074) | 171,287 | 174,730 |
| `fullsuite.sh all` | 4m00s | 5m55s |

Compile time is at gcc -O1 parity with the row on. The 1.45× is against zcc's own
no-inline build, which is faster than the referee.

**WHEN / WHERE.** 2026-08-28, M1 Pro under Docker, aarch64-linux musl release
zcc, gcc -O1 referee, sqlite 3 amalgamation and the 49-program taxonomy suite.
The off/on geomeans in the last table are from different box sessions; only the
three-pair table above is interleaved, and it is the one the removal rests on.

### M25. Division by a constant: the theorem is right and it loses ON THE M1 PRO — SUPERSEDED by `M47`

> **STATUS: REVERSED.** The row this entry removed was rebuilt and shipped in
> `M47`. Everything below is a correct measurement of an Apple M1 Pro, and its
> conclusion does not survive the crossing to Neoverse V2, where `udiv` costs 4.98
> dependent adds and the same theorem is worth 7% of the whole suite. Read this
> entry as the record of how a row is deleted on one core's evidence.

**WHAT THIS IS.** Granlund–Montgomery division-by-multiplication was implemented,
proven, measured, and REMOVED. This is the measurement, recorded so the row is
not rebuilt on the strength of the textbook.

**WHAT WAS BUILT.** A HIR pass rewriting `UDiv`/`URem`/`SDiv`/`SRem` by a
non-power-of-two constant into the high half of a product plus a shift — Hacker's
Delight §10-9 `magicu` and §10-4 `magic`, at 32 and 64 bits, the 32-bit case as a
widening product with its top half taken (what `umull`/`smull` are). Powers of
two, `d = 0` and `|d| = 1` refused, remainder derived as `n - (n / d) * d`.

**THE TRANSCRIPTION IS CORRECT, and that is not the question it answers.** The
multiplier and shift were checked against the real division over every divisor
from 3 to 2000 at the boundaries of the width — 0, 1, 2, the powers, `INT_MIN`,
`INT_MAX`, `UINT_MAX` — and at 64 bits over a divisor sample including
`0x1_0000_0001` and `u64::MAX - 1`, signed and unsigned, positive and negative
divisors. Not one case disagreed. The row was correct; it was not profitable.

**WHAT IT MEASURED**, 49-program taxonomy suite, against the shipped compiler:

| | shipped | + divmagic | + divmagic + `ZCC_HOIST` |
|---|---|---|---|
| EXEC geomean | 1.023 | 1.045 | 1.034 |
| INSN geomean | 1.0717 | 1.1101 | 1.1440 |
| `a2_udiv_mod` | 1.117 | 1.100 | — |
| `a3_sdiv_mod` | 1.128 | 1.148 | — |
| `e3_struct_byval` | 1.052 | 1.322 | — |

**WHY IT LOSES, and it is not the theorem.** gcc -O1 emits the same rewrite on
the same programs and is faster, so the decision is not what separates them —
the ENCODING is. For `k % 5` in `e3_struct_byval`'s loop zcc emits nine
instructions and gcc five:

```
sxtw x10, w8
movz x11, #26215
movk x11, #26214, lsl #16    ← the multiplier, rebuilt every iteration
mul  x11, x10, x11
asr  x11, x11, #32
asr  w11, w11, #1
add  w11, w11, w11, lsr #31
movz w12, #5                 ← and the divisor, also every iteration
msub w11, w11, w12, w8
```

Three of the nine are constant materialization inside the loop, which gcc hoists.
So the row is a DEPENDENT of the loop-invariant constant hoist, and that hoist is
itself off because it costs 3.0% of INSN geomean across the suite (this section's
third column is the pair measured together: still a loss on both axes).

**AND THE DIVIDER ON THIS CORE IS NOT SLOW**, which is the fact the textbook
assumes and this machine denies. `a2_udiv_mod` runs three million iterations of a
loop containing TWO `udiv` in 4,068 us — 1.36 ns, about 4.3 cycles per iteration.
A divider that were "tens of cycles, not pipelined" could not produce that
number. The folklore is a Cortex-A53-era fact; it is not a fact about this core,
and there is no vendor optimization guide to have told us otherwise, which is
precisely why this section exists.

**WHAT WOULD HAVE TO CHANGE** before it is worth rebuilding: the loop-invariant
constant hoist must pay for itself first, and the emitted sequence must reach
gcc's five instructions rather than nine. Until both hold, the shipped `udiv` is
the faster code.

**WHEN / WHERE.** 2026-08-28, M1 Pro under Docker, aarch64-linux musl release
zcc, gcc -O1 referee, the 49-program taxonomy suite.

### M26. Where zcc's instructions actually go, against gcc -O1

**WHAT THIS IS.** Three rows were built or tested on the strength of reading one
program's assembly, and all three lost (`M25`, and the refutations beside it).
This is the measurement that should have come first: the whole 49-program suite
compiled by both compilers, every mnemonic counted, and the excess attributed.
It says what to work on and, more usefully, what not to.

**THE TOTAL.** zcc 7,551 instructions, gcc -O1 6,598 — **+953, or +14.4%.**

**SPELLING FIRST, or the table lies.** gcc writes `mov w7, 18725` where zcc
writes `movz w8, #1`, and `bne` where zcc writes `b.ne`. Uncombined, `movz`
reads as +432 against a gcc that never emits it and `b.lt` as +195 against zero.
Both are the same instruction. The families below are combined.

| family | zcc | gcc | Δ | share of the +953 |
|---|---|---|---|---|
| `mov` register→register | 1,006 | 339 | **+667** | 70% |
| load/store slots (`ldr`+`ldp`, `str`+`stp`) | 1,107 | 967 | +140 | 15% |
| `cmp` + `subs` | 484 | 359 | +125 | 13% |
| `mul` + `madd` + `msub` | 187 | 70 | +117 | 12% |
| `sxtw` + `sbfiz` | 161 | 93 | +68 | 7% |
| `mov` register→immediate | 608 | 790 | **−182** | — |

The last row is the one that says the constant-sharing row works: zcc
materializes fewer constants than gcc does, not more.

**WHERE THE COPIES ARE**, classified by what follows them — a `bl` within the
next few instructions (argument placement), a branch or a label (a block edge,
which is where SSA destruction puts a phi), or neither:

| | zcc | gcc | Δ |
|---|---|---|---|
| at a block edge | 519 | 56 | **+463** |
| in the body | 312 | 206 | +106 |
| placing a call argument | 175 | 77 | +98 |

**So half of zcc's entire instruction excess over gcc -O1 is a phi copy at a
block edge** — 519 against 56, a factor of nine. It is not a missing
optimization row: it is the copy SSA destruction leaves and coalescing does not
remove. The same shape was measured independently on the sqlite amalgamation,
where register-to-register moves are +10,464 of a 20,264-instruction gap (52%).

**AND 67 OF THEM COPY A REGISTER TO ITSELF** (gcc: none). `k1_dispatch` ends
every one of its switch arms with the identical `mov w10, w10` — a zero-extension
of a value the preceding `and x10, x11, #7` already left zero above bit 32.
`mir/pass/ext.rs` turns a redundant extension into a `Copy`, colouring then gives
it the same register, and `emit.rs` prints every `Copy` without asking whether
its two ends are the same. Removing it is NOT unconditionally sound — a `w`-form
write zeroes bits 63:32, and at `Width::W32` the lattice proves a fact about the
low half only — so it wants the fact restated at full width first, not a peephole.

**WHAT THIS RETIRES.** Ranking work by what the suite's assembly actually
contains, rather than by what one program's inner loop suggests: the three rows
tried before this measurement (magic division, switch trees, shift-and-add for a
constant multiply) address families worth 12%, 0% and 12% of the gap, and each
one measured a loss. The 70% family was not touched by any of them.

**WHEN / WHERE.** 2026-08-28, M1 Pro under Docker, aarch64-linux musl release
zcc, gcc -O1 referee, the 49-program taxonomy suite, both compilers at `-S`.

**⚠ TWO OF THE ROWS ABOVE ARE WRONG. Read `M26-correction` before using this
table.** The `+667` and the `−182` are the same instructions counted twice with
opposite signs.

### M26-correction. The zero register is not a copy, and it broke both headline rows

**THE DEFECT WAS IN THE INSTRUMENT** — Law 2's measurement exception, claimed
only after two independent formulations converged on the same answer.

`M26` classified a `mov` by whether its second operand begins with `#` or a
digit. `mov w9, wzr` begins with neither, so every one of them landed in the
register→register row. **gcc never emits that form**: it writes `mov w9, 0`,
which landed in the register→immediate row. One activity — materializing a
constant zero — was charged to zcc as coalescer excess and to gcc as constant
materialization, once each, and it is exactly the trap the paragraph above it
warns about, one spelling further down.

Re-counted with `[wx]zr` excluded from the copy row and folded into the constant
row, same corpus, same day:

| family | zcc | gcc | Δ | was reported as |
|---|---|---|---|---|
| register→register copy (true copy) | 712 | 295 | **+417** (44% of the gap) | +667, 70% |
| constant materialization (`movz`/`movk`/`mov #imm`/`mov …, zr`) | 951 | 790 | **+161** | −182 |

So zcc materializes **more** constants than gcc, not fewer; the sentence claiming
the constant-sharing row is proven by that column is withdrawn.

**THE SECOND ANGLE, which is what allows the claim.** The compiler's own
counters do not read assembly at all. `regalloc::coalesce_report`
(`ZCC_COALESCE`) classifies every SSA-destruction pair at the moment it is
created, and `destruct::movkind_report` (`ZCC_MOVKIND`) counts what the
sequentializer emits. Their columns close exactly — 203 + 26 + 289 = 518 edge
pairs on the suite, 4,965 + 222 + 2,604 = 7,791 on sqlite — and the third column
is the one that names the cause: **every pair with a physical end has the ZERO
REGISTER as its argument. 289 of 289 on the suite, 2,604 of 2,604 on sqlite.**
Not one is a call result. A constant zero passed along an edge, nothing else.

**THE CORRECTED ATTRIBUTION** of the suite's 518 block-edge "copies":

| bucket | n | share | what it is |
|---|---|---|---|
| `mov wN, wzr` | 289 | 56% | a constant, not a copy. gcc pays one instruction for the same thing |
| FREE | 203 | 39% | both ends virtual, the argument dies on the edge: the merge was legal and biased colouring missed it. **The whole reachable ceiling** |
| BOUND | 26 | 5% | the two names genuinely coexist; no colouring removes it |
| permutation cycle | 3 | 0.6% | a swap. Costs copies however it is coloured |

**WHAT THIS RETIRES.** "Half the entire gap is phi copies coalescing never
removed" (a factor of nine, 519 against 56) is withdrawn: the reachable coalescing
ceiling on this suite is **203 instructions of a 953 gap, 21%**, and the copy
family as a whole is 44% rather than 70%. It also retires the recorded conclusion
that the hint refusals need eviction — see `M27`.

**AND THE 67 SELF-COPIES ARE NOT FREE EITHER.** `ZCC_R42RES` classifies the
survivors: 61 `wide-read`, 1 `unknown-form`, 0 `no-abi-reader`. A `wide-read`
survivor has a reader that genuinely looks past 32 bits, so `mov w10, w10` IS the
zero-extension that reader needs and deleting it is a miscompile. gcc reaches
zero by not NEEDING the extension, which is a row in `ext.rs`/HIR, not a
peephole in the emitter. The `M26` paragraph suggesting the fact be restated at
full width stands, but it buys an extension-elimination row, not 67 free
instructions.

### M27. The coalescing hint is refused by the ABI, not by an occupant

**WHAT WAS BELIEVED.** The recorded conclusion from the sqlite hint census was
"14,615 hints refused because the register was already OCCUPIED ⟹ this needs
EVICTION or priority colouring". Three ordering fixes were then tried against it
and refuted.

**WHY IT WAS WRONG.** `free(p, occ)` is a conjunction of four clauses —
allocatable, unoccupied, no physical conflict, and the AAPCS64 §6.1.1 half — and
the instrument counted every failure of the conjunction under the word
"occupied". Split by clause, on the same corpora:

| refusal cause (PHYSICAL partner) | suite | sqlite |
|---|---|---|
| register not allocatable (it is `xzr`) | 269 | 1,567 |
| ABI: value crosses a call, hint is caller-saved | 116 | 8,147 |
| physical conflict | 100 | 4,790 |
| **genuinely occupied** | **3** | **136** |

**Occupancy is 0.6% of the refusals on the suite and 0.9% on sqlite.** Eviction
and priority colouring are aimed at a bucket that is not there, which is why the
three ordering fixes measured nothing.

The virtual-partner twin (`ZCC_VHINT`, the pairs the physical instrument could
not see, and where `M26-correction`'s FREE column lives) says the same thing one
register class over: of 191 refusals on the suite and 2,447 on sqlite, the
AAPCS64 half accounts for 72 and 1,554 — the largest single cause on both.

**THE CAUSE IS THE ALLOCATION ORDER, and it is doing its job.** `GPR_ORDER`
offers the caller-saved half first so that short-lived values do not squat in the
callee-saved registers that call-crossing values have no alternative to. The
side effect is that a short-lived value which is the COPY PARTNER of a
call-crossing value takes a register the partner may never join, and SSA
destruction pays a `mov` on that edge for ever.

**THE ROW: partner-aware half selection** (`color::assign`, `ZCC_CSBIAS`,
default 1). A value that does not itself cross a call, but has an uncoloured copy
partner that does, is offered the callee-saved half FIRST — and only registers
the function has ALREADY committed to, because a fresh one costs a prologue save
and an epilogue restore, two instructions to buy a merge worth one.

| | suite insn | suite EXEC | sqlite insn | sqlite runtime |
|---|---|---|---|---|
| baseline | 7,551 | 1.0238 | 171,743 | 1.000 |
| level 1 (shipped) | 7,545 | 1.0255 | 171,276 | **0.9740** |
| level 2 (measured, not shipped) | 7,569 | — | 170,542 | 0.9937 of level 1 |

The suite EXEC difference is 0.0017 against a ±0.007 session spread — a wash, not
a loss. The sqlite runtime figure is the one that carries the row: an
**interleaved A/B of the two zcc builds inside ONE box session**, best-of-5 over
the 11 `tests/bench/sql` phases, geomean 0.9740. Taken as two separate
`realprog.sh` runs it read 1.1298 then 1.1590 — and the gcc side, which cannot
have changed, moved 7.6% between them. That is the ±0.007 trap at real-program
scale, and it is why the ratio against gcc is not the instrument for a row this
size.

Level 2 widens "already committed" from the register to the half. It is a further
0.9937 on sqlite runtime and costs the suite +18 instructions; a deterministic
regression on the size axis is not bought with 0.6% sitting near the noise of the
axis above it.

**LAW-4 RESIDUAL.** The row fires only where a partner is uncoloured at the
moment of the decision, and 203 FREE pairs remain on the suite — of which the
ABI clause explained 72. The rest are 77 genuinely occupied and 42 where the
transitive walk reached a different partner first. Neither is discharged.

**WHEN / WHERE.** 2026-08-28, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, gcc -O1 referee. Gate `fullsuite.sh all`: 15 PASS / 0 RED.

### M28. The sign-extended index, and what the IVX trip gate is really refusing

Two residual measurements taken the same day, both on the 49-program suite.

**THE IVX TRIP GATE IS NOT THE BINDING ONE.** `value_candidates` refuses a loop
whose trip count SCEV cannot bound above 32, and its own comment classes that as
a Law-4 category (b). It is the second-largest refusal — 653 sites, 101 of them
in `n2_varint_record`, whose EXEC ratio is 1.435. Removing it (`ZCC_IVTRIP=0`)
changes the suite by **zero instructions**: every one of the 653 is refused again
by a later condition, mostly the silent `inside < 2` and `escaped` pair. On
sqlite it moves 2 instructions of 171,276. So the gate is very nearly dead
weight, and — more usefully — relaxing it buys nothing, which retires it as a
row. The residual with it removed:

| refusal | sites |
|---|---|
| `scev-refused` | 1,489 (78%) |
| `invariant-address` | 289 |
| `no-multiply-to-remove` | 109 (fundamental — there is no multiply) |
| `multiply-shared` | 18 |

**SCEV's own coverage is the frontier**, not any gate written on top of it.

**THE SIGN-EXTENDED INDEX — counted, then REFUTED on the clock in the same
session.** Read to the end before acting on the table.

| | zcc | gcc -O1 |
|---|---|---|
| memory operands of the form `[base, wN, sxtw]` | 141 | 11 |
| standalone `sxtw` | 161 | 65 |
| post-indexed accesses | 66 | 0 |

Concentrated: `k1_dispatch` 78 of the 141, `k2_live_pressure` 17,
`n2_varint_record` 10 — the dispatch and parsing loops, which are also where the
EXEC ratio is worst.

The cause is that a C `int` index stays 32 bits in zcc's HIR, so `a[i]` must
sign-extend at every use; gcc widens the induction variable to 64 bits once (it
may: signed overflow is undefined, ISO 9899 6.5p5) and then addresses with a
plain 64-bit index. **This is a Law 3c fact before it is a size fact**: the
extension folded into the addressing mode costs no instruction and one cycle
(`MEASURED M1`, extended-register operands are 2 cycles against 1), and it sits
on the ADDRESS path of a load inside a dispatch loop.

It also reopens `MEASURED M2` on its own terms rather than contradicting it. M2
refused the unit-stride pointer walk because "the scaled index rides for free" —
which is true of a 64-bit index and false of a 32-bit one, where the same
addressing mode carries an extension M1 prices at a cycle. M2's measurement
(`j2_histogram`) is not disturbed; its scope is.

**AND THE HAND EDIT REFUTES IT.** The count is large and the clock does not care.

`k1_dispatch` holds 78 of the 141 sites. 73 of them are provably rewritable with
no analysis at all: 63 index registers are defined by `and wD, wS, #255` and 10
by `ldrb`, and a `w`-form write ZEROES bits 63:32 (DDI 0487 B1.2.1, the same fact
`destruct::drop_self_moves` rests on), so the register already holds the
zero-extended value and the index is in [0,255] where `sxtw` and `uxtw` and the
identity all agree. Rewriting `[xB, wI, sxtw #k]` to `[xB, xI, lsl #k]` at those
73 sites is therefore sound by inspection. Output identical, instruction count
identical — 1,597 both sides, which is the point: this row can only pay in
cycles.

| | best-of-10 | best-of-40, alternating |
|---|---|---|
| hand / base | 0.9798 | **0.9946** |

**0.5%, on the program that carries 55% of the whole suite's sites.** The first
reading said 2.0% and was noise of the same magnitude as the effect — the fourth
time this project has been told that by a single reading.

**THE HARDWARE FACT THIS ESTABLISHES, and it is a correction to `M1`'s SCOPE.**
`M1` measured an extended-register operand at 2 cycles against 1 and the operative
rule in Law 3c was written from it. That is an **ALU** fact. In a MEMORY
addressing mode the extension is absorbed by address generation and costs
nothing measurable on this core: 73 of them removed from the hot path of a
dispatch loop bought half a percent. So `[base, wN, sxtw #k]` is NOT a Law-3c
liability, the 141-against-11 count is not a gap, and induction-variable widening
cannot be justified from it — whatever it is worth, it is not worth this.

The narrow peephole underneath (a `sxtw` whose source is the immediately
preceding 32-bit ALU result) is 30 instructions on the suite, is a size row
rather than a time one, and is not worth a pass on its own either.

### M29. The weighted count, and the copy the static model could not see

**THE PROGRAM.** `n7_nested_subq` is the suite's cleanest anomaly: 162
instructions against gcc's 154 (+5%), the SAME number of conditional branches on
both sides once the spellings are combined, and an EXEC ratio of **1.370**.
Nothing in the static picture accounts for it.

**WHAT IT WAS.** Its inner loop runs 5,760,000 times and carries a counter that
the allocator spilled and `promote` then bound to `x28`. Because `promote` runs
AFTER SSA destruction, the slot's reload and store came back as ordinary moves
that nothing coalesces, and the store sat alone in the block
`split_critical_edges` had made for the latch:

```
.Lmain_7:  mov x8, x28 ; sub x8, x8, #1 ; add … ; cbnz x8, .Lmain_67
.Lmain_67: mov x28, x8 ; b .Lmain_6
```

Three executed instructions and one extra taken branch, in a fifteen-instruction
body. Hand-edited to `sub x28, x28, #1 ; cbnz x28, .Lmain_6`: **0.8585**, at an
instruction count that does not move — 161 both sides, because the trampoline
block stays in the listing as dead code.

Measured apart, on the same build:

| edit | Δ static insn | ratio |
|---|---|---|
| propagate the reload (`sub x8, x28, #1`) | −1 | 0.996 |
| move the split block next to the latch, keeping the copies | +1 | 0.983 |
| sink the store into its producer, branch straight back | 0 | **0.8585** |

**THE LESSON, and it is about the instrument.** `cost(f) = |MIR(f)|` is exact for
SIZE and Law 3c already names its blindness to dependence CHAINS. This is a
third blindness and a simpler one: **a static count weighs an instruction in a
latch executed 5.76M times exactly as it weighs one in a cold arm.** The INSN
geomean moved 1.0714 → 1.0706 for a change that moved this program 1.370 → 1.195.
zcc already computes the frequencies (`hir::freq::annotate`, carried into
`MBlock.weight`), so the weighted count `Σ_b weight(b)·|insts(b)|` is available
and is the cost model an EXEC claim should be predicted on. It is not built.

**THE ROW, banked.** `promote::sink_stores` (step 7) with the three side
conditions the batteries forced: the promoted register dead on the latch's other
edges, the produced register dead on ALL of them (the first cut asked only about
the edge it followed, and three allocator batteries answered `⟦mir_v⟧ ≠ ⟦mir_p⟧`
because a loop-carried ACCUMULATOR is read on the loop's exit edge), and a
producer that defines that register and nothing else, plainly. Plus the step-5b
half: the invariant filter on the copy propagation was the propagation's side
condition, not the theorem's, and comes off once the scan stops at a definition
of the promoted register.

| | suite EXEC | suite INSN | n7 | k1_dispatch | k2_live_pressure |
|---|---|---|---|---|---|
| before | 1.0235 | 1.0714 | 1.370 | 1.178 | 1.119 |
| after | **1.0210** | 1.0706 | **1.195** | 1.175 | 1.112 |

Gate `fullsuite.sh all`: 15 PASS / 0 RED. Battery:
`promotion_sinks_the_latch_store_into_its_producer`, both halves.

**WHEN / WHERE.** 2026-08-28, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, gcc -O1 referee, 49-program taxonomy suite.

### M30. The weighted count, built — and the first thing it found

`M29` asked for `Σ_b weight(b)·|insts(b)|` and named it the cost model an EXEC
claim should be predicted on. It is now `mir::cost::weighted`, reported by
`ZCC_WCOST=1`.

**IT NEEDS `ZCC_WEIGHTS=1` TO MEAN ANYTHING, and that is a finding of its own.**
`hir::freq::annotate` computes the frequencies but is OFF by default, because
R5.1 measured its two CONSUMERS (layout and spill) as a loss and put all three
behind one switch. So `MBlock.weight` is 1 everywhere in a default build and the
weighted count degenerates to the static one. The annotation and the consumers
want separate switches; until then, take the ranking with `ZCC_WEIGHTS=1` and
apply it to a default build.

**WHAT IT FOUND IMMEDIATELY.** `m1_resp_parse` (EXEC 1.44, the suite's worst):

```
WCOST main total=18,455,179
  b6  11 insts x weight 500,000 = 5,500,000  (30%)
  b10 11 insts x weight 477,270 = 5,249,970  (28%)
```

**58% of the program in two eleven-instruction blocks**, and the static count
ranks them nineteenth. Both held

```
movz w13, #97 ; add w12, w13, w12          gcc -O1:  add w0, w0, 97
```

**A64 puts the immediate on the RIGHT and has no mirror form**, and `isel::binop`
offered only its `b` operand to `imm::as_rhs`. A commutative operation written
with its constant on the left — `'a' + i % 26`, which is how C source usually
says it — therefore materialized the constant into a register and added two
registers: two instructions where the ISA has one, inside whatever loop the
expression sits in.

**THE ROW.** Swap the operands of `Add`/`Mul`/`And`/`Or`/`Xor` when the left is
an immediate and the right is not (ISO 9899 6.5 — all five commute, including
unsigned wrap-around; `Sub`, the divisions and the shifts do not and are not
listed). `ZCC_NOCOMMUTE=1` is the A/B seam.

| | EXEC (two interleaved pairs) | INSN |
|---|---|---|
| off | 1.0214 · 1.0236 | 1.0706 |
| on | **1.0206 · 1.0202** | **1.0688** |

Both axes, and the row wins both pairs — a single earlier reading had said 1.0293
and was noise of twice the effect's size. Gate `fullsuite.sh all`: 15 PASS / 0
RED.

**WHERE THE SUITE STANDS after the day's four rows** — and the shape of it
matters more than the geomean: EXEC **1.0204**, INSN **1.0688**, median 1.02,
**but 11 of 49 programs above 1.1× and a worst of 1.44**. The geomean is 2% from
parity and the tail is not. Law 3c's rule applies to the next session's choice of
row: a geomean already at 1.02 cannot be moved by anything, while `m1` 1.44,
`n1` 1.33, `m2` 1.32 each hold a `n7`-sized win — that one was 13% of a program
for 0.0008 of the INSN geomean.

**WHEN / WHERE.** 2026-08-28, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, gcc -O1 referee, 49-program taxonomy suite.

### M31. The switch-arm ORDER is the biggest single lever left, and it is free

**THE SHAPE.** A byte-at-a-time protocol parser is a `switch` on a state inside
the read loop. zcc lowers a switch too sparse for a jump table to a LINEAR
compare chain in the arms' source order, so a state tested late costs one
`cmp`+`b.eq` pair per byte for every arm ahead of it. The two worst programs in
the suite are both this shape and nothing else:

| | arms | chain length | EXEC |
|---|---|---|---|
| `m1_resp_parse` | 6 | 6 | 1.44 |
| `m2_http_parse` | 9 | 9 | 1.32 |

`n3_vdbe_loop` has no such chain, and `d1_switch` is a jump table.

**WHAT IT IS WORTH,** hand-edited into the `.s`, output verified identical, best
of 21 alternating runs, and **at an instruction count that does not move**:

| edit | m1 | m2 |
|---|---|---|
| hot arm tested FIRST | **0.9308** | **0.7754** |
| self-transition arms first, source order among them | — | **0.8566** |
| balanced binary search over the sorted case values | — | 1.0741 |

m2 at 0.7754 is **1.318 → 1.02**; at the profile-free heuristic, 1.318 → 1.13.

**THE BINARY SEARCH LOSES, which confirms `M4` from the other side.** `cmp #3 ;
b.eq ; b.lt` over `{0..5}` costs 7.4% MORE than the linear chain: the split adds
an unconditional branch on every path and buys nothing when the hot arm can
simply be first. gcc reaches the same place by a different route — it tests the
median for equality first and that median happens to be `m1`'s hot state.

**THE PROFILE-FREE SIGNAL, and it is structural.** The hot arm of a state
machine is the one that STAYS: `S_BULK` consumes payload bytes and `S_HVALUE`
consumes a header value, and each re-enters itself until a delimiter. In SSA that
is visible without any profile — the arm's edge back to the loop header passes
the switch's OWN operand as the state parameter, unchanged. Ordering those arms
first, keeping source order among them, is what the 0.8566 row measures.

**WHAT IS NOT SETTLED.** The heuristic recovers 14.3% of m2's available 22.5%
because m2 has FOUR self-transition arms and the two hot ones are third and
fourth among them. m1 has four as well and its hot arm is fourth, so the
heuristic is expected to buy little there while the ideal order buys 6.9%.
Ranking WITHIN the self-transition set needs something this measurement does not
have. The row is therefore worth building at the heuristic strength and
measuring on the whole suite; the gap to ideal is a Law-4 residual, not a
failure.

**WHERE IT SITS.** `isel::lower`, `Term::Switch` — the chain is built from the
HIR arms in order, so the reordering belongs in an HIR pass ahead of it, where
the loop header and its parameters are still visible. Reordering the arms of a
switch is semantics-preserving by construction (the cases are mutually exclusive
equality tests on one value), so the commuting square is trivial and the effect
assertion is the chain's position of the self-transition arm.

**WHEN / WHERE.** 2026-08-28, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, gcc -O1 referee.

### M32. The inliner refuses a four-instruction leaf seventeen times, and the obvious fix loses

**THE SITE.** `n1_btree_page` (EXEC 1.31, the third-largest contributor to the
suite's log mass) reads every page header through
`static unsigned get2(const unsigned char *p) { return (p[0] << 8) | p[1]; }`.
**gcc inlines it everywhere — zero calls. zcc emits seventeen `bl get2`.**

**WHAT IT COSTS,** measured by inlining it in the SOURCE (a macro), which
changes the input and not the compiler:

| | zcc | zcc + get2 inlined | gcc -O1 |
|---|---|---|---|
| time | 47,103 us | **45,063 us (0.9567)** | 35,909 us |
| vs gcc | 1.312 | **1.255** | — |
| instructions | 540 | **527** | 455 |

It gets SMALLER as well as faster, because the loads then fold into their
addressing modes.

**WHY IT MATTERS BEYOND THE 4%.** All ten callee-saved registers are live in
`main`, so the loop-invariant LCG constants cannot be hoisted and are rebuilt
every iteration — eight `movz`/`movk` in a forty-instruction block that is 54% of
the program's weighted cost. gcc hoists one of the two (`x27`). The pressure
comes from the calls: AAPCS64 §6.1.1 makes every value live across a `bl`
callee-saved. So the refused inline is upstream of the constant row, not beside
it.

**THE DIAGNOSIS, which stands.** `body_size` counts HIR NODES and `call_cost`
counts MACHINE INSTRUCTIONS, and the comparison `bs <= base` puts the two units
against each other. HIR is more verbose than A64 and the disagreement is
systematic AGAINST inlining. `get2` is six HIR nodes (two loads, two zero
extensions, a shift, an or) against a call sequence of four.

**AND THE OBVIOUS FIX LOSES.** Subtracting zero-extensions from `body_size` — a
real spec fact, DDI 0487 B1.2.1 plus the zero-extending narrow load forms — did
NOT admit `get2` (still seventeen calls, so its node count is high for another
reason) and DID admit other callees: **INSN geomean 1.0688 → 1.0739**, a
regression on the deterministic axis. Reverted.

**AND SO DOES THE SECOND OBVIOUS FIX** (2026-08-29). The diagnosis above says
the two sides of `bs <= base` are in different units, so the fix that suggests
itself is to put `body_size` in the unit `call_cost` already uses — machine
instructions — by lowering the callee and counting `|MIR|`. Measured with
`isel::lower_func` wired straight into `body_size`: `get2` scores **7 there too**,
not the 3 machine instructions it finally becomes. MIR before the MIR passes and
the allocator still carries the parameter copies, the `ret`, and an `add` that
has not yet folded into an addressing mode. Seventeen `bl get2` survive, the
program is 540 instructions either way, and it runs 47.9 ms against 47.8 ms.
**The unit half of the diagnosis is right and the cheap way to fix it is not:**
`get2` only becomes three instructions after the MIR passes and regalloc, so an
honest count means running most of the backend per callee, which costs more
compile time than the row buys. Two ways in now measure zero.

**WHAT THE ROW ACTUALLY NEEDS**, and it is why the cheap versions all fail
Article E: the honest cost of a call at a site is
`args + 3 + |values live across it|`, because that last term is what forces the
caller into callee-saved registers and is the whole of the pressure measured
above. HIR has no general liveness — `sroa.rs` carries a bespoke one for its own
pieces — so the term is not available, and every substitute for it is a
threshold somebody picked. `call_cost`'s own comment already makes the point in
the other direction: its bound is "derived from the ABI rather than picked".
Build the liveness or leave the row.

**WHEN / WHERE.** 2026-08-28, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, gcc -O1 referee, 49-program taxonomy suite.

### M33. The annotation is not a consumer — the instrument could not measure the compiler

**THE DEFECT WAS IN A SWITCH.** `ZCC_WEIGHTS` turned on `freq::annotate` AND
both of its consumers (`layout`'s block chaining, the spiller's eviction
ranking). Both consumers are measured losses — EXEC 1.0206 → 1.0873 when the
pair shipped — so the annotation was off by default, which left `MBlock.weight`
at 1 everywhere and `mir::cost::weighted` reading a constant.

That left the instrument with no valid reading available: weights of 1 from the
compiler under test, or real weights from a compiler that is **not** the one
under test. It read the 1s, and three separate findings in one session traced
back to it — `M29`'s ranking, `M30`'s worklist, and the `ZCC_HOIST` policy
question, which needs to know which loop is hot and cannot ask.

**THE FIX.** `ZCC_WEIGHTS` now means exactly "compute the annotation", and each
consumer opts in by its own name. Nothing reads `weight` unless a consumer is
named, so the annotation is codegen-neutral **by construction** — and that is
checkable rather than argued:

```
ZCC_WEIGHTS=1 is codegen-neutral on 49/49 programs, 0 differ
```

Byte-identical across the whole suite, which is Article E's refactor gate applied
to an instrument seam. The batteries get a thread-local `set_consumers` twin of
`set_weights`, because two tests running in parallel share the environment.

**WHAT IT DOES NOT FIX.** The estimate is still DEPTH-based, so it ranked
`m1_resp_parse`'s two doubly-nested setup loops at 58% of the program while the
single-nested parse loop beside them runs sixty times more often; two hand edits
aimed there measured 0.997 and 0.999. Making it accurate means feeding SCEV's
trip counts into `estimate` — `s.trips` already exists and the IVX gate already
reads it. That is the next row, and it is what would let `ZCC_HOIST` be gated on
"this loop is hot" instead of hoisting into cold paths, which is the whole of why
it loses at +17 instructions for 0.5%.

**AND SCEV CANNOT FIX IT — the row was built and reverted.** The obvious repair
is to use SCEV's proven trip count per header instead of `TRIPS = 10`, and it was
wired in: `estimate_with`, `LoopScev::analyze` per loop in `annotate`, still
byte-identical on 49/49. **The ranking did not move.** SCEV bounds a loop only
when its trip count is a compile-time constant, and the loops that matter never
are: `m1`'s setup loop runs `kl = 3 + k % 9` times and its parse loop runs `n`
times, where `n` is a parameter. Both fall back to `TRIPS`, and depth decides
again. Reverted — it costs a SCEV analysis per function and buys nothing
measured.

**SO BOTH ROWS THIS SESSION THAT ASKED "WHICH PATH IS HOT" TERMINATED AT THE SAME
ANSWER.** `M31` needed to know which switch arm a state machine spends its time
in, and no static rule separates them because it is a property of the INPUT.
`M33` needs to know which loop of two runs sixty times more often, and SCEV
cannot say because the bound is a runtime value. Neither is a missing analysis;
both are missing MEASUREMENTS of a real execution. That is the project's first
convergent argument for profile-guided optimization, and it is worth recording as
such rather than as two separate residuals: a counter per block, dumped at exit
and read back on a second compile, answers both — and would also make
`mir::cost::weighted` an instrument rather than an estimate.

**WHEN / WHERE.** 2026-08-28, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, 49-program taxonomy suite.

### M34. A dense forty-arm switch got no jump table, because ONE edge had arguments

**THE REFUSAL.** `isel::jump_table` gives an ARM that carries edge copies its own
trampoline block — the table entry points at the trampoline and the copies live
there — and the comment beside it explains why, naming `sqlite3VdbeExec`'s 196
opcodes as the case the refusal used to cost. The DEFAULT got no such
trampoline, and one line above refused the whole switch when its default edge
carried arguments.

`k1_dispatch` is that in miniature: **forty dense arms (0..39), span equal to the
arm count, well past `MIN_CASES = 24` — and no table.** Not the case count, not
the span: `ZCC_JT=2` still produced none. One edge out of forty-one had
arguments.

**THE FIX** is the mechanism already there, applied to the default. It appears in
two places and only one is a problem: the out-of-range `Bcc` is an ordinary MIR
edge and carries its copies as any edge does; the TABLE cannot, because an entry
is an address and both the range's holes and the `default` field are filled with
it. So the table gets a trampoline and the `Bcc` keeps the real edge.
`ZCC_NOJTDFLT=1` is the seam.

| `k1_dispatch` | compare chain | jump table |
|---|---|---|
| `cmp` against an immediate | 41 | **2** |
| instructions | 1,567 | 1,665 |
| time | 13,597 us (1.151 × gcc) | **13,057 us (1.105 × gcc)** |
| | | **0.9603** |

Interleaved, best of 24 alternating runs, output identical to both the old build
and gcc's.

**AND THE SUITE GEOMEAN DOES NOT MOVE, which is the honest headline.** Two
interleaved pairs split — off 1.0212 / 1.0189, on 1.0198 / 1.0208 — and INSN
regresses deterministically, 1.0688 → 1.0701, because a table is bigger than the
chain it replaces. One program of forty-nine cannot move a geomean, and the
+0.13% on the size axis is real. It ships on the reasoning this session has been
measuring toward: **the tail is the scoreboard**, `k1_dispatch` moves 1.151 →
1.105, and a dense forty-arm switch refused a table by an unrelated edge is a
STRUCTURAL defect rather than a tuning choice — the kind that is worth removing
at a known small size cost.

**IT DOES NOT REACH SQLITE, AND THE FIRST EXPLANATION OF THAT WAS WRONG.**
sqlite is byte-for-byte unchanged by the fix — five jump tables before and after,
170,963 instructions both ways — and this entry first recorded that as an unfound
blocker in `sqlite3VdbeExec`, whose 196-opcode dispatch `MEASURED M16` puts 85%
of sqlite's runtime gap in.

**It is not a blocker. `sqlite3VdbeExec` ALREADY GETS ITS TABLE:**

```
JT sqlite3VdbeExec arms=183 span=185 ACCEPTED
```

The wrong reading was self-inflicted and is worth recording as such: the
`ZCC_JTDBG` output was piped through `sort -rn | head -12`, which ranks by
FREQUENCY, and the one accepted 183-arm switch appears once while the small
refused ones repeat. The single most important line in the instrument's output
was cut off by the command reading it. **An instrument summarized by frequency
answers "what happens most", never "what matters most"** — the same shape of
error as `M26-correction`'s census, one layer up, in the shell rather than in the
classifier.

**WHAT THE INSTRUMENT DOES SAY** once read whole: the refusals that remain are
all SPAN, `span > arms × 2` —

| function | arms | span | ratio |
|---|---|---|---|
| `jsonTranslateTextToBlob` | 37 | 240 | 6.5 |
| `sqlite3ExprCodeTarget` | 51 | 163 | 3.2 |
| `strftimeFunc` | 27 | 83 | 3.1 |
| `yy_destructor` | 50 | 115 | 2.3 |

The standard answer is to split the range into dense clusters and give the
outliers a compare chain. None of these is on sqlite's RUNTIME path — they are
its parser, its expression coder and its JSON reader — so the row is a size and
compile-time one, not a speed one, and it is not opened on this evidence.

**WHEN / WHERE.** 2026-08-28, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, gcc -O1 referee.

### M35. The suite widened to ninety, and the number went UP by seven points

**WHY IT WAS WIDENED.** Every row shipped this session was measured on the
49-program suite, which is the definition of tuning to a benchmark, and Law 3c
names the classes that suite does not sample: heavy floating point, working sets
past cache, deep call graphs and indirect dispatch, varargs, bitfields. Forty-one
programs were written against exactly those names plus the shapes real C is made
of — strings, 64-bit arithmetic, allocation, unions, error ladders, sorting,
codecs, graphs.

**THE RESULT, and it is the honest kind:**

| | 49 programs | 90 programs |
|---|---|---|
| EXEC geomean | 1.020 | **1.0916** |
| INSN geomean | 1.0701 | **1.0892** |
| above 1.1× | 11 | **29** |
| worst | 1.44 | **5.05** |

**Seven points of EXEC appeared the moment the surface stopped being the one the
rows were tuned on.** That is not a regression — nothing got slower — it is the
measurement catching up with the compiler, and it is why Law 3c says the surface
is to be widened rather than defended.

**THE COMPILER DID NOT FALL OVER, which is the other half.** Forty-one programs
written to hit documented blind spots, none of them ever tuned for, and the
spread is 0.918 to 5.045 with a median of 1.038. **zcc BEATS gcc -O1 on eight of
them** — `y3_radix` 0.918, `q3_callback` 0.924, `y2_heap` 0.955, `r2_va_mixed`
0.966, `z4_matmul_int` 0.976, `o4_fp_fft` 0.987, `o6_fp_poly` 0.995,
`z1_crc32` 1.004 — and the memory-bound ones sit near or below parity because a
stalled machine does not care about an instruction. An overfitted compiler does
not do that.

**THE ONE THAT MATTERS.** `x1_goto_cleanup` — **5.045×**, by a factor of three
the worst thing this project has measured, and it is not an exotic kernel: it is
the `goto out;` cleanup ladder, C's only error-handling mechanism, the shape of
every kernel driver, every parser and every library entry point. Its CFG is a
fan-in — many early exits converging on one block, values live across all of it.
The 49-program suite had nothing of the kind. `z2_rle` (1.61) and
`u4_popcnt64` (INSN 2.88) are the next two.

**WHAT THIS RETIRES.** Any statement of zcc's speed taken before this entry. The
number to quote is 1.09 over ninety programs, and the tail is 29 of them.

**WHEN / WHERE.** 2026-08-29, `mir-rearch`, M1 Pro under Docker, aarch64-linux
musl release zcc, gcc -O1 referee, `tests/bench/exectime.sh`.

**BUILT, and it wins at the heuristic strength predicted.** `isel::order_switch_arms`
(`ZCC_NOARMORD=1` is the seam) partitions the arms STABLY, staying arms first.

The predicate took three cuts, and the two that failed are the interesting part:

1. *"the arm's edge to the header passes `v` itself"* — fired on NOTHING. Every
   arm of a state machine merges into the same join before the back edge, so the
   value the header receives is one value for all of them.
2. *"…or a value transitively fed by `v`"* — fired on EVERYTHING, for the same
   reason from the other side: that one join parameter has `v` among its inputs,
   so every arm reaching it qualifies.
3. *"this arm's own region carries `v` forward on some edge"* — discriminates.
   An arm that stays hands the OLD state along; one that transitions hands a
   fresh one.

| | before | after |
|---|---|---|
| `m2_http_parse` | 1.318 | **1.242** |
| `m1_resp_parse` | 1.44 | 1.42 |
| suite EXEC | 1.0204 | **1.0185** |
| suite INSN | 1.0688 | 1.0688 (a pure reordering) |

Interleaved A/B, two pairs in one box session: off 1.0216 / 1.0211, on 1.0215 /
1.0196 — on wins both, and the margin is small because the geomean divides one
program's 5.8% by 49. That ratio is the point rather than a caveat: **the
scoreboard to watch is how many programs LEAVE the tail**, not the geomean delta.
Gate: 15 PASS / 0 RED (provenance re-run after a comment-only citation fix).

**THE RESIDUAL, CHASED TO ITS END — and it stops at a profile.**

The first shipped predicate missed exactly the arms that matter, and the reason
is `ifconv`: `if (--want == 0) st = S_CR;` collapses to a `Select`, so on the
common path the old state is not an edge ARGUMENT but an OPERAND. Following `v`
through a `Select` as well fixes the identification —

```
m1  stay=[2,1]        → [0,2,1,3,4]   go=[5]
m2  stay=[0,1,4]      → [0,1,2,4,5,6,8]  go=[3,7]
```

— and buys **m2 1.242 → 1.229**, m1 unchanged. But it also shows the rule has run
out of discrimination: five of m1's six arms and seven of m2's nine now qualify,
so the partition barely reorders anything. **The problem was never identifying
the staying arms; it is RANKING them.**

Three candidate rankings, all measured against the shipped build (m2 at 1.229):

| ranking | ratio | m2 becomes |
|---|---|---|
| hot arm first (the answer, taken by hand) | 0.8198 | **1.008** |
| states on the header cycle `{4,5,6,7}` first | 0.8927 | 1.097 |
| shipped (staying arms first, source order) | 1.000 | 1.229 |

**The cycle rule measures well and is NOT ADOPTED, and the reason is the point.**
Its premise is that `S_HNAME → S_HCOLON → S_HVALUE → S_HEOL → S_HNAME` is a cycle
the other states are not on. The state graph says otherwise: `S_DONE → S_METHOD`
closes it, so all nine states are ONE strongly-connected component and no
structural rule separates them. Shortest-cycle length does not either — `S_VER ↔
S_EOL` is a 2-cycle and is cold. A rule that scores 0.8927 while resting on a
premise the program's own graph refutes is fitted to this benchmark, which
Article E's mandatory question ("the spec's number, or my convenience's?")
answers plainly.

**SO THE REMAINING 20% IS A PROFILE, and that is a finding rather than a
failure.** Which arm of a state machine is hot is a property of the INPUT — HTTP
header values are long, methods are short — and no analysis of the source can
know it. This is the first row in the project whose residual names
profile-guided optimization as the mechanism, and it is worth ~0.11 of the
suite's 0.97 total log-mass on one program.

The residual is unchanged and now measured on the shipped rule: the predicate
finds three of m1's staying arms and misses `S_BULK`, and finds three of m2's and
misses `S_HVALUE`, which is why m2 reaches 1.242 rather than the hand-edited
1.02. Ranking WITHIN the staying set is the Law-4 residual.

### M36. The worst program in the project was a FENCE, not a codegen defect

**M35 named `x1_goto_cleanup` at 5.045× and asked what codegen shape the `goto`
ladder needs. It needs none.** The program was not compiled badly; it was not
compiled at all past the frontend. `Block::labels` — the C labels landing on a
block — pinned every one of them, and five separate places refuse a pinned
block: `inline.rs::inlinable` (the whole callee), and cfg_simplify's threading,
merging and two known-condition identities (`cfg.rs` (c), (d), (e), (g)), plus
`ifconv.rs` and `layout.rs`. So a function containing one `goto` lost the
interprocedural row and most of the control-flow row at once.

**THE MEASUREMENT THAT SETTLED IT, before any patch** (Law 2 — locate
mechanically, classify after). The same program, same semantics, the ladder
rewritten as nested `if`s instead of `goto`, both compiled by the unmodified
compiler:

| | zcc µs | ratio vs gcc -O1 |
|---|---|---|
| `goto` ladder | 19,330 | **4.90×** |
| identical nested `if`s | 5,941 | **1.51×** |

Two thirds of the worst number this project has measured was one `bool` in a
refusal predicate. No `.s` was read to find it and no codegen row was needed.

**WHY THE FENCE WAS THERE, and why it is not needed.** `Block::labels`'s own
comment names its two readers exactly: C99 6.8.6.1 (a `goto` leaving a VLA's
scope deallocates back to the frame base at the label) and EXT(gcc) `&&label`,
whose address a static initializer or `goto *e` may hold. A function with no VLA,
no `SymAddr(Sym::Label)` and no label named in the data segment has labels that
no run can observe — they reach the emitter as text and nothing else. `cfg.rs`'s
`delabel` (SQUARE `labels_are_not_observable`) drops exactly those, first in the
module ladder, and the three refusals are its battery.

**MEASURED, interleaved pairs on one box session, 90-program suite:**

| | before | after |
|---|---|---|
| `x1_goto_cleanup` EXEC | 5.014 | **1.469** |
| `x1_goto_cleanup` INSN | 1.630 | **1.239** |
| suite EXEC geomean | 1.0871 | **1.0715** |
| suite INSN geomean | 1.0892 | **1.0854** |
| sqlite binary bytes | 1,151,312 | **1,139,744** |

`k1_dispatch` (INSN 1.185 → 1.123) and `k2_live_pressure` (1.330 → 1.255) come
along for the same reason. `x2_nested_break` pays 4.5% of INSN for threading its
one label and its EXEC does not move (1.055 → 1.056), which is the tail-
duplication trade M11 already records. sqlite's runtime is UNCHANGED — two
interleaved pairs read 1.1698/1.1395 before and 1.1337/1.1722 after, a ±0.02
spread that swallows the difference, and the single non-interleaved pair that
first read it as a 2.9% loss is the measurement lying exactly as Law 2 warns.
Compile time pays 3.7% for the inlining the row unfences.

**THE RESIDUAL.** `x1` is 1.47×, not 1.0 — the nested-`if` form measures the same
1.51, so what is left is a shape the frontend produces either way and is a real
codegen question, unlike the fence. Whether other refusal predicates in the
compiler are fences of the same kind is now the open question this row raises;
`has_vla` and `!b.params.is_empty()` are the two that guard the most sites.

---

### M37. A spec table was written, tested, and never wired to the compiler

**THE SITE.** `isa::fp_imm8` transcribes DDI 0487 C7 `VFPExpandImm` — the 8-bit
`fmov` immediate, sign · 2^e · (1 + m/16) — and `mir/tests.rs` checks it against
1.0, 0.5, −2.0, 31.0 and three refusals. `isel/imm.rs` wraps it as
`fp_is_imm8`, with a comment explaining exactly when to prefer it. **Nothing
called it.** Every floating constant in every program zcc has ever compiled went
through a general register: `movz` chain, then `fmov d, x` ACROSS the register
files, two instructions and a crossing where the ISA offers one instruction and
no crossing.

This is the Law-1 failure in its cleanest form. The table is on the spec side of
the decomposition, correct and cited; the algorithm side simply did not read it.
No measurement was needed to know it was wrong — only to know what it cost.

**WHAT IT COST, hand-edited first** (`o3_fp_mixed`, whose inner loop rebuilds
1.0f before an `fcmp` and 0.5f in the other arm, both on the dependence chain):

| | µs | vs gcc -O1 |
|---|---|---|
| gcc -O1 | 17,435 | — |
| zcc | 24,116 | 1.388 |
| zcc, two lines rewritten to `fmov #imm` | 20,053 | **1.151** |

**BUILT** (`MInst::FMovImm`, one instruction, verified against `fp_imm8` in
`mir::verify` so an unrepresentable constant cannot reach the emitter; shared by
`const_share` on the same key as `MovImm`):

| | before | after |
|---|---|---|
| suite INSN geomean (deterministic) | 1.0854 | **1.0794** |
| suite EXEC geomean | 1.0729 | 1.0705 (inside the noise band) |
| `o3_fp_mixed` | 1.377 | **1.199** |
| `f2_double_poly` | 1.000 | **1.070** |
| sqlite | byte-identical | byte-identical |

**THE RESIDUAL HAS A NAME, AND IT BELONGS TO THE SCHEDULER.**
`f2_double_poly` receives TEN FEWER INSTRUCTIONS (59 → 49) and runs 6% SLOWER,
4,977 µs → 5,275 µs, reproducible across interleaved pairs. Every instruction it
receives is correct and cheaper; what changed is which pipe they occupy. Horner
evaluation is a chain of dependent `fmul`/`fadd` — the FP pipe is the bottleneck
— and the old `movz` sat in the GENERAL pipe, executing for free beside it.
`fmov #imm` moves that work into the pipe that is already full. In `o3` the
opposite holds: the constant is ON the chain feeding `fcmp`, so removing the
crossing is a straight win.

**Law 3c, sharper than the usual statement.** It is not only that fewer
instructions can be slower; it is that an instruction's cost depends on WHICH
EXECUTION RESOURCE it competes for, and a count cannot see a resource. Keeping
isel at two instructions to hide this would be treating a scheduling problem in
the selector, which Article B forbids — the row is wired at the layer that owns
the ISA table, and `f2` is left as a named, reproducible test case for the
scheduler: a program where the better instruction sequence loses.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-musl,
gcc 14.2.0 -O1. Gate 15/0 at FUZZ_N=300 (356 s wall), cargo 208/0.

---

### M38. How much the ninety-program suite can actually be trusted, measured

**THE QUESTION Law 3c forces and nobody had answered with a number:** a suite is
always narrower than the language, so what is the ERROR BAR on the geomean it
prints? Four statistics over the 90-program table (EXEC 1.0688, INSN 1.0794):

| test | result | what it settles |
|---|---|---|
| drop-one (jackknife) | worst shift **0.009** EXEC / 0.012 INSN | no single program steers the number any more — on the 49-program suite `x1` alone was worth 0.018 |
| split-half, 2000 random 45/45 splits | median \|geo(A)−geo(B)\| **0.025**, p90 0.059 | **a suite of ~45–49 resolves to ±0.03 at best**, so the old suite's 1.020 was never trustworthy to a percent |
| bootstrap, 4000 resamples | 95% CI **[1.033, 1.105]**, width 0.072 | the honest statement of zcc's speed is **1.07 ± 0.04**, not 1.0688 |
| prefix convergence | a random subset of 10/20/30/45/60 misses the full geomean by 0.035/0.020/0.017/0.012/0.009 | ~60 programs is where the number settles; past that, only NEW SHAPES pay |

**THE DISTINCTION THAT KEEPS THIS FROM BEING MISREAD.** The ±0.04 is the error
of *"how much slower is zcc than gcc -O1 on C in general"* — it is about
generalizing to programs not in the suite. It is NOT the error of an A/B: when
the same 96 programs are compiled twice, each program is its own control, so a
0.006 move on the deterministic INSN axis is real. Two different questions, two
different error bars, and conflating them would either sink every row or
validate every row.

**corr(INSN, EXEC) = 0.196.** On ninety programs the two axes are very nearly
independent. The `geo40`-era reading — 1.75 against 1.77, "they track, NOT
decoupled" — was an artifact of a suite narrow enough for the two to coincide.
The consequence is structural: **ranking rows by static instruction count ranks
them on an axis that barely predicts time**, which is exactly the pattern this
session produced — `delabel` won both axes, `fmov #imm` won INSN with EXEC flat,
the licm header-load row won neither while being correct.

**WHAT WAS MISSING, and it was not more kernels.** Comparing the suite against
sqlite on shape rather than score:

| | suite (90) | sqlite |
|---|---|---|
| functions > 200 instructions | 3 | 154 |
| functions > 1000 instructions | 0 | 18 |
| instructions touching `[sp` | 710 | 21,331 |

**The register allocator — over half the measured size gap — had never been
sampled by a timed program.** In a function that small, thirty-one registers are
always enough and the allocator never decides anything. That is the whole of why
the suite reads 1.079 where sqlite reads 1.108.

**SIX PROGRAMS ADDED (90 → 96),** each for a shape with no sample, not for
volume: `aa1_spill_interp` (48-arm dispatch over sixteen locals — 452
instructions, spills), `aa2_wide_live` (forty live words through four mixing
rounds — 989 instructions, zcc spills 306 against gcc's 322 and is still 6%
slower), `ab1_setjmp` (C99 7.13.2.1 non-local exit), `ab2_format` (the snprintf
digit loop: division by a constant on a real hot path), `ab3_volatile_mmio`
(C99 6.7.3p6 — a fence no pass may cross, and the question of whether the code
AROUND it still gets optimized), `ac1_huffman` (a bitstream with two loop-carried
recurrences and variable shifts).

They score 0.883, 0.956, 1.067, 1.162, 1.402, 1.504 — **zcc wins two of the six**,
which is the sign the set was chosen by shape and not by where zcc looks good.
Suite 96: **EXEC 1.0720, INSN 1.0753**. `ac1_huffman` at 1.504 over 76 ms is now
the second-largest single contributor to the suite's log mass, and its INSN ratio
is 1.068 — another program where the count cannot see the cost.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-gnu,
gcc 14.2.0 -O1, glibc (both sides link the same libc; `-S` counts user code only).

---

### M39. The gates audited against themselves — four ways to be green while checking nothing

**WHY THIS AUDIT.** Law 0 ranks purity above every number, and this session had
spent itself on exec and size. The question it left unasked is the one that
matters most: **can any gate report PASS without having checked anything?** A
compiler whose verdicts are hollow is worse than one with no verdicts, because
the hollow ones are believed. Four holes were found, all four closed, and the
method that found them is worth more than the holes.

**FOUND 1 — `determinism.sh` never saw the speed suite.** It scanned
`tests/cases`, `tests/bench/*.c` and `refactor_gate/stress`, and NOT
`tests/bench/suite/`. So the 96 programs the scoreboard is taken from — the
largest and most varied C in the repository, and the only functions big enough
to make the allocator spill — had never been checked for emission determinism.
**91 programs → 187.** Still green, which is the good outcome: the property held,
it simply was not being asked.

**FOUND 2 — `regalloc::tests::same` passed silently on a left-hand trap.** The
match arm was `(Err(_), _) => {}`: if `⟦mir_v⟧` produced no answer, no equality
was ever compared and the allocator could have emitted anything. Instrumenting
the arm found **one** live case — `abi_boundary_truncation_leaves_no_instruction`
calls `h`, which the test never defined. The structural half of that test (no
`x16` cycle-break, no narrow self-move) was real; its `same()` call proved
nothing. `h` is now defined, and defined WITH A LOOP so the inliner cannot
dissolve the ABI boundary the test exists to measure. The arm is now a panic.

**FOUND 3 — the same arm in `isel::tests::equiv`, and here the comment defending
it was half right.** A trap the SOURCE earns — division by zero, a null
dereference, the step budget — is ⊥ in the semantics and the machine layer may
refine it, so the case still stands. `NoSuchFunction` is not that: it means the
program names a callee the test never wrote, so neither interpreter ran the code
in question. The two are now separated; zero cases were relying on it.

**FOUND 4 — `musl-box.sh` could pass having built NOTHING.** `make -k … || true`
swallows every error and the verdict counted only `.err` FILES, so a build that
produced nothing left an empty failure list and printed MUSL-BOX PASS. The one
application gate in the project could be green with no application compiled.
Fixed with the positive artifact count Article E asks for: **479 test binaries
linked from 464 sources, 73 err-files**, and a refusal at zero. `decay.sh` had
the same shape — two empty outputs also compare equal — and now refuses at zero
observations. Both verdicts now CARRY their evidence, because `fullsuite` prints
only the last line and a count that lives anywhere else is a count nobody reads.

**THE METHOD, and it is the transferable part.** Every one of these was found by
asking a gate to prove it had done work, never by reading it for correctness.
The mechanical form: instrument the branch that means "nothing was compared" and
count how often it is taken; require a POSITIVE artifact count, not the absence
of failures; and put the count in the verdict line.

**AND THE AUDIT'S OWN TOOL LIED TWICE, which is the lesson underneath.** `grep
SQUARE` over `src/` reported that regalloc — 5,573 LOC — had no commuting square
at all, and the conclusion "build translation validation for the allocator" was
one step away. It is false: `regalloc/verify.rs` states the obligation in its
header and `regalloc/tests.rs` discharges it by running BOTH interpreters. What
regalloc lacks is the WORD, not the proof. Minutes later `cargo test regalloc` —
a name filter — reported zero vacuous cases, and the full run found one. **The
instrument that audits the gates is itself un-audited**, and both errors ran in
the same direction: they made the compiler look worse and would have bought
weeks of rebuilding what exists. Presumption-of-guilt (Part A) applies to the
auditor too.

**WHEN / WHERE.** 2026-08-29, `main`. Gate 15/0 at FUZZ_N=300, 355 s wall;
cargo 208/0.

---

### M40. The inline-copy bound was set on the wrong axis, and a `bl memcpy` costs twelve instructions the static count cannot see

**VALUE.** `INLINE_COPY_MAX = 128` in `isel/lower.rs`, raised from the 32 that
`M14` derived. The measurement seam is `ZCC_ICM=<bytes>`.

**WHY M14's ANSWER WAS RIGHT FOR ITS QUESTION AND WRONG FOR THIS ONE.** M14 swept
the bound against **sqlite's static instruction count** and found a clean minimum
at 32, which it is: a call is four instructions whatever the length, while the
open-coded form grows with it, so on the SIZE axis the call wins early and keeps
winning. That axis cannot see what the call then *executes* — the branch, the
return, the argument setup, and libc's own entry — and `M38` had just measured
`corr(INSN, EXEC) = 0.196` over ninety programs, which is the general statement of
why a size-derived constant may not be believed on the clock. Law 3c names this
exact failure and Law 0 ranks `exec > size`.

**THE INSTRUMENT.** Wall-clock could not settle it: `v3_struct_copy` runs 1.4 ms,
where the ±20 % of a millisecond clock swamps the effect — the first sweep read
1.307 / 1.255 / 1.066 at bounds 96 / 128 / 192 whose emitted assembly is
BYTE-IDENTICAL (`md5` 7bc3c6 at all three). What settled it is **dynamic
instruction count** (`callgrind` Ir), which is deterministic, has no error bar at
all, and is a far better predictor of time than the static count it replaces.

**THE CROSSOVER, measured.** 200,000 struct assignments per point, always-call
against always-inline, same program:

| bytes | Ir per copy, called | Ir per copy, open-coded | saving |
|---|---|---|---|
| 16 | 33.6 | 15.6 | 18.0 |
| 32 | 33.6 | 17.6 | 16.0 |
| 64 | 33.6 | 21.6 | 12.0 |
| 128 | 39.6 | 29.6 | 10.0 |
| 256 | 57.6 | 45.6 | 12.0 |
| 512 | 89.6 | 77.6 | 12.0 |

**There is no crossover out to 512 bytes.** The open-coded form wins at every size
by an almost constant ~12 instructions, which is the call's own overhead: musl's
`memcpy` moves sixteen bytes per iteration exactly as the expansion does, so the
two agree on the payload and differ only by the call. The bound is therefore NOT
set by a crossover — it is set by what is worth spending static size on, which is
the axis M14 was measuring all along.

**WHERE 128 COMES FROM.** sqlite at six bounds, everything else identical:

| bound | sqlite instructions | `bl memcpy` sites |
|---|---|---|
| 0 (always call) | 174,050 | 321 |
| 32 | 173,963 | 258 |
| 64 | 173,994 | 247 |
| 96 | 174,041 | 238 |
| **128** | **174,094** | **233** |
| 256 | 174,094 | 233 |

Two facts pin the value. The size cost of 32 → 128 is **+131 instructions, 0.075 %**,
which Law 0 spends without argument for a win on the clock. And the curve is FLAT
from 128 to 256 — 233 sites at both — so **no compiler-generated copy in the whole
amalgamation exceeds 128 bytes**; the 233 that remain are `memcpy` the source
itself calls. That is the row's exhaustion proof (Law 3): the residual is entirely
category (a), a real boundary, with nothing left to realize.

**WHAT IT BOUGHT.** `v3_struct_copy` was calling `bl memcpy` FOUR times per
iteration for its 64- and 96-byte structs:

| | ICM=32 | ICM=128 |
|---|---|---|
| exec vs gcc -O1 (interleaved, min of 31) | 1.574× | **1.099×** |
| dynamic Ir vs gcc -O1 | 2.410 | **1.420** |
| static instructions | 137 | 141 |

Suite 96: EXEC 1.0784 → **1.0739**, INSN 1.0753 → 1.0757. **Exactly one program of
the ninety-six changes its assembly at all** (checked by `md5` over the whole
suite at both bounds), so the suite move is a floor, not the row's value — the
row pays wherever a 33-to-128-byte aggregate is assigned, which is struct-heavy
real code rather than a kernel suite.

**THE STANDING CORRECTION TO M14.** M14's number was not a mistake, it was an
answer to the size question; what was wrong was letting a size-derived constant
stand unexamined on the time axis. Every other constant swept the same way —
`MIN_CASES`, the inliner's bounds — is now a candidate for the same re-derivation,
and dynamic Ir is the instrument that makes it cheap.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-gnu,
gcc 14.2.0 -O1, musl in-box; `callgrind` from valgrind for every Ir figure.
Gate 15/0 at FUZZ_N=300; cargo 208/0.

---

### M41. Three shapes that were obviously right and measured at zero

**WHY THESE ARE TOGETHER.** Each is a hoist — pull work out of a loop that does
not need to repeat it — and each was read straight off the assembly beside gcc's,
which is the method that has won four rows this month. All three bought nothing,
and the reason they bought nothing is the same reason `M40` needed a new
instrument: **what a loop costs is not what a loop contains.**

**REFUTED 1 — hoisting a one-instruction constant out of `a2_udiv_mod`.** Its
loop rebuilds `#7` and `#13` every iteration:

    movz w10, #13 / udiv / msub / movz w11, #7 / udiv / add / add / add / cmp / b.ls

Two of eleven instructions are pure repetition, and gcc hoists both. Hand-edited
into dedicated registers in the preheader, exactly as gcc does:

| variant | us | vs gcc -O1 |
|---|---|---|
| gcc -O1 | 3943 | 1.000 |
| zcc | 4277 | 1.085 |
| **+ constants hoisted** | **4289** | **1.088** |
| + Granlund-Montgomery for `/7` and `%13` | 4085 | 1.036 |

**Removing 18 % of the loop's instructions cost 12 us — that is, nothing.** The
loop is bound by two `udiv`s, and everything issued in their shadow is free. The
same edit is what made the division rewrite look attractive; on the whole suite
that rewrite is worth about 0.1 %, over two programs, for a magic-number table
and a divisor-range proof, so it stays unbought.

**REFUTED 2 — the invariant load and the split latch in `z2_rle`.** Its run-count
loop reloads `in[i]` — loop-invariant, and gcc holds it in `w3` — and its decode
loop reloads `enc[i+1]` and spends a `mov` plus an unconditional branch on a latch
gcc merges. Both were hand-edited to gcc's exact shape:

| variant | us | vs gcc -O1 |
|---|---|---|
| zcc | 55963 | 1.587 |
| + encode-loop load hoisted, `i+255` precomputed | 55895 | 1.586 |
| + decode-loop load hoisted, latch merged | 55715 | **1.580** |

**0.4 % for two instructions per iteration off the hottest loop in the program.**
`z2_rle` is branch-bound: its run lengths are drawn from a hash precisely so the
loop exit is unpredictable, and a mispredict costs more than the whole body. Its
dynamic instruction ratio is 1.170 against an exec ratio of 1.626 — the program
says, in the only two numbers that can say it, that its problem is not
instructions. `licm` is not the row for it and neither is block layout.

**REFUTED 3 — filtering the constant hoist by chain length.** Given refutation 1,
the obvious guard for `const_share`'s loop hoist is to lift only constants whose
`movz/movk` chain is two instructions or longer: keep `v2_freelist`'s two 64-bit
literals, which are eight instructions of a twenty-five-instruction body, and stop
paying a register for a lone `movz`. Built, and measured on dynamic Ir:

| program | hoist off | hoist all | chain ≥ 2 | static insn, all / ≥2 |
|---|---|---|---|---|
| `v2_freelist` | 1.738 | **1.266** | 1.266 | +0 / +0 |
| `o2_fp_stencil` | 1.546 | **1.346** | 1.347 | +0 / +0 |
| `w2_tagged` | 1.125 | **1.063** | 1.125 | +0 / +0 |
| `k2_live_pressure` | 1.331 | **1.054** | 1.251 | +21 / +0 |
| `m3_dict_rehash` | 1.161 | 1.210 | 1.166 | +19 / +6 |
| `n6_pcache_lru` | 1.196 | 1.217 | 1.211 | +34 / +29 |

The guard does what it was built to do — it removes almost all of the static cost —
and it **gives back the two largest wins on the list**. `k2_live_pressure` goes
from 1.331 to 1.054 on one-instruction constants alone. So refutation 1 does not
generalize: a lone `movz` is worthless in a loop bound by division and worth 28 %
in a loop bound by nothing, and **chain length does not know which loop it is in.**
A guard that works has to read pressure, not the constant. Reverted; the pass is
back to hoisting every `MovImm` and `Adrp`.

**THE COMMON SHAPE, and it is the session's finding.** All three rows were chosen
by counting instructions in a loop body, and all three were settled by an
instrument that measures what the loop is actually waiting for — a divider, a
branch predictor, a register file. `M38` measured `corr(INSN, EXEC) = 0.196` and
called static count a poor proxy; these three say what to do about it, which is to
ask what the loop is BOUND by before counting anything in it.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-gnu, gcc
14.2.0 -O1; hand-edited `.s` assembled with gcc, every variant output-gated
against gcc's; Ir from `callgrind`.

---

### M42. The loop-constant hoist, priced on the suite — and see `M44`, which priced it on a real program and took it back out

> **STATUS: the row is OFF.** Everything below is correct and was enough to ship
> it for the length of one session. `M44` then measured it on the sqlite CLI and
> on compile time and reverted it. Read the two together: `M42` is what the suite
> can see, `M44` is what it cannot.

**VALUE.** `const_share::hoist_invariant_consts`, off since R5, turned ON here
and back OFF by `M44`. `ZCC_HOIST` turns it on.

**THE ARGUMENT IT HAD TO BEAT**, quoted from the code it lived in: EXEC about four
tenths of a percent, "inside the run-to-run spread", bought with 2.3% of
instructions that is not — and *THE ULTIMATUM asks for 1x on BOTH axes*, so the
row traded the axis zcc wins for the one it loses. That is a coherent argument
and it rested on two things that have since been measured.

**WHAT RETIRED IT.**

  * `M38` measured **`corr(INSN, EXEC) = 0.196`** over ninety programs. "1x on
    both axes" reads as a single goal only while the two axes are believed to
    track. They do not, so a static-instruction cost cannot veto an exec row on
    the grounds of being the same question asked twice — it is a different
    question, and Law 0 already ranks `exec > size` between them.
  * The old reading could not resolve four tenths of a percent, because the
    42-program suite could not: `M38`'s split-half says a suite of ~45 resolves
    to ±0.03 at best. "Inside the run-to-run spread" was true and was a statement
    about the instrument, not about the row.

**THE MEASUREMENT.** One frozen binary, both arms from the same build (the seam
is an environment variable precisely so no rebuild difference enters the
comparison), three interleaved pairs over the 96-program suite:

| pair | EXEC OFF | EXEC ON | median OFF | median ON | >1.1x OFF/ON |
|---|---|---|---|---|---|
| 1 | 1.0774 | 1.0618 | 1.059 | 1.037 | 34 / 34 |
| 2 | 1.0762 | 1.0731 | 1.063 | 1.044 | 36 / 31 |
| 3 | 1.0749 | 1.0696 | 1.060 | 1.043 | 37 / 31 |
| **mean** | **1.0762** | **1.0682** | **1.0607** | **1.0413** | |

EXEC geomean **−0.74%**, the sign the same in all three pairs. INSN 1.0757 →
1.0935.

**WHICH STATISTIC TO BELIEVE — and the first answer written here was WRONG, which
is the part worth keeping.** The ON arm's geomean spread came out at **0.0113**
against the OFF arm's **0.0025**, and this section originally explained it: the
row raises register pressure, a program that spills touches memory where it used
to touch registers, and memory is sensitive to cache state in a way an ALU chain
is not — "the row's own cost is what makes its measurement noisy." It is a
plausible mechanism and it was not the cause.

**THE CAUSE WAS A STUCK MEASUREMENT.** A `callgrind` job launched two hours
earlier had never exited: one core pegged at **99% for 93 minutes of CPU time**,
spanning every timing in this section, in `M44`'s bisection, and in the sqlite
pairs. Killed and re-measured on an idle machine, the SAME suite reads spreads of
**0.0003** and **0.0016** — eight times tighter. The pressure story explained a
number that belonged to a background process.

This is Law 2's exception — *the measurement lied* — and the charter allows
claiming it only after independent formulations converge. Two did: the container
was observed at 99% CPU directly (`docker top`, 01:33:45 of CPU), and the spread
collapsed by 8x the moment it was killed. **The reflex the charter warns about is
blaming the test; the failure here was the opposite and rarer one — inventing a
compiler-shaped mechanism for noise that was not the compiler's.** A plausible
mechanism is not evidence for itself, and the check that would have caught it
costs one command: look at what else is running before trusting a spread.

The MEDIAN reading stands: **1.0607 → 1.0413, −1.83%**, within-condition spread
0.004 to 0.007, and the count of programs above 1.1x falls from 37 to 31.

**ON THE DETERMINISTIC AXIS**, where there is no spread at all — dynamic
instructions from `callgrind`, against gcc -O1:

| program | hoist off | hoist on |
|---|---|---|
| `k2_live_pressure` | 1.331 | **1.054** |
| `v2_freelist` | 1.738 | **1.266** |
| `o2_fp_stencil` | 1.546 | **1.346** |
| `w2_tagged` | 1.125 | **1.063** |
| `m3_dict_rehash` | 1.161 | 1.210 |
| `n6_pcache_lru` | 1.196 | 1.217 |

`v2_freelist` rebuilds two 64-bit literals — eight `movz`/`movk` — inside a
twenty-five-instruction loop body, every iteration, and gcc hoists both.

**THE RESIDUAL (Law 3).** The cost is PRESSURE, and it is named rather than
hidden: a constant hoisted out of a loop is live across it, so a function one
value short of spilling now spills. Two programs of ninety-six go backwards,
both by that mechanism. Filtering by the constant's `movz`/`movk` chain length
was built to collect the cost back and was REFUTED (`M41`) — it removes almost
all the static cost and returns the two largest wins, because chain length does
not know what the loop is bound by. **A guard that works has to read pressure**,
and that is this row's open frontier. Category (b), not (a): the row is not
exhausted.

**WHAT IT UNLOCKS.** `M25` removed Granlund–Montgomery division-by-constant and
recorded the precondition for rebuilding it: *"the loop-invariant constant hoist
must pay for itself first, and the emitted sequence must reach gcc's five
instructions rather than nine"* — three of those nine were constant
materialization inside the loop. The first half of that precondition is now met.
`ab2_format` is where it would pay: its dynamic instruction ratio is 0.931 —
**zcc executes FEWER instructions than gcc** — against an exec ratio of 1.388, a
CPI ratio of 1.49, and its hot path is `u % 10` and `u /= 10` on a 64-bit value.

**THE QUESTION THE SUITE COULD NOT ANSWER, ANSWERED.** The +1.65% of instructions
is a real cost and the suite is structurally blind to what it buys or costs:
every one of its 96 kernels fits in L1i, so instruction FOOTPRINT is free there
and is a performance term in a 173k-instruction application with a
10.8k-instruction interpreter loop (`quickapp.sh`'s own header). If the row were
paying for suite time with application time, this is where it would show. Three
interleaved pairs of the sqlite CLI, 200,000 rows, bulk insert plus a nested
join, output-gated before any timing:

| pair | hoist ON | hoist OFF |
|---|---|---|
| 1 | 1.191 | 1.186 |
| 2 | 1.180 | 1.180 |
| 3 | 1.188 | 1.188 |
| mean | 1.1863 | 1.1847 |

**+0.13%, and two of the three pairs are identical to three decimals.** The row is
NEUTRAL on the application: it buys suite time and costs nothing where footprint
is priced. The feared i-cache trade did not appear at this resolution.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-gnu, gcc
14.2.0 -O1, musl in-box; suite of 96; `INLINE_COPY_MAX` already at 128 (`M40`) in
both arms. Shipped together with `M40` behind one gate: 15/0 at FUZZ_N=300
(determinism 187 programs, torture 1694/0, opt-parity 1552/0, csmith 254/0, musl
479 binaries), cargo 208/0.

---

### M43. `a % b` and `a / b` are one divide, and no pass above isel could have seen it

**VALUE.** `const_share` numbers `udiv`/`sdiv` as well as `MovImm`/`FMovImm`/`Adrp`.

**THE DEFECT.** A64 has no remainder instruction, so `isel/lower.rs` expands
`a % b` into a divide plus an `msub` (C99 6.5.5p6). A program that also writes
`a / b` — every integer formatter, every base conversion — therefore gets the
divide TWICE:

```
.Lmain_26:                          gcc .L3:
  udiv x25, x21, x1     u/10          umulh x0, x22, x8
  msub x25, x25, x1, x21  u%10        lsr   x0, x0, 3        u/10, twice
  add  w25, w25, #48                  add   x3, x0, x0, lsl 2
  add  w24, w23, #1                   sub   x3, x22, x3, lsl 1   u%10 FROM it
  strb w25, [x10, w23, sxtw]          ...
  udiv x21, x21, x1     u/10 AGAIN
  cbnz x21, .Lmain_84
```

**WHY NO EARLIER PASS COULD FIX IT.** `hir/pass/gvn` numbers every pure
expression against a dominating equal one, and it would have caught this
instantly — except the first divide has no HIR value. It comes into existence
BELOW isel, inside the rem expansion. This is the same gap `const_share` was
built for and says so in its own header: value numbering "applied to the two
instructions that have no HIR value to be numbered as". A divide minted by a rem
is a third. The fix is four lines of key, not a new pass.

**THE ONE THING THAT MADE IT FIRE, and without it the row is worth almost
nothing.** The first implementation shared 5 divides on sqlite and **zero on
`ab2_format`, the program it was built for.** The divisor of `u % 10` is a
literal, so isel mints a `MovImm` at each use; `const_share` merges those, but it
applies the merge to the instruction stream only AFTER the whole dominator walk.
During the walk the two divides still name two different vregs for the same ten,
and two different vregs are two different keys. Reading the operands through
`rename` during the walk is exact — `rename` is filled in dominator order, so a
divisor merged earlier is merged by the time its divide is reached — and it took
the row from 5 shared divides to 43:

| | before | after keys | after operands resolved |
|---|---|---|---|
| `ab2_format` `udiv` | 2 | 2 | **1** |
| suite divides | 71 | 69 | **68** |
| sqlite divides | 369 | 364 | **326** |
| sqlite instructions | 177,169 | 177,164 | **177,130** |

**WHAT IT BOUGHT** (interleaved, min of 21, against gcc -O1):

| program | before | after |
|---|---|---|
| `ab2_format` | 1.389 | **1.227** |
| `u2_div_var` | 1.096 | **0.825** |
| `a2_udiv_mod` | 1.094 | 1.092 |
| `a3_sdiv_mod` | 1.087 | 1.094 |

Suite 96: EXEC 1.0599 → **1.0574**, median 1.042 → **1.036**, INSN 1.0935 →
1.0929. **Both axes move the same way**, which is unusual enough to say why: this
row deletes an instruction rather than moving one, so there is no pressure trade
to pay for it — unlike `M42`, which buys time with size.

**THE HAND-EDIT THAT AUTHORIZED THE BUILD, and it is the session's cleanest
instance of Law 3c.** Before touching the compiler, the second `udiv` in
`ab2_format`'s `.s` was replaced by hand with a `mov` from the first quotient:

```
Ir  gcc = 292,253,651    zcc = 254,959,510    hand-edited = 254,959,510
    gcc -O1  17,704 us
    zcc      24,217 us   1.368x
    shared   21,875 us   1.236x
```

**The dynamic instruction count is IDENTICAL to the last instruction** — one
`udiv` became one `mov` — and the program is 9.7 % faster. Count could not have
found this row, and no count-based model can rank it. zcc already executed FEWER
instructions than gcc here (Ir 0.931) while running 1.388× slower.

**THE RESIDUAL (Law 3).** Sharing is cut at every `Call`, inherited from the
constant rows where the trade is a materialization against one of ten
callee-saved registers. A divide is not a constant: recomputing one costs a
multi-cycle operation, not one instruction, so the cut is likely too
conservative HERE and is category (b), not (a). It has not been measured
separately. Also unclosed: gcc replaces the divide by a constant entirely
(`umulh` + `lsr`), which `M25` built, measured and removed — `M42` has now met
the first of the two preconditions `M25` set for rebuilding it, and this row
removes half the divides that rewrite would have targeted.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-gnu,
gcc 14.2.0 -O1, musl in-box; 96/96 suite programs output-matched against gcc,
cargo 208/0.

---

### M44. The hoist was priced on the wrong program, and the price was 26% of compile time

**WHAT HAPPENED.** `M42` turned the loop-constant hoist on by default on the
strength of three interleaved pairs over the 96-program suite: EXEC geomean
−0.74%, median −1.83%, sign the same in all three. Every number in it is correct
and none of them was the right question, because **not one of them was taken on a
real program.** Reverted the same session. `M42` is not withdrawn — it is priced.

**THE TWO NUMBERS THAT SETTLED IT.**

| | hoist OFF | hoist ON |
|---|---|---|
| suite 96, EXEC geomean | 1.0762 | **1.0682** |
| sqlite CLI runtime, 3 interleaved pairs | 1.1847 | 1.1863 |
| **sqlite compile, `-S`, best of 3, idle machine** | **6.77 s** | **8.72 s** |

**+28.7% of compile time for a row that is neutral on the application.** The
bisection is unambiguous: the same binary with this seam left off returns 6.77 s,
matching the session's first commit to 0.2%, and `ZCC_NOSHARE=1` — which disables
the constant sharing but not the hoist — makes it worse still, so the cost is
this row and nothing else in the session.

**THE FIRST READING OF THIS ROW WAS TAKEN ON A BUSY MACHINE** — 7.95 s against
9.99 s, +26% — while the stuck `callgrind` job of `M42` held a core. Re-taken
idle, the absolute times fall by 15% and the RATIO grows slightly. Recorded
because it is the useful shape of the contamination: a shared background load
moves both arms of an interleaved pair together, so it inflates absolute times
and leaves a large ratio roughly intact. It is fatal to a 1% question and nearly
harmless to a 26% one — which is why `M42`'s suite pairs had to be thrown out and
this bisection did not.

**WHY LAW 0 DOES NOT SAVE IT.** `purity ≫ exec > size > compile speed` ranks exec
above compile time, so a real exec win could buy a compile regression. There is
no real exec win to spend: the win is confined to 96 kernels that fit in L1i, and
this session measured the suite at 1.057 against sqlite's 1.18 on the clock and
1.192 on dynamic instructions. **The suite does not predict the application**, so
a suite-only win is not an exec win, it is a suite number. Nothing sits on the
winning side of the trade to rank against compile speed.

**THE FIX THAT WAS BUILT AND MEASURED AT ZERO.** `hoist_invariant_consts` builds
`cfg`, `DomTree` and `LoopForest` per function, and `const_share::run` — called
immediately before it — has already built the first two. The obvious repair is to
skip functions with no back edge, since an amalgamation has 1,260 functions and
most are small: a depth-first grey/black cycle test, O(V+E) with one byte per
block, ahead of the whole analysis. Built, proven byte-identical on sqlite, and
measured at **10.36 s → 10.28 / 10.38 / 10.53 s — nothing.** An amalgamation's
functions mostly DO have loops, so the cost is the analysis itself and not
wasted calls. Removed under Article A(2): a mechanism measured at zero is not
kept for its elegance.

**WHAT WOULD MAKE THE ROW SHIPPABLE**, and it is now two conditions rather than
the one `M42` thought it had met: (1) an exec win that survives on a real
program, not only on the kernel suite, and (2) `DomTree`/`LoopForest` shared with
whatever pass built them first, rather than rebuilt — the same shape as the
scan-where-a-lookup-belongs family, a whole-function analysis paid per function
with no reuse.

**THE PROCESS DEFECT, which is worth more than the row.** Three rows were landed
this session on suite EXEC before any of them was checked against `realprog.sh` or
against compile time, and the suite was the wrong instrument for exactly the
reason this session itself established. **A row is not measured until it is
measured on a program someone would actually run.** `quickapp.sh` costs two
minutes and `realprog.sh` eleven phases; neither was run until the numbers were
already committed.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-gnu,
gcc 14.2.0 -O1, musl in-box; sqlite 3 amalgamation; cargo 208/0.

---

### M45. The two rows that survived the day buy 59 instructions of 798 million on sqlite

**THE NUMBER, and it is deterministic so there is nothing to argue about.** The
sqlite CLI, built by the session's first commit and by its last, run over the
standard workload under `callgrind`:

| build | dynamic instructions | vs gcc -O1 | binary bytes |
|---|---|---|---|
| gcc -O1 | 679,672,989 | 1.0000 | 1,243,888 |
| zcc, session start (`8cfda4e`) | 797,986,364 | 1.1741 | 1,139,752 |
| zcc, after `M40` + `M43` | 797,986,305 | **1.1741** | 1,139,752 |

**−59 instructions. −0.0000074%.** The binaries are the same size. Output
identical across all three.

**AND THE SUITE, on an idle machine, two interleaved pairs:**

| | EXEC | within-condition spread | INSN |
|---|---|---|---|
| session start | 1.0761 / 1.0758 | 0.0003 | 1.0753 |
| after `M40` + `M43` | **1.0678 / 1.0694** | 0.0016 | 1.0751 |

**ON THE CLOCK, three interleaved pairs each, idle machine:**

| | `realprog` TOTAL | `quickapp` |
|---|---|---|
| session start | 1.154 / 1.148 / 1.153 (mean 1.1517, spread 0.006) | 1.190 / 1.205 / 1.189 (mean 1.1947, spread 0.016) |
| after `M40` + `M43` | 1.161 / 1.160 / 1.168 (mean 1.1630, spread 0.008) | 1.167 / 1.222 / 1.202 (mean 1.1970, spread 0.055) |

`realprog` says the new build is **0.98% SLOWER**, the same sign in all three
pairs against a within-condition spread of 0.006–0.008. `quickapp` says +0.19%
with the sign not consistent and a spread of 0.055, which swallows any effect of
this size — it cannot arbitrate and should not be quoted as if it could.

**AND THAT SLOWDOWN CANNOT BE THE ROWS.** The two builds execute 797,986,364 and
797,986,305 instructions — the same work to within 59 — and produce byte-for-byte
the same binary SIZE. Identical work and identical size with a 1% clock
difference leaves one explanation: **placement**. The same bytes in a different
order meet the instruction cache and the branch predictor differently. Three
pairs at 3/3 agreement is p = 0.25 under a null of no effect, so the evidence is
weak as well as unattributable; what it is NOT is a measurement of these two rows
doing more work, because the deterministic instrument already answered that
question exactly.

**−0.74% on the suite, zero work change on the application.** Both numbers are
real and they are answers to different questions. `M40` fires where a 33-to-128-byte aggregate
is assigned in a hot loop, which `v3_struct_copy` does 160,000 times and sqlite
does approximately never; `M43` fires where `a / b` and `a % b` are written on
the same operands, which every integer formatter does and sqlite's inner loops do
not. Neither row is wrong. **Neither row is worth anything to a user of sqlite.**

**WHY THIS ENTRY EXISTS.** It would have been easy to close the day on
"EXEC 1.0784 → 1.0678, three rows banked" — every number in that sentence is
true. It is also the number for 96 kernels that fit in L1i, and this same session
measured that suite at 1.057 against the application's 1.174 on the same
compiler. A suite delta is not a project delta, and **a session that reports the
first as if it were the second is measuring its own activity, not the compiler.**

**WHAT THE DAY ACTUALLY PRODUCED**, stated so the ledger is not flattering:

* one row reverted after shipping (`M44`), for +26% of compile time;
* two rows that hold on the suite, change the application's executed instruction
  count by 59 in 798 million, and leave a ~1% clock difference that is placement
  rather than semantics;
* one instrument that is new and did the real work — **dynamic instruction count**
  — which found `M43` (9.7% faster at an IDENTICAL executed instruction count),
  refuted three obvious hoists (`M41`), and priced `M42` off the tree;
* the structure of sqlite's own gap, measured for the first time on the dynamic
  axis: **126M excess instructions, of which `sqlite3VdbeExec` alone is 69.7M
  (55%)**, and inside it reg-reg `mov` +1,084 and frame loads +969 of a +4,890
  static excess — the register allocator, which `M38` had already shown the suite
  cannot sample.

**THE STANDING RULE THIS BUYS.** `quickapp.sh` costs two minutes and the
deterministic Ir comparison above costs five. **A row is not landed until one of
them has run.** The suite says whether a row fires; only a real program says
whether it matters.

**WHEN / WHERE.** 2026-08-29, `main`, M1 Pro under Docker, aarch64-linux-gnu,
gcc 14.2.0 -O1, musl in-box, sqlite 3 amalgamation; every timing on an idle
machine after the stuck `callgrind` job of `M42` was killed.

---

### M46. A second core, and two of the biggest rows in the project were invisible on the first one

**WHY A SECOND CORE AT ALL.** Every performance number this project has ever
taken was measured on an Apple M1 Pro, under Docker, on a laptop running a UI.
`ARM64.md` §2 says so in its own standing caution — *a measured fact is evidence
about the measuring machine first* — and until 2026-08-29 nobody had tested it.
A Graviton4 (Neoverse V2) spot box in us-west-2 costs **$0.00157 per minute** and
runs zcc's exact target natively, no emulation and no container.

**THE CONTROL THAT MAKES THE COMPARISON WORTH ANYTHING.** Same binaries, same
Debian 13, same gcc 14.2.0 -O1 referee, same 96 programs:

| | M1 Pro | Graviton4 |
|---|---|---|
| **INSN geomean** | 1.0751 | **1.0751** |
| EXEC geomean | 1.0686 | **1.1716** (three runs: 1.1712 / 1.1718 / 1.1719) |
| programs above 1.1x | 34 | **49** |
| worst | `z2_rle` 1.61 | **`a2_udiv_mod` 5.52** |

**The instruction counts agree to four figures**, because they are the same
binaries. So the entire EXEC difference is microarchitecture, measured rather
than argued. And the quiet box is a better instrument than the laptop: the
three-run spread is **0.0007**.

**THE TWO ROWS, both at 4.5x, both hand-edited to parity, both invisible on the M1.**

| program | M1 Pro | Graviton4 | hand-edited on Graviton4 |
|---|---|---|---|
| `i1_global_acc` | **0.709** (zcc BEAT gcc) | 4.507 | **1.015** |
| `a2_udiv_mod` | 1.12 | 4.504 | **1.021** |

*`i1_global_acc`* is `gsum += gtab[i&255]` in a loop, `gsum` a global. gcc loads
it once into a register before the loop and stores once after; zcc emits `ldr` and
`str` on the accumulator EVERY iteration, putting a memory round trip on the
loop-carried dependence. The M1's store-to-load forwarding hides it so completely
that zcc was 30% FASTER there. Neoverse V2 charges for it: 4.5x. The fix needs no
alias analysis — `gsum` and `gtab` are distinct declared objects and C says they
cannot alias — and it is Law 3c's opening rule applied to memory: *never leave a
multi-cycle operation in front of a loop-carried value.* Hand-edited (load hoisted
to the preheader, store sunk past the latch): **4.507 → 1.015**.

*`a2_udiv_mod`* is division by a constant. `M25` built Granlund–Montgomery,
proved it over every divisor 3..2000, measured it, and REMOVED it, on this
reasoning: *"AND THE DIVIDER ON THIS CORE IS NOT SLOW, which is the fact the
textbook assumes and this machine denies… The folklore is a Cortex-A53-era fact;
it is not a fact about this core."* Every word of that is true about the M1 Pro.
`latency.sh` on Neoverse V2 measures `udiv` at **4.98 dependent adds**, and the
same hand-edit that bought 4.5% on the M1 buys **4.504 → 1.021** here.

**THE LATENCY TABLE, second column** (`ARM64.md` §2 now carries it): `mul` is
2.00 on V2 against "about an `add`" on the M1; `udiv`/`sdiv` 4.98 against an
inferred ≈2; `add …, sxtw` is 2.00 on both. `nop_control` reads 0.11, so the
harness is not measuring itself.

**WHAT THIS CHANGES, and it is not a small correction.** Roughly 3.9% of the
Graviton suite geomean sits in three programs — `i1_global_acc`, `a2_udiv_mod`,
`a3_sdiv_mod` — against the 0.74% that a full day of grinding on the M1 produced.
**Neither row was findable on the M1 at all**: one of them showed zcc WINNING.

**THE RULE.** A row may not be deleted on one core's evidence, and a suite
geomean may not be quoted without naming the core (Law 3c already said the second
half). The M1 Pro is a laptop; the notional target is generic AArch64-Linux, of
which Neoverse is far more representative. Where the two disagree, the server
core decides — and where a row is cheap on both, it ships regardless.

**COST AND OPERATIONS.** $0.00157/min all-in (spot $0.0911/hr + gp3 30 GB
$0.0033/hr; the instance-store NVMe is free and dies with the box). A one-time
spot box was reclaimed by AWS after ~15 minutes mid-session, which for a
benchmark is not half a measurement but none — remote work goes in ONE batch that
prints as it finishes. Details are commented in `tests/tf/variables.tf`, not in a
document of their own.

**WHEN / WHERE.** 2026-08-29, `main` at `rc9`, c8gd.2xlarge spot in us-west-2,
Debian 13 trixie arm64, gcc 14.2.0 -O1, zcc built native (8.5 s, no external
crates). Hand-edited `.s` assembled with gcc; every variant output-gated against
gcc before timing; best of 21.

---

### M47. Granlund–Montgomery, rebuilt — the row `M25` deleted was deleted on one core's evidence

**VALUE.** `hir/pass/divmagic.rs`. Division and remainder by a constant become a
multiply. `ZCC_NODIVMAGIC` turns it off. **Suite 96 on Graviton4: EXEC 1.1716 →
1.0902**, three runs at 1.0898 / 1.0899 / 1.0909.

**−6.95% from one pass**, against the 0.74% a full day of grinding on the M1 Pro
produced.

| program | before | after |
|---|---|---|
| `a2_udiv_mod` | 5.521 | **1.192** |
| `a3_sdiv_mod` | 2.103 | **0.512** |
| `ab2_format` | 1.410 | **0.882** |
| `u2_div_var` | 1.489 | 1.014 |
| INSN geomean | 1.0751 | 1.1091 |

Two programs end up FASTER than gcc -O1. `a3_sdiv_mod` at 0.512 is not claimed as
a win until it is understood — gcc spends 9,523 us there against zcc's 4,874, and
the reason is an open question, not a result.

**WHAT `M25` GOT RIGHT AND WHY IT STILL HAD TO BE REVERSED.** `M25` built this
row, proved it over every divisor 3..2000, measured it, and removed it on one
sentence: *"AND THE DIVIDER ON THIS CORE IS NOT SLOW… the folklore is a
Cortex-A53-era fact; it is not a fact about this core."* Every word is true of an
Apple M1 Pro. `latency.sh` on Neoverse V2 measures `udiv` at **4.98 dependent
adds** (`M46`), and the machine that runs zcc's declared target is Neoverse, not
Apple silicon. **The theorem never changed; the machine did.**

**AND M25's SECOND PRECONDITION TURNED OUT NOT TO BE ONE.** It required the
emitted sequence to reach gcc's five instructions rather than nine, three of the
nine being constant materialization inside the loop — making the row a dependent
of the loop hoist that `M44` has since reverted. Measured on Neoverse V2 with the
magic constant rebuilt EVERY ITERATION, which is exactly what this pass emits with
no hoist: **4.455 → 1.111**. Hoisted it would be 1.011. A `udiv` is expensive
enough that four extra one-cycle instructions do not begin to pay for it, so the
row ships independent of `const_share`.

**TWO PROOFS, AND THEY ANSWER DIFFERENT QUESTIONS.** This distinction is the
transferable part.

* The **batteries** check the CONSTANTS: `(n·M >> W) >> s` against real division
  over a dense divisor range at the boundaries of the numerator — 40,000+ cases
  for 32-bit unsigned and again for signed, plus 64-bit samples. That is a
  statement about Granlund–Montgomery, not about this pass.
* The **square** checks the PASS: `⟦f⟧ = ⟦divmagic f⟧` through the reference
  interpreter. It is what catches a correct multiplier wired to the wrong operand,
  and the signed case deliberately uses `n = −100, d = −7` because that is the one
  input that forces both corrections to show — the multiplier's sign disagreeing
  with the divisor's, and the arithmetic shift flooring where C99 6.5.5p6
  truncates toward zero.

**THE BUG THE BATTERY CAUGHT, and it would have shipped a wrong compiler.**
Hacker's Delight computes `q2` in W-bit arithmetic, so `q2 + 1` WRAPS there.
Carried in `u128` it does not, and `d = 7` came out as the 33-bit `0x1_2492_4925`;
the first battery run failed on `7 / 7 = 536870913`. The missing `2^W` term is
precisely what the `add` correction restores — which is why `add` is set on
exactly the divisors whose multiplier overflows. Masking to W bits reproduces the
wrap and yields `0x2492_4925`, which is gcc's constant.

**THE PROCESS DEFECT, which cost a gate run.** The pass was written after the
session's last `provenance.sh` and the gate was requested without re-running it.
`provenance` came back RED with two findings — `pub fn run` cited no THEORY
section and named no SQUARE — while all fourteen other stages were green,
including 554 random differential programs through a rewritten division. Nothing
was incorrect; the PROOF was missing, which is what Law 0 ranks above every
number with `≫` rather than `>`.

**WHEN / WHERE.** 2026-08-29, `main`, gate **15 PASS / 0 RED at FUZZ_N=300 run
NATIVELY on the c8gd.4xlarge Graviton4 box** — the first time zcc's gate has run
on its own target ISA without a container — torture 1694/0, opt-parity 1552/0,
csmith 254/0, yarpgen 300/0, musl 479 binaries, determinism 187 programs; cargo
215/0; provenance 29 passes, 50 citations.

---

### M48. The accumulator a loop keeps in memory, and the edge `mem.rs` cannot cross

**VALUE.** `hir/pass/loopmem.rs`. A memory cell a loop reads and writes every
iteration is forwarded across the back edge into a header parameter, so the LOAD
leaves the loop. `ZCC_NOLOOPMEM` turns it off. **Suite 96 on Graviton4: EXEC
1.0902 → 1.0598** against gcc -O1, on top of `M47`.

**THE SHAPE, and it is the commonest accumulator in C:**

    void accumulate(int n){ for (int i=0;i<n;i++) gsum += gtab[i&255]; }

`mem.rs` owns store→load forwarding and cannot make this one. Its reasoning is
block-local plus a single-predecessor edge — the scope where its oracle is exact
without a memory SSA, and its header says so — while the store at the end of
iteration k feeds the load at the start of iteration k+1. The forward crosses the
BACK edge. So zcc emitted `ldr` and `str` on `gsum` every iteration, putting a
round trip through memory on the loop-carried dependence, where gcc holds the
value in a register for the whole loop.

**WHY IT SURVIVED A YEAR.** On an Apple M1 Pro store-to-load forwarding hides the
entire cost: `i1_global_acc` measured **0.709 — zcc FASTER than gcc -O1**. The
program was in the suite, at the top of the winners list, and there was nothing
to investigate. On Neoverse V2 the same binary reads **4.51** (`M46`).

**THE SMALLER HALF OF THE TRANSFORM, DELIBERATELY.** The pass forwards the store
and LEAVES THE STORE IN PLACE. Memory is therefore written exactly as before at
every point, so no loop exit needs a fix-up and no path can observe a difference —
the store-sinking half needs the value on every exit edge and is a different
proof. What the loop loses is the load, which is the half that sits on the
dependence chain:

    .Laccumulate_2:                     .Laccumulate_2:
      and   w11, w10, #255                and   w12, w10, #255
      ldrsw x11, [x9, w11, sxtw #2]       ldrsw x12, [x9, w12, sxtw #2]
      ldr   x12, [x8]         <- gone     add   x11, x11, x12
      add   x11, x12, x11                 add   w10, w10, #1
      add   w10, w10, #1                  str   x11, [x8]
      str   x11, [x8]                     cmp   w10, w0
      cmp   w10, w0                       b.lt  .Laccumulate_2
      b.lt  .Laccumulate_2

**THE CONDITION THAT MADE IT FIRE, and the pass was worth nothing without it.**
The first build refused `i1_global_acc` outright, reporting "another access may
alias". `mem.rs`'s oracle reads an address only where a `SymAddr` or `SlotAddr`
defines it DIRECTLY; `gtab[i&255]` is `SymAddr(gtab)` plus an offset, so it came
back as a `Ptr` — and `Ptr` against `Sym` is "may alias". **A different global
might be the same global.** Walking the base symbol through address arithmetic
fixes it and is sound for the reason `Loc::Sym` already exists: it means the WHOLE
object, offset untracked, and `&g + k` is an access to `g` or it is undefined
(C99 6.5.6p8). Two different symbols stay disjoint, which is the only question
this pass asks. A sum with a symbol on BOTH sides stays a `Ptr` — `&a − &b` is not
an address into either.

**THE PROOF.** Four side conditions, each of which the square would catch:
one reader and one writer in that order, in a block that dominates every latch;
nothing else in the loop may alias, and no call, `alloca`, `memcpy` or volatile
access; the address is defined outside the loop; and the location is a linker
symbol, so the preheader load cannot fault even where the body never runs — which
is what removes the `entered` obligation `licm` carries for its own hoists.

**WHEN / WHERE.** 2026-08-29, `main`, c8gd.4xlarge Graviton4 spot in us-west-2,
Debian 13 arm64, gcc 14.2.0 -O1, zcc native; 96/96 suite programs output-matched.

---

### M49. The referee moves to `gcc -O2`, and the number nearly triples

**VALUE.** `exectime.sh`, `quickapp.sh`, `realprog.sh` and `perfn.sh` default to
`GCC_OPT=-O2`. `GCC_OPT=-O1` restores the old column. Every ratio in this file
dated before 2026-08-29 is against `-O1`.

**WHY THE OLD CHOICE WAS DEFENSIBLE AND STILL HAD TO GO.** `-O1` is the fair
comparison for a compiler with no loop, vector or unrolling passes: it answers
*how good is this codegen* without charging zcc for transformations it never
claimed. That is a question about the COMPILER. It is not the question a user
asks, because no one builds software at `-O1` — distributions, `./configure`,
`Makefile` defaults and the Linux kernel all stop at `-O2`. A number published
without its level is read as `-O2` by everyone who reads it, so scoring against
`-O1` and saying "gcc" is a claim about a build nobody performs.

**THE COST OF TELLING THE TRUTH**, 96 programs, Graviton4, two runs each:

| referee | EXEC geomean | zcc's speed | INSN geomean | programs > 1.1× |
|---|---|---|---|---|
| `gcc -O1` | 1.060 | 94% | 1.109 | 41 of 96 |
| **`gcc -O2`** | **1.3148 / 1.3148** | **76%** | **0.9922** | 54 of 94 |

and on the sqlite CLI, **1.576× — 63.5% of gcc -O2's speed**.

**TWO THINGS THE NEW COLUMN SAYS THAT THE OLD ONE COULD NOT.**

*The code is SMALLER.* INSN geomean **0.9922** against `-O2`: zcc emits fewer
static instructions than gcc does at the level everyone ships, because `-O2`
unrolls and vectorizes. The sqlite binary is 1,139,744 bytes against 1,359,104 —
**0.84×**. "Smaller code, 24% slower" is a real and defensible trade, and it was
invisible while the referee was `-O1` (where zcc is 1.109 on the same axis).

*The gap is not where a year of grinding assumed it was.* The worst program is
`g1_memcpy_loop` at **30.8×** — gcc replaces the loop outright — and TWO programs
leave the geomean entirely because `-O2` deletes their loop. Excluding the 30.8×
outlier the geomean is still ~1.27. **The distance to `-O2` is dominated by two
transformations zcc does not have at all — vectorization and loop deletion — not
by per-instruction codegen.** No peephole reaches a loop that was deleted, so the
row-by-row grind that `-O1` rewards is the wrong instrument for this column.

**WHAT THIS OBLIGES.** Every public number carries its level and its core, or it
is not published (Law 3c already required the core; this adds the level). And the
next campaign is chosen against `-O2`'s tail, not `-O1`'s geomean.

**WHEN / WHERE.** 2026-08-29, `main`, c8gd.4xlarge Graviton4 spot in us-west-2,
Debian 13 arm64, gcc 14.2.0, zcc native, 96-program suite and the sqlite CLI.

---

### M50. A fourth decision of the same vintage, re-measured — and this one holds

**WHY IT WAS ASKED.** `M25`, `M14` and `M42` were all set on an Apple M1 Pro
against `gcc -O1`, and all three were reversed on 2026-08-29 once the core and the
referee moved. That is a reason to re-ask every decision of that vintage, not a
reason to assume they all fall. The inliner's `!hl(gi)` fence — *an external
callee that CONTAINS a loop is not spliced into a call site inside a loop* — is
the next one down the list, and it stands in front of `g1_memcpy_loop`, the worst
program in the suite at 30.8× against `-O2`.

**THE MEASUREMENT**, 96 programs, Graviton4, `gcc -O2`:

| | EXEC | INSN | programs > 1.1× |
|---|---|---|---|
| fence ON (shipped) | **1.3160** | **0.9922** | 56 |
| fence OFF | 1.3199 | 1.0226 | 53 |

and `h2_revbits`, the program the fence was written for, goes **2.169 → 2.886**.

**THE FENCE HOLDS, AND FOR ITS ORIGINAL REASON.** Its comment says splicing one
loop into another changes what the allocator must hold across the inner one. That
is a property of a register file, not of a particular microarchitecture, which is
why it crosses where a divider's latency does not. Dropping it costs 3% of code
size and buys negative time.

**WHAT SEPARATES THE THREE THAT FELL FROM THE ONE THAT DID NOT.** `M25` reasoned
from a latency it had measured on one core and generalized ("the folklore is a
Cortex-A53-era fact"); `M14` swept the right curve on the wrong axis; `M42` read a
suite that could not resolve the effect it claimed. This fence measured the
program it was about, stated the mechanism, and the mechanism is
core-independent. **A measurement that names its mechanism travels; one that
names only its number does not.**

**AND THE ROW IT GUARDS IS UNAFFECTED.** `g1_memcpy_loop` stays at 30.6× with the
fence dropped, because inlining `mycopy` yields a byte-copy loop and a byte-copy
loop is what zcc already emits. gcc's 30× is the SECOND step — recognizing the
idiom and calling `memcpy`, which it can only do after inlining proves the two
pointers are distinct globals. The row is loop-idiom recognition with an overlap
proof, and it is not blocked by this fence.

**WHEN / WHERE.** 2026-08-29, `main`, c8gd.4xlarge Graviton4 spot in us-west-2,
Debian 13 arm64, gcc 14.2.0 -O2, zcc native, 96-program suite.

---

### M51. The worst program in the suite measures a shape no real program has

**THE ROW THAT WAS NOT BUILT.** `g1_memcpy_loop` is 30.6× against `gcc -O2`, the
largest single gap in the suite and worth about 3.6% of the geomean on its own.
The cause is exact and the fix is a named transformation: gcc inlines

    void mycopy(char *d, const char *s, int n){ for(int i=0;i<n;i++) d[i]=s[i]; }

into `main`, where `d` and `s` are two distinct globals, proves they cannot
overlap, and calls `memcpy` — sixteen bytes an instruction against zcc's one.
Nothing about the loop itself is worse: gcc's own out-of-line `mycopy` is the
same five-instruction byte loop zcc emits.

**WHAT WAS BUILT INSTEAD, AND WHY.** A DETECTOR — read-only, no transform —
because the rewrite is not a peephole. The loop is DEFINED when the regions
overlap and `memcpy` is not; `memmove` does not match either, since a forward
loop with `d > s` reads bytes it has already written and propagates them while
`memmove` behaves as if the source were copied to a temporary. So the row needs
either a proof of disjointness or a runtime guard and a second path — CFG
surgery, several hundred lines. `M41` is three shapes that were obviously right
and bought nothing; an afternoon spent detecting is cheaper than a day spent
building.

**WHAT THE CORPUS SAYS:**

| corpus | candidate loops |
|---|---|
| 96-program suite | **3** — `mycopy` (`disjoint=false`, `const_trips=false`), and two in `main` (`disjoint=true`, `const_trips=true`) |
| sqlite amalgamation | **0** |
| musl, 400 source files | **0** |
| lua, 34 source files | **0** |

**The cheap version of the row — fire only where disjointness is PROVABLE — would
have hit exactly the two `main` loops, which are initialisations that run once.**
It would have measured zero, and it would have taken a day to find that out. The
only site worth anything is the one that needs the guard.

**AND THE REASON IS OBVIOUS IN HINDSIGHT, WHICH IS WHY IT IS WORTH WRITING DOWN.**
Real C does not write copy loops; it calls `memcpy`. musl, lua and sqlite contain
zero of them across 435 files. `g1_memcpy_loop` exists because the suite is built
by SHAPE, and a shape chosen for coverage is not evidence that the shape occurs.

**WHAT THIS SAYS ABOUT THE SUITE, and it is not an excuse for the number.** The
largest single contributor to the `-O2` geomean is a program whose transformation
no program in the corpus needs. That is a fact about the suite's
representativeness, not a licence to discount the ratio — `M38` already measured
that the suite under-predicts sqlite. The correct response is to note it beside
the number and to pick the next row from shapes the corpus actually contains,
not to drop `g1` because it is inconvenient.

**WHEN / WHERE.** 2026-08-29, `main`, detector run over `tests/bench/suite`, the
sqlite 3 amalgamation, 400 musl 1.2.5 sources and 34 lua 5.4.7 sources; the
detector is removed under Article A(2) now that it has answered.

---

### M52. Hardware counters, at last — and the first thing they did was correct me

**WHAT BECAME POSSIBLE.** Every performance question this project has ever asked
was answered with a wall clock or a static count, because Docker on macOS exposes
no PMU: `MECHANISM.md` and the session notes both carry the line *"neither
reachable without hardware counters"*. On the Graviton4 box `perf` is native and
real. `kernel.perf_event_paranoid` ships at 3 and has to be lowered to 1.

**THE PROGRAM.** `m2_http_parse` is an nginx-shaped request parser — a switch over
a state inside a byte loop — and it reads **2.14× against `gcc -O2`**. Diffing the
assembly showed something specific: gcc emits **fifteen `ldrb` sites, each after a
different label**, where zcc emits **one** and fifteen unconditional branches back
to it. gcc has TAIL-DUPLICATED the dispatch into the end of every state, which is
exactly why a computed-goto interpreter beats a `switch` one.

**THE HYPOTHESIS THAT FOLLOWED, and it was wrong.** Tail duplication is textbook
branch-prediction work: each dispatch site gets its own predictor entry instead of
sharing one. So the gap should be mispredicts. `perf stat`:

| | instructions | cycles | IPC | branches | branch-misses |
|---|---|---|---|---|---|
| gcc -O2 | 472,918,822 | 86,742,883 | 5.45 | 102,460,432 | 33,033 (0.03%) |
| zcc | 922,116,374 | 185,143,293 | 4.98 | 285,787,792 | 701,101 (0.25%) |

**zcc executes 1.95× the instructions and 2.79× the branches.** The mispredicts are
21× worse in ratio and about **7% of cycles** in absolute terms — real, and not the
main term. IPC is 5.45 against 4.98, so neither side is stalling.

**SO THE TRANSFORMATION IS RIGHT AND THE MECHANISM WAS NOT.** Tail duplication
wins here by DELETING INSTRUCTIONS, not by fixing prediction: each of gcc's arms
branches straight to the next state's code, so the whole dispatch sequence —
`adrp`, `add`, `ldrsw`, `add`, `br` — is never executed. zcc pays it every byte.
Had this been settled by reading assembly alone, the row would have been built
against the wrong cost model and its profit predicted from the wrong number.

**THE INSTRUMENT LADDER, now complete enough to state.** Static count answers
*what did the compiler emit*; dynamic count (`callgrind`) answers *what did it
execute*; `perf` answers *what did the machine charge for*. `M38` measured that
the first predicts the third at corr 0.196. This entry is the first time the third
was available at all, and it separated two hypotheses that the first two could not
tell apart.

**WHEN / WHERE.** 2026-08-29, `main`, c8gd.4xlarge Graviton4 spot in us-west-2,
`perf` 6.12.105, `kernel.perf_event_paranoid=1`, gcc 14.2.0 -O2, zcc native,
output-gated before counting.

---

### M53. The dispatch, copied into the arm it came from — and the ceiling that stopped it halfway

**VALUE.** `hir/pass/tailjump.rs`, with `MAX_BLOCK = 12` and `MAX_GROWTH = 800`.
`ZCC_NOTAILJUMP` turns it off. Suite 96 against `gcc -O2` on Graviton4: **EXEC
1.3177 → 1.3098**, INSN 0.9921 → 1.0073.

**THE CENSUS THAT AUTHORIZED THE BUILD**, run before a line of transform was
written, because `M51` had just cost an afternoon proving that the previous
obviously-right row fired on nothing. A loop whose header ends in a `Switch`:

| corpus | dispatch loops |
|---|---|
| sqlite amalgamation | **7** — including `sqlite3VdbeExec`, 184 arms, header **three** instructions |
| lua 5.4.7 | **84** |
| 96-program suite | 29 |
| musl, 200 sources | 0 |

**Both constants come from that table.** `MAX_BLOCK = 12` admits every dispatch it
found — `sqlite3VdbeExec`'s is three instructions, `m2_http_parse`'s is two — and
refuses the loop bodies that merely end in a switch. `MAX_GROWTH = 800` admits the
largest real case, 184 arms times a three-instruction header = 552, and stops a
pathological switch from turning a function into its own jump table. Neither is a
taste: the corpus set both.

**AND MUSL HAVING ZERO IS THE POINT.** This is an interpreter and parser shape.
The corpora that contain interpreters contain it in quantity; a libc does not have
one. That is what `M51` asks of every row before it is built.

**WHAT THE ROW ACTUALLY DOES.** It copies a block into each predecessor that
reaches it by an unconditional jump, so an arm's tail carries its own copy of the
loop's latch and dispatch instead of branching back to a shared one. Duplication,
not motion: the copy runs exactly when that edge is taken, on the same values, and
no instruction is added to or removed from any PATH.

**THE CEILING, and it is the honest result of the day.** The pass stops at the one
block worth duplicating. `m2_http_parse`'s dispatch loads the byte its arms read,
and that byte is used PAST the arm's first block — so a copy would need a phi, and
the pass refuses rather than emit a wrong argument list. `m2` therefore moves only
2.094 → 2.075 where `M52`'s counters say 1.95× the instructions are on the table.
Two ways past it:

  * **SSA reconstruction over the dominance frontier** — the general answer, and
    SHARED infrastructure: `unroll.rs` names the identical gap in its own comment.
  * **Remat, then duplicate** — copy the load down into each arm that uses it,
    which removes the live-out and is free on the clock, since every path still
    executes exactly one load and only the static count grows. Built and REVERTED
    the same hour: the first cut deleted the load from the dispatch without
    placing it correctly, and `m2` went from one `ldrb` to none and hung. The idea
    is sound; that implementation was not.

**AND TWO PROGRAMS ACCUSED THIS PASS OF A MISCOMPILE THAT WAS NOT ITS.**
`k1_dispatch` and `k2_live_pressure` diverged with the row on — and diverged with
it OFF as well. `M54` is what they turned out to be.

**WHEN / WHERE.** 2026-08-29, `main`, c8gd.4xlarge Graviton4 spot in us-west-2,
Debian 13 arm64, gcc 14.2.0 -O2, zcc native; 96/96 output-matched, cargo 219/0.

---

### M54. Four of the ninety-six timed programs were UNDEFINED, and every gate was silent

**HOW IT SURFACED.** `tailjump` (`M53`) was accused of miscompiling
`k1_dispatch` and `k2_live_pressure`. Turning the pass off left both diverging,
which took the accusation off the pass and put it somewhere else:

| | `k1_dispatch` | `k2_live_pressure` |
|---|---|---|
| gcc -O0 | 4090464163158582321 | −6311100434712172540 |
| gcc -O1 | 4090464163158582321 | −6311100434712172540 |
| **gcc -O2** | **−8133137304650377657** | **5490126384503059296** |
| zcc | 4090464163158582321 | −6311100434712172540 |

Three readings agree and `-O2` is the outlier, which is the signature of undefined
behaviour rather than of a compiler bug. UBSan named it exactly:
`signed integer overflow: 271182 * 7919 cannot be represented in type 'int'`.
**Nobody was miscompiling.** C99 6.5p5 leaves signed overflow undefined, `-O2`
assumes it cannot happen, and `-O0`/`-O1`/zcc happen to wrap. All four are
conforming.

**THE SCAN, and the number it returned.** Running UBSan over all ninety-six:

| program | defect |
|---|---|
| `e2_many_args` | `46338 * 46345` — four of five products left at `int` width |
| `k1_dispatch` | signed overflow, twice: the `i*7919` seed and the `long` accumulators |
| `k2_live_pressure` | the same, forty accumulators |
| `u3_shift_var` | `u >> (32u - k ? 32u - k : 1u)` — the guard tests the TRUTH of `32−k`, which is 32 when `k` is 0, so it shifts by the width |

**Four of ninety-six timed programs had no defined answer.** `e2_many_args` is one
of the ten worst against `-O2`; its 2.41× was never a measurement of anything.
Fixed: widen the products, make the VM's value type unsigned (wrapping is defined,
C99 6.2.5p9), and mask the rotate. Re-scanned: **0 of 96**.

**WHY NO GATE SAW IT, and the gate is not at fault.** `exectime.sh` compares zcc's
output against the referee's, which is the right question for a MISCOMPILE and is
silent about a program neither compiler can get right. At `-O1` the two agreed by
accident and the row was timed; at `-O2` they disagreed and the harness printed
`DIVERGE` for both, twice, correctly.

**THE READER WAS AT FAULT.** The table was filtered with `awk 'NF==5 && $5+0>0'`,
a `DIVERGE` row carries `-` in that column, and the geomean was then reported as
"over 94" without anyone asking where the other two had gone. That is the third
instance in one session of the same error — a glob that failed silently and took
its command with it, a `callgrind` job that held a core for 93 minutes, and now
two `DIVERGE` lines eaten by a filter. **The instrument said so every time.**

**THE GATE THAT NOW EXISTS.** `tests/ubscan.sh`, in the sci-gate beside
`provenance`. It compiles every suite program at `-O0` with
`-fsanitize=undefined -fno-sanitize-recover=all` and runs it; a non-zero exit is
RED. `-O0` on purpose — an optimizer may delete the undefined operation before the
sanitizer sees it, which would make the gate quieter without making the corpus
cleaner. It refuses to pass on an empty scan, per `M39`.

**AND EVERY `-O2` NUMBER BEFORE THIS ENTRY IS AFFECTED.** The geomeans quoted
earlier on 2026-08-29 were over 94 programs with two excluded and two more
included that should not have been. The corrected suite reads **EXEC 1.3177 over
96, 0 DIVERGE**.

**WHEN / WHERE.** 2026-08-29, `main`, gcc 14.2.0 UBSan at `-O0`, c8gd.4xlarge
Graviton4 and the local box; scan is `tests/ubscan.sh`.

---

### M55. Four is the jam factor because four is the lane count, not because it measured best

**THE FACT.** `hir::pass::jam` unrolls the outer loop of a two-deep nest by
FOUR, and the number is the lane count of a `q` register at a 32-bit element
(DDI 0487 C1.3.2), not a tuning result.

**WHY IT IS NOT A KNOB.** The row exists to make the SIMD form reachable: four
jammed lanes are one `mla v.4s`. A jam factor that is not the vector width would
have to be re-jammed before that could be built, so the two numbers are the same
number and only one of them is free to move. `Arr::V4S` is where it is written
down; this constant is that one, spelled where the loop transform needs it.

**WHAT WAS MEASURED, and it is smaller than the shape promised.** On
`z4_matmul_int`, jam alone takes 0.022 s to 0.019 s against gcc -O2's 0.007 —
13%, not the 2.5x the instruction count suggested. The reason is visible in the
result and is the next row: each lane gets its OWN strength-reduced pointer into
`B`, four pointers advancing by the row stride, so the four loads are four
addresses instead of four lanes of one. They differ by a CONSTANT — 4, 8, 12
bytes — and sharing one pointer with an immediate displacement is what turns
them back into one `ldr q`.

**WHEN / WHERE.** 2026-08-29, `main`, c8gd.4xlarge Graviton4, gcc 14.2.0 -O2.

---

### M56. The four jammed lanes became one vector multiply-accumulate, and the row landed where it was priced

**THE FACT.** `mir::pass::vecmla` recognizes, on SSA MIR before register
allocation, four unit-strided 32-bit loads at one base with displacements
`0,4,8,12`, four `MAdd` sharing one other operand, and four loop-carried block
parameters they accumulate into. It replaces them with one `Load` of `MemOp::Q`,
one `VDup`, a `VInt::Mul` and a `VInt::Add` over ONE `Q` parameter, and extracts
the four lanes at the exit edge with `VExt` — `umov`, added with the same
assembler check the rest of the vector forms got.

**THE PRICE WAS TAKEN ON THE MODEL FIRST, and it was honest.** PLAN.md predicted
`z4_matmul_int` would reach about **1.4–1.5x**, not 1.0x, because the form still
costs a `dup` and a `q` load per four lanes. Measured: **2.932 -> 1.370**. Suite
EXEC geomean 1.2245 -> 1.2168 in the interleaved pair, INSN 1.0154 -> 1.0147, and
`z4` stopped being the worst program in the suite.

**THE DEFECT IT COST, and it is a Law-2 Side-I.** The first cut indexed the exit
block's arguments with the back edge's parameter layout. A back edge and an exit
edge do not carry the same parameters, and the pass panicked on an out-of-bounds
index rather than producing a wrong answer — which is the shape a block-parameter
IR gives this class of mistake, and the reason it was found in minutes.

**THE UNIT TEST COULD NOT BE WRITTEN NAIVELY.** The pattern only exists after
`jam` has run and after the `iv` displacement row has folded the four walks onto
one base, so the battery's fixture needs the whole HIR ladder, 64-bit loop
indices, and a non-power-of-two stride — a hand-built MIR function reaches the
recognizer and matches nothing. Recorded because the same will be true of every
later MIR row that consumes a HIR transform's output.

**WHEN / WHERE.** 2026-08-29, `main` `dd80a8d`, c8gd.4xlarge Graviton4, gcc
14.2.0 -O2, 96-program taxonomy suite. Gate 16/0.

---

### M57. The map vectorizer was built, switched on, and bought nothing — twice

**THE FACT.** `hir::pass::vecmap` — four lanes at a time through a unit-stride
map loop — has shipped default-OFF since it was written, pending its A/B. The A/B
was run: two interleaved OFF/ON pairs over the 96-program suite.

| | EXEC | INSN |
|---|---|---|
| OFF pass 1 | 1.2014 | 1.0147 |
| ON  pass 1 | 1.2021 | **1.0178** |
| OFF pass 2 | 1.2107 | 1.0147 |
| ON  pass 2 | 1.2081 | **1.0178** |

**READ THE DETERMINISTIC COLUMN.** EXEC moves by less than its own ±0.007 spread
and changes sign between the pairs, which is the definition of no signal. INSN is
a fold over the emitted stream with no measurement noise at all, and it is worse
by 0.31% in both directions — the guard, the tail and the vector prologue are
real code and nothing pays for them. The row stays off.

**WHY IT MISSED, and the census said so before the clock did.** `vecprobe` finds
59 `map` loops in the suite and 54 in sqlite, so the shape is not rare — but on
the seven programs whose measured gap is largest, `ZCC_VECMAP=1` emits **zero**
vector instructions. Every one of those seven carries a value across the back
edge, which is what makes them `reduce` and not `map`. A pass can fire on a
hundred sites and still miss the ones that are hot; a site count is demand for
BUILDING a row and is not evidence the row will pay.

**WHEN / WHERE.** 2026-08-29, `main` `dd80a8d`, c8gd.4xlarge Graviton4, gcc
14.2.0 -O2, `ZCC_VECMAP=1`.

---

---

# Part G — the pipeline, layer by layer

The architecture and the seam between each layer. Section numbers here are
`§G<n>`; `src/` cites them by that name.

### §G0 WHERE THE DEFECTS LIVE — a layer-by-layer field guide

**Read this before hunting.** zcc is an educational compiler, and the most
transferable thing a session produces is not the patch but WHERE the defect
turned out to live. Every row below was found and paid for; each names the layer,
the SHAPE of the defect that layer produces, and the measurement that exposed it.
The pattern across all of them is that **the layer where a defect HURTS is almost
never the layer where it is VISIBLE**.

---

**LAYER 0 — the instrument.** The most expensive defects are in the measurement,
because everything downstream inherits them.

*Shape:* a classifier that splits on SYNTAX when the question is about SEMANTICS.
The whole-suite census classified a `mov` by whether its second operand started
with `#` or a digit. `mov w9, wzr` starts with neither and landed in the
register-copy column; gcc writes `mov w9, 0` for the same thing and landed in the
constant column. One activity, charged twice with opposite signs, and it
overstated the coalescing campaign's target by 1.6× and inverted the sign of the
constant-materialization row (`M26-correction`).

*Second shape:* a conjunction reported by the name of one conjunct. `free(p, occ)`
tests four things — allocatable, unoccupied, no physical conflict, and the AAPCS64
half — and the instrument counted every failure as "occupied". Genuine occupancy
turned out to be 3 of 488 refusals on the suite and 136 of 14,640 on sqlite,
which retired an eviction row three earlier fixes had already been aimed at
(`M27`).

*The lesson, and it is the general one:* **a count is a hypothesis about a
partition.** Before trusting one, ask what ELSE could land in each bucket, and
find a second instrument that answers from the other side. Here the compiler's
own counters (`ZCC_COALESCE`, `ZCC_MOVKIND`) close arithmetically — 203 + 26 +
289 = 518 — and never read a character of assembly.

---

**LAYER 1 — the cost model.** A model is exact for what it measures and blind to
everything else BY CONSTRUCTION, and the blindness is not a bug to fix but a
scope to state.

`cost(f) = |MIR(f)|` is exact for SIZE — one `MInst` is one machine instruction.
It has two named blindnesses:

* **chains** (Law 3c, `M10`): matmul at 1.638× with an IDENTICAL instruction
  count, because a multiply stood in front of a load;
* **frequency** (`M29`): a static count weighs an instruction in a latch executed
  5,760,000 times exactly as it weighs one in a cold arm. Removing two executed
  instructions from `n7_nested_subq`'s inner loop moved that program 1.370 →
  1.195 and the suite's INSN geomean 0.0008.

*The lesson:* **when a program is slow at parity instruction count, the model is
being asked a question it cannot answer.** Both duals are built —
`mir::cost::recurrence` for chains and `mir::cost::weighted` for frequency — and
a codegen row should be ranked on the one that matches its claim.

---

**LAYER 2 — the HIR passes.** Target-independent, and the defects here are about
COVERAGE rather than correctness.

*Shape:* a gate that refuses a great deal and binds on nothing. The
consumer-blind IV row refuses any loop whose trip count SCEV cannot bound above
32 — 653 sites on the suite, its own comment classing it a Law-4 convenience
truncation. Removing it changes the suite by **zero** instructions, because every
one of those sites is refused again by a later condition. The frontier was SCEV's
own coverage, 78% of the residual (`M28`).

*The lesson:* **measure a gate by what changes when you remove it, not by how
much it refuses.** A residual counter placed before the other conditions
attributes to the first gate everything the later ones would have caught anyway.

---

**LAYER 3 — isel, the lowering.** The defects here are the cheapest to fix and
the easiest to overlook, because the IR is right and only the ENCODING choice is
wrong.

* **The immediate on the wrong side** (`M30`). A64 has `add wd, wn, #k` and no
  mirror form, and `binop` offered only its right operand to the immediate
  encoder. `'a' + i % 26` — the ordinary way to write it in C — therefore
  materialized 97 into a register and added two registers. Two instructions where
  the ISA has one, inside whatever loop the expression sits in. The fix is to
  commute, and commutativity is exact for `+ * & ^ |` (ISO 9899 6.5).
* **Source order taken as a policy** (`M31`). A sparse `switch` becomes a linear
  compare chain in the arms' source order, so a state tested late costs a
  `cmp`+`b.eq` per byte for every arm ahead of it. The two worst programs in the
  suite are exactly this and nothing else.

*The lesson:* **every place the IR has an order and the machine has a preference
is a lowering decision, and an unexamined one defaults to whatever the front end
happened to build.** Grep for the places a lowering reads its operands or its
arms in sequence.

---

**LAYER 4 — regalloc.** Two distinct defect families, and they want opposite
work.

* **Policy** — the allocation ORDER is a cost argument, and a correct one can
  still be wrong for a neighbour. `GPR_ORDER` offers the caller-saved half first
  so short-lived values do not squat in the callee-saved registers that
  call-crossing values have no alternative to. Correct in itself; the side effect
  is that a short-lived value which is the copy PARTNER of a call-crossing value
  takes a register the partner may never join, and SSA destruction pays a `mov`
  on that edge for ever (`M27`).
* **Ordering in the PIPELINE** — a pass that runs after SSA destruction cannot
  have its copies coalesced, because the coalescer has already run. `promote`
  turns a spill slot into a register and its former store and reload come back as
  ordinary moves that nothing removes; on `n7_nested_subq` that was three
  executed instructions and a taken branch in a fifteen-instruction inner loop
  (`M29`).

*The lesson:* **ask what has already run.** A pass placed after the machinery
that would have cleaned up after it must clean up after itself.

---

**LAYER 5 — emit.** By charter `emit.rs` makes no decisions and re-parses
nothing, so a defect that reaches it is a defect from above wearing a different
spelling. The 67 `mov r, r` self-copies look like an emitter oversight and are
not: 61 of them have a reader that genuinely looks past 32 bits, so the
instruction IS the zero-extension that reader needs and deleting it miscompiles.
gcc reaches zero by not NEEDING the extension — an `ext.rs`/HIR row (`M26-
correction`).

*The lesson, and it is the charter's rule restated:* **a peephole in the emitter
is a defect being treated at the layer where it is visible instead of the layer
where it is decided.**

---

**THE METHOD THAT FOUND ALL OF THEM,** in the order it must be run:

1. Rank the failing program's blocks by EXECUTIONS
   (`ZCC_WEIGHTS=1 ZCC_WCOST=1`), never by static count.
2. Read the top two blocks against gcc's SAME loop, instruction by instruction.
3. Hand-edit the `.s`, link it, verify the output is identical, and time it with
   ALTERNATING best-of-N — never two separate sessions.
4. Only then touch the compiler, and let the batteries answer.

Step 4 is not a formality. The first cut of `promote::sink_stores` was refused by
three allocator batteries with `⟦mir_v⟧ ≠ ⟦mir_p⟧` because it clobbered a
loop-carried accumulator on the loop's EXIT edge — a wrong answer, caught at the
middle exactly as Law 3 intends, before any suite ran.

And step 1 is not a formality either: `ZCC_WCOST` uses `hir::freq`'s DEPTH-based
estimate, which ranked `m1_resp_parse`'s two setup loops at 58% of the program
when the parse loop beside them runs sixty times more often. Two hand edits on
the setup loops measured 0.997 and 0.999. **A frequency estimate that cannot see
trip counts cannot rank two loops at the same depth**, and the ranking is a
starting point for reading, not a verdict.
### §G1 Ground rules — what dies, what survives

| category | items | rule |
|---|---|---|
| **DIES** (written from zero) | `src/ir.rs` (1,858 LOC), `src/opt/*` (8,683), `src/codegen/*` (6,614) = **17,155 LOC, 70% of zcc** | delete at R0 start (`git rm`); never copy code from them |
| **SURVIVES as INPUT** | `src/lexer.rs`, `src/preprocess.rs`, `src/parser.rs`, `src/ast.rs`, `src/ext.rs`, `src/main.rs` (~7.1k) | the AST + TyTab boundary (charter Article B). The failure is entirely below it. `main.rs` loses its call into `codegen`; `ext.rs` keeps its frontend hooks; IR-side `EXT(...)` lowerings are re-implemented in the new layers |
| **SURVIVES as REFERENCE ONLY** | `THEORY.md` (A7 pass ladder = the list of theorems to re-realize; II-3 AAPCS64; II-4 ELF; II-5 arch), `SEMANTICS.md` (`⟦·⟧` definitions, re-targeted to HIR + extended to MIR), `tests/` (all suites, oracles, science gates, `bench/`, `corpus25.sh`, `exectime.sh`), `MECHANISM.md Part A` | theorems and measurement, never structure |

Constraints that do not change: Rust, edition 2024, **zero external crates**, single crate, AArch64-ELF
only (macOS = clang oracle only), strict C99 + marked `EXT(...)`, Laws 1–3 + Article E gates.

---

### §G2 Pipeline and module layout

```
C ──cpp/lex/parse──► AST + TyTab
   ──lower (Braun on-the-fly SSA)──► HIR (SSA, target-independent, block parameters)
   ──HIR passes (tree-SSA half)──► HIR
   ──isel (maximal munch + AAPCS64 automaton)──► MIR (SSA, virtual registers, arm64 ops)
   ──MIR-SSA passes (cmp-elim, auto-inc, sxtw-lattice, ldp/stp)──► MIR
   ──regalloc (Braun-Hack spill → chordal color → biased coalesce → SSA destruct)──► MIR (physical)
   ──frame lowering / shrink-wrap / block layout / structured peephole──► MIR (final)
   ──emit (1:1 print from the ISA table)──► .s
```

MIR before and after allocation is ONE type in two lifecycle states (virtual/SSA vs physical/ordered),
exactly as LLVM's "MIR". MIR is arm64-specific by design (Article B: one module per target); a
target-independent middle layer is deferred until a second target exists.

```
src/hir/mod.rs        HIR types: Func, Block, Inst, Term, Value, Ty, Effect
src/hir/build.rs      AST → HIR lowering. NOTE (R0.9 audit): Braun's SSA construction is NOT
                      here yet and this line used to claim it was. R0/R1 keep every local in
                      memory (Part H), so no φ/block-parameter insertion runs at all; Braun arrives
                      with `pass/sroa.rs` at R2.2, which is also where mem2reg uses it.
src/hir/verify.rs     SSA dominance property, arity, typing
src/hir/interp.rs     ⟦hir⟧ — the executable reference semantics (SEMANTICS.md)
src/hir/dom.rs        preds/succs, dominator tree (Cooper-Harvey-Kennedy), loop forest + depth
src/hir/alias.rs      the memory oracle (THEORY A7 "ALIAS ANALYSIS", re-derived)
src/hir/pass/*.rs     one file per theorem family (see §G4)
src/mir/mod.rs        MIR types: MFunc, MBlock, MInst, Reg, Operand, AddrMode, constraints
src/mir/isa.rs        Side-II tables: register files, classes, encodable-immediate predicates, ISA shapes
src/mir/verify.rs     SSA (virtual phase) / constraint satisfaction (physical phase)
src/mir/interp.rs     ⟦mir⟧ — one interpreter for virtual and physical MIR
src/mir/pass/*.rs     cmp_elim, auto_inc, ext_lattice, ldst_pair, frame, shrink_wrap, layout, peephole
src/isel/lower.rs     HIR → MIR, per-block bottom-up munch over single-use trees
src/isel/pattern.rs   the pattern table (each row = one theorem, one battery test)
src/isel/abi.rs       AAPCS64 C.1–C.15 automaton, varargs, HFA, sret, stack args
src/isel/imm.rs       immediate legalization (imm12 / logical / movz-movk / shifts)
src/regalloc/live.rs  liveness on SSA with block parameters
src/regalloc/spill.rs Braun-Hack Belady spilling + rematerialization + SSA reconstruction
src/regalloc/color.rs chordal greedy coloring in dominance preorder, biased coalescing
src/regalloc/destruct.rs  block-arg → parallel copies → sequentialized moves
src/regalloc/verify.rs    interference / constraints / slot dataflow / clobber safety
src/emit.rs           MIR(final) → text (+ ELF directives, Side-II II-4)
```

---

### §G3 HIR — target-independent SSA

#### 3.1 Design decisions
- **SSA from birth.** `build.rs` lowers the AST straight into SSA with Braun et al. 2013 ("Simple and
  Efficient Construction of Static Single Assignment Form", on-the-fly, no dominance frontiers — already
  a proven theorem in this project). Scalar locals become values; aggregates and address-taken locals
  become `alloca` stack objects accessed by typed load/store. SROA + mem2reg (Braun again, on allocas)
  promote the split pieces later. **There is no `out_of_ssa` anywhere in HIR.**
- **Block parameters instead of φ instructions** (Cranelift/MLIR/Swift style):
  `br %c, bb1(%a, %b), bb2(%c)`. Edge semantics are explicit; the verifier and the interpreter are
  simpler; SSA destruction (in MIR) becomes literally "one parallel copy per edge". HIR and MIR share
  this model — one mental model.
- **Closed scalar types**: `Ty = I8 | I16 | I32 | I64 | F32 | F64`; pointers are `I64`. Signedness and
  width live in the **opcode**, not in a TyTab lookup (`sdiv/udiv`, `srem/urem`, `sext/zext/trunc`,
  `icmp.slt/ult`, `ashr/lshr`). After lowering, HIR is independent of the frontend's `TyTab`.
  SEMANTICS.md §G3 (`canon_τ`, `⟦op⟧_τ`, `⟦cast⟧`) becomes a closed definition over this `Ty`.
- **Effect class per instruction** — `Effect = Pure | Read | Write | Call | Control`. DCE, CSE, GVN,
  LICM, sinking legality are a table lookup, never a per-pass hand-list.
- **Calls carry the C signature** (param types incl. composites by (size, align, class-hint), return
  type, `nfix` for variadics). ABI classification is NOT an HIR concern — it is isel's Side-II job.

#### 3.2 Instruction set
```
Value operands: %v (SSA value) | const (iconst ty k | fconst ty bits) | sym (global/function address)
Arithmetic  : add sub mul  sdiv udiv srem urem   and or xor  shl lshr ashr      (ty ∈ I*)
Float       : fadd fsub fmul fdiv fneg                                          (ty ∈ F*)
Compare     : icmp.{eq,ne,slt,sle,sgt,sge,ult,ule,ugt,uge}  fcmp.{oeq,one,olt,ole,ogt,oge,uno}  → I32 0/1
Convert     : sext zext trunc (int↔int)  fptosi fptoui sitofp uitofp fpext fptrunc  bitcast(I64↔F64, I32↔F32)
Memory      : load ty %addr [aclass]   store ty %addr %val [aclass]   alloca size align → I64   memcpy %dst %src n   memset %dst n
              (aclass = the C effective type's alias class, assigned by the frontend lowering: the hook for
               type-based alias analysis (TBAA, O2 `-fstrict-aliasing`) — cheap to carry from day one, expensive to retrofit)
Address     : addr_global sym  addr_func sym  addr_label sym (EXT computed goto)   (address arithmetic = plain add/mul)
Select      : select ty %c %a %b
Call        : call sig callee(args…) → %r?      (callee = sym | %fnptr)
Intrinsics  : va_start %ap  va_arg ty %ap → %r  va_area → %r  overflow.{add,sub,mul}.{s,u}.ty %a %b %rp → %flag
              sync.{fetch_add,…} …  asm "tmpl" operands   (each = Effect::Call-class, opaque to passes)
Terminators : jmp bb(args)  br %c, bb(args), bb(args)  switch %v, [(k, bb(args))…], default bb(args)
              ret %v?  unreachable  goto_ptr %v (EXT)
```
`switch` is NEW (the old IR had none — hence no jump tables, hence the "5.1 switch quarantined" note).

#### 3.3 Data structures (sketch)
```rust
pub struct Func { name, sig: Sig, blocks: Vec<Block>, values: Vec<ValueInfo>, allocas: Vec<Alloca>, entry: BlockId }
pub struct Block { params: Vec<Value>, insts: Vec<Inst>, term: Term, weight: Freq /* static branch-probability estimate (Ball-Larus heuristics); drives layout + spill next-use weighting; PGO hook */ }
pub struct ValueInfo { ty: Ty, def: Def /* Inst(bi, ii) | Param(bi, k) | FuncParam(k) */ }
pub enum Inst { Bin{dst, op: BinOp, ty, a, b}, Un{..}, Cmp{..}, Cvt{..}, Load{..}, Store{..}, Alloca{..},
                Addr{..}, Select{..}, Call{..}, Intrinsic{..} }
pub enum Term { Jmp(Target), Br(Operand, Target, Target), Switch(Operand, Vec<(i64, Target)>, Target),
                Ret(Option<Operand>), Unreachable, GotoPtr(Operand) }
pub struct Target { block: BlockId, args: Vec<Operand> }
```
Analyses (cached on `Func`, invalidated by any CFG edit): `Cfg{preds,succs}`, `DomTree` (Cooper-Harvey-
Kennedy iterative — simpler than Lengauer-Tarjan, adequate), `LoopForest{header, body, depth, latch,
preheader}`, `Alias` (the B1 oracle: allocas non-escaping ⟹ disjoint; globals by symbol; TBAA-free
otherwise = may-alias).

#### 3.4 Interpreter `⟦hir⟧` and verifier
`interp.rs`: Σ = ⟨values: Vec<Bits>, memory: flat byte array (LP64 layout, globals materialized),
call stack⟩; big-step per SEMANTICS.md §G4, block-argument transfer replaces φ-select. Returns
`Result<Bits, Trap>` — a trap (UB: div-by-zero, misaligned/OOB access) is `⊥` and any transform may
refine `⊥` (commuting squares compare only on non-⊥ inputs). Externals: a small builtin table
(memcpy/memset/strlen/printf-subset) so corpus functions run under the interpreter.
`verify.rs`: every use dominated by its def; block-arg arity and types match every incoming edge;
opcode/type consistency; exactly one terminator; entry has no params.

---

### §G4 HIR passes — the tree-SSA half (re-realized from THEORY A7)

Order mirrors gcc -O1 (`-ftree-*`). Bounded fixpoint over the sequence, max 3 rounds.

| # | pass | file | theorem (THEORY A7 row) | proof |
|---|---|---|---|---|
| 0 | delabel | `pass/cfg.rs` | an unaddressed C label is not observable, so the block it pins is free (M36). Runs ONCE, before everything: it is what unfences rows 1 and 7 on any function containing a `goto` | battery + 3 refusals |
| 1 | cfg_simplify | `pass/cfg.rs` | block merge, unreachable elim, jump threading of trivial blocks | `⟦f⟧=⟦P f⟧` battery |
| 2 | sroa + mem2reg | `pass/sroa.rs` | non-escaping aggregate → scalar allocas → Braun promotion | battery + alias oracle |
| 3 | sccp | `pass/sccp.rs` | Wegman-Zadeck lattice over reachability | battery |
| 4 | gvn | `pass/gvn.rs` | dominator-based value numbering; absorbs CSE, copy-prop, constant folding, algebraic normalization (`⟦L⟧=⟦R⟧` rewrite table) | battery + rewrite table exhaustively checked |
| 5 | load_elim / dse | `pass/mem.rs` | store→load forwarding, dead store, gated by the alias oracle | battery |
| 6 | dce | `pass/dce.rs` | Effect table: `Pure` with no uses is dead | battery |
| 7 | inline | `pass/inline.rs` | β-reduction; gcc -O1 = called-once + small (size threshold = dated policy constant) + interprocedural purity (the #24 `pure_functions` theorem) | battery on caller |
| 8 | licm | `pass/licm.rs` | pure, trap-free, invariant → preheader. **Unconditional at O1** — no register-pressure guard; the allocator owns pressure | battery |
| 9 | iv / strength-reduce / pointer-iv / LFTR | `pass/iv.rs` | derived IV rewrite, address recurrence, linear-function test replacement | battery |
| 10 | if_convert | `pass/ifconv.rs` | side-effect-free diamond → `select` | battery |
| 11 | rotate / final-value / invariant-pure-call hoist | `pass/loop.rs` | loop rotation; the #24 4-fence theorem; SCEV closed forms for counted loops | battery |
| 12 | sink | `pass/sink.rs` | the dual of licm: a pure trap-free instruction with ONE using block, dominated by here and no deeper in a loop, moves down to it. Added at R3 rather than planned: the excess histogram, Part F measured register pressure as the largest remaining item, and this is the cheapest thing that shortens a live range | battery |

Battery = the existing method: small-domain-exhaustive inputs + boundary values, `⟦f⟧ ≡ ⟦P f⟧` on every
corpus function, run under `cargo test`. Ported in spirit from `opt/tests.rs`; the *tests* are theorems
and may be re-derived from the old file — the *pass code* may not.

---

### §G5 MIR — the load-bearing layer

#### 5.1 Registers and classes (Side-II, AAPCS64 §G6.1.1 — the full table, no convenience truncation)
```
Class GPR : x0–x30 minus reserved {sp, x29 (fp when a frame pointer is required), x30 (lr), x16, x17 (IP0/IP1 — scratch for parallel-copy cycles and veneers), x18 (platform)}
            allocatable order: caller-saved first x0–x15 (x0–x7 are also argument regs), then callee-saved x19–x28
Class FPR : v0–v31 minus reserved {v31 (FP scratch for copy cycles)}; caller v0–v7,v16–v30; callee v8–v15 (low 64 bits)
Class FLAGS: k=1, the NZCV register. `cmp/cmn/tst/adds/subs/ands/fcmp` define it; `b.cc/csel/cset/cinc/ccmp` use it.
Reg = V(VReg) | P(PReg);  VRegInfo { class, width: W32|W64|S|D }
```
Modeling NZCV as a k=1 class makes compare-elimination a GVN over flag definitions and makes "two flag
values live at once" an ordinary interference the allocator resolves by rematerializing the `cmp`
(flags are always rematerializable: their producer is pure).

#### 5.2 Operands, constraints, addressing modes
```
Operand   = Reg(Reg) | Imm(i64) | FImm(bits) | Mem(AddrMode) | Sym(Symbol, Reloc) | Cond(CC) | Slot(StackSlot)
AddrMode  = BaseImm{base: Reg, off: i32 /*scaled-unsigned or signed-9*/}
          | BaseReg{base, idx: Reg, ext: None|Uxtw|Sxtw|Lsl, shift: u8}
          | PreIdx{base, off} | PostIdx{base, off}       (both DEFINE a new base vreg in SSA phase)
          | PcRel{sym, page: bool} | Slot{id, off} | SpArg{off}
Constraint on each register operand (regalloc2 model):
   Use | Def | UseFixed(PReg) | DefFixed(PReg) | Clobber(RegSet /*on Call*/) | Reuse(def = use k) (rare on arm64)
```
Every instruction exposes `operands(&self) -> impl Iterator<(OperandRef, Constraint)>` and
`operands_mut`, plus `effects(&self) -> MemEffect` (`None | Read(aclass) | Write(aclass) | Barrier`).
The allocator, liveness, verifier and interpreter use ONLY these visitors — no per-opcode special
cases outside `isa.rs`. `effects()` is also the dependence oracle a list scheduler needs (the O2 shelf, `THEORY.md` Appendix),
so scheduling costs no new IR surface later.

#### 5.3 Instruction families (enum by arm64 shape; `isa.rs` owns encodability)
```
AluRR{op, w, dst, a, b}  AluRI{op, w, dst, a, imm12<<sh}  AluRRS{op, w, dst, a, b, shift, amt}  AluRRX{op, w, dst, a, b, ext, amt}
Mul{w,dst,a,b}  Madd/Msub{w,dst,a,b,c}  Smull/Umull  Div{s/u,w,dst,a,b}  Logic{op,w,dst,a,b|logimm}  Shift{op,w,dst,a,b|imm}
MovZ/MovK/MovN{w,dst,imm16,shift}  Mov{w,dst,src}  Ext{sxtb/sxth/sxtw/uxtb/uxth}  Bfx{u/s,dst,src,lsb,width}  Bfi/Bfxil
Ld{width,ext,dst,mem}  St{width,src,mem}  LdP/StP{w,r1,r2,mem}  Adrp{dst,sym}  AddLo12{dst,base,sym}  LdrGot
Cmp/Cmn/Tst{w,a,b|imm}→FLAGS  AddS/SubS/AndS  Csel/Csinc/Csinv/Csneg{w,dst,a,b,cc}  Cset/Cinc  Ccmp
B(target)  Bcc{cc,target}  Cbz/Cbnz{w,reg,target}  Tbz/Tbnz{reg,bit,target}  Br(reg)  Ret
Bl{sym}  Blr{reg}                           (always wrapped by the Call pseudo below)
FP: Fmov(rr, r↔g, imm8)  Fadd/Fsub/Fmul/Fdiv/Fneg/Fabs/Fsqrt  Fcmp→FLAGS  Fcsel  Fcvt{s↔d}  Scvtf/Ucvtf  Fcvtzs/Fcvtzu  LdF/StF
Sync: Ldaxr/Stlxr/Ldar/Stlr/Dmb            (EXT __sync_*)
Pseudo (exist only before their lowering pass):
  Call{callee, args: fixed uses, rets: fixed defs, clobbers: caller-saved set, stack_bytes, tail: bool /* sibling call → `b` after epilogue (O2 -foptimize-sibling-calls) */}
  Copy{dst,src}  ParallelCopy{pairs}  Spill{slot,src}  Reload{dst,slot}  FrameAddr{dst,slot}  Asm{tmpl,ops}
  JumpTable{index, table: Vec<BlockId>}     (lowered to adr+ldr+br in layout)
```
#### 5.4 SSA, interpreter, verifier
Virtual phase: every VReg defined once; block parameters carry values across edges; `PreIdx/PostIdx`
define a fresh base vreg. `interp.rs`: machine state ⟨regs (a map for V, an array for P), NZCV,
memory, sp/frame⟩; one interpreter for both phases so `⟦hir⟧ = ⟦mir_v⟧ = ⟦mir_p⟧ = ⟦mir_final⟧` is
checkable end-to-end per function. `verify.rs`: virtual phase = SSA + arity + width consistency +
FLAGS def-before-use; physical phase = no V left, every Fixed constraint met, clobbered regs not live
across the clobber, every Slot resolved.

---

### §G6 isel — HIR → MIR

Per block, bottom-up over the SSA use-def graph, **maximal munch on single-use trees** (a value with
one use may be folded into its user; multi-use values are materialized once). The pattern table
(`isel/pattern.rs`) is the theorem table — each row = one `⟦hir-tree⟧ = ⟦mir-seq⟧` battery test:

| HIR tree | MIR | note |
|---|---|---|
| `load(add(b, shl(i, k)))`, k∈{0..3} matching width | `ldr [b, i, lsl #k]` | also `sxtw/uxtw` extend when `i` is I32 |
| `load(add(b, c))`, c encodable | `ldr [b, #c]` | scaled-unsigned or signed-9 per width |
| `load(addr_global s)` | `adrp; ldr [x, :lo12:s]` | GOT form for externs |
| `br(icmp.cc a b)` | `cmp a, b; b.cc` | `cbz/cbnz` when b=0 and cc∈{eq,ne}; `tbz/tbnz` for single-bit tests |
| `select(icmp…, a, b)` | `cmp; csel` | `csinc/csinv/csneg/cset` special forms |
| `add(mul(a,b), c)` / `sub(c, mul(a,b))` | `madd` / `msub` | `smull/umull` for widened products |
| `and(lshr(a,s), mask)` / `shl+ashr` | `ubfx` / `sbfx` | bit-field extract |
| `add(a, sext(b))` etc. | `add a, b, sxtw` | operand-extend folding |
| `mul(a, const)` | shift/add sequence | Side-II cost table, otherwise `mov+mul` |
| immediates | `imm12`, `imm12<<12`, logical-imm, `movz/movk` chain, `mov wzr`, `movn` | `isel/imm.rs` predicates |
| `switch` | jump table (`adr+ldr+br`) when density ≥ threshold else balanced compare tree | thresholds = gcc defaults, dated policy constants |

**ABI (`isel/abi.rs`)** = the AAPCS64 §G6.4–6.8 C.1–C.15 automaton over the call's C signature (THEORY
II-3): NGRN/NSRN/NSAA state, composites ≤16B in registers, HFA/HVA, >16B by reference (caller copy),
sret in x8, C.11 lock, variadics (nfix; the 192-byte register save area; `va_start/va_arg` lowering),
long double via soft-float calls. Emits fixed constraints on the `Call` pseudo + explicit `str` to
`SpArg` for stack args; function entry materializes params from `DefFixed` or `SpArg` loads.

---

### §G7 Regalloc on MIR-SSA — the core

References: Hack 2007 (thesis: SSA interference graphs are chordal; dominance preorder is a perfect
elimination order), Braun & Hack 2009 ("Register Spilling and Live-Range Splitting for SSA-Form
Programs"), Boissinot et al. 2009 (fast liveness / out-of-SSA), Braun et al. 2013 (SSA reconstruction).

1. **Liveness** (`live.rs`): iterative backward dataflow on SSA with block parameters (a target's
   argument is a use on the edge). Cheap enough; Boissinot's dominance-based variant is an optional
   later optimization.
2. **Spilling** (`spill.rs`) — Braun-Hack, per register class. **As built (R2.2, entry sets
   widened at R4.1), with two deviations recorded here rather than left implicit:** SSA
   RECONSTRUCTION is absent because it is not needed. Through R3 that held because a reload's
   register never left the block that created it. Since **R4.1** a reload copy is carried into
   the successors, but only where EVERY predecessor holds that same copy — and a copy has one
   definition, so that condition says every path to the use runs through the definition, which
   IS dominance. The use is dominated by its definition for the same reason as before, with no
   block parameter and no renaming; `mir::verify` re-derives it after every spill in debug
   builds. The second deviation is unchanged: a spilled BLOCK PARAMETER is removed from the IR
   rather than stored at its definition, since its definition is the block head. One slot per SSA WEB (parameter ∪ its arguments), merged
   only where the members do not interfere:
   - Walk blocks in dominance order. For each block compute the entry set `W_entry` (≤ k values) from
     the predecessors' exit sets, preferring values with the nearest next use (loop-aware next-use
     distance: uses outside the current loop count as "far").
   - Walk instructions applying Belady MIN: for each use not in `W`, insert `Reload` (a **new vreg**);
     when `|W| + defs > k`, evict the value with the furthest next use, inserting a `Spill` at its
     definition (once per value, lazily). Values marked **rematerializable** (`iconst`, `adrp+add`,
     `mov`-of-immediate, extends of a value still in W) are recomputed instead of reloaded.
   - At block boundaries reconcile `W_exit(pred)` with `W_entry(succ)`: insert reload/spill on the edge
     (critical edges are split first, in `dom.rs`).
   - Fixed constraints (`UseFixed/DefFixed`, `Clobber`) count against `k` at that instruction; Hack's
     method: a `ParallelCopy` is inserted before a constrained instruction so the constraint is local.
   - Reloads create new definitions ⟹ **SSA reconstruction** (Braun 2013 again) rewires uses to the
     nearest reaching definition, inserting block parameters as needed.
   - Post-condition (verified): register pressure ≤ k(class) at every program point. **This is
     live-range splitting** — a value lives in a register where hot and in its slot elsewhere.
3. **Coloring** (`color.rs`): dom-tree preorder over blocks, instructions in order; maintain the live
   set incrementally; at each definition assign the lowest free color respecting the constraint and
   the class order (caller-saved first, callee-saved last, so values not live across a call avoid
   prologue saves). Block parameters are colored at the block head (after the predecessors' copies
   are accounted for). **Theorem: never fails after step 2** (chordality + pressure ≤ k). A call's
   `Clobber` set is treated as fixed definitions live across the instruction, so a value live across a
   call cannot receive a caller-saved color — **no special "crossing" logic exists.**
4. **Coalescing**: biased coloring — prefer a copy partner's color (`Copy`, block-argument pairs,
   `PostIdx` base pairs) when free. Never merges nodes, never breaks the pressure guarantee. Upgrade to
   Boissinot merging only if the measured residual copies (Law-4 residual check, Part F) justify it.
5. **SSA destruction** (`destruct.rs`): each edge's parameters become a `ParallelCopy` (already colored);
   sequentialize with the standard windmill algorithm; cycles broken with the reserved scratch
   (x16 for GPR, v31 for FPR). Spill/Reload become `str/ldr` to `Slot` operands.
6. **Verify** (`verify.rs`, run in debug builds on every function + in the battery): (a) no two values
   simultaneously live (pre-destruction liveness) share a color; (b) every Fixed constraint met; (c)
   every `Reload` slot is dominated by a `Spill` of the same value; (d) no value in a caller-saved
   register is live across a `Call`; (e) `⟦mir_v⟧ = ⟦mir_p⟧` on the corpus.

---

### §G8 MIR passes — the O1 back-half (gcc -O1 has NO instruction scheduler; none here)

Pre-allocation (on SSA):
- `cmp_elim`: GVN over FLAGS definitions; `sub`+`cmp` → `subs`, `and`+`tst` → `ands` when the flags
  consumer is the only other user. (gcc `-fcompare-elim`.)
- `auto_inc`: `ldr [p]; add p', p, #k` → `ldr [p], #k` defining `p'` (post-index), pre-index dual.
  (gcc `-fauto-inc-dec`.)
- `ext_lattice`: known-width dataflow ("value is already sign/zero-canonical in its low 32 bits")
  eliminating redundant `sxtw/uxtb/uxth`. Replaces the five old text sxtw levers with one pass.
- `ldst_pair`: adjacent same-base accesses → `ldp/stp` (THEORY A7 "LDP/STP PAIRING").
Post-allocation (physical):
- `frame`: assign slots (spills, allocas, outgoing-arg area, callee-saved save area, vararg save
  area); one frame adjust (`-fcombine-stack-adjustments` by construction); frame pointer only when a
  VLA/alloca exists (`-fomit-frame-pointer`); prologue/epilogue with exactly the callee-saved set used.
- `shrink_wrap` (R3): place prologue/epilogue at the nearest common dominator of the blocks that need
  callee-saved registers or the frame (`-fshrink-wrap`).
- `layout`: block order = RPO with loop bodies contiguous; invert conditions for fall-through; drop
  `b .next`; lower `JumpTable`.
- `peephole` (structured, on MIR — never on text): self-move elimination, `mov wzr`, dead defs.

---

### §G9 Emit

`emit.rs`: `fn fmt(inst: &MInst) -> String` driven by `isa.rs`; sections, symbols, relocations,
TLS per THEORY II-4 (`adrp/:lo12:`, `:got:`, `:tprel_*`, no `_` prefix). Determinism seal: identical
MIR ⟹ identical bytes. Confirmation: `as` accepts every emitted file; the suites confirm.

---

### §G10 Proof map (Law 3 — certify at the middle) and the cost model

| layer | obligation | mechanism | where it runs |
|---|---|---|---|
| AST → HIR | faithful lowering | HIR verifier + differential suites (c99 referee) | `cargo test` + box gates |
| HIR pass P | `⟦f⟧ = ⟦P f⟧` | exhaustive small-domain battery on the corpus under `hir::interp` | `cargo test` |
| isel | `⟦tree⟧ = ⟦seq⟧` per pattern; `⟦hir⟧ = ⟦mir_v⟧` per function | pattern battery + whole-function translation validation with generated inputs | `cargo test` |
| MIR-SSA pass | `⟦m⟧ = ⟦P m⟧` | battery under `mir::interp` | `cargo test` |
| regalloc | renaming bisimulation | mechanical verifier (§G7.6 a–d) + `⟦mir_v⟧ = ⟦mir_p⟧` | debug builds + `cargo test` |
| frame / layout | `⟦mir_p⟧ = ⟦mir_final⟧` | interpreter with sp/frame semantics + verifier | `cargo test` |
| emit | determinism; assembler acceptance | md5 seal; `as` | box |
| whole compiler | CONFIRMS, never discovers | opt-parity (HIR passes off vs on), torture, csmith300, yarpgen300, cts, musl | box |

**Cost-square exact by construction:** one `MInst` = one machine instruction after `frame/layout`, so
`cost(f) = |MIR_final(f)|` needs no separate model. Δinsn of any transform is computed on MIR before
emitting anything: **predict → apply → confirm** becomes cheap. (The lesson of lever ㉕.2 — a build
without a prior prediction — is fixed structurally, not by discipline.)

---

---

# Part H — the decision log

### §14 Decision log (settled; reopen only with a stated reason)

| decision | choice | why |
|---|---|---|
| frontend | keep as input | failure is entirely below AST; parser is an independent proven artifact |
| SLP layer (R5.3) | a MIR pass, NOT a HIR pass with `Ty::V128` | the vector data path already exists one layer down — `Width::Q`, `MemOp::Q`, the FPR class, 16-byte slots, all carried since `long double` — so what was actually missing was arithmetic. HIR would have needed a new type in every exhaustive `match Ty` in the frontend half plus a lane semantics in `hir::interp`, for a type the frontend can never produce |
| scheduler position (R5.4) | post-allocation but PRE-frame-lowering | post-RA so no schedule can create a live range; pre-frame so it never sees a prologue, an epilogue, or an sp-writeback address. Learned the hard way: the first cut ran after `frame_fold` and the box returned corpus-wide SIGSEGV, because two memory READS are unordered and one of them was the epilogue's sp-restoring load |
| TBAA opt-out granularity (R5.2) | whole translation unit | `may_alias` and `optimize("-fno-strict-aliasing")` set one flag for the unit. The finer answer is a bit per `TypeId` beside `vol`; both gcc torture cases put the pun in `main`, where per-type buys nothing. Conservative direction: costs an optimization, never an answer |
| SSA representation | block parameters (HIR and MIR) | explicit edges, trivial destruction, one model |
| HIR types | closed `Ty` enum, signedness in opcodes | passes independent of TyTab; closed semantics |
| allocation | on SSA, Braun-Hack spill first, chordal greedy color | polynomial + optimal for the spill set; splitting free |
| SSA reconstruction after spilling | never built; R4.1 got the effect without it | carrying a copy only where every predecessor holds it IS dominance, so the copy's def dominates its uses and no φ/parameter is needed. §13n planned reconstruction; the measurement made it unnecessary |
| coalescing | biased coloring first; Boissinot merge only on measured residual | never breaks the pressure guarantee |
| the edge's parallel copy (2026-08-27) | its locations are registers **AND** spill slots | `evict_params` puts a slot on the edge, so read-before-write has to hold across the register/slot boundary; the register-only reading let a pointer rotation overwrite a slot another argument on the same edge still had to read, and the zcc-built sqlite CLI SIGSEGV'd on every two-table join. See THEORY A7 |
| `regalloc::verify` (2026-08-27) | runs on EVERY compile, post-allocation and pre-frame-lowering | it was called only from unit tests, so its obligations held on fixtures and were never asked of real input. Pre-frame-lowering is not a convenience: obligation (b) is stated in the `Spill`/`Reload` vocabulary and `ldst_pair` spends it, so the same check after `finish` reports a false `reload of unstored slot` |
| finding allocator defects | generated SHAPE families, not more corpus | the sqlite segfault survived 20,000 generated programs, torture 1694 and opt-parity 1552. No generator writes a pointer rotation under enough pressure to evict a parameter; 40 programs written to that shape found it in one run. A corpus corroborates, it does not discover |
| call-crossing values | modeled as `Clobber` constraints, no special logic | falls out of constraint-respecting greedy coloring |
| flags | k=1 register class | compare-elim = GVN; conflicts = liveness |
| scheduler | none | gcc -O1 has none; YAGNI |
| middle target-independent IR | deferred | one target |
| migration | big-bang on `mir-rearch`; `rc3` is the fallback | user directive; incremental rejected |
| scratch registers | x16, x17 (GPR), v31 (FPR) reserved | AAPCS64 IP0/IP1; parallel-copy cycle breaking |
| R0/R1 local storage | every C local stays in ONE frame slot (memory); promotion is R2.2 SROA+mem2reg | the parser reports `Var(off)`, not variable identity — two locals in disjoint scopes may share an offset, so promotion at build time would rest on an unproven disambiguation. Consequence: R0/R1 exercise the allocator on expression temporaries only, and the R1 allocator KPI is re-measured at R2.2 (noted in §12 R1) |

---
