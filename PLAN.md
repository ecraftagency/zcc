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

## THE GRIND: the four jammed lanes become one `mla v.4s`

**WHERE IT STANDS.** `z4_matmul_int` is 2.94x against gcc -O2 and **1.087x
against gcc -O1** — the one place -O2's SIMD shows and the worst program left in
the suite. `jam` and the `iv` displacement row took its inner loop from seven
instructions per outer iteration to three; what remains is the width.

**THE LOOP TODAY — twelve instructions for four values of `j`:**

    ldr  w26, [x0, x20, lsl #2]   ; A[i][k], shared by all four lanes
    ldp  w25, w27, [x21]          ; B[k][j+0], B[k][j+1]
    madd w22, w26, w27, w22
    madd w5,  w26, w25, w5
    ldr  w27, [x21, #8]  ; add x25, x21, #800 ; ldr w21, [x21, #12]
    madd w23, w26, w27, w23 ; madd w24, w26, w21, w24
    add  x20, x20, #1 ; cmp x20, #200 ; b.lt

**THE ROW — seven, and every piece it needs is already in the tree:**

    ldr  w26, [x0, x20, lsl #2] ; dup v1.4s, w26 ; ldr q0, [x21]
    mla  v2.4s, v0.4s, v1.4s
    add  x21, x21, #800 ; add x20, x20, #1 ; cmp ; b.lt

`VInt`, `VDup`, `VAddv` and `MemOp::Q` shipped in `ae0a721`, every form assembled
against `as` on the box. `mla` is not among them and is `VInt::Mul` + `VInt::Add`
until it is; that is two instructions, not one, and still five fewer than today.

**WHY IT MUST BE A MIR PASS.** The accumulator crosses the inner loop's back edge
as a VECTOR. Article B puts a vector width in the ISA tables and the emitter, not
in HIR — and MIR already carries it: `Width::Q` is a real width and a `Q` vreg is
a legal block parameter. `IntrinKind::VecMap` cannot do this: an intrinsic has no
value living in a register across iterations.

**WHAT TO RECOGNIZE**, on SSA MIR before regalloc: a loop block with four
`Load` of `W32` at one base and displacements `0,4,8,12` (some already fused into
`ldp`), four `Alu::MAdd` each reading the SAME other operand, and four
loop-carried block parameters they accumulate into. Replace with one `Load` of
`MemOp::Q`, one `VDup`, one `VInt::Mul` + `VInt::Add`, and ONE `Q` parameter; the
exit needs the four scalars back, which is `VAddv`'s sibling problem — the four
lanes are four DIFFERENT `j`, so they are extracted, not summed. **`umov`/`mov
Wd, Vn.S[i]` is the extract and is NOT in the tree yet: add it with the same
assembler check the others got.**

**PRICE IT BEFORE BUILDING.** Dynamic instructions on `z4` today are **163.9M**
against gcc's **73.7M**; the form above should reach ~95M. That is `z4` at about
**1.4–1.5x**, not 1.0x. And on the suite geomean, `z4` alone is worth little:

    z4 -> 1.45 :  1.2180 x (1.45/2.941)^(1/96) = 1.2091
    z4 -> 1.00 :  1.2180 x (1.00/2.941)^(1/96) = 1.2044

**Sub-1.2 needs this row AND one more.** The next candidates, from the same
`perf` run: `e1_recursion` 2.05x (what `tailrec` left — the non-tail call),
`a1_int_mix` 2.79x and `e2_many_args` 2.75x (both INSN < 0.4, so gcc is inlining
where zcc is not), `m1_resp_parse` branch-misses **124x**.

⚠️ **The counters already warn about this row.** The `iv` displacement change cut
dynamic instructions 10.4% and cycles only 2.3%: IPC fell 3.79 -> 3.47 and backend
stalls DOUBLED, because four loads now contend on one base. A `ldr q` replaces
those four with one, which is the right direction — but measure cycles, not
instructions, and expect less than the count suggests.

**Method that worked all session:** census before building, price on the model
first, `perf` rather than `.s`, `md5(.s)` as the vacuity test, and a driver
sweeping `n = 1..13` against gcc before believing any loop transform. Three of
`jam`'s defects and `tailrec`'s only one were found that way — and `tailrec`'s
was found by `c-testsuite/00181`, not by any unit test written for it.
