# `arm64_elf.md` — emitting GOOD AArch64, not merely correct AArch64

> **Scope.** This is the TARGET-KNOWLEDGE file for the backend — `isel/`, `mir/`,
> `regalloc/`, `emit.rs`. It is deliberately NOT a theorem file: `THEORY.md` says
> which theorem a pass realizes and `SEMANTICS.md` says what an instruction
> means. This one answers a different question, the one you actually need in
> front of an `.s` file:
>
> > **What does this shape cost on the machine, what does `gcc -O1` emit for it,
> > and why is its version faster than mine?**
>
> Every row below was found the same way: diff zcc's `.s` against gcc's for one
> program, hand-edit ONE instruction, link both, run both, and time them. No row
> is here on reasoning alone. Where a number has no vendor document behind it, it
> is cited into `MECHANISM.md` Part F, because Apple publishes no optimization guide for
> this core and inventing a latency table would be inventing provenance.
>
> The predecessor of this file was `src/codegen/arm64_elf.md` on the pre-rearch
> tree. That one catalogued which ACADEMIC ALGORITHM each stage ran. This one
> catalogues the MACHINE. Both matter; they are different files on purpose.

---

## §1 — The one rule

```
Fewest instructions is not fastest code.
Judge a codegen row by the longest dependence chain it leaves.
```

That is **Law 3c** in `CLAUDE.md`, and the reason it had to become a law is on
this page: zcc emits FEWER instructions than gcc -O1 in seven of the eighteen
timed programs of the taxonomy suite and is not faster in any of them.

`cost = |MIR|` — the project's cost model — is exact for SIZE and blind to TIME
**by construction**, because one `MInst` is one machine instruction. So it scores
the two matmul loops below identically at seven instructions, and one of them
takes 64% longer. Every row in §3 is a place where that blindness cost real time.

---

## §2 — What is actually known about this core

Apple publishes no Software Optimization Guide for the M1, so there is no latency
table to transcribe. What follows is only what has been MEASURED, with the
program that measured it. Anything not listed here is **not known** — do not
reason from a number that is not on this list.

**AND THERE ARE NOW TWO CORES, WHICH IS THE POINT OF THE SECTION.** Every row
below was taken on an Apple M1 Pro; on 2026-08-29 the same `latency.sh` ran on a
Graviton4 (Neoverse V2) and two rows do not survive the crossing. The section's
own standing caution — *a measured fact is evidence about the measuring machine
first* — is no longer a caution, it is a measurement (`MEASURED M46`).

| form, in units of a dependent `add` | M1 Pro | Neoverse V2 |
|---|---|---|
| `add xN, xN, wM, sxtw` | 2.00 | 2.00 |
| `mul` | ≈1 ("about an `add`") | **2.00** |
| `udiv` / `sdiv` | ≈2 (inferred, `M25`) | **4.98** |
| `load` from L1 | — | 3.98 |
| `csel`, `sxtw`, `ubfx`, `rev`, `lsl` | — | 1.00 |
| `madd`, accumulator operand | — | 1.00 |
| `madd`, multiplicand operand | — | 2.00 |

**The divider is the row that cost a decision.** `M25` removed
Granlund–Montgomery division-by-constant because "the divider on this core is not
slow… the folklore is a Cortex-A53-era fact". That sentence is true of the M1 Pro
and false of Neoverse V2 by a factor of five, and `a2_udiv_mod` runs at **4.50x**
gcc -O1 there against 1.12x on the M1. **A row deleted on one core's evidence is a
row deleted on one core's evidence.**

| fact | measured | entry |
|---|---|---|
| `add xN, xN, wM, sxtw` costs **2 cycles** where `add xN, xN, xM` costs 1 | j3_prefix_sum: recurrence bound predicted 2.0 → 1.0, measured **1.940 → 1.000** (3% error) | `MEASURED M1` |
| A `mul` at the head of a chain ending in a strided load costs far more than an `add` there — **1.638× on the whole program**, at identical instruction count | matmul: `madd` address 113 ms vs pointer walk 69 ms, both 7-instruction loops | `MEASURED M9` |
| A `mul` sitting in a basic block, off any chain, costs about an `add` | the old category-(a) verdict, still true in that narrow form | `THEORY.md` A7b (corrected) |
| An indirect branch on a data-dependent index loses to a compare chain even while emitting MORE instructions | d1_switch, 8 cases: table **15 ms** vs tree **12 ms**, tree emits 12 more instructions | `MEASURED M4` |
| Case COUNT does not predict table-vs-tree: a synthetic sweep at 4, 6, 8, 12, 16, 24, 32 cases put them within 1 ms of each other everywhere | `MEASURED M4` | |
| A post-index writeback on a UNIT-stride load does not pay on this core | j2_histogram: identical instruction count, **13% slower** | `MEASURED M2` |

