# Allocator Splitting Restructure — R4 Capstone Design

**Date:** 2026-08-26
**Branch:** `mir-rearch` (fallback: `main` at tag `rc4`)
**Status:** design approved, pending spec review → implementation plan

## 1. Problem

zcc reaches gcc-O1 **speed** parity (geo40 exec geomean ≈ 1.05×) but lags on
**size**: sqlite is 182,956 static instructions = 1.1648× gcc-O1. Grinding within
the current allocator design yields ≈ 0.1% per iteration — the signature of a
*structural* ceiling, the same shape old-main hit at 1.8×/1.6× on its single-layer
IR before the mir-rearch layer split broke through.

The size gap decomposes into four "fronts" — spill traffic (9,270 frame
instructions over gcc), reg-reg `mov` / coalescing (6,813), constant
materialization (4,861), and misc (~4,800). Measurement shows these are **not
independent**: const-sharing, coalescing, and keeping values register-resident all
lengthen live ranges, which needs register headroom, which the allocator does not
have. They are one root cause — **the allocator's missing live-range-splitting
layer** — seen three ways.

Evidence: `sqlite3VdbeExec` (too large to inline, a clean allocator comparison)
touches the stack **zero** times in gcc-O1's 6,041 instructions; zcc emits 12,568
instructions with **2,810 frame stores and 1,130 frame loads**. Slot `[sp,#600]`
is stored **227 times** — a loop-carried value re-spilled every iteration. gcc
keeps it in a callee-saved register across the loop. `promote` (R4.16) can only
rescue values a *wholly-free* register can hold; VdbeExec has exactly one (x28),
so promote cannot touch `[sp,#600]`. The `ZCC_SPILLCEIL` model reports **7,661 of
12,479 sqlite reloads are in-loop (61%)** — the re-spill-each-iteration churn a
splitting allocator removes.

## 2. Goal and KPI

Complete the allocator so it keeps values register-resident in low-pressure
regions and spills only across tight regions — the register headroom gcc-O1 has.
Reaching the ULTIMATUM (gcc-O1 parity on both axes) does **not** require R5's O2
techniques: gcc-O1 does not schedule (`-fschedule-insns` is O2), unroll, or
vectorize. This allocator restructure is the **last** O1-level lever; R5 (O2
headroom) is beyond the stopping point.

**KPI (raw, never "slot-touches" — that metric mis-marked the prior attempt
"refuted"):**

- sqlite frame `str` + `ldr`: **21,991 → toward gcc's 12,721**.
- VdbeExec `[sp,#600]`: **227 stores → ~1**; in-loop reloads down.
- sqlite total instructions: **182,956 → down** (toward ~1.0×, floor ~1.10×
  before the other fronts open).
- Every step predicts its effect on the `ZCC_SPILLCEIL` model **before** building.

## 3. Current architecture

`regalloc::allocate` (src/regalloc/mod.rs):

```
prune → split-critical-edges → spill_and_color → destruct → promote
```

`spill_with` (src/regalloc/spill.rs) is a fixpoint:

- Each round computes liveness and calls `simulate`, which walks blocks in RPO
  maintaining a per-block working set `W` of register-resident values (Belady
  eviction by next-use distance, size ≤ `k`).
- `simulate` returns `Sim::Plan(p)` (success — reloads/spills to insert) or
  `Sim::More(vs)` (**these whole values become memory-resident for their whole
  life**).
- The fixpoint monotonically adds to `spilled` until a plan is found; it
  terminates in ≤ |vregs| rounds because a value never leaves the spilled set.
- `carried` (spill.rs:577–594) crosses a **forward** edge only when **every**
  predecessor holds the value under one name — the dominance special case of SSA
  reconstruction, needing no phi (R4.1). Back-edges (loop headers) carry nothing:
  residency restarts each iteration.
- `apply` materializes the plan into `Spill`/`Reload`/`Copy` MInsts.

Two limits define the ceiling: (a) eviction is **whole-web**, never regional;
(b) cross-edge carry is **dominance-only**, never reconstructed with a phi.

## 4. The restructure

Add the missing layer — live-range splitting with SSA reconstruction — by
generalizing exactly the two limits above. Big-bang: all joins and loop headers in
one restructure (git/RC4 makes the revert free; a bold attempt beats a timid one).

### 4.1 Generalized cross-edge carry (the heart)

When building block B's entry working set, for each value V live-in to B that is
register-resident at the exit of **some** (not necessarily all) predecessors:

1. Insert a **block-parameter `P_V`** at B's head (a fresh vreg, same class/width).
2. For each predecessor P:
   - if P holds V in register `r_P` at its exit → the edge-arg for `P_V` on P→B is
     `r_P`;
   - else (V memory-resident at P) → insert a **reload of V on the P→B edge** into
     a register, and that is the edge-arg.
3. B and every use of V dominated by B read `P_V` instead of reloading.

This is Braun-2013 reconstruction: `P_V` is the phi, each predecessor supplies its
reaching definition. R4.1's existing carry becomes the special case where every
edge-arg is the same register and no phi is materialized (destruct coalesces the
identity copies away).

### 4.2 Loop headers

