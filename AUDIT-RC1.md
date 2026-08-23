# AUDIT-RC1 — Release-Candidate-1 codebase audit (ssa-qbe fork)

Date: 2026-08-24. Three parallel adversarial auditors over the whole optimizer +
backend, per the overnight runbook (OPT.md §0 Phases B/C). Governing lens: Law-1
(every LOC ↔ theorem∪spec), Law-3 (every pass carries its commuting-square /
translation-validation), Article E (every constant is the spec's number).

## Verdict

**No miscompile. No unprovenanced constant. RC1 is clean.**

- **Side-I (LOC ↔ theorem):** every optimization pass carries an in-comment
  `⟦f⟧=⟦P(f)⟧` commuting-square (or, for regalloc, a renaming-bisimulation); every
  backend text-peephole is built on the safe side of the shared `reg_uses` oracle
  (unknown ⟹ boundary ⟹ KEEP/flush). No LOC found outside theorem∪spec space.
- **Side-II (constant ↔ ultimate-fact):** every numeric bound traces to a real
  ARMv8-A / AAPCS64 / ELF fact (imm7/imm12/imm9 ranges, ubfx/ubfiz field math,
  register-31 ZR-vs-SP discipline, budget→physical maps, 192 B / va_list / stack-arg
  offsets, relocations, section flags). No magic number.
- **Determinism:** verified byte-identical (Phase E, below).

## Actioned findings (all committed in the RC1 hardening commit)

1. **`post_index` — store `Rt==Rn` writeback (Side-II over-claim → HARDENED).**
   The guard excluded the CONSTRAINED-UNPREDICTABLE `mem xP,[xP],#k` only for loads
   (`is_load && rt==base`). ARMv8-A makes base-writeback with transfer-reg == base-reg
   (base ≠ 31) UNPREDICTABLE for **stores too** (a store may write an UNKNOWN value,
   not the pre-increment one). Guard tightened to `rt==base` (both directions); comment
   corrected. sqlite insn count UNCHANGED (296,591) ⟹ the pattern is never emitted; the
   fold declined nothing real, the hole is now closed in principle. Also relabeled the
   `0<k≤255` bound as the *positive subset* of simm9 −256..255 (the `sub`/negative half
   is a Law-4 coverage residual, not a bug).

2. **`cbz_fuse` — cross-block NZCV invariant (provenance gap → DOCUMENTED).**
   The flag-liveness scan inspects only the fall-through successor, not the taken-branch
   target. Sound under a standing zcc invariant — **NZCV is never live-IN to a basic
   block** (SSA lowering emits each flag producer and its consumer within one block,
   producer-before-consumer) — which holds by construction and is confirmed by the gate,
   but the local proof was incomplete. The invariant is now stated explicitly in-comment.

3. **Test-coverage gaps (Phase D → CLOSED).** Added inline teeth tests:
   - `pointer_iv_declines_scalar_loop` — the pass declines on a loop with no
     pointer-linear term (returns 0, ⟦·⟧ preserved).
   - `dead_static_fns_gate_has_teeth` — unreferenced static removed; called /
     address-taken / exported functions kept (the one pipeline stage previously with
     zero unit tests).
   - Levers 5/6/7 backend peepholes: `post_index` (fold + 3 teeth), `cbz_fuse`
     (eq→cbz / ne→cbnz + flags-live teeth), `drop_wform_sxtw` (5 cases),
     dead in-place sxtw. cargo 122 → 136.

## Confirmed-sound, no action (informational)

- **Lever 7 `drop_wform_sxtw` (R1) + dead-sxtw (R2):** every value-observing reader of
  bits 32..63 is an x-form read; x-registers are unsplittable comma tokens, so
  `token_present` cannot miss one; partial-writers (bfi/bfxil/ins) are absent from
  `reg_uses`' lists ⟹ degrade to boundary ⟹ KEEP. `reads==writes` is a correct in-place
  guard. Sound.
- **`reg_uses` oracle:** no GP-writer is ever classified read-only; over-approximation
  is always toward KEEP/flush. Real GP-writers `subs`/`smull`/`umull`/`negs` fall to the
  `else` ⟹ boundary ⟹ flush (under-optimization, never a miscompile).
- **All opt/ passes:** to_ssa, out_of_ssa (swap/lost-copy handled), licm (default-OFF),
  strength_reduce (ℤ/2ⁿ-exact), cse (load-cache killed by-construction), gvn, sccp,
  inline (whitelist gate), regalloc (interference-invariant + verify_abi). Gates decline
  correctly on address-taken / volatile / computed-goto / VLA / aliasing / pressure.
- **Standing invariant to guard going forward:** volatile correctness is now a
  PER-ACCESS `is_volatile_access` check in every memory pass (the whole-function -O0
  fallback was removed); `equiv` is blind to access multiplicity, so any *future* memory
  pass that omits the check would silently miscompile volatile code. Covered today by the
  `volatile_accesses_preserved` multiset teeth test.
- **Honest Law-3 exceptions (documented, not defects):** `fwd_canonical`
  store→load forwarding width-1/2 hazard is guarded (`Val::Tmp ⇒ size≥4`) but `equiv`
  cannot see it (interp canonicalizes eagerly) ⟹ the torture corpus is its oracle.

## Phase E — determinism (the RC1 gate)

Same input → byte-identical output, verified in-box:
- sqlite3.c: `.s` IDENTICAL (3× compiles), `.o` IDENTICAL (3×).
- 2 csmith cases (c0001, c0005): `.s` + `.o` IDENTICAL.
- 1 yarpgen case: `.s` IDENTICAL.

**zcc is 100% deterministic.**