**The standing caution.** Every number was taken on Apple M1 Pro cores under
Docker while the notional target is generic AArch64-Linux. A measured fact is
evidence about the measuring machine first.

---

## §3 — The shapes

Each row: the C, what gcc -O1 emits, what zcc emitted, why gcc's is faster, and
where the fix lives. These are the jewels — the things that were expensive to
learn and cheap to forget.

### 3.1 A loop-invariant constant belongs in the preheader

```c
for (j = 0; j < 100000; j++) a = a * 1103515245UL + 12345UL;
```

```
zcc   movz x10,#20077 ; movk x10,#16838,lsl 16   ← rebuilt EVERY iteration
      movz x10,#12345                             ← and this
      ...
      movz x10,#34464 ; movk x10,#1,lsl 16        ← and the loop bound
      cmp  x1,x10
gcc   (all three materialized once, before the loop)
```

Five instructions per iteration for values that never change. HIR's LICM cannot
see them: until `isel` runs they are `Operand::Imm`, not instructions, so there
is nothing to hoist. `const_share` value-numbers them within a dominance scope
but does not MOVE them. **The hoist has to happen on MIR, after isel and before
regalloc** — the spiller can rematerialize a `MovImm` instead of reloading it, so
hoisting is monotone under pressure. Row **R4.6**.

### 3.2 `a*K + C` is ONE instruction, and a literal multiplier is not an excuse

```
zcc   mul  x9,x9,x10 ; add x9,x9,x11          2 instructions, ~5 cycle chain
gcc   madd x3,x3,x11,x10                      1 instruction, ~4
```

The multiply-accumulate munch row read the multiply's operands as VALUES and
refused itself on an `Imm`. The refusal bought nothing: the literal has to reach
a register before `mul` can read it either way, so the register was already paid
for and the `add` left standing was one the ISA never asked for. `AluFold::Mul3`
carries `Operand`s now.

`loops.c` 24 → 22 instructions per iteration, **1.245× → 0.905×**, because `a` is
the recurrence every other value in that loop hangs off. Commit `a98993a`,
§17 row 23.

### 3.3 A ROW-strided address walks a pointer; it is never rebuilt with a multiply

```c
for (k = 0; k < N; k++) s += A[i][k] * B[k][j];   /* B walks a 1920-byte row */
```

```
zcc   madd x12,x11,x4,x1 ; ldr x12,[x12,x9]      7 insns/iter   113 ms
gcc   ldr x3,[x0]        ; add x0,x0,1920        6 insns/iter    69 ms
E1    ldr x12,[x14]      ; add x14,x14,#1920     7 insns/iter    69 ms
```

**E1 is the finding.** It keeps zcc's own counter and its own instruction count
and only removes the multiply — and it is already at gcc's time. gcc's other two
tricks (post-index on the `A` load, dropping the counter for a pointer-limit
test) buy exactly zero. The whole 64% was one multiply standing in front of a
strided load.

Why the pass that existed did not fire — two separate defects, both worth
remembering because both are shapes that recur:

* **The default-off verdict was over-broad.** `MEASURED M2` was taken on a
  UNIT-stride address, and generalized to "pointer IVs do not pay here". One
  verdict was covering two different theorems (§4.1 says why they differ).
* **`scev::AddRec` holds ONE symbolic base.** `&B + k*1920 + j*8` has TWO
  loop-invariant symbolic terms around one recurrence, so the address refused to
  evaluate at all and the load kept its multiply even with the pass forced on.
  `iv::affine` splits the top-level `add` and asks again.

Commit `35e87ef`, `MEASURED M9`, `MECHANISM.md` Part F.

### 3.4 `inv + k` should BE the counter

```c
for (k = 0; k < n; k++) s += (i*j + k) & 31;
```

```
zcc   add w7,w5,w4 ; and w7,w7,#31 ; add x6,x6,x7 ; add w4,w4,#1 ; cmp w4,w0 ; b.lt
gcc   and x2,x1,31 ; add x0,x0,x2  ; add w1,w1,1 ; cmp w1,w3    ; bne
```

