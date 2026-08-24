# `arm64_elf.md` — the backend theory (render + allocate)

> **Scope.** This is the constitution for the `src/codegen/arm64_elf/` module (+ the allocator
> it imports from `src/opt/`). It fixes, once, *which* academic algorithm each backend stage
> realizes, *which* AAPCS64/ARMv8 fact it is instantiated over, and *where the current code
> falls short of its own theorem*. It obeys the project charter verbatim: every line of the
> backend lies on **Side I** (a theorem → an algorithm) or **Side II** (a spec line → a
> constant), and every performance-pass carries the **Resource-fidelity** obligation of
> Article E — it must be instantiated over the *full* hardware fact, never a convenient
> truncation. The target is **`gcc -O1` parity on BOTH size and speed** — and O1 is the
> stopping point (điểm dừng).

---

## §0 — Why this document exists

The middle-end (`opt.rs`: SSA construction, LICM, strength-reduction, GVN-lite, DCE) can be
perfect and it still buys nothing if the backend lowers each SSA value into a *dumb*
instruction sequence. The backend is the **last gate**: every upstream value passes through
it, and any waste it emits is multiplied across the whole translation unit. Measured on
`sqlite3.c` (2026-08-22, commit `18570ff`):

```
                gcc -O1     zcc        ratio    excess
  mov            33,608    157,177     4.7×     +123,569
  b               8,466     65,656     7.8×      +57,190
  sub             2,147     55,885    26.0×      +53,738
  ldr            22,332     72,441     3.2×      +50,109
  str             9,462     38,336     4.1×      +28,874
  add            11,684     34,986     3.0×      +23,302
  sxtw               ~0     20,031      ∞        +20,031
  TOTAL         157,074    582,064     3.71×     ~425,000 excess
```

**3.71× is not "missing optimizations." It is a structural disease in lowering.** A sane
non-optimizing backend is ~1.3–1.6× of O1; 3.71× is the signature of a *memory-homed value
model with a truncated register file*. This document names the disease theorem-by-theorem
and prescribes the cure, staged, each stage shipping under Law 3 (commuting-square /
machine translation-validation, gate 0-DIVERGE).

---

## §1 — The pipeline

```
IrFunc (SSA, out-of-SSA'd)                       [opt.rs — middle-end, DONE]
   │
   ├─(A) REGISTER ALLOCATION      abi_alloc()     [opt.rs::color_abi — Chaitin/Briggs]
   │        temp → physical GPR/FPR home, or spill slot
   │
   ├─(B) INSTRUCTION SELECTION    emit_inst()      [arm64_elf.rs — maximal munch + folds]
   │        IR tree → ARMv8 insns; addressing-mode / madd / load-op fusion
   │
   ├─(C) FRAME + PROLOGUE         emit_params/EPILOGUE
   │        stack layout, param spill, callee-save
   │
   ├─(D) BLOCK LAYOUT             emit_ir_body()   [arm64_elf.rs — block order]
   │        emit blocks; branch/fall-through
   │
   └─(E) MACHINE PEEPHOLE         peephole_*()      [arm64_elf.rs — Davidson-Fraser]
            text-level: copy-prop, dead-move, ldp/stp pairing, redundant-load
```

Each stage below: **the theorem**, **the spec-fact it stands on**, **zcc's realization**,
**the debt** (measured), **the fix**.

---

## §2 — The academic canon, and the O1 boundary

*(Answers: "are these the most advanced theories, or old ones veterans no longer use?")*

The single most important framing: **we are matching `gcc -O1`, not LLVM `-O3`, not a
profile-guided build.** GCC-O1 itself is built on the *classic-but-current* canon — graph
coloring, machine-description maximal-munch, greedy fall-through layout, Davidson-Fraser
peephole. These are **in production in GCC in 2026**; they are not obsolete. The genuinely
newer theories are O2/O3/PGO/JIT territory and adopting them would over-engineer *past* our
stopping point.

