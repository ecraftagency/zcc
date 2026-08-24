# `arm64_elf.md` — the backend theory (render + allocate)

> **Scope.** The constitution for the `src/codegen/arm64_elf/` module (+ the allocator it
> imports from `src/opt/`). It fixes, once, *which* academic algorithm each backend stage
> realizes and *which* AAPCS64/ARMv8 fact it is instantiated over. It obeys the charter
> verbatim: every backend line is **Side I** (a theorem → an algorithm) or **Side II** (a spec
> line → a constant), and every performance-pass carries the **Resource-fidelity** obligation
> (Article E) — instantiated over the *full* hardware fact, never a convenience truncation.
> The target is **`gcc -O1` parity on BOTH size and speed**, and O1 is the stopping point
> (điểm dừng).
>
> **This doc is the durable *theory*.** The measured size/speed campaign against these stages
> — the sqlite histograms, the per-lever debt, and the staged attack order — is *execution*
> and lives in **`OPT.md §0`** (the SUPREME PLAN spine + scoreboard); its dated snapshots are
> in git history. Theorems here; numbers there.

---

## §1 — The pipeline

```
IrFunc (SSA, out-of-SSA'd)                       [opt/ — middle-end]
   │
   ├─(A) REGISTER ALLOCATION      abi_alloc()     [opt/regalloc.rs — Chaitin/Briggs]
   │        temp → physical GPR/FPR home, or spill slot
   │
   ├─(B) INSTRUCTION SELECTION    emit_inst()      [emit.rs — maximal munch + folds]
   │        IR → ARMv8 insns; addressing-mode / madd / load-op fusion
   │
   ├─(C) FRAME + PROLOGUE         emit_params/EPILOGUE   [lower.rs]
   │        stack layout, param spill, callee-save
   │
   ├─(D) BLOCK LAYOUT             emit_ir_body()   [emit.rs — RPO order + fall-through]
   │        emit blocks; branch/fall-through
   │
   └─(E) MACHINE PEEPHOLE         peephole_*()      [peephole.rs — Davidson-Fraser]
            text-level: copy-prop, dead-move, ldp/stp pairing, redundant-load
```

Each stage below carries **the theorem** (Side I) and **the spec-fact it stands on** (Side II).
Where a stage exploits a hardware budget, the Side-II citation *is* its Resource-fidelity
declaration.

---

## §2 — The academic canon, and the O1 boundary

*(Answers: "are these the most advanced theories, or old ones veterans no longer use?")*

The framing that governs every choice: **we match `gcc -O1`, not LLVM `-O3`, not a
profile-guided build.** GCC-O1 is built on the *classic-but-current* canon — graph coloring,
machine-description maximal-munch, greedy fall-through layout, Davidson-Fraser peephole.
These are **in production in GCC in 2026**; they are not obsolete. The genuinely newer
theories are O2/O3/PGO/JIT territory and adopting them would over-engineer *past* the
stopping point.

| stage | classic (what GCC-O1 uses today) | genuinely newer (O2/O3/JIT — SKIP for O1) |
|---|---|---|
| **Reg-alloc** | Chaitin 1981 → Briggs-Cooper-Torczon 1994 (optimistic coloring) → George-Appel 1996 (iterated coalescing). **GCC's global allocator is a Chaitin-Briggs derivative.** | LLVM *Greedy* (Olesen 2011): live-range splitting + eviction + spill-cost. SSA-chordal RA (Hack-Grund-Goos 2006; Bouchez 2007) — SSA interference graphs are **chordal ⟹ colorable in P**, no NP-hard spill. |
| **Insn-sel** | Maximal munch / BURS / iburg (Fraser-Hanson-Proebsting 1992; Aho-Ganapathi-Tjiang 1989). GCC = machine-description pattern match. | LLVM SelectionDAG (DAG covering) → GlobalISel (2017+). More power than tree-BURS, needed for O2/O3. |
| **Block layout** | Pettis-Hansen 1990 basis; un-profiled greedy trace / RPO with fall-through = GCC-O1. | Profile-guided ext-TSP / BOLT (Panchenko 2019). Needs a profile — N/A at O1. |
| **Peephole** | Davidson-Fraser 1980 (= GCC `combine.c` + `peephole2`). | Superoptimization (Souper 2017), ML-guided. Research frontier, not O1. |
| **Scheduling** | List scheduling (Gibbons-Muchnick 1986) — *speed only, not size*. | Software pipelining / SMS. O2+; irrelevant to the size gap. |