gcc runs `i*j + k` AS the induction variable — it starts at `i*j`, the exit bound
becomes `n + i*j`, computed once — so the add that rebuilds the value every
iteration is gone and the mask reads its input a cycle earlier. Six instructions
against five; **14 ms against 10**.

**The parameter has to be 64 bits, and that is not a style choice.**
`SEMANTICS.md` defines signed overflow as WRAPPING, so gcc's "signed overflow is
undefined" argument is unavailable and the rewrite must be exact under wrapping.
In I32 it is not — shifting `k <s bound` by `inv` flips at the sign boundary, and
the corner (`inv + bound - 1 == INT_MAX`) exits on the FIRST test instead of the
last. In I64 it is exact with no side condition, because both terms are
32-bit-ranged and the sum needs 33 bits. `iv::substitute`.

### 3.5 The truncation a wide IV leaves must disappear into its consumer

Having done 3.4, the value the loop reads is `trunc(q)`, and that came out as a
real `mov w7, w5` — handing back exactly the instruction 3.4 had saved.

```
ext(trunc(x) & m) = x & m        for 0 <= m <= INT_MAX
```

The mask clears every bit the truncation or the widening could have touched, so
both are no-ops and the whole sandwich is one 64-bit `and`: `and x7, x5, #31`.
`fold::narrow_mask`. Not special to that pass — any `(int)(long_expr) & MASK`
promoted back to `long` has this shape.

### 3.6 Put the extension in the LOAD, not in the ALU operand

```
zcc   ldr w1,[x2] ; add x0,x0,w1,sxtw       the add is 2 cycles (MEASURED M1)
gcc   ldrsw x1,[x2] ; add x0,x0,x1          the add is 1
```

Same instruction count. When the value feeds a loop-carried chain the difference
is the whole recurrence bound. j3_prefix_sum **1.940 → 1.000**. Row R4.7.

### 3.7 Switch: the table crossover is ~24 arms, and a balanced tree never wins

zcc emitted `adrp`/`ldr`/`br x16` from FOUR cases up — 4% shorter than gcc's
compare chain and 50% slower. `MIN_CASES = 4` was taste, not measurement.

Swept properly (4…64 arms, pseudorandom index and repeating index, both agree):
the **chain** wins to ~20 arms, the **table** from ~24. `MIN_CASES` is 24 now,
and d1_switch went **1.500 → 1.200**.