| stage | classic (what GCC-O1 uses today) | genuinely newer (O2/O3/JIT — SKIP for O1) |
|---|---|---|
| **Reg-alloc** | Chaitin 1981 → Briggs-Cooper-Torczon 1994 (optimistic coloring) → George-Appel 1996 (iterated coalescing). **GCC's global allocator is a Chaitin-Briggs derivative.** | LLVM *Greedy* (Olesen 2011): live-range splitting + eviction + spill-cost. SSA-chordal RA (Hack-Grund-Goos 2006; Bouchez 2007) — SSA interference graphs are **chordal ⟹ colorable in P**, no NP-hard spill. |
| **Insn-sel** | Maximal munch / BURS / iburg (Fraser-Hanson-Proebsting 1992; Aho-Ganapathi-Tjiang 1989). GCC = machine-description pattern match. | LLVM SelectionDAG (DAG covering) → GlobalISel (2017+). More power than tree-BURS, needed for O2/O3. |
| **Block layout** | Pettis-Hansen 1990 basis; un-profiled greedy trace / RPO with fall-through = GCC-O1. | Profile-guided ext-TSP / BOLT (Panchenko 2019). Needs a profile — N/A at O1. |
| **Peephole** | Davidson-Fraser 1980 (= GCC `combine.c` + `peephole2`). | Superoptimization (Souper 2017), ML-guided. Research frontier, not O1. |
| **Scheduling** | List scheduling (Gibbons-Muchnick 1986) — *speed only, not size*. | Software pipelining / SMS. O2+; and irrelevant to our size gap. |

**Verdict for zcc:** adopt the **left column** faithfully and completely — it *is* the O1
algorithm set. The only left-column piece zcc has (Chaitin coloring) is present but crippled
(§3). We do **not** need LLVM-Greedy, GlobalISel, or BOLT to reach O1; using them would be
building past the finish line. **One modern insight is worth banking for free:** zcc's IR is
already SSA, and SSA interference graphs are chordal, so zcc's coloring is *already* on the
polynomial-easy side of the theory — a strength to exploit, not a debt.

---

## §3 — Register allocation (THE KEYSTONE) — `opt.rs::color_abi` / `abi_alloc`

