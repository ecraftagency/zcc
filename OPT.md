# OPT.md — the single optimization working-doc `[ssa-qbe fork]`

> **One file. Transient.** This is the *only* optimization doc. When the fork's opt work is
> done it is **deleted** — the durable facts cook into `THEORY.md` (theorems/tables) and
> `SEMANTICS.md` (⟦·⟧). It replaces the three scattered files (`OPTIMIZATION-ROADMAP.md`,
> `OPT-THEORY-MAP.md`, `IR.md`) whose proliferation caused drift.
>
> **Decisions come from §1 (the scoreboard), never from §6 (the catalog).** The catalog is a
> reference shelf; the scoreboard is the one measured surface. If a technique is not on the
> path from §1's measured gap, it does not get built — no matter how good its textbook name.
>
> **THE TARGET IS gcc -O1 — and it is the STOPPING POINT (điểm dừng).** Not O2, not O3. No toy
> compiler has matched gcc-O1; reaching parity is the finish line for this fork. The scoreboard
> below is scored against O1; O0/O2 are context columns only. When the geomean-vs-O1 reaches
> ~1.0 the optimizer is DONE — remaining O2/O3 distance is explicitly out of scope.

---

## §1 — Scoreboard: the one number (measured, `zcc-box` docker, ELF aarch64-musl)

**The finish line = gcc -O1 parity. The gap = the two loop-nest kernels; everything reduces to them.**

Ratio = zcc_time / gcc_time (**lower is faster; 1.0 = parity**). Measured bench, best-of-3:

| kernel | vs gcc-O0 | **vs gcc-O1 (TARGET)** | was (pre-pointer_iv) | note |
|---|---|---|---|---|
| fib    | 1.16 | **1.06 — ✅ PARITY** | 1.05 | O1≈O0 here |
| loops  | 0.35 | **1.02 — ✅ PARITY** | 1.06 | already O1-class |
| matmul | 0.51 | **1.71 — ⬇ closing** | 3.44 | pointer-IV SR+LFTR fired at default k=10, spill-free |
| sieve  | 0.36 | **2.10 — ⬇ closing** | 2.60 | pointer-IV SR fired; residual = index arith + mem |
| **geomean** | **0.52** | **1.40× (target 1.0)** | ~1.78× | measured 2026-08-22, quiet box, best-of-3 |

**Reading it:** **fib + loops MATCH gcc-O1** (1.06, 1.02). matmul + sieve — the two loop-nest kernels —
**dropped 3.44→1.71× and 2.60→2.10×** this batch, pulling the geomean **1.78→1.40×**. Against gcc-O0
zcc is now ~2× *faster* (0.52×). Residual O1 gap lives entirely in matmul/sieve index arithmetic + memory.

**SIZE axis (the second finish-line number — must ALSO reach 1.0; user: "match O1 on size AND speed").**
Metric = instruction count on sqlite3.c (amalgamation, musl headers), same mnemonic-line count on
zcc & gcc-O1 `.s`. gcc-O1 = 157,074 insn (the target).

| milestone | sqlite insn | gap ×gcc-O1 | lever |
|---|---|---|---|
| pre-size-work | ~1.95M | 12.4× | value-contract verbosity |
| sp-addressing-fold (`778f82b`) | 1.05M | 6.65× | fold local addr into `[sp,#pos]` |
| redundant-load-after-store (`e848888`) | (879k on its own metric) | — | store→load identity |
| **type-aware volatile (this batch)** | **603,513** | **3.84×** | opt turned ON for real code |
| next: coalescing (mov 5.3×) → branch → sxtw → spill | → ~1× | **TARGET 1.0** | §3b histogram |

**3.84× is tcc-tier — NOT the finish.** The remaining gap is now REGALLOC/CODEGEN QUALITY (mov 5.3×,
branch 7.8×, spill 3.5×, sxtw ∞), not "opt is off." Ranked levers + numbers: §3b post-unlock histogram.

**RESOLVED (this batch) — the "register / `k`" framing was a MISDIAGNOSIS; the real defect was two
Side-I pass bugs (Law 2).** The lever gcc-O1 uses is **pointer-IV strength-reduction + LFTR** (index
`mul` → marching pointer `p+=stride`; counter test → pointer-limit test, counter dies). Earlier this
pass regressed at default k=10 (5.47×) and only reached 1.80× at a k=18 probe, which *looked* like a
register-budget wall. It was not. Measurement located two defects in the pass itself:

1. **Empty-base reduction** (`opt.rs:2933`, `&& !base.is_empty()`) — the recognizer reduced *bare*
   induction variables (empty base), including the marching pointers it had *just emitted*, so every
   fixpoint round re-reduced its own output → code explosion (matmul 211→1231 insns, `x=x·1+0` unrolled
   ~40 deep) → the spill that masqueraded as "needs more registers." The theorem's own hypothesis
   (`base` = a loop-invariant address to FOLD) was simply missing as a guard.
2. **Stale locators** (`opt.rs:2792` `def_locations`, rebuilt at 2959 + before LFTR) — `def_of`
   (block,index) was cached before materialization, but inserting a header φ shifts indices → the clone
   read the wrong instruction → the s0272/s0078 OPT-COMPILE-FAIL panic.