**The balanced search TREE was built and refuted — it loses at every size from 4
to 64.** At 16 arms: chain 62 ms, table 65, tree 84. It asks strictly fewer
questions (four against the chain's seven on d1) and takes more time. The
chain's tests FALL THROUGH; the tree spends a taken branch per level and
scatters the arms across the function. This is Law 3c pointing the other way,
and it is the reason the law says "longest dependence chain" and not "fewest
questions" — a not-taken branch is nearly free and a taken one is not.

Removed rather than kept behind a flag: no measured size wants it.

What is LEFT of d1's gap is not the dispatch at all. gcc flattens the tiny arms:

```
gcc   tbnz x1, 2, .L4                              range split in one instruction
      sub x3,x0,#2 ; cmp w2,2 ; csinc x0,x3,x0,eq  case 2 + default, NO branch
      add x0,x0,7  ; cmp w2,4 ; csel  x0,x0,x3,eq  case 4 + default, NO branch
```

That is if-conversion of the ARMS. `MEASURED M4`.

### 3.8 Down-count when the bound needs a register

```
zcc   add x1,x1,#1 ; movz x10,#34464 ; movk x10,#1,lsl 16 ; cmp x1,x10 ; b.lo    5
gcc   subs x7,x7,#1 ; bne                                                        2
```

`subs` sets the flags the branch needs, so the compare disappears, and counting
DOWN to zero removes the bound register entirely. Only worth it when the bound
does not fit an immediate — for `j >= 0` zcc's `tbz` is already one instruction
and there is nothing to win (`MEASURED M2`, shape 2, category (a)).

### 3.9 `mul` by a constant, on a recurrence, is shifts and adds

```
zcc   mul x5,x5,x3                    x3 holds 3; ~4 cycle chain
gcc   add x2,x2,x2,lsl 1              ~2
```

Same instruction count, shorter chain, and it frees the register holding the
constant. `fold.rs` currently reduces only `k & (k-1) == 0` — powers of two. §17
row 24 claims "shifts+adds" and is truncated to that. Cap any chain at 3 terms so
it can never grow the count.

### 3.10 A unit-stride pointer walk does NOT pay here

The mirror of 3.3, and the reason 3.3 needed its own gate. `p[i]` rides the
scaled index for free, so replacing it with a walking pointer trades a free
addressing mode for an explicit `add`, and the add only vanishes if `auto_inc`
folds it into a post-index. Measured: j2_histogram identical count, **13%
slower**. `MEASURED M2`. Off by default, behind `ZCC_IV`.

---

## §4 — The addressing modes, and exactly what they reach

### 4.1 The scaled index scales by the ACCESS SIZE and by nothing else

```
ldr Xt, [Xn, Xm, lsl #3]        ✓ 8-byte access, stride 8
ldr Xt, [Xn, Xm, lsl #4]        ✗ there is no such form for an 8-byte access
```

DDI 0487 C6.2.130. This single fact separates §3.3 from §3.10 and is the reason
one verdict could not cover both:

| stride | how the address is built | is a pointer worth it? |
|---|---|---|
| == access size | scaled index, FREE | no — `MEASURED M2` |
| a power of two | `fold::canon` makes it `k<<n`, isel folds `add x, base, x, lsl #n` — one shifted `add` | no |
| anything else (1920) | an honest `mul`, every iteration | **yes** — `MEASURED M9` |

`iv.rs` tests exactly that, and the residual print `ZCC_IVDBG=1` names which
branch declined each in-loop load.

### 4.2 Post-index needs the unscaled signed 9-bit offset

`ldr x, [p], #k` requires `-256 <= k <= 255`. A 1920-byte stride cannot use it —
which is fine, because §3.3 showed the writeback was never where the money was.

`STR Xt, [Xn], #imm` with `t == n` is CONSTRAINED UNPREDICTABLE, which is why
the pointer-IV row is loads-only.

---

## §5 — Traps

* **Fewer questions, more time.** The balanced switch tree asked four questions
  where the chain asked seven and lost at every size measured. "Shorter" and
  "fewer decisions" are both proxies; only the clock is the arbiter. A not-taken
  branch is nearly free, a taken one is not, and no instruction-count model of
  any kind can see that difference.
* **A constant nobody measured.** `MIN_CASES = 4` sat in `isel/lower.rs` from
  R3.3 and cost d1_switch 50%. Article E asks of every resource constant: "is
  this the spec's number, or my convenience's number?" — this one was neither, it
  was a guess. Sweeping it took under an hour.
* **A number that looked free.** A `mul` "costs about an `add` on an
  out-of-order core" was written into `THEORY.md` as a category-(a) closure, and
  it stood for weeks. It is true of a `mul` in a basic block and false of a `mul`
  at the head of a chain. One sentence of plausible micro-architecture reasoning
  closed a row that was worth 64% on a benchmark.
* **A verdict that was broader than its experiment.** `MEASURED M2` measured
  unit stride and was written as if it measured pointer IVs. Always write the
  entry as narrowly as the experiment actually was.
* **The instrument reading zero.** geo40's INSN geomean is IDENTICAL with the
  row of §3.3 on and off — that suite has no site for it. A geomean that does not
  move is not evidence the row is worthless; check whether it fires at all first.
* **`cts` and `ext` reporting `0 pass, 0 fail`.** That is a suite that was not
  found, not a suite that passed.
* **Timing under 30 ms.** ±1 ms of a 4 ms run is ±25%. Read the INSN column for
  short programs and the clock only for long ones.

---

## §6 — How to establish a codegen claim

The method that settled both of today's rows, and the reason neither needed a
speculative compiler change first:

1. Compile the program with zcc `-S` and with `gcc -O1 -S`. Diff the hot loop.
2. **Hand-edit ONE instruction** in zcc's `.s` into gcc's shape. Nothing else —
   same registers, same block structure, same everything.
3. Assemble and link both, plus gcc's, in the box. **Check they print the same
   thing** before looking at any time.
4. Time best-of-N. If the hand-edit closes the gap, the claim is established and
   the pass is now a known-value implementation task. If it does not, the theory
   was wrong and no compiler code was written for it.

This is Law 3's "certify at the middle" applied to the machine: the `.s`
CONFIRMS, and here it also DISCOVERS, but it discovers with a controlled
experiment rather than with a patch and a suite run.

### 6.1 A CEILING IS NOT A COUNT

The most expensive habit this project has recorded, and it cost three rows in
one day (2026-08-27). Each time, a plan carried a number that looked like an
opportunity, a mechanism was built on it, and the number turned out to count ONE
of the conditions the mechanism needed:

| the number in the plan | what it actually counted | what it was worth |
|---|---|---|
| 8,696 hints refused with a "locally evictable" occupant | occupants whose LAST USE is in this block — silent about whether they were also live-IN, and a live-in range reaches back through blocks the colourer keeps no record of | **7 firings in all of sqlite** |
| 3,020 unpaired accesses "a layout could pair" | pairs considered one at a time, silent about the fact that making one adjacent separates another — `ldp`/`stp` consume RUNS | **negative**; two orderings both lost to the shipped one |
| 5,130 "missing `ldp`/`stp`" against gcc | gcc's PAIRS, not zcc's removable INSTRUCTIONS — gcc pairs more because it spills more. Counted as instructions, zcc emits 22,070 frame instructions to gcc's 24,720 and is **2,650 ahead** | **~1,009** |

The test that catches all three takes thirty seconds: **name a case inside the
count where the mechanism would still refuse.** If you can, it is not a ceiling.

A ceiling is a hand-edit — the shape built by hand in the `.s`, linked, checked
for the same output, and timed. Everything else is a hypothesis wearing a
number. On the same day the hand-edit refuted two rows that had been carried as
plans for weeks (a loop-invariant constant hoist and small-struct SROA, both
worth **zero** on the program they were written for) and found the one that was
worth 2.2× — each in about four minutes.

### 6.2 gcc's OUTPUT IS AN EXISTENCE PROOF, NOT A TEMPLATE

The fact underneath every row in §7 is not a theorem: it is that **gcc's binary
runs faster on the same source.** That is evidence a better shape exists, and
its assembly is a witness naming the shape. The work is then to (a) enumerate
the differences, (b) price each ONE AT A TIME by hand-edit, and (c) derive the
transform ourselves and ship it with its commuting square.

Copying gcc is not the goal and would be a ceiling of its own: on
`e3_struct_byval` the hand-edited zcc shape ran **3,332 µs against gcc's
3,889** — 14% faster — because zcc's other choices (`msub`, folding `k & 255`
into `uxtb`) were already better than gcc's. gcc -O1 is a LOWER BOUND on what is
reachable, not the target.