### Theorem (Side I)
Global register allocation = graph coloring. Build the **interference graph** (`u ~ v` iff
both live at some definition point; `opt.rs::interference` over `opt.rs::liveness`), then
color with `k` = |usable physical registers of the class|. Chaitin simplify (remove degree
`< k` nodes, push on a stack, pop and assign) with Briggs *optimistic* spilling; George-Appel
*biased coalescing* to remove Copies (`color_abi`'s `move_adj`/`bias`). SSA-chordality
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
**Usable as allocation homes: x0–x15 (16 caller-saved) ∪ x19–x28 (10 callee-saved) = 26.**
The caller/callee split is the whole game: a temp that is **not** live across any `bl` can
live in a caller-saved register *for free* (no prologue cost, no clobber problem, because it
is dead by the time any call clobbers it). A temp that **is** live across a call
(`crossing[t]`) must be confined to callee-saved (survives the `bl`).

### zcc's realization + the DEBT
`color_abi` **already implements the caller/callee split** exactly (`crossing[]` confines
crossing temps to `[ncaller, k)`; non-crossing prefer `[0, ncaller)`). The infrastructure is
correct and complete. It is crippled by **one constant**:

```rust
const GP_BUDGET: ClassBudget = ClassBudget { k: 10, ncaller: 0 };   // arm64_elf.rs:40
```

`ncaller = 0` ⟹ **there are zero caller-saved colors.** `k = 10` ⟹ the GP pool is *exactly*
x19–x28. Consequences, both measured:
1. **Every** allocated temp lands in a callee-saved register ⟹ every non-leaf function pays
   prologue save/restore for registers it only needed as scratch.
2. Only **10** temps can be register-resident at once. In sqlite's large functions,
   everything past 10 **spills to the stack** → the `mov`/`ldr`/`str` explosion (~200k of the
   425k excess is this one truncation: values round-tripping through memory because the
   register file is used at 10/26).

**This is a textbook Article-E violation.** The charter's worked example is *literally*
`GP_BUDGET.k=10 vs AAPCS64's ~18 leaf-usable GPRs`. A truncation posing as a Side-II constant
is a **Law-1 defect** (the algorithm does not faithfully realize its side, being instantiated
over a convenience-truncated fact), catchable as **Law-2 Side-II**. Fixing it is
*constitutionally mandated*, not optional.

### The fix
Expand `GP_BUDGET` to the full usable file and carve out a **minimal fixed scratch set** the
lowering may hardcode. Today the lowering hardcodes x0/x1 (value/addr funnel), x9/x10
(`lea_local`), x2–x5 (div/bitfield), x8/x11 (params), d0/d1/d7 (FP scratch). The refactor
(Stage 1, §10) must:
- Reserve a small fixed scratch set (proposal: **x16, x17** as the two GP scratch, plus
  existing FP scratch) — everything else becomes allocatable.
- Rework the funnel-based lowerings so they never hardcode an allocatable register (the
  `compute-into-home` work already moves this direction — Bin/Copy/Br now target homes).
- Set `GP_BUDGET { k ≈ 24, ncaller ≈ 14 }` (14 caller-saved homes for non-crossing temps, 10
  callee-saved for crossing) and update `gp_phys` to map color→physical accordingly.
- **Proof obligation:** `verify_abi` (opt.rs:1082 — the two-invariant checker: (1) interfering
  temps get distinct colors, (2) no call-crossing temp got a caller-saved home) must pass, and
  opt-parity 0-DIVERGE. This is the machine translation-validation for the allocator.

Expected: the bulk of the +200k `mov`/`ldr`/`str` collapses, since most temps stop spilling.

---

## §4 — Instruction selection — `emit_inst` + the fold trilogy

### Theorem (Side I)
Maximal munch (BURS): tile the IR expression tree with the largest ISA pattern at each step,
so one instruction covers a maximal subtree. Cover `Load(Add(b,i)) → ldr [b,i]`,
`Add(Mul(x,y),c) → madd`, `Load/Store(Lea(local)) → ldr/str [sp,#pos]`, `add/sub #imm12`, etc.

### Spec-fact (Side II) — ARMv8 addressing modes
`[Xn, #simm]` (scaled unsigned imm12, or `ldur/stur` unscaled ±256); `[Xn, Xm]`
(register offset, full 64-bit add); `madd/msub`; `add/sub #imm12` (optionally `<<12`).

### zcc's realization + DEBT
Three folds exist and are correct where they fire: `try_fuse_addr` (`[b,i]`), `try_fuse_madd`
(`madd`), `try_fuse_local` (`[sp,#pos]`). **The debt is coverage, not correctness:**
- `try_fuse_local` only matches an **explicit `Inst::Lea(Local)` immediately followed by its
  single-use Load/Store**. The 15,513 unfolded const-offset `sub x?,x29,#imm; [x?]` pairs +
  23,304 dynamic pairs come from paths that **bypass the IR peephole**: spilled-temp access
  (`tmp_load`/`tmp_store`, arm64_elf.rs:1034/1048), prologue param-spill, and callee-save —
  all emit `lea_local` (`sub x9,x29,#off`) *inline*, outside the `Inst` stream, so no
  `try_fuse_local` can see them. And `sp_slot` falls back to `lea_local` whenever the slot
  offset exceeds the scaled-imm12 range (large frames) → the const-offset misses.
- **Fix (two prongs):**
  1. Make `tmp_load`/`tmp_store` and the frame emitters address slots via `[sp,#pos]` /
     `[x29,#-off]`/`ldur-stur` directly, splitting a large offset as `add x_scr, x29, #hi;
     [x_scr, #lo]` (still cheaper than the current per-access `sub`), never the naked
     `sub;[reg]`. Best subsumed by §3 (fewer spills ⟹ far fewer of these exist at all).
  2. A **universal machine-peephole** (§7) `sub xN,x29,#imm ⋯ [xN] → [x29,#-imm]/[sp,#pos]`
     when `xN` is dead after — Davidson-Fraser, catches *every* path regardless of origin.

---

## §5 — Frame lowering — `emit_params`, prologue/`EPILOGUE`, `sp_adjust`

### Theorem (Side I)
Compute the total frame size **once** at prologue (locals + spill slab + reg-save area,
aligned to 16). One `sub sp, sp, #TOTAL`. Assign each local/spill a fixed offset; optionally
**stack-slot color** non-overlapping-lifetime slots to shrink the frame (Chaitin-on-stack).

### zcc's realization + DEBT
The frame is grown **per-variable** — the sample shows `sub sp,sp,#16 … sub sp,sp,#64`
interleaved with body code (4,929 `sub sp,sp,#imm` in sqlite). This is a naive scoped-alloca
model. `fframe + variadic + ir_tspill` (arm64_elf.rs:472) already knows the total — the
incremental growth is pure waste and also **defeats sp-relative addressing** (sp keeps
moving, so `sp_slot` can't be used ⟹ forces `lea_local`, feeding §4's debt).
- **Fix:** single-shot prologue frame; freeze sp for the function body (VLA excepted, which
  already has the `has_vla`/`reset_sp_base` machinery). Once sp is frozen, `sp_slot` succeeds
  for far more slots ⟹ the const-offset fold (§4) fires without any peephole.

---

## §6 — Block layout — `emit_ir_body` / `emit_term`

### Theorem (Side I)
Order basic blocks so that the most likely successor **falls through** (no branch emitted).
Un-profiled: reverse-postorder / greedy trace; drop an unconditional branch to the physically
next block; for a conditional, emit `b.cond` to the taken edge and fall through to the other.

### zcc's realization + DEBT
**No layout pass, no fall-through.** `Term::Jmp(b) → b label` unconditionally, even when `b`
is the next block. `Term::Br` *always* emits the 3-instruction far-relaxation form
`cbz xN,Ln; b then; Ln: b else` — two `b` + a `cbz` per conditional, regardless of distance.
This is the +57k `b` (7.8×). The far form is only needed when a target is >±1MB away (huge
`-O0` fuzzer functions); for the overwhelmingly common near branch it is 2–3× the code.
- **Fix:**
  1. Lay blocks in RPO; if a `Jmp`'s target is the next emitted block, **emit nothing**
     (fall through).
  2. `Br`: emit `b.cond taken` (or `cbz/cbnz`) directly to the label and fall through to the
     other edge — the **near** form (1 insn + fall-through). Keep the far veneer form **only**
     when the function's instruction count exceeds a measured imm19 threshold (branch
     relaxation as a *conditional* pass, not the unconditional default).
  - **Proof:** control-flow is unchanged by reordering blocks + choosing an equivalent branch
    encoding; opt-parity 0-DIVERGE certifies.

