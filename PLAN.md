# PLAN — the one grind in progress

**THIS FILE IS ALLOWED TO BE WRONG.** Every line is a HYPOTHESIS about a
compiler that does not exist yet. Nothing here may be cited from `src/`, and
nothing here is evidence of anything.

**The contract.** It holds **one grind**, not a list. At most 100 lines. A row
that cannot be stated in ten lines is not understood well enough to be here. It
is TRUNCATED, never appended to, and when the grind closes the file is emptied
first — every row leaving by exactly one of two doors: baked into `MECHANISM.md`
because it won, or written into its Part F as a refutation because it lost.

---

## THE GRIND: UNROLL-AND-JAM, the transform that unlocks four lanes

**WHERE THE GAP IS, measured with `perf` and not with `.s`.** zcc emits 1.0144x
gcc -O2's STATIC instructions and executes **1.3250x** its DYNAMIC ones, at a
BETTER average IPC. The gap is instructions EXECUTED. Of the 50 programs above
1.1x cycles: 22 dominated by dynamic count, 15 both, 13 IPC/chains.

**THE ONE PROGRAM THAT NAMES THE PRIZE.** `z4_matmul_int` is 3.42x against gcc
-O2 and **1.087x against gcc -O1** — it is not a regression, it is the one place
gcc -O2's SIMD shows. gcc -O1 emits ZERO SIMD there and gcc -O2 is 3.3x faster
than its own -O1. At TWO lanes the same gcc buys 3% and loses to zcc's scalar
code (`tests/bench/matmul.c`, long: zcc 0.904 vs gcc -O2). **The prize is four
lanes: elements of 32 bits.**

**THE INNER LOOP, and why it is 7 instructions for ONE value of `j`:**

    ldr  w20, [x19]              ; B[k][j]  — x19 += 800 per iteration
    ldr  w21, [x2, x7, lsl #2]   ; A[i][k]  — unit stride in k
    madd w6,  w21, w20, w6       ; t += A*B
    add  x7,  x7, #1 ; add x19, x19, #800 ; cmp x7, #200 ; b.lt

**THE ROW: jam the `j` loop by four, scalar, in HIR.** One `A[i][k]` load serves
four lanes; `B[k][j..j+3]` are four CONSECUTIVE words, so two `ldp`; four
`madd`; one set of pointer updates and one compare. ≈11 instructions for four
values of `j` — **2.75/j against 7/j, about 2.5x** — with no SIMD instruction at
all. And it is the ENABLING transform: four jammed lanes are one `mla v.4s`
afterwards, which is the row after this one.

* Accumulator splitting is REFUTED before being built: `z4`'s IPC is **5.77**
  against gcc's 4.38, so the `madd` chain is not the bottleneck. Only the
  instruction COUNT is, and only a nest transform reduces it.
* `vecmap` (shipped, default-OFF) is the single-loop case and buys **0.02%** on
  the suite against a 0.3% noise floor. A candidate is not a hot loop.

**SHAPE TO RECOGNIZE.** An outer counted loop `j` whose body is: a preheader, an
inner counted loop, and a tail that STORES one value at an address affine in `j`
with stride = the element width. `j` must appear only in addresses affine in it.

**HOW IT IS PROVEN.** The four jammed copies are four iterations of the outer
loop run together; each computes exactly what it computed alone, because nothing
the outer body defines is read by another `j` (that is what makes them
independent, and it is the same check `vecmap` makes). A runtime guard covers the
trip count not being a multiple of four; the original nest is the tail. The
square is `⟦f⟧ = ⟦jam f⟧` on the HIR interpreter, plus a non-vacuity assertion
that the jammed body exists.

**WHAT IT DOES NOT DO.** Nothing for the 13 IPC/chain programs, and nothing for
`m1_resp_parse`'s **124x** branch-miss ratio. Those are separate rows.

**THE VECTOR SURFACE IS ALREADY IN THE TREE AND IS WAITING.** `VInt`, `VDup`,
`VAddv` in MIR, verified against the assembler, plus `MemOp::Q` which has carried
128-bit loads since `long double`. `IntrinKind::VecMap` shows how a lane
operation reaches the machine without a vector type in HIR.

⚠️ **A vector type must NOT be added to HIR.** Article B: target knowledge lives
in the ISA tables, the ABI automaton and the emitter. A vector WIDTH is target
knowledge, which is why `Arr`/`VAlu`/`Width::Q` are MIR facts. The accumulator
that must cross a back edge as a vector is a `Width::Q` vreg in a MIR block
parameter — MIR already carries it, and HIR must not learn about it.

**Method:** census before building (`ZCC_COPYPROBE`, `ZCC_VECPROBE`), price on
the model before the build, and read `perf` rather than `.s` — with static INSN
at parity the assembly listing cannot answer the question. The harness's
same-binary noise floor is **±0.3%** EXEC geomean; nothing smaller is a result.
