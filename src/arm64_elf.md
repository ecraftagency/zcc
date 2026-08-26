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
> is cited into `MEASURED.md`, because Apple publishes no optimization guide for
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

Commit `35e87ef`, `MEASURED M9`, REARCH §13q.

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

### 3.7 Switch: predictability decides, not case count

zcc emits `adrp`/`ldr`/`br x16` for 8 cases; gcc emits a compare chain of direct
conditional branches. zcc's is 4% SHORTER and 50% slower, because an indirect
branch on a data-dependent index mispredicts and a compare chain on a repeating
pattern does not. `MEASURED M4` recorded the symptom and left it unsettled
because the case COUNT is not the variable. Law 3c names the variable:
**branch predictability**, which `cost = |MIR|` cannot see.

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
| the measured facts themselves | `MEASURED.md` |

Nothing above `isel` may name a machine register. HIR is target-independent by
construction; MIR is AArch64 by design. A second target adds a second MIR and a
second isel, never a conditional in these files.