And re-price after every row: the parameter copy in that same program was worth
**nothing** while the call stood, and **35%** once the call was inlined.
Removing one cost promotes the next.

---

## §7 — The big-win ledger

**The standing rule: any change that takes a program from 1.3–1.5× to parity or
below gets a row here, with the instruction that changed.** Those are the ones
worth learning from — a 0.5% lever teaches nothing, and a 40% one teaches what
the machine actually charges for. Numbers are static instruction counts on
`sqlite3.c` unless the row says otherwise; ratios are against `gcc -O1`.

| what changed | measured | the lesson |
|---|---|---|
| **SROA + mem2reg + the Braun-Hack spiller** (R2.2) — every local stopped being a memory cell | sqlite **473,253 → 322,606**, −150,647; **2.997× → 2.043×**; `add` 133,264 → 35,357; geo40 INSN 2.5168 → 1.5244 | The single largest win in the project. Nothing in the backend matters while every local is a load-store pair. **And it was a REGRESSION first — 571,648** — until one slot per SSA WEB landed: a spilled block parameter had been copying between slots on every edge, 110,000 stores in `sqlite3VdbeExec` alone |
| **Loop rotation turned ON**, once the loop-carried copy was coalesced (R2.4 → §13f) | EXEC geomean **1.6232 → 1.4276**, INSN 1.3043 → **1.2437**, sqlite 240,774 → 236,886, branches 19,151 → **14,711**; h1_popcount **0.960**, g3_reverse 1.000 | Rotation had shipped OFF because it paid for itself exactly — the branch it removed was the branch the split block added back. The fix was six lines in `regalloc/color.rs` freeing a dying operand before placing the destination. **A pass measured worthless can be one line away from the biggest exec win of its milestone** |
| **§17 isel rows, verified one at a time** (R4.7) | j3_prefix_sum **1.940 → 1.000** · d4_goto **1.400 → 1.000** · i1_global_acc **1.333 → 0.750** · d2 2.111 → 1.500; sqlite 217,160 → 212,066; EXEC 1.3386 → **1.2044** | Four programs from the 1.3–1.9 band to parity or better, and the j3 row was PREDICTED from the latency table to 3% before the build. This is where `MEASURED M1` came from |
| **ABI-boundary truncation is a no-op** (R4.2) | sqlite **232,214 → 218,776**, −13,438 | A truncating copy into an argument register, out of a result register, or before `ret` writes bits the reader never looks at |
| **Booleans stay flags + memory crosses one edge** (R4.5 + R4.9) | sqlite **212,066 → 199,979**, −12,087; EXEC 1.2044 → 1.1490; j5 2.857 → 1.940 | zcc branched on materialized booleans where gcc branched on flags: `cmp; movz; csel; cbnz` per iteration against `cmp; ble` |
| **Parallel-copy self-source + frame slots + one epilogue** (R4.3 + R4.4) | sqlite **201,727 → 189,279**, −12,448; EXEC 1.0777 → 1.0357 | |
| **`madd`/`msub` accept a literal multiplier** (§3.2, `a98993a`) | `loops.c` 24 → 22 insns/iter, **1.245× → 0.905×** | The row refused itself on an `Imm` for no gain — the literal needed a register either way |
| **Row-strided load walks a pointer** (§3.3, `35e87ef`) | `matmul` **1.638× → 1.000×**, instruction count UNCHANGED at 7 | The purest instance of Law 3c on record. One multiply, in front of one strided load, 64% |
| **`inv + k` becomes the counter** (§3.4) | `d2_nested_loops` 6 → 5 insns/iter, **1.400 → 1.000** | Needed `fold::narrow_mask` (§3.5) or the saving came straight back as a `mov w,w` |
| **The frame adjust becomes an ordinary instruction** and folds into the save pair (R4.15) | sqlite **186,705 → 183,253**, −3,452; ≈3,300 `sub sp`/`add sp` absorbed by pre/post-index | |
| **Belady's distance is measured along the TRACE** (`MECHANISM.md` Part D S1, `eeb15b2`) | `nestjoin.c` **8 ms → 1 ms = gcc**; 6 of the inner loop's 11 instructions were frame traffic and 0 remain; sqlite −792; `VdbeExec` frame slots 244 → 200 | `linear_positions` numbers instructions in reverse postorder, and **a back edge runs backwards in that order** — so `next_use` from a latch found nothing and answered `usize::MAX`, "never used again". The loop index, the loop pointer and the accumulator were therefore ranked as the BEST possible eviction candidates while 24 values used only after the loop kept their registers. Belady's rule is a theorem about a trace; measuring its distance in text order inverts it exactly where it matters |
| **A switch arm with edge arguments gets a trampoline** (`5ed5648`) | **sqlite SQL geomean 1.651 → 1.159**, worst phase 2.8× → 1.30×, `p01_insert` 1.988× → 1.279×; sqlite −517 instructions | The jump-table row refused any switch whose arms carry edge arguments — a table entry is an address with nowhere to put the copies — and its own comment deferred the fix. sqlite's `sqlite3VdbeExec` dispatches 196 opcodes with EVERY arm carrying arguments, so the hottest dispatch in the program was a **183-deep linear compare chain** walked ~1.4M times, against gcc's one indirect branch. The arm gets its own block, the edge goes there, the table points at that: one `b` against ninety compares. **The largest single win of the campaign, and it was invisible to instruction counts** — the chain is 380 instructions of a 174,000-instruction file |
| **The if-conversion join may have other predecessors** (`m1_resp_parse`, ifconv) | `m1_resp_parse` **1.63× → 1.442×** (93,920 → 82,173 µs); sqlite −397; suite EXEC 1.0282 → **1.0190** | `ifconv` required the join to have EXACTLY TWO predecessors, which refused the commonest shape in real code: a small `if` inside a `switch` arm joins at the arm's `break` — the LOOP LATCH, shared by every arm — so the join has one predecessor per arm. `convert` never read the count. One condition, `!= 2` to `< 2`, and eight `csel`s appear in one parser. The branch it removes (`if (--want == 0) st = S_CR;`, run for every payload byte) is data-dependent and mispredicts, which is why gcc if-converts at -O1 though the instruction count is a wash |
| **A composite parameter no longer blocks inlining, and its copy is elided** (`820cc22` + `af19bfd`) | `e3_struct_byval` **1.93× → 1.045×** (7,399 → 3,990 µs); the hot loop goes from 4 stores + `ldp`/`stp` + 4 loads + a `bl` to **no memory access at all**; suite INSN 1.0772 → 1.0725 | Two one-line-shaped causes behind a 2× program. `args_match` accepted only scalar parameters, so **every** by-value struct call in the language was un-inlinable. Then the callee's C 6.9.1p9 parameter copy — which `SROA` marks as an ESCAPE, disqualifying both objects from promotion — is provably dead when the callee writes no memory, and removing it let mem2reg promote the whole struct. Neither cause was visible from instruction counts; both came from diffing gcc's assembly || **The inline-copy bound was re-derived on the TIME axis** (`MEASURED M40`, `INLINE_COPY_MAX` 32 → 128) | `v3_struct_copy` **1.574× → 1.099×** (2,227 → 1,554 µs), dynamic instructions 2.410 → **1.420**; sqlite +131 static instructions, +0.075% | `M14` swept the bound against sqlite's STATIC count and found a clean minimum at 32, which it is — a call is four instructions whatever the length. What the static count cannot see is what the call then EXECUTES: measured against always-inline at ten sizes, `bl memcpy` costs a near-constant **12 extra dynamic instructions and never wins, out to 512 bytes**. The program was calling `memcpy` four times per iteration to assign a 64- and a 96-byte struct. **A constant derived on the size axis is not a constant on the time axis**, and `M38`'s corr(INSN, EXEC) = 0.196 is the general statement of why |