With both fixed, **pointer_iv fires at the DEFAULT k=10, spill-free** — matmul 3.44→1.71×, sieve
2.60→2.10× (measured above). No register work was needed; `k` stays 10. **Default-ON**, gate green:
equiv 102/102, torture 1378/0, csmith+yarpgen 1000+1000 = **0 DIVERGE** on the local Debian-13 zcc-box
AND a native AWS Graviton4 (Debian-13, environment-matched). The old Path-A/Path-B (raise-`k`) plans are
**DISSOLVED** — there was no register wall, only two lines that did not faithfully realize the theorem.

**Also shipped (default-ON):** add/sub-imm12 peephole (`mov #k;add`→`add #k`; pressure-free).

**RESOLVED (compile-time perf + latent correctness, 2026-08-23 seal):** yarpgen `s0940` (& the fuzz
mega-function tail) compiled CORRECT but the optimizer took ~2.6min CPU on it — a super-linear path in
the allocator + φ-cleanup. Three algorithmic fixes, each ⟦·⟧-preserving (same result, faster walk):
(1) `interference` — full-`nt` bitvector scan per def → SPARSE live set (Σ live-set-size ≈ O(edges));
(2) `color_abi` SIMPLIFY — per-step `(0..nt).find`/`max_by_key` → min-heap worklist (O(nt log nt));
(3) `remove_trivial_phis` — O(#φ×#insts) repeated full-scan → round-based fixpoint (same unique fixpoint).
s0940 2.6min→~13s; yarpgen 250 = 0 CTIMEOUT (was the CTIMEOUT source). The speedup EXPOSED a latent
backend bug: giant-frame functions emit >imm19 (±262144-insn) spans, so a near-form `cb(n)z`/`b.cc` fell
out of range → GNU-as rejected the build (s0025/s0035/s0228 OPT-COMPILE-FAIL). Fixed with a **two-pass
emit-measure-re-emit** in `emit_ir_body` (measure the near-form body; if its newline count — an
over-approximation of insns — could exceed imm19, discard and re-emit with far forms) + far-safe forms in
`emit_cbr`'s fall-through arms. All three now PARITY; GNU-as accepts.

**DETERMINISM SEAL (2026-08-23):** while proving byte-identity of the above, discovered zcc was
**nondeterministic across runs** (pre-existing): `LICM natural_loop` returned `HashSet<BlockId>`, and the
hoist `'scan` picked the first candidate in Rust's per-process-random hash order → different hoist → a
different (still-correct) `.s` each run. A commuting-square BYTE-proof is impossible over a
nondeterministic transform, and size-progress toward 1× is unmeasurable if each build differs — so
determinism is a **precondition** for the size campaign, not a nicety. Fixed by returning
`BTreeSet<BlockId>` (program-order, theorem-clean — "process the loop body top-to-bottom", no heuristic).
A read-only audit of every HashMap/HashSet iteration in `opt.rs`+`arm64_elf.rs` (~30 sets) then found ONE
more latent hole: `out_of_ssa`'s `append_to: HashMap<BlockId,…>` was iterated to mint cycle-breaking
fresh temps from a shared counter → hash-order predecessor iteration numbered those temps differently
across runs (unobserved on the corpus — needs ≥2 cycle-breaking preds — but real). Fixed → `BTreeMap`
(sorted-by-block-id). Every other site is lookup-only or re-sorted before use (e.g. φ-materialization
sorts by temp id at `opt.rs:1637`).
Proof: same zcc + same input.c → byte-identical `.s` (md5-stable, 3 runs) on a 16-file diverse sweep
(sqlite, shell, 6 yarpgen incl. s0940, 8 csmith) = 0 nondeterministic. Cost: sqlite obj
+0.46% (the deterministic program-order hoist set differs from the old lucky hash-order subset) — a
noise-level, honest deterministic cost, reclaimed many times over by the size campaign. NB: determinism ≠
absence of heuristics — register allocation stays a HEURISTIC (coloring is NP-complete, `opt.rs` §C2); a
heuristic is deterministic + commuting-square-certified, not theorem-optimal.

---

## §2 — The one theorem: why proof-faster ≠ machine-faster (the reusable insight)

Two different categories. `⟦f⟧=⟦opt(f)⟧` lives in the category of **values** (inputs→outputs,
machine-independent) — it proves identical *output* and is **silent on cost**. Cost lives in a
different category: `C_M : IR → ℝ⁺`, the cost on machine `M` with **finite registers (k=10 GP)**,
a memory hierarchy, a pipeline. `C_M` is **not** a homomorphism of `⟦·⟧`, and crucially **the
*sign* of `C_M(opt(f)) − C_M(f)` is not decided by the IR rewrite** — it is decided by how the
rewrite collides with scarce registers. So a `⟦·⟧`-proof cannot, in principle, prove a speedup.

**Mechanism (LICM):** hoisting invariant `x=e` out of a trip-`N` loop saves `(N−1)` recomputes in
the infinite-register model, but forces `x` **live across the whole body**. If live-count exceeds
`k` at any point, the allocator **spills** → 1 store + `N` reloads → traded `(N−1)` 1-cycle ALU
recomputes for `N` 4-cycle reloads → **≈N·3 cycles slower**. `⟦·⟧` cannot see it because registers
are not in `⟦·⟧`. **Count ≠ cost.**

**The fix that stays inside CbC — the decidable pressure guard.** Let `P = max` GP-temps live at
any point in the loop (from `liveness()`, no tuned weight). Each hoist raises live-count by ≤1, so
cap `#hoists ≤ k − P` ⟹ pressure stays ≤ k ⟹ k-colouring survives ⟹ **no new spill** ⟹ each hoist
strictly deletes ops with zero added memory traffic ⟹ `C_M` strictly decreases. Speed-positivity
becomes a **theorem about a guarded transform**, not a gamble. `k` = the one Side-II ABI constant
`GP_BUDGET.k`, threaded from the backend.

**Residual honesty:** `P` is SSA-pressure, measured *before* `out_of_ssa`; φ-destruction inserts
edge copies that can bump real pressure. The guard is a sound-ish **proxy**, not airtight with the
backend — a model with its own residual (the very phenomenon, one level up). That residual is what
the box A/B closes → the guarded passes stay **default-OFF pending the box**, never flipped on
faith. Law 3 in its purest form: proven at the earliest decidable layer, *confirmed* in the box.

---

## §2b — Resource-fidelity: `k=10` is a convenience-truncation (the root of every flat pass)

The Article-E **resource-fidelity gate** asks of every resource-constant: *"is this the spec's
number, or my convenience's number?"* Apply it to `GP_BUDGET = { k: 10, ncaller: 0 }`
(`arm64_elf.rs`):

- **Spec's number (Side-II, AAPCS64 §6.1.1):** 31 GPRs. Callee-saved x19–x28 (10); temporary /
  caller-saved x9–x15 (7); arg x0–x7 (8). In a **leaf** context (no `bl`) the caller-saved temps
  are free homes too → **~18 GPRs are honestly allocatable**, minus the emitter's live scratch.
  Measured scratch footprint (this session): the emitter simultaneously uses only **{x0, x1, x2,
  x9}** — x3–x8 and x10–x18 (**15 registers**) are *entirely unused*, neither home nor scratch.
- **Convenience's number:** `ncaller: 0` means **zero caller-saved registers are allocatable** —
  the allocator was built to touch x19–x28 *only*, to avoid ever reasoning about call-clobbering.
  That is a **Side-I algorithmic shortcut wearing a Side-II costume.** `k=10` is not the ABI's
  number; it is the number that let us skip call-crossing analysis.

**Verdict under the gate:** `k=10` is a **Law-2 Side-II defect** (a spec-value wrongly injected),
not a missing feature. It fails resource-fidelity: Chaitin coloring instantiated over 10 of ~18
usable colors is not a *faithful* realization of graph-coloring allocation — it is graph-coloring
restricted to a convenient subset.

**This single defect is the common root of every "flat / INERT" verdict in §3 and §4.** LICM's
guard is `#hoists ≤ k − P`; strength-reduction needs `P + 2 ≤ k`; rematerialization only pays when
something spills. With `k=10` and matmul's inner-loop `P=13`, headroom is **negative** — so all
three are *correctly* refusing to fire (firing would spill and lose). They are not weak passes;
they are **register-starved by a truncated `k`.** The "default-OFF pending box" residual in §2 and
the "INERT on matmul" rows in §3 are the *same* symptom, one cause.

**Path A = restore `k` to the ABI-true number** (add caller-saved registers as homes; pay the
call-crossing analysis the `ncaller: 0` shortcut skipped). It is not "an optimization we chose" —
it is *fixing the Side-II defect the gate caught*, after which the already-proven LICM/SR/remat
fire on their own with positive headroom. The scoreboard target is unchanged; the lever moved from
"write more passes" to "un-starve the passes that exist."

---

## §3 — Done ledger (one line each; measured effect; on/off)

Always-on IR: const-fold · DCE · copy-prop · CSE · GVN · SCCP · CFG-simplify · register-coalescing
(biased) · backend peephole (**machine copy-propagation** + redundant + dead-move elim).

| pass | measured effect | state |
|---|---|---|
| #1 compute-into-home isel | geomean 0.98→0.81× (killed the x0-funnel at source) | ON |
| #2 addressing-mode fold | matmul 1.38→1.25× | ON |
| #3 madd fusion | geomean 0.78→0.74× | ON |
| #5 inlining (β-reduction, depth-1) | geomean 0.74→0.69×, **fib 1.38→1.05×** | ON |
| **P1 machine copy-propagation** (Tier-A, backend) | matmul inner-loop 39→32 insn, reg-reg movs 10→1; **geomean-O0 0.69→0.64**; fib/loops confirmed at O1 parity. Pressure-FREE (removing a copy frees a register). | **ON** |
| B1 lightweight alias (4-pt lattice, 1 RPO pass) | enabler for B2/B4; escape falls out free | ON |
| B2 load-elim / store→load forwarding | flat on the 4 kernels (no hot store→reload) | ON |
| B4 csel if-conversion + ldp/stp pairing | isolated win on branchy shape (csel×2, cond-branch 3→1, PAR w/ O0) | ON |
| LICM (pressure-guarded) | **INERT on matmul** — does NOT hoist the invariant adrp (§4); flat elsewhere | **OFF** |
| strength-reduction (pressure-guarded) | **INERT on matmul** — does NOT reduce the index mul (§4) | **OFF** |
| #26 rematerialization (operand-free pure defs) | **flat** — nothing spills to relieve | **OFF** |
| **add/sub-imm12 peephole** (backend) | `mov #k;add`→`add #k`; matmul inner 12→9 insn; marginal on bench (3.44→3.40) but universal + pressure-free | **ON** |
| **pointer_iv (SR + LFTR + dead-counter)** | FIRES on matmul (gcc's 7-insn form); **k=10 5.47× (spills), k=18 1.80×** ✅; equiv 102/102. Default-OFF: regresses at k=10, is FOUNDATION for the k-decouple (§4) | **OFF** |
| **backend sp-relative addressing-fold** (local slot → `[sp,#pos]`) | **SIZE lever.** Folds `sub xN,x29,#off; ldr/str [xN]` → one `ldr/str [sp,#pos]` for every foldable local access (`Lea(Local)+Load/Store` fusion `try_fuse_local` + the `tmp_load/store` spill path). **sqlite3.c: 1.95M→1.05M insn (−46.3%), .text 7.89M→4.28M B (−45.8%), `sub` 728k→79k (−89%); gap-to-gcc 8.1×→4.4×.** Guarded: `!fhasvla && !fdynstack(alloca) && sp_at_base` (ir_call_abi/ir_asm clear it mid-marshalling) — same effective byte (`sp=x29−frame_total`), machine-translation-validated. Bench-perf FLAT (kernels are register-resident; the win is on memory-heavy real code + as/ld + compile-speed). | **ON** |
| **redundant-load-after-store** (backend, store→load identity) | **SIZE lever, airtight at ANY opt level.** Deletes `ldr xN,[sp,#m]` immediately preceded by `str xN,[sp,#m]` — value-independent no-op (∵ adjacent ⟹ ρ(xN)=μ[m] already). Frame-slot (`[sp,`) only ⟹ never volatile/aliased ⟹ valid even on the -O0 path (runs before the regalloc-gated move peepholes; now gated `!has_volatile` — defensive). **sqlite3.c: ldr 319k→153k (−52%), total 1.05M→879k insn (−15.9%); gap-to-gcc 4.41×→3.71×.** 166,019 pairs = 52% of ALL loads = the value-contract's per-use spill/reload, invisible to IR-level B2 (§5). The veteran machine-level pass: gcc `postreload-cse`, LLVM MachineCSE + store→load forwarding, QBE `load.c`. | **ON** |
| **type-aware volatile** (frontend: `TyTab.vol` bit rides the TypeId to each lvalue) | **SIZE unlock — turns opt ON for real code.** Was: any file-scope `volatile` (musl typedefs) → whole-TU O0 ⟹ opt DARK on every program that includes libc. Now: `Func::has_volatile` computed TYPE-accurately per function (node-range scan), `has_global_volatile` deleted (a volatile global is read through a volatile-typed node → flags the function directly). **sqlite3.c 1,045,515 → 603,513 insn (−42.3%), gap-to-gcc-O1 6.65×→3.84×** (spills collapse as regalloc finally runs). Sound: volatile access always lowers from a volatile-typed node ⟹ flagged ⟹ -O0; verified on local/pointer/global/complex. Zero IR change (access TypeId already on Load/Store). See §3b. | **ON** |

**P1 note (levels are distinct):** copy-propagation is a **backend peephole on emitted `.s` text**
(post-isel). It does NOT touch the SSA IR, so it does NOT change the SSA pressure the LICM guard
reads — P1 and the loop passes (§4) are at *different levels* and P1 does **not** unblock LICM. Its
win is real where movs are on the critical path (call-heavy fib, loops); on matmul, whose bottleneck
is address arithmetic not movs, it is small. Self-certified: machine translation-validation
(opt-parity **1552 PARITY / 0 DIVERGE**) + torture **1378/0** + cargo **100/100**. The one subtle
bug it had — treating a truncating `mov w,w` as a 64-bit copy — is fenced (`parse_mov_xx`, x-only).

**Gates green, full stack default-ON:** cargo **96/96** · torture **1378/0** · opt-parity **1552/0
PARITY** · csmith 300 = **254/0** (rest = skip). Default build output **byte-unchanged** by the
guarded-OFF passes (only an inert `gp_k` param threaded).

**Why the OFF three are not waste (the WIN-or-FOUNDATION gate):** each is proven `⟦·⟧`-preserving +
speed-safe and shipped OFF; their lasting value is the **pressure-guard infrastructure** (measured
`P`, `k−P` headroom) that any pressure-aware backend work reuses. That is the *foundation* leg of
the gate. As standalone wins they are flat — so **no further investment in IR scalar opt**.

### §3b — Per-function volatile gate (Law-2 Side-I fix) + the sqlite ceiling

**The defect (whole-TU volatile gate).** The opt gate was `!ast.has_volatile` — a whole-TU token
scan: a SINGLE `volatile` token *anywhere* forced the ENTIRE translation unit to -O0. The IR does
not model volatile, so ⟦·⟧-preservation is proven only for volatile-free code — but the *faithful*
scope of that constraint is **per function**, not per TU. Disabling opt for every function because
one function (or a header typedef) mentions volatile is a convenience-truncation, a Law-2 Side-I
defect (algorithm not faithfully realizing its side), exactly the §2b pattern one level up.

**The fix (proven sound).** Optimize function `f` iff `f` is volatile-free — its *definition span*
(return type + params + body) carries no `volatile` token (`Func::has_volatile`) — AND the TU has no
file-scope volatile (`Ast::has_global_volatile`: a `volatile` token outside every function span).
SOUNDNESS by scope: every volatile *access* needs a volatile-qualified type in scope at the access;
that type's `volatile` token is EITHER in the accessing function's own span (→ that function stays
O0) OR at file scope (→ `has_global_volatile` → whole-TU O0). No volatile access can hide in a
function whose span AND file scope are both volatile-free. Inlining respects it too (a volatile
callee is never spliced into optimized code — `callee_ok` in `opt::inline`). Measured on a mixed TU:
`hot()` optimizes (0 spills, x19–x28 regalloc) while `vol()` (local volatile) stays O0. Gate green
(torture 1378/0, opt-parity 1552/0, csmith 254/0 — csmith stresses volatile heavily).

**The sqlite ceiling this exposes (the #1 remaining SIZE lever, ~5× the sp-fold).** Forcing opt ON
over the volatile gate measures the ceiling: **sqlite3.c 1.05M → 548k insn (−47.6%), gap 4.4×→2.3×**
(str 287k→23k −92%, ldr 319k→57k −82% — the spills collapse under regalloc). The per-function gate
does NOT reach it: **musl's threading typedefs** (`pthread_mutex_t` &c. in `bits/alltypes.h`) carry
`volatile` members at FILE scope ⟹ `has_global_volatile` ⟹ sqlite stays whole-TU O0. That is the
*correct conservative* verdict (a volatile struct member COULD be accessed). Breaking it requires
**type-aware volatile**: retain the qualifier in the type system, mark the Load/Store volatile at
lowering from the accessed lvalue's type, and have passes skip only those — the veteran approach
(LLVM volatile MemoryDef). Then never-accessed header typedefs stop poisoning the TU. This is a
frontend change (Article-B boundary) = **the next size batch**, deferred here as its own careful
work under the full csmith/yarpgen volatile gate.

**DONE — type-aware volatile SHIPPED (this batch).** Carried the qualifier as a bit PARALLEL to
`TyTab.tys` (`vol: Vec<bool>`, `is_volatile`/`volatile_of`) rather than a `Ty` variant — qualified &
unqualified share the same `Ty` (all size/align/signedness queries ignore the bit) and differ only as
distinct interned TypeIds, so the qualifier rides the TypeId through decl→typedef→pointee→member to
each lvalue node's type and thus onto the IR `Load`/`Store` it lowers to — **zero IR-field / lowering
change** (the access TypeId was already on `Load`/`Store`). Gate flags recomputed TYPE-accurately:
`Func::has_volatile` = any node in the function's arena range (or any param) has a volatile-qualified
type; `Ast::has_global_volatile` DELETED — a volatile global reached here is read through a
volatile-typed `GVar`/`Deref` node, so the *access* (not the token position) flags the function. This
**closes the volatile-typedef-used-in-a-function hole** the token scan could not see, and — the point
— stops musl's file-scope volatile typedefs from poisoning the whole TU. One regression found & fixed
by the reject-diff: `cplx_elem` matched complex types by exact TypeId, so a `volatile _Complex`
(distinct id, same `Ty::Struct`) was unrecognized → made volatile-agnostic (match by struct index;
complex-7 restored). Backend `drop_redundant_loads` gated `!has_volatile` (defensive — makes its
soundness local, not reliant on the sp-fold invariant). **Measured (same mnemonic-line metric on all
three files): sqlite3.c 1,045,515 → 603,513 insn (−42.3%), gap-to-gcc-O1 (157,074) 6.65× → 3.84×** —
approaching the §3b force-opt ceiling (−47.6%); the residual gap to it = functions that GENUINELY
touch volatile types, correctly kept -O0. Gate: cargo 110/110, torture 1378/0 (reject-diff vs
pre-change = 0 new), opt-parity 0 DIVERGE, complex-7 runs exit-0.

**Post-unlock histogram (zcc 603k vs gcc-O1 157k) — the gap RE-PROFILED; new levers, ranked:**

| mnemonic | zcc | gcc-O1 | ratio | excess | lever |
|---|---|---|---|---|---|
| **mov** | 178,618 | 33,608 | **5.3×** | ~145k | **coalescing / kill canonicalization-mov (the #1 lever now)** |
| **b** | 65,656 | 8,466 | **7.8×** | ~57k | block-layout / jump-threading / redundant-branch-elim |
| ldr+str | 110,785 | 31,794 | 3.5× | ~79k | regalloc spill quality (measured spill count vs gcc — a REAL k-gap, distinct from the dissolved pointer-IV "wall") |
| sub | 55,886 | ~few k | large | ~50k | addressing-fold residual (Lea multi-use, param-spill, OOR) |
| sxtw | 20,031 | ~0 | ∞ | ~20k | **sxtw-elim (Move 2)** — canonicalization tax gcc doesn't pay |

**3.84× is tcc-tier, NOT the finish** (user: match O1 on size AND speed, no other option). This batch was
the PREREQUISITE — opt was OFF on every real program (all include musl → file-scope volatile typedef →
whole-TU O0), so every size/perf lever was dark on real code; now they apply. **Next size batches, ranked
by the table:** (1) **mov / copy-coalescing** — ~145k excess, also a hot-loop perf lever (both axes at
once); decompose the 178k mov into canonicalization-mov vs φ-copy vs spill-reload first. (2) branch/block
layout. (3) sxtw-elim. (4) regalloc spill quality (this is where k=10 genuinely bites on SIZE — measured,
not the misdiagnosed perf "wall").

---

## §4 — Next: close the O1 gap = make the loop passes fire on matmul + sieve

**The whole remaining job is O1 parity on the two loop-nest kernels** (§1). P1 removed the x0-funnel
(pressure-free win, done); backend isel is *not* the lever it was framed as — the matmul bottleneck
is **address arithmetic** (invariant `adrp`/base recomputed + index `mul`s per iteration), which is
**IR loop-optimization territory**, exactly what gcc-O1 does.

**CONFIRMED, box-measured (the §4 diagnostic, now run):** turning the existing loop passes ON
(`ZCC_OPT_ON=licm,strength_reduce,remat`) changes matmul by **NOTHING** — adrp count stays 7, main
insns 217→218, runtime 230µs→230µs, output identical. So the passes are **INERT on the matmul shape**,
not merely OFF. The open causes narrow to **(a)** the SSA pressure guard reads `P ≥ k` in the inner
loop and caps hoists at 0, and/or **(c)** the invariant address `Lea(A)`/`Lea(B)` and the affine index
`k·8`, `k·1920` are not in the single-def form LICM/SR recognize. Cause **(b)** (genuinely pressure-
bound) is *unlikely* — gcc-O1 keeps the same kernel register-resident, so the registers exist.

**RESOLVED, box-measured (2026-08-22, SSA-level `ZCC_DBG_LICM` + `ZCC_DUMP_IR` + k-probe isolation).**
The diagnostic ran to the bottom; both open causes are now answered, and the answer **inverts the
plan**:

- **Cause (a) is real but NOT the lever.** Inner k-loop: `P=13, gp_k=10, headroom=0, candidates=4`
  (Cast·i, Cast·j, Lea·A, Lea·B) — LICM refuses, SR refuses (`P+2=15>10`). So at k=10 the passes are
  register-starved, exactly as (a) said.
- **But un-starving them does NOT reach O1.** Isolation probe raising `k` (extra colors → caller-saved
  regs the loop doesn't scratch), full LICM+SR firing: matmul **3.47× → 2.92×** (correctness-gated,
  output identical to gcc). k=16 (partial): 3.30×. **Ceiling of (raise-k ⊕ current LICM+SR) ≈ 2.92× —
  the gap to gcc-O1 (1.0) does NOT close by adding registers.** So **Path A (raise k / free caller-saved
  homes) + the scratch-band decouple it needs are OFF the O1 critical path** — proven insufficient.
  `k=10` stays a real resource-fidelity debt (§2b) but is **parked**, not pursued.
- **Cause (c) is the lever — the current SR is the wrong SHAPE.** gcc-O1's inner loop (measured) is the
  textbook pointer-IV + LFTR form, **6 insns, zero `mul`, zero `adrp`:**
  `ldr x4,[x2],8 · ldr x3,[x0] · madd x1,x4,x3,x1 · add x0,x0,1920 · cmp x0,x5 · bne`.
  Two marching pointers (A post-indexed `+8`, B `+=1920`), loop terminated by **comparing a pointer to
  a precomputed limit** (LFTR), madd-fused. zcc's inner loop *even fully un-starved* KEEPS a per-iter
  `mul x,k,stride` + `add base,index` + counter `cmp` — the current `strength_reduce` builds a derived-IV
  accumulator but never **folds the base into the IV init**, never does **LFTR**, never reaches pointer
  form. That residual multiply + address-add + live counter is the whole 2.92→1.0 gap.

**THE ONE REMAINING LEVER (revised §4 target): a proper pointer-IV strength-reduction + LFTR pass.**
- Recognize `Load/Store [ base_invariant + iv·stride ]` (iv = the loop's basic induction φ).
- Materialize a pointer IV: `p = base` in the preheader, use `[p]`, `p += stride` on the latch.
- **LFTR:** replace the counter test `iv < N` with `p != limit` (`limit = base + N·stride`, preheader-
  computed) so the counter and its `mul`/`cmp` die.
- Backend then folds `[p]`,`p+=stride` into post-indexed `ldr x,[p],#stride` (small isel peephole; madd
  already fused, #3).
- **Crucially PRESSURE-REDUCING:** two pointers + accumulator + limit ≈ 4 live values REPLACE {counter,
  base, i·stride, index-mul-result} — so it fits under **k=10 with NO Path A.** Cost falls, pressure
  falls: the guard is satisfied trivially, not fought.
- Ships under the CbC gate: commuting-square `⟦f⟧=⟦sr(f)⟧` (induction proof already in §-SR theorem,
  extended with the base-fold + LFTR legs) + machine translation-validation (opt-parity 0 DIVERGE) +
  isolation number on matmul/sieve BEFORE default-ON.

---

**RESULT (this batch, box-measured) — the pass is BUILT and correct; ONE §4 prediction was wrong.**
`opt::pointer_iv` implements exactly the above (recognize `base+iv·stride` through Cast/Mul/Shl/**Copy**;
materialize pointer φ + `p+=c·stride`; LFTR the counter test to `p<limit`; DCE-accurate liveness so the
orphaned index chain doesn't block LFTR; dead-counter-cycle elimination since naive DCE can't kill a
self-referential φ↔increment). Plus an add/sub-imm12 backend peephole so `p+=stride` is one `add #k`, not
`mov;add`. cargo 102/102, equiv green on 4 kernels, LFTR fires (matmul inner loop = gcc's 7-insn form).

**The wrong prediction: "pressure-REDUCING ⟹ fits k=10 with NO Path A." FALSE — measured.** matmul's inner
loop is `P=13` (diagnostic, headroom=0 at k=10). pointer-IV kills the counter (−1) but materializes TWO
marching pointers + a limit (+3 long-lived) — **net pressure does NOT drop below 10.** At k=10 the full
reduction SPILLS: **matmul 3.42 → 5.47× (WORSE).** Reducing only ONE pointer avoids the spill (2.11×) but
leaves the other index explicit. So pointer-IV alone is NOT a k=10 default win.

**The real synthesis: BOTH the right pass-shape AND enough registers are needed — neither alone.**
- pass-only, k=10:  5.47× (spill)  — the pass without registers
- k-only, OLD SR:   2.92× (ceiling) — registers without the right pass (the earlier probe)
- **pass + k=18:    1.80×** ✅ — both together reach the finish

This **puts Path A (raise k) BACK on the O1 critical path** — the earlier "2.92× ceiling ⟹ Path A off-path"
verdict was measured against the *old* SR shape; with the correct pointer-IV shape, raising k reaches 1.80×.
`k=10` is no longer just a §2b fidelity debt — it is **the** remaining O1 blocker.

### §4-next — two safe paths to bank the 1.80× (pick one; both keep k=10 correct)

Raising k is a **documented hang graveyard** (`arm64_elf.rs:35–38`, pr64006). Do it carefully or route around:

- **Path B (lower risk, partial): pressure-guarded pointer-IV, default-ON at k=10 → ~2.1×.** Reduce only as
  many pointers as fit under `k−P` (reuse the LICM pressure guard, §2). At k=10 that reduces 1 pointer →
  matmul ~2.11×, sieve ~2.1× — a real default-ON win banked NOW, pure-IR (equiv-gated), ZERO register risk.
  Banks ~40% of the gap. Does not reach 1.80×.
- **Path A (higher risk, full): scratch-band reclaim → k≈15 → pointer-IV default-ON → 1.80×.** Methodically
  move wide-lowering scratch (struct-copy, VLA, atomics, **ext.rs overflow x14/x15**) off x11–x15 into a
  reserved band, set `GP_BUDGET {k:15, ncaller:5}` (caller-saved x11–x15 confined to non-cross-call temps
  by the existing `abi_alloc` ncaller machinery), gp_phys maps colors→physregs. Re-hammer struct/overflow
  paths with csmith BEFORE trusting it (that is where pr64006 bit). Reaches the finish; touches Article-F
  ABI/frame/atomics code, so it is its own careful batch, not a tail-end edit.

**STOP when geomean-vs-O1 ≈ 1.0.** Measure before writing: the pass enters only if it shows, on
matmul/sieve, the inner loop collapsing toward the 6-insn form and the vs-O1 ratio toward 1.0.

---

## §5 — QBE cross-check verdict (why the IR is already done)

QBE's stated goal is ours (70% perf, 10% code). Source-verified against `ref/qbe/`: QBE hits its
target with **no GVN, CSE, LICM, PRE, strength-reduction, unrolling, vectorization** — all of which
zcc already has. So **on the mid-level IR, zcc is already past QBE.** QBE's entire edge is three
theorems in **memory + register allocation**, not the IR:

- **alias** (`alias.c`, 167 LOC) → **B1 DONE** (4-point base lattice + offset intervals, 1 RPO pass).
- **loadopt** (`load.c`) → **B2 DONE** (store→load forwarding gated on the alias oracle).
- **spill+rega** (Hack chordal alloc + Belady spill) → **BELOW the CbC-purity line, NOT built:** the
  colorability kernel is a clean theorem, but it is inseparable from a **tuned loop-weight spiller**
  (empirical constant, no provenance) → fails the admission test. Register allocation is the one
  place zcc already sits below the purity line (Chaitin, NP-hard by necessity); we do not grow it.

**B3 (sxtw-elim) deferred:** low reward here (backend already ext's at def, width≥4) + high
miscompile risk (the lazy width<4 canonicalization that tripped the pr81913 load-elim bug). Reward <
complexity tax.

**Prioritization consequence:** more IR passes (Tier-2 PRE/VRP/reassoc/ADCE/IV-elim, Tier-3 loop
work) are **past the 10% budget** — QBE reaches 70% without any of them. Defer each until a real
`.c` demands it.

---

## §6 — Catalog (REFERENCE SHELF — not a plan; decisions come from §1)

Kept only so a future item has its theorem + proof-obligation on hand. Building any of these
requires a §1-measured gap pointing at it **and** the §4 gate (win or foundation, measured first).

**Tier-2 (classical IR, payoff needs values register-resident):** #6 PRE/lazy-code-motion
(Knoop–Rüthing–Steffen; subsumes CSE+LICM) · #7 value-range propagation (interval lattice) · #8
reassociation/instcombine (ℤ/2ⁿ ring identities) · #10 ADCE (post-dominator frontier) · #11
IV-elim/LFTR.

**Tier-3 (loop restructuring, Rice-boundary — trip-count/dependence):** #12 unrolling · #13
unswitching · #14 scalar-replacement of array elements · #15 interchange/tiling/fusion (polyhedral;
the real -O3 matmul cache win, large) · #16 auto-vectorization (SLP+loop NEON).

**Tier-4/5 (IPA / machine):** #17 TCO · #18 IPA-CP (amplifies inlining) · #20 dead-fn/global elim ·
#22 instruction scheduling · #23 ldp/stp (**done in B4**) · #24 csel (**done in B4**) · #27 block
layout/branch alignment.

Each carries the same CbC obligation: ship with the commuting square `⟦f⟧=⟦pass f⟧` (IR passes, via
`equiv`) **or** machine translation-validation (backend passes, via opt-parity 0 DIVERGE), + torture
0 FAIL, + the perf delta measured in the box — never asserted.

---

## §7 — IR contract + opt.rs↔theory audit (compact; the durable bits cook into THEORY/SEMANTICS)

**IR shape (settled, shipped, SSA):** typed linear 3-address, explicit CFG (blocks + one terminator
each), explicit memory (Load/Store, no implicit lvalues). **CORE vs OPAQUE split:** passes touch
only CORE (Bin/Un/Load/Store/Lea/Cast/Call/Copy/Select + terminators — interp-evaluable,
verifier-covered); the exotic tail (atomics, Overflow, Asm, Va*, Alloca/VLA, SRet, nested-fn tramp,
TLS) is wrapped OPAQUE and lowered 1-to-1, untouched by passes. *(NB: the old `IR.md` "NON-SSA
settled" is superseded — the fork went SSA via Braun mem2reg + Cytron out-of-ssa.)*

**The 3-artifact contract (bug-resistance of the standard):** (1) **verifier** — well-formedness
automaton run after each pass (typed, def-before-use, one terminator/block, no dangling refs); (2)
**interp** — reference evaluator = semantic ground truth (`SEMANTICS.md` LEVEL-1, state Σ=⟨ρ,μ⟩);
(3) **commuting square** — every pass must commute with interp, lifted to an **executable theorem**:
`commuting_square_structural_exhaustion` (312 exprs × 5 passes = 1560 squares) + `_selfproof`
(anti-blindness). A pass may be written only when all 4 parts are stated: input invariant · rewrite
rule · preservation theorem · output invariant. UB filter is a root rule.

**opt.rs audit verdict (Law Zero, reproducible by `grep`):** every non-index numeric literal is
discharged to a spec table (`TyTab` LP64 · ARM ARM x0–x30 · AAPCS64) or a value-numbering
injectivity tag; the one flagged construct `for _ in 0..32` (fixpoint cap) is discharged by the
**correctness-invariance theorem** (composition of ⟦·⟧-preserving passes is ⟦·⟧-preserving for any
iteration count ⟹ the cap affects only *how-optimized*, never `⟦f⟧`). Every function maps to a named
theorem (Braun SSA · Cytron out-of-ssa · Wegman–Zadeck SCCP · Alpern–Wegman–Zadeck GVN ·
Cocke–Kennedy CSE · Chaitin–Briggs regalloc · Aho dominance/loops · Allen–Cocke–Kennedy SR). The two
correctness-bearing operator families (`wrapping_*` = the ℤ/2ⁿ ring; `dom`/`degree<k`/`defcnt==1`
predicates) are exactly the operators their theorems require. **No line lies outside {theory ∪
spec}.** A future edit adding a constant/operator updates this section (then, at opt-end, THEORY.md).