---

## §7 — Machine peephole — `peephole_*` (text-level, Davidson-Fraser)

### Theorem (Side I)
A retargetable peephole over the emitted stream: slide a window, replace a recognized
instruction pair/triple with a cheaper equivalent, iterate to a fixpoint. Each rule is a
local semantic identity ⟹ machine translation-validated.

### zcc's realization
A real Davidson-Fraser layer already exists and is a strength: `peephole_moves`,
`propagate_copies`, `pair_ldst` (ldr/str → ldp/stp), `drop_dead_moves`,
`drop_redundant_moves`, `drop_redundant_loads` (store→load forwarding). These are correct and
tested (unit tests at arm64_elf.rs:2819+).
- **Add (this campaign):** the frame-addressing fold rule (§4 prong 2) and any residue the
  structural fixes (§3/§5/§6) leave behind. The peephole is the *mop-up* net; the structural
  fixes are the *source* fixes — do the source fixes first, let the peephole catch the rest.

---

## §8 — Value contract — `ext`/`ext_r`, the `sxtw` debt

### Theorem (Side I) + current contract
Every scalar is kept **canonical in a 64-bit register**: sub-word values sign/zero-extended
to 64 bits so comparisons/uses are width-correct (SEMANTICS §1). After a 32-bit-defining op,
`ext_r` re-canonicalizes.