**What the ledger says when you read it as a whole.** The three largest entries
are not peepholes — they are a data-structure decision (locals in registers), a
pass that was one bug away from working, and eight ISA rows verified
individually. None of them was found by staring at code. Every one was found by
measuring, and two of them were found only after the first measurement said the
opposite.

---

## §8 — Where each fact lives in `src/`

| kind of target knowledge | file |
|---|---|
| register file, encodability, instruction forms, mnemonics | `mir/isa.rs` |
| AAPCS64 argument/return classification | `isel/abi.rs` |
| sections, relocations, symbol syntax | `emit.rs` |
| addressing-mode and fusion selection | `isel/lower.rs::munch` |
| machine passes (`ext`, `cmpelim`, `const_share`, `autoinc`, `ldstp`, `layout`) | `mir/pass/` |
| loop shapes (`licm`, `iv`, `rotate`, `scev`, `fold`) | `hir/pass/` |
| the measured facts themselves | `MECHANISM.md` Part F |

Nothing above `isel` may name a machine register. HIR is target-independent by
construction; MIR is AArch64 by design. A second target adds a second MIR and a
second isel, never a conditional in these files.

---

### §17 arm64 leverage table — the isel exhaustion checklist (Law-4 applied to instruction selection)

The A64 ISA (ARM DDI 0487) is the Side-II ultimate fact for isel. The pattern table (§6) is **exhausted**
only when every ISA feature below that removes an instruction has a pattern row with its battery proof,
and the corpus excess histogram shows the corresponding mnemonic at gcc parity. Each row = one isel
lever; ✔ marks features gcc -O1 uses routinely on sqlite (the ones the old backend measurably lacked).