A loop-carried value register-resident at the **latch** gets a header block-param
fed by the preheader (initial reaching def) and the latch (carried def). Because
blocks are walked in RPO, the latch is simulated *after* the header within a
round; the fixpoint reads the **prior round's** latch exit-residency to seed the
header. This is where the fixpoint must iterate to a register-residency fixpoint,
not only the current memory-residency one (§4.4).

### 4.3 Eviction = split, not whole-web

`Sim::More` no longer forces a whole value to memory. When pressure > k at a
point, Belady still picks the farthest-next-use value to evict, but eviction means
the value leaves `W` *here* and re-enters a register at its next use via a reload;
the register-resident segments before and after reconcile through the block-params
of §4.1 at any join between them. A value is memory-resident only in the regions
where pressure actually forced it out. Colombet-2011 optimal split-point choice is
a later refinement; the base uses Belady, which the current code already computes.

### 4.4 The fixpoint

The current fixpoint is monotone in one direction (memory-residency only grows).
The restructure adds register-residency propagation across back-edges, which is a
**second** monotone lattice: a value's set of "register-resident at block entry"
points can only grow as latch residency feeds headers, and is bounded by liveness.
Termination is the product of two monotone fixpoints, each bounded — the
termination proof is redone as a lattice-height argument (bounded by
|vregs| × |blocks|), and a hard round cap with a Law-2 assertion (exceeding it is a
defect, not a budget) backstops it. **This is the delicate core and the first place
a bounded attempt can fail.**

## 5. Correctness

The obligation is unchanged: `⟦mir_before_alloc⟧ = ⟦mir_after_alloc⟧`, checked by
the regalloc `same()` battery (src/regalloc/tests.rs) running the interpreter on
both sides. Every block-param is a proper SSA phi — defined at its block head, fed
by every predecessor's reaching definition — so SSA holds by construction, and
`mir::verify` already checks "every reload dominated by its spill, no vreg
survives, SSA well-formed." `destruct` already lowers block-params to parallel edge
copies. No new proof machinery is needed; the existing checks are the net.

The battery must be *non-vacuous*: it must run on inputs that actually spill and
split (the promote-style spill-heavy programs plus new loop-carried cases), or the
square proves nothing. `tests/provenance.sh` enforces this in the gate.

## 6. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Fixpoint termination (new register-residency lattice) | Lattice-height proof; hard round cap + Law-2 assertion; the first bounded-attempt failure point |
| Block-param explosion (params everywhere → bloat + pressure) | Add `P_V` only when it *removes* reloads (V register-resident at ≥1 pred and used at/below B); Braun minimal-SSA pruning |
| Pressure miscount (block-params not counted → coloring fails ≤k) | `simulate` counts block-params as short-lived edge values in the working set |
| Compile-speed regression (the `dominates_all` timeout warning) | Every step near-linear; measure csmith/yarpgen compile time in the gate; the multi-store O(stores×reloads) mistake is the anti-pattern |
| Miscompile (the promote fixed-use segfault class) | Full gate + `mir::verify` at every step; interp battery on spill-heavy inputs; big-bang reverts to RC4 wholesale |

## 7. Scope discipline

One bounded Law-2 attempt. If the restructure cannot green the full gate after one
bounded fix, **quarantine**: revert `mir-rearch` to the RC4-equivalent state, mark
the lever BLOCKED with the measured reason, and RC4 stands as the milestone (speed
parity achieved, size residual named). No second direction is authored without an
explicit re-plan.

## 8. Testing

1. **Commuting-square battery** — `same()` on: the promote spill-heavy programs;
   new loop-carried cases (accumulator in a loop with a call); wide-join cases
   (switch fan-out); nested loops. Non-vacuity asserted structurally (block-params
   present, reload count down vs promotion-off baseline).
2. **Full gate** — `sh tests/fullsuite.sh`: provenance, shape/cpp/decay/alg/abi,
   determinism 88×8, torture, opt-parity, csmith 300, yarpgen 300, musl. Green at
   completion.
3. **KPI measurement** — raw sqlite frame `str`+`ldr`, VdbeExec `[sp,#600]`,
   sqlite insn, `ZCC_SPILLCEIL` model, geo40 exec/insn (regression check).

## 9. References

- Rastello & Bouchez-Tichadou (eds.), *SSA-based Compiler Design*, Springer 2022 —
  the definitive text; SSA register allocation, spilling, coalescing chapters.
- Braun, Buchwald, Hack, Leißa, Mallon, Zwinkau, *Simple and Efficient Construction
  of SSA Form*, CC 2013 — the reconstruction algorithm (§4.1).
- Colombet, Brandner, Darte, *Studying Optimal Spilling in the Light of SSA*,
  2011/2015 — optimal split-point choice (§4.3 refinement).
- Braun & Hack, *Register Spilling and Live-Range Splitting for SSA-Form Programs*,
  CC 2009 — the algorithm zcc half-implements; this restructure completes it.
- Hack, Grund, Goos, *Register Allocation for Programs in SSA Form*, CC 2006 — the
  chordal foundation zcc keeps.
- Cranelift `regalloc2` (Fallin, 2021+) — modern splitting/bundle mechanics
  reference.