### DEBT
20,031 `sxtw` in sqlite, ~0 in gcc. The contract re-extends **eagerly** even when the value
is already in range (e.g. immediately after a `ldrsw`/`ldrsh` that already sign-extended, or
when the only consumer is a same-width op that ignores the high bits). gcc tracks
known-range/known-bits and elides.
- **Fix:** a redundant-extension elimination — drop `sxtw xD, wD` when the defining
  instruction already produced a sign-extended 64-bit value, or when every use is
  width-≤32 (the high bits are unobserved). Lightweight known-bits, on the IR or as a
  peephole. −20k. Ships with a ℤ/2ⁿ argument per removed extension + opt-parity.

---

## §9 — The measured gap and the attack order (by payoff)

| # | lever | mechanism | est. insns | §ref | proof |
|---|---|---|---|---|---|
| 1 | **un-truncate reg-alloc** | k=10→~24, caller-saved homes | ~½ of mov+ldr+str (~150k+) | §3 | `verify_abi` + opt-parity |
| 2 | **single-shot frame + sp-freeze** | one prologue `sub sp`; unlock `sp_slot` | −4.9k sub, unlocks #3 | §5 | opt-parity |
| 3 | **frame-addressing fold** | universal `sub x,x29,#i;[x]`→`[sp,#pos]` | −15.5k sub (+dyn) | §4/§7 | opt-parity |
| 4 | **block layout + near-branch** | RPO fall-through, `b.cond` direct | −~50k b | §6 | opt-parity |
| 5 | **redundant-`sxtw` elim** | known-bits / use-width | −20k sxtw | §8 | ℤ/2ⁿ + opt-parity |

Order rationale: **#1 first** — it is the keystone, the constitutional obligation, and it
*shrinks the input* to every later stage (fewer spills ⟹ fewer frame accesses ⟹ #2/#3 have
less to fix). Then #2 unlocks #3 mechanically. #4 and #5 are independent and can interleave.

---

## §10 — Refactor plan (staged, never big-bang)

The backend passes **1,378 torture + csmith 254 + yarpgen 250 + opt-parity 1,552, all
0-DIVERGE, today.** Correctness is not negotiable. Every stage below is a *separate commit*,
each gate-green before the next, each carrying its Law-3 proof. No stage lands on a red gate.

- **Stage 0 — this document + baseline.** ✅ (`18570ff`).
- **Stage 1 — reg-alloc un-truncation (§3).** The keystone. Reserve fixed scratch (x16/x17);
  audit every hardcoded allocatable-register use in lowering and route through
  reserved-scratch or compute-into-home; expand `GP_BUDGET`; update `gp_phys`. Gate:
  `verify_abi` green + full fuzz gate 0-DIVERGE. **Highest risk, highest payoff — do it
  first, carefully, with the smallest correct increment (e.g. add caller-saved homes for
  non-crossing temps only, keep k conservative, then widen).**
- **Stage 2 — single-shot frame + sp-freeze (§5).**
- **Stage 3 — frame-addressing fold, universal (§4 prong 2 / §7).**
- **Stage 4 — block layout + near-branch encoding (§6).**
- **Stage 5 — redundant-`sxtw` elimination (§8).**

After each stage: re-measure the sqlite histogram, update §9's actuals and `OPT.md §1`
scoreboard. The gap number (3.71× today) is the single tracked metric to drive → 1.0×.

### Invariants that must never break
1. Every emitted rewrite is `⟦IR⟧`-preserving, proven at the IR (commuting square) or the
   asm (machine translation-validation via opt-parity 0-DIVERGE). csmith/yarpgen only
   *confirm*.
2. No Side-II constant is a convenience-truncation of its hardware fact (Article E). The
   allocator budget in particular is `= usable-register-count`, with any gap dated-justified.
3. One target per file; ABI/section/asm-syntax stay in this file. LP64 in TyTab.