| feature | ISA form | HIR tree it absorbs | saves |
|---|---|---|---|
| ✔ shifted-register operands | `add/sub/and/orr/eor/cmp x, x, x, lsl/lsr/asr #n` | `op(a, shl(b,n))` | the shift |
| ✔ extended-register operands | `add/sub/cmp x, x, w, sxtw/uxtw/sxtb… #n` | `op(a, sext/zext(b))`, with shift | the extend (+shift) |
| ✔ register-offset addressing | `ldr/str [x, x, lsl #k]`, `[x, w, sxtw/uxtw #k]` | `load(add(b, shl(i,k)))`, `i` I32 | `lsl` + `add` (+extend) |
| ✔ immediate addressing | `[x, #imm12·size]`, `[x, #simm9]` | `load(add(b, c))` | the `add` |
| ✔ pre/post-index | `ldr x, [p], #k` / `[p, #k]!` | load + pointer bump in loops | the `add` |
| ✔ load/store pair | `ldp/stp x, x, [base, #imm7·8]` | two adjacent accesses (struct fields, spills, prologue) | one mem op |
| ✔ extending loads | `ldrb/ldrh/ldrsb/ldrsh/ldrsw` | `sext/zext(load narrow)` | the extend |
| ✔ 32-bit ops zero-extend for free | `w`-form ALU | `zext32(op32)` | `uxtw` |
| ✔ zero register | `xzr/wzr` as operand or dest | `iconst 0`, discarded results, `cmp x, #0` via `cmp`/`cbz` | a `mov` |
| ✔ flag-setting ALU | `adds/subs/ands/adcs/sbcs`, `cmn`, `tst` | `op` + `cmp op 0` / `cmp a, -b` / `and`+`cmp` | the `cmp` |
| ✔ compare-and-branch | `cbz/cbnz`, `tbz/tbnz` | `br(icmp eq/ne x 0)`, sign-bit / single-bit tests | the `cmp` |
| ✔ conditional select family | `csel/csinc/csinv/csneg/cset/csetm/cinc/cinv/cneg` | `select`, `c?a+1:a`, `c?-a:a`, `c?1:0`, `c?~a:a`, min/max, abs | branches |
| ✔ conditional compare chains | `ccmp/ccmn` | `&&`/`||` of relations feeding one branch/select | branch + extra `cmp` |
| ✔ multiply-accumulate | `madd/msub/mneg`, `smull/umull/smaddl/umaddl/smulh/umulh` | `add(mul)`, `sub(mul)`, `neg(mul)`, widened products | the `add`/`sext` — **residual taken 2026-08-26**: the row read the multiply's operands as VALUES and refused itself on an `Imm`, so `a*K + C` with literal `K` kept a separate `add`. A literal multiplier has to reach a register before `mul` can read it either way, so the register was already paid for; category (b), now closed (`AluFold::Mul3` carries `Operand`s). `tests/bench/loops.c` 24 → 22 insns/iteration, **1.245× → 0.905× gcc-O1** |
| ✔ mul/div by constant | shifts+adds, `umulh/smulh` magic (Granlund & Montgomery 1994), `lsr` for pow2 | `mul/udiv/sdiv/urem/srem` by const | the `mul`/`div` |
| ✔ bit-field ops | `ubfx/sbfx/ubfiz/sbfiz/bfi/bfxil`, `extr` (funnel shift), `rbit/clz/cls/rev/rev16/rev32` | `and(lshr)`, `shl(and)`, insert masks, rotates, `__builtin_clz/bswap` | 1–3 ops each |
| ✔ inverted-operand logic | `bic/orn/eon` | `and(a, not b)`, `or(a, not b)`, `xor(a, not b)` | the `mvn` |
| ✔ logical immediates | bitmask-imm encoding (`and/orr/eor/tst #imm`) | masks, `x & ~0x7`, alignment ops | `mov` of the mask |
| ✔ constant materialization | `movz/movk/movn`, `orr #logimm`, `adr`, `ldr literal`, `fmov #imm8` | any constant | 1–3 `mov`s |
| ✔ symbol addressing | `adrp` + `:lo12:` folded into `ldr/str/add`, `:got:` | globals | one `add` |
| ✔ frame | omit frame pointer (x29 allocatable), single `sub sp`, `stp x29,x30,[sp,#-N]!` pre-index prologue | prologue/epilogue | 1–2 insns per function |
| FP | `fmadd/fmsub/fnmadd/fnmsub`, `fmin/fmax/fminnm`, `fcsel`, `fabs/fneg/fsqrt`, `scvtf/ucvtf/fcvtzs/fcvtzu` with int operands, `fmov` reg-reg free width switch | FP trees | 1 each |
| register file | 31 GPR + 32 FPR (§5.1 full table) | — | the spill floor → ~0 for most functions |
| LSE atomics (armv8.1+) | `ldadd/swp/cas` | `__sync_*` | LL/SC loops — only under `-march`, off by default |
| NEON | `ld1/st1`, vector ALU, `addp`, `cnt`, `uaddlv` | §16 row 13 (vectorization), `__builtin_popcount`, memcpy/memset inline | many — the last shelf |

Method for exhaustion: (1) after R3, run `corpus25.sh`; (2) for each mnemonic where zcc > gcc, diff a
sample of functions and name the missing row above; (3) add the pattern + battery row; (4) re-measure.
Discovery aid: superoptimization / equality saturation over the pattern table (§16 row 16) finds rows
a human misses. The table is complete when every remaining excess is category (a) fundamental.

---