**Verdict for zcc:** adopt the **left column** faithfully and completely — it *is* the O1
algorithm set. We do **not** need LLVM-Greedy, GlobalISel, or BOLT to reach O1; using them
would be building past the finish line. **One modern insight is banked for free:** zcc's IR is
already SSA, and SSA interference graphs are chordal, so zcc's coloring is *already* on the
polynomial-easy side of the theory — a strength to exploit, not a debt.

---

## §3 — Register allocation (THE KEYSTONE) — `opt/regalloc.rs` (`abi_alloc` / `color_abi`)

### Theorem (Side I)
Global register allocation = graph coloring. Build the **interference graph** (`u ~ v` iff both
live at some definition point) over liveness, then color with `k` = |usable physical registers
of the class|. Chaitin simplify (remove degree `< k` nodes, push, pop-and-assign) with Briggs
*optimistic* spilling; George-Appel *biased coalescing* to remove Copies. SSA-chordality
guarantees a perfect elimination order exists, so simplify rarely spills a colorable graph.

### Spec-fact (Side II) — AAPCS64 §5.1.1 / §6.1.1, the ARMv8 GPR file
```
  x0–x7    argument / result / caller-saved (volatile across bl)     — 8
  x8       indirect result location                                   — 1 (scratch)
  x9–x15   caller-saved temporaries (volatile across bl)             — 7
  x16,x17  IP0/IP1 — linker veneers, avoid as homes                  — 2 (reserve)
  x18      platform register — reserved on Linux                      — 1 (reserve)
  x19–x28  callee-saved (must be preserved by callee)                — 10
  x29 FP · x30 LR · sp                                                — fixed
```
The caller/callee split is the whole game: a temp **not** live across any `bl` can live in a
caller-saved register *for free* (dead by the time any call clobbers it). A temp that **is**
live across a call (`crossing[t]`) must be confined to callee-saved (it survives the `bl`).
`color_abi` implements this split exactly (`crossing[]` confines crossing temps to callee-saved
colors; non-crossing prefer caller-saved).

### The budget = the usable-register-count (Resource-fidelity, Article E)
The allocatable pool is a `ClassBudget { k, ncaller, narg }` in `encoding.rs`, **not** a
convenience constant — its number is the AAPCS64 usable-register-count above. `k` = total homes,
`ncaller` = of which caller-saved (for non-crossing temps), `narg` = of which argument registers.
Any gap between the budget and the full leaf-usable file is a **dated, justified** truncation,
never silent (a silent truncation posing as a Side-II constant is the Article-E / Law-1 defect
the charter's worked example names). The mapping color→physical (`gp_phys`/`fp_phys`) and the
allocator's proof obligation `verify_coloring` / `verify_abi` (interfering temps get distinct
colors; no call-crossing temp got a caller-saved home) are the machine translation-validation
for this stage.

---

## §4 — Instruction selection — `emit_inst` + the fold family (`emit.rs`)

### Theorem (Side I)
Maximal munch (BURS): tile the IR expression tree with the largest ISA pattern at each step, so
one instruction covers a maximal subtree — `Load(Add(b,i)) → ldr [b,i]`,
`Add(Mul(x,y),c) → madd`, `Load/Store(Lea(local)) → ldr/str [sp,#pos]`, `add/sub #imm12`.

### Spec-fact (Side II) — ARMv8 addressing modes
`[Xn, #simm]` (scaled unsigned imm12, or `ldur/stur` unscaled ±256); `[Xn, Xm]` (register
offset, full 64-bit add); `madd/msub`; `add/sub #imm12` (optionally `<<12`). The fold is
*exhausted* (Law 3 / Article E) only when every residual un-folded site is a genuine ISA
encoding boundary (offset beyond imm12), not a path that merely bypasses the folder.

---

## §5 — Frame lowering — `emit_params`, prologue/`EPILOGUE`, `sp_adjust` (`lower.rs`)

### Theorem (Side I)
Compute the total frame size **once** at prologue (locals + spill slab + reg-save area, aligned
to 16); one `sub sp, sp, #TOTAL`; assign each local/spill a fixed offset. A frozen sp for the
function body (VLA excepted — `has_vla`/`reset_sp_base`) is what lets sp-relative addressing
(`[sp,#pos]`) subsume the per-access `lea_local`. Optionally stack-slot-color
non-overlapping-lifetime slots to shrink the frame (Chaitin-on-stack).

### Spec-fact (Side II)
AAPCS64 §6.2.2 prologue (`stp x29,x30,[sp,#-16]!`), 16-byte SP alignment, the 192B variadic
reg-save area (128B VR + 64B GP) below the frame.

---

## §6 — Block layout — `emit_ir_body` / `emit_term` (`emit.rs`)

### Theorem (Side I)
Order basic blocks so the most likely successor **falls through** (no branch emitted).
Un-profiled: reverse-postorder / greedy trace; drop an unconditional branch to the physically
next block; for a conditional, emit `b.cond`/`cbz`/`cbnz` to the taken edge and fall through to
the other. The 3-instruction far-relaxation form (`cbz xN,Ln; b then; Ln: b else`) is a
*conditional* fallback, emitted only when a target exceeds the imm19 range — not the default.

### Spec-fact (Side II)
ARMv8 branch offset ranges: `b`/`b.cond` ±128MB (imm26/imm19), `cbz`/`tbz` narrower. Reordering
blocks and choosing an equivalent branch encoding preserves control flow; **opt-parity
0-DIVERGE** certifies (machine translation-validation).

---

## §7 — Machine peephole — `peephole_*` (text-level, Davidson-Fraser) (`peephole.rs`)

### Theorem (Side I)
A retargetable peephole over the emitted stream: slide a window, replace a recognized
instruction pair/triple with a cheaper equivalent, iterate to a fixpoint. Each rule is a local
semantic identity ⟹ machine translation-validated (opt-parity 0-DIVERGE); it is the *mop-up*
net for residue the structural stages (§3/§5/§6) leave behind, never a substitute for the source
fix. All register operands are decoded through the single audited grammar `xreg`/`wreg`/`gpreg`
(the `x`/`w`\<N\> AAPCS64 token over one physical reg N) — the sole operand-decode point.

### Spec-fact (Side II)
The existing rules (`peephole_moves`, `propagate_copies`, `pair_ldst` → ldp/stp, `drop_dead_moves`,
`drop_redundant_moves`, `drop_redundant_loads` = store→load forwarding) each stand on an ARMv8
instruction-semantics identity; the load/store-pair rule stands on AAPCS64 alignment + the ldp/stp
encoding.

---

## §8 — Value contract — `ext`/`ext_r`, the `sxtw` canonical form (`emit.rs`)

### Theorem (Side I)
Every scalar is kept **canonical in a 64-bit register**: sub-word values sign/zero-extended to 64
bits so comparisons/uses are width-correct (SEMANTICS §1); after a 32-bit-defining op, `ext_r`
re-canonicalizes. A re-extension is *dead* — and must be elided under a ℤ/2ⁿ argument — when the
defining instruction already produced a sign-extended 64-bit value (`ldrsw`/`ldrsh`…) or when
every use is width-≤32 (the high bits are unobserved). Elision ships with opt-parity 0-DIVERGE.

### Spec-fact (Side II)
ARMv8 `sxtw`/`sxth`/`sxtb`/`uxt*` semantics and the w-form write-zero-extends-to-x rule (a `w`-form
write clears the top 32 bits), which is what makes a following `sxtw` dead when the consumer is
w-form.

---

## §9 — Invariants that must never break

1. Every emitted rewrite is `⟦IR⟧`-preserving, proven at the IR (commuting square) or the asm
   (machine translation-validation via **opt-parity 0-DIVERGE**). csmith/yarpgen only *confirm*.
2. No Side-II constant is a convenience-truncation of its hardware fact (Article E). The
   allocator budget in particular is `= usable-register-count`, any gap dated-justified.
3. One target per file; ABI/section/asm-syntax stay in this module; LP64 lives in TyTab.
