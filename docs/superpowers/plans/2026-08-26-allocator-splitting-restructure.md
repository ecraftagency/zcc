# Allocator Splitting Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete zcc's SSA register allocator with a live-range-splitting layer so it keeps values register-resident in low-pressure regions and spills only across tight regions — closing the sqlite size gap (1.16× → toward 1.0×) that is gated by register headroom.

**Architecture:** Generalize the spiller's existing dominance-only cross-edge carry (`carried`, spill.rs:577) to full SSA reconstruction: insert a block-parameter (phi) at any join or loop header where a value is register-resident at *some* predecessors, each predecessor supplying its holding register or a cold-edge reload. Eviction becomes a *regional split* instead of a whole-web spill. `destruct` already lowers block-params to parallel edge copies and `mir::verify` already checks the SSA property, so no new proof machinery is needed.

**Tech Stack:** Rust, zero external crates. Target AArch64-ELF Linux (musl). Docker box for the authoritative gate.

**Spec:** `docs/superpowers/specs/2026-08-26-allocator-splitting-restructure-design.md` (read it first — the plan argues from it).

## Global Constraints

- **Correctness-by-construction:** every code task ships a commuting-square test (the regalloc `same()` battery in `src/regalloc/tests.rs`) AND the effect assertion proving the pass fired. A green square on inputs that do not spill/split is vacuous and `tests/provenance.sh` will reject it.
- **The obligation is `⟦mir_before_alloc⟧ = ⟦mir_after_alloc⟧`** — the interpreter runs both sides and requires equality. `same(src)` and `same_all(&[...])` in `src/regalloc/tests.rs` do this.
- **`mir::verify` is the SSA net:** it checks every reload is dominated by its spill, no vreg survives, no parallel copy is left, SSA is well-formed. It runs inside the gate. Never bypass it.
- **Full gate is the release net:** `sh tests/fullsuite.sh` (self-builds a release musl ELF, runs in the docker box, ~400s): provenance, shape/cpp/decay/alg/abi, determinism 88×8, torture, opt-parity, csmith 300, yarpgen 300, musl. Must be 15 PASS / 0 RED at completion.
- **Predict on the model before building:** `ZCC_REL=1 sh tests/box.sh s 'SQ=/suites/sqlite; ZCC_SPILLCEIL=1 zcc -S -o /dev/null "$SQ/sqlite3.c" 2>/tmp/c.txt; awk "/^SPILLCEIL/{tot+=\$3;ceil+=\$4;inl+=\$6;allp+=\$7;allpl+=\$9;rm+=\$10} END{printf \"reloads=%d ceil=%d inloop=%d allpreds=%d allpreds-in-loop=%d remat=%d\n\",tot,ceil,inl,allp,allpl,rm}" /tmp/c.txt'`
- **KPI is raw sqlite frame `str`+`ldr`** (never "slot-touches"): `ZCC_REL=1 sh tests/box.sh s 'SQ=/suites/sqlite; zcc -S -o /tmp/z.s "$SQ/sqlite3.c" 2>/dev/null; awk "(\$1==\"ldr\"||\$1==\"str\")&&/\[(sp|x29)/{c[\$1]++} END{print \"frame ldr\",c[\"ldr\"],\"str\",c[\"str\"]}" /tmp/z.s'`. Baseline (RC4): frame `str` 11,316 + `ldr` 10,675 = **21,991**; gcc-O1 = **12,721**. sqlite total insn baseline **182,956 = 1.1648×** (gcc 157,074).
- **One bounded Law-2 attempt, then quarantine.** If a task cannot green the gate after one bounded fix, revert `mir-rearch` to the RC4-equivalent state (`git reset --hard rc4` on a scratch branch, or cherry-pick out the failing task), mark BLOCKED with the measured reason. **Fallback: tag `rc4` on `main`.**
- **Branch:** work on `mir-rearch`. `main`/`rc4` is never touched during implementation.

---

## Context primer — how the spiller works (read before Task 1)

`src/regalloc/spill.rs`, `spill_with()` is a fixpoint. Each round: compute
liveness, call `simulate()`, which walks blocks in RPO maintaining a per-block
working set of register-resident values (`Res`), evicting by Belady next-use
distance to keep pressure ≤ `k`. `simulate` returns:

- `Sim::Plan(Plan)` — success. `Plan { reloads, subs, ncopies, wexit }`:
  - `reloads[b]` = `(before_inst_i, value, copy_id)` reloads to insert;
  - `subs[b]` = `(at_inst_i, value, copy_id)` — read `value` from `copy` here;
  - `wexit[b]` = **what is register-resident when block b ENDS**, each under its
    name (`(VReg, Option<CopyId>)`; `None` = original name, `Some` = a reload
    copy). **This is the residency the successors' entry sets are built from.**
- `Sim::More(Vec<VReg>)` — these whole values must become memory-resident; the
  fixpoint adds them to `spilled` and re-runs.

The entry-set builder is `carried` (spill.rs:577–594): a value is carried into
block `bi` (no reload needed) only when **every** predecessor holds it in `exits`
under the same `(v, copy)` key — the dominance special case (R4.1). Back-edge
predecessors (loop headers) are not yet simulated, so they hold nothing → loop
residency restarts each iteration. `apply()` (spill.rs:1058) materializes the plan
into `Spill`/`Reload`/`Copy` MInsts.

Block-parameters already exist in MIR: `MBlock.params: Vec<Reg>` and
`MTarget.args: Vec<Reg>` (the phi and its per-edge arguments). `destruct` lowers
them to parallel edge copies. So "insert a phi" = push a fresh vreg onto the
header's `params`, push the reaching value onto each predecessor terminator's
`MTarget.args`, and rename the uses.

The commuting-square test pattern (copy this for every task):

```rust
// in src/regalloc/tests.rs — same(src) runs the interpreter before and after
// allocation and asserts equal return value from main().
same_all(&[
    "int e(int x); int hot(int p){ /* spill-forcing body */ } int main(void){return hot(2);}",
]);
```

---

## Task 0: Baseline and model prediction (no code)

**Files:** none (measurement only).

- [ ] **Step 1: Confirm the tree is at RC4-equivalent and gate-green**

Run: `git -C . log --oneline -1` (expect the R4.16 commit or later on mir-rearch), then `sh tests/fullsuite.sh`
Expected: `== 15 PASS / 0 RED ==`

- [ ] **Step 2: Record the KPI baseline**

Run the KPI command (Global Constraints). Record: frame `str`/`ldr`, sqlite total insn.
Expected: frame str≈11,316 ldr≈10,675 (=21,991); sqlite≈182,956.

- [ ] **Step 3: Record the model prediction**

Run the `ZCC_SPILLCEIL` command (Global Constraints). Record: reloads, ceiling, in-loop, all-preds, all-preds-in-loop, remat.
Expected: reloads≈12,479, in-loop≈7,661, all-preds-in-loop≈780.

- [ ] **Step 4: Write the baseline into the plan's progress log**

Append the three numbers to the top of this file under a `## Progress` heading so later tasks measure against them.

---

## Task 1: Register-residency fixpoint across back-edges (spec §4.4 — the delicate core)

**Files:**
- Modify: `src/regalloc/spill.rs` (the fixpoint loop ~121–160 and `simulate`'s use of predecessor `exits`)
- Test: `src/regalloc/tests.rs`

**Interfaces:**
- Consumes: `Plan.wexit` (per-block register-residency at exit), `cfg.preds`, `cfg.rpo`.
- Produces: a per-block `entry_resident: Vec<Vec<(VReg, Option<CopyId>)>>` computed from ALL predecessors' `wexit` INCLUDING back-edges, available to `carried` in the next round. The fixpoint reads the PRIOR round's `wexit` for not-yet-simulated (back-edge) predecessors.

- [ ] **Step 1: Write the failing test — a loop-carried value must be register-resident at the header after the fixpoint converges**

```rust
#[test]
fn residency_carries_across_the_back_edge() {
    // A value used every iteration, register-held at the latch, must be marked
    // register-resident at the loop header once the fixpoint converges — not
    // reloaded fresh each iteration. Meaning must be preserved regardless.
    same_all(&[
        "int e(int); int hot(int p){int s=0,i;for(i=0;i<20;i++)s+=e(i)+p;return s+p;} int main(void){return hot(3);}",
        "int e(int); int hot(int p,int q){int s=0,i;for(i=0;i<15;i++)s+=e(i)*p+q;return s;} int main(void){return hot(2,5);}",
    ]);
}
```

- [ ] **Step 2: Run it to verify it passes for MEANING but capture the reload count as the effect baseline**

Run: `cargo test -q residency_carries_across_the_back_edge`
Expected: PASS (meaning holds even before the change — this test guards it). Then compile the first program with `ZCC_NOPROMOTE=1` and count `MInst::Reload` in `hot` before the change (record it; Step 6 asserts it drops).

- [ ] **Step 3: Implement the two-lattice fixpoint**

In `spill_with`, keep the existing memory-residency fixpoint (monotone, adds to `spilled`). Add a SECOND fixpoint over register-residency: after each `simulate` round produces `wexit`, recompute each block's entry-residency using ALL predecessors' `wexit` (back-edges read the prior round's value; the first round treats them as empty). Iterate the pair until BOTH stabilise. Bound: `|vregs| * |blocks| + 2` rounds; exceeding it is a `debug_assert!`-backed Law-2 defect, not a budget. Termination argument: memory-residency only grows (≤|vregs|), register-residency at a block entry only grows within a round's fixpoint and is bounded by liveness — the product is finite. Write the termination reasoning as a doc comment citing spec §4.4.

- [ ] **Step 4: Feed the back-edge residency into `carried`**

Change `carried` (spill.rs:577) so a back-edge predecessor is no longer treated as "holds nothing": use its prior-round `wexit`. For THIS task, only PROPAGATE the residency (mark the value carriable); the block-param insertion that makes it sound is Task 2 — so gate this behind an internal flag defaulting OFF until Task 2 lands, to keep the tree green between tasks.

- [ ] **Step 5: Run the meaning test + full cargo suite**

Run: `cargo test -q`
Expected: `test result: ok. NNN passed`.

- [ ] **Step 6: Commit**

```bash
git add src/regalloc/spill.rs src/regalloc/tests.rs
git commit -m "regalloc: register-residency fixpoint across back-edges (spec §4.4)

Second monotone fixpoint propagates register residency from a latch to its loop
header, read one round behind. Flag-gated OFF until reconstruction (Task 2) makes
the back-edge carry sound. Termination: product of two bounded lattices."
```

---

## Task 2: SSA reconstruction — the block-param insertion helper (Braun 2013)

**Files:**
- Create: `src/regalloc/reconstruct.rs` (the block-param insertion + use-rename helper)
- Modify: `src/regalloc/spill.rs` (call it from `apply`), `src/regalloc/mod.rs` (`pub mod reconstruct;`)
- Test: `src/regalloc/tests.rs`

**Interfaces:**
- Produces: `fn insert_phi(f: &mut MFunc, block: MBlockId, class: Class, width: Width, args: &[(MBlockId, Reg)]) -> VReg` — pushes a fresh block-param onto `f.blocks[block].params`, pushes each `(pred, reg)` onto that predecessor's terminator `MTarget{block, args}` for the edge into `block`, and returns the new param vreg. Caller renames the uses of the reconstructed value in `block` and its dominated successors to the returned vreg.

- [ ] **Step 1: Write the failing test — a hand-built diamond where a value is register-resident on one arm and reloaded on the other must reconcile via a phi and preserve meaning**

```rust
#[test]
fn reconstruct_reconciles_a_join_with_a_phi() {
    // Meaning-preserving on real programs whose join reconstructs a value:
    same_all(&[
        "int e(int); int hot(int p){int a; if(p>0){a=e(p);}else{a=e(-p);} return a+p+e(p);} int main(void){return hot(4);}",
        "int e(int); int hot(int p){int a=e(p); if(p&1)a+=e(p+1); else a+=e(p+2); return a+e(p);} int main(void){return hot(7);}",
    ]);
    // Effect: build a tiny physical MFunc with a value register-resident at one
    // pred and memory at another feeding a join, call insert_phi, assert the join
    // gained a block-param and the predecessors gained matching args.
}
```

- [ ] **Step 2: Run to verify the effect assertion fails (insert_phi does not exist)**

Run: `cargo test -q reconstruct_reconciles_a_join_with_a_phi`
Expected: FAIL to compile — `insert_phi` undefined.

- [ ] **Step 3: Implement `insert_phi`**

Push a fresh vreg (record class/width in `f.vregs`) onto `f.blocks[block].params`. For each `(pred, reg)` in `args`, find `pred`'s terminator target whose `.block == block` and push `reg` onto its `.args` (Bcc/Cbz/Tb/Switch/B all carry `MTarget`s — handle each). Return the vreg. The caller does the use-rename. Document the commuting square: a block-param defined at the head and fed by every predecessor's reaching value is a proper SSA phi; `destruct` lowers it to edge copies; `mir::verify` checks it.

- [ ] **Step 4: Run the test**

Run: `cargo test -q reconstruct_reconciles_a_join_with_a_phi`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/regalloc/reconstruct.rs src/regalloc/mod.rs src/regalloc/tests.rs
git commit -m "regalloc: insert_phi — SSA reconstruction at a join (Braun 2013)"
```

---

## Task 3: Generalized carry at non-loop joins

**Files:**
- Modify: `src/regalloc/spill.rs` (`carried`, and `apply` to call `insert_phi` + rename uses)
- Test: `src/regalloc/tests.rs`

**Interfaces:**
- Consumes: `insert_phi` (Task 2), the back-edge residency (Task 1, still OFF for loops), `Plan.wexit`.
- Produces: `carried` returns, in addition to the dominance-carried values, the values register-resident at SOME preds; `apply` reconstructs them with a phi and reloads on the cold edges.

- [ ] **Step 1: Write the failing effect test — a switch fan-out reloading a value in every arm must reload it far fewer times after reconstruction**

```rust
#[test]
fn generalized_carry_cuts_switch_reloads() {
    // Meaning:
    same_all(&[
        "int e(int); int hot(int p){int s=0,i; for(i=0;i<40;i++){switch(i%5){case 0:s+=e(i)+p;break;case 1:s+=e(i)*p;break;case 2:s+=p-e(i);break;case 3:s+=e(i)+p+p;break;default:s+=e(i);}} return s+p;} int main(void){return hot(3);}",
    ]);
    // Effect: compile with ZCC_NOPROMOTE=1, count MInst::Reload of the p-value in
    // hot with the pass ON vs an internal-flag OFF; assert ON < OFF.
}
```

- [ ] **Step 2: Verify effect fails (carry not yet generalized)**

Run: `cargo test -q generalized_carry_cuts_switch_reloads`
Expected: FAIL on the effect assertion (ON == OFF).

- [ ] **Step 3: Implement the generalized carry (non-loop) + apply reconstruction**

Extend `carried`: collect values register-resident at ≥1 (not-back-edge) predecessor and live-in to the block; for each, `apply` calls `insert_phi` with each pred's holding register (or a reload copy minted on the cold edge) and renames the block's uses to the phi. Prune: only when it removes ≥1 reload (Braun minimal-SSA). Count the phi in `simulate`'s pressure.

- [ ] **Step 4: Verify meaning + effect pass**

Run: `cargo test -q generalized_carry_cuts_switch_reloads` then `cargo test -q`
Expected: PASS.

- [ ] **Step 5: Full gate**

Run: `sh tests/fullsuite.sh`
Expected: `== 15 PASS / 0 RED ==`. If RED for compile-speed, see Task 6's near-linear rule; one bounded fix then quarantine.

- [ ] **Step 6: Measure KPI + commit**

Run the KPI command. Record frame str/ldr and sqlite insn vs baseline.

```bash
git add src/regalloc/spill.rs src/regalloc/tests.rs
git commit -m "regalloc: generalized cross-edge carry at non-loop joins (reconstruction)"
```

---

## Task 4: Loop-header carry (turn on the back-edge residency)

**Files:**
- Modify: `src/regalloc/spill.rs` (enable the Task-1 back-edge residency for loop headers; `apply` inserts the header phi fed by preheader + latch)
- Test: `src/regalloc/tests.rs`

**Interfaces:**
- Consumes: Task 1 fixpoint, Task 2 `insert_phi`, Task 3 apply-reconstruction.
- Produces: loop-carried values register-resident at the latch get a header block-param; per-iteration reload/store removed.

- [ ] **Step 1: Write the failing effect test — a loop-carried accumulator kept in a register (VdbeExec `[sp,#600]` shape)**

```rust
#[test]
fn loop_header_carry_keeps_the_accumulator_in_a_register() {
    same_all(&[
        "int e(int); int hot(int p){int acc=0,i; for(i=0;i<50;i++){acc=acc+e(i)+p;} return acc; } int main(void){return hot(2);}",
        "int e(int); int hot(int p){int a=0,b=0,i; for(i=0;i<30;i++){a+=e(i)*p; b+=e(i)+a;} return a+b; } int main(void){return hot(3);}",
    ]);
    // Effect: the accumulator's slot is stored/reloaded O(1), not O(iterations):
    // compile with ZCC_NOPROMOTE=1, assert the max store-count to any single
    // spill slot in hot is small (< 4), not ~iterations.
}
```

- [ ] **Step 2: Verify effect fails (per-iteration re-spill)**

Run: `cargo test -q loop_header_carry_keeps_the_accumulator_in_a_register`
Expected: FAIL — a slot stored ~iterations times.

- [ ] **Step 3: Enable loop-header reconstruction**

Turn on the Task-1 back-edge residency for loop headers; `apply` inserts the header phi (preheader arg = initial reaching def, latch arg = carried register). `mir::verify` must accept the resulting SSA (the header param is defined at the head, fed by both edges).

- [ ] **Step 4: Verify meaning + effect + verify pass**

Run: `cargo test -q loop_header_carry_keeps_the_accumulator_in_a_register` then `cargo test -q`
Expected: PASS.

- [ ] **Step 5: Full gate + KPI**

Run: `sh tests/fullsuite.sh` then the KPI + `ZCC_SPILLCEIL` commands, and confirm VdbeExec `[sp,#600]` store count dropped (spec §1):
`ZCC_REL=1 sh tests/box.sh s 'SQ=/suites/sqlite; zcc -S -o /tmp/z.s "$SQ/sqlite3.c" 2>/dev/null; awk "/^sqlite3VdbeExec:/{p=1} p{print} /\.size[[:space:]]+sqlite3VdbeExec/{p=0}" /tmp/z.s | grep -c "str .*\[sp, #600\]"'`
Expected: gate green; `[sp,#600]` stores ≈ 1 (was 227).

- [ ] **Step 6: Commit**

```bash
git add src/regalloc/spill.rs src/regalloc/tests.rs
git commit -m "regalloc: loop-header carry — keep loop-carried values register-resident"
```

---

## Task 5: Eviction as regional split (retire whole-web `Sim::More`)

**Files:**
- Modify: `src/regalloc/spill.rs` (`simulate` eviction path; `Sim::More` semantics)
- Test: `src/regalloc/tests.rs`

**Interfaces:**
- Produces: a value evicted at a pressure peak is memory-resident ONLY in the region between eviction and its next register re-entry; segments reconcile via Task-2/3/4 reconstruction. `Sim::More` is emitted only when NO split relieves pressure (a value with no register-resident interval anywhere).

- [ ] **Step 1: Write the failing effect test — a value spilled only in the tight region, register-resident elsewhere**

```rust
#[test]
fn eviction_splits_regionally_not_whole_web() {
    same_all(&[
        "int e(int); int hot(int p){int a=e(p); int t=e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a)+e(a); int b=a+t+p; return b+a; } int main(void){return hot(2);}",
    ]);
    // Effect: `a` is spilled across the pressure spike (the long e(a) chain) but
    // read from a register before and after — assert `a`'s slot has reloads only
    // in the spike region, and `a` is NOT whole-web memory-resident (there is at
    // least one register-resident interval in wexit for it).
}
```

- [ ] **Step 2: Verify effect fails (whole-web spill)**

Run: `cargo test -q eviction_splits_regionally_not_whole_web`
Expected: FAIL — `a` whole-web memory-resident.

- [ ] **Step 3: Implement regional split**

Change the eviction so a Belady-evicted value re-enters a register at its next use (reload) and its register-resident intervals feed reconstruction; only escalate to `Sim::More` when a value cannot hold a register anywhere (genuinely over-pressured for its whole live range). Preserve termination (a value with no register interval still becomes memory-resident, bounding the fixpoint).

- [ ] **Step 4: Verify meaning + effect + full cargo**

Run: `cargo test -q`
Expected: PASS.

- [ ] **Step 5: Full gate + KPI**

Run: `sh tests/fullsuite.sh` then KPI.
Expected: gate green; frame str+ldr materially below baseline (toward gcc's 12,721).

- [ ] **Step 6: Commit**

```bash
git add src/regalloc/spill.rs src/regalloc/tests.rs
git commit -m "regalloc: eviction splits regionally instead of spilling whole webs"
```

---

## Task 6: Prune, count pressure, hold compile-speed near-linear

**Files:**
- Modify: `src/regalloc/spill.rs` / `src/regalloc/reconstruct.rs`
- Test: `src/regalloc/tests.rs`

**Interfaces:** no new public interface; tightens the internals of Tasks 1–5.

- [ ] **Step 1: Write the failing test — no block-param that removes zero reloads, and compile stays fast on a large function**

```rust
#[test]
fn reconstruction_is_pruned_and_pressure_is_counted() {
    // Meaning on nested loops + wide joins:
    same_all(&[
        "int e(int); int hot(int p){int s=0,i,j; for(i=0;i<10;i++)for(j=0;j<10;j++){switch((i+j)%4){case 0:s+=e(i)+p;break;case 1:s+=e(j)*p;break;case 2:s+=p;break;default:s+=e(i*j);}} return s+p;} int main(void){return hot(2);}",
    ]);
    // Effect: assert every block-param inserted removes >=1 reload (no dead phi),
    // and coloring never exceeds k (mir::verify passes, which the gate enforces).
}
```

- [ ] **Step 2: Verify + implement pruning and pressure counting**

Drop any candidate phi that does not remove a reload (Braun minimal-SSA). Ensure block-params are counted in `simulate`'s working-set pressure. Ensure the two-lattice fixpoint is near-linear per round (no O(stores×reloads) scans — the multi-store `dominates_all` timeout was that mistake; precompute residency keys once per round).

- [ ] **Step 3: Compile-speed check in the box (csmith/yarpgen are the stress)**

Run: `sh tests/fullsuite.sh` and confirm csmith/yarpgen show `0 TIMEOUT / 0 CTIMEOUT` (a timeout means a compile-speed regression — the Task-anti-pattern).
Expected: green, no timeouts.

- [ ] **Step 4: Commit**

```bash
git add src/regalloc/spill.rs src/regalloc/reconstruct.rs src/regalloc/tests.rs
git commit -m "regalloc: prune dead phis, count block-param pressure, keep the fixpoint near-linear"
```

---

## Task 7: Final measurement, bank or quarantine

**Files:** `REARCH.md` (the plan of record — ladder row + a §13 section), `docs/superpowers/plans/2026-08-26-allocator-splitting-restructure.md` (progress log).

- [ ] **Step 1: Full gate green on the final tree**

Run: `sh tests/fullsuite.sh`
Expected: `== 15 PASS / 0 RED ==`. If not green after one bounded Law-2 fix → **quarantine**: `git reset --hard rc4`-equivalent, mark BLOCKED in REARCH with the measured reason, stop. RC4 stands.

- [ ] **Step 2: Record the final KPI vs baseline and gcc**

Run KPI + `ZCC_SPILLCEIL` + geo40 (`ZCC_REL=1 sh tests/box.sh s 'SUITE=/work/zcc/tests/bench/suite ZCC=/usr/local/bin/zcc N=7 sh /work/zcc/tests/bench/exectime.sh'` — confirm EXEC did not regress).
Record: frame str+ldr, sqlite insn/ratio, VdbeExec stores, geo40 EXEC/INSN.

- [ ] **Step 3: Update REARCH.md in place (anti-fragmentation)**

Add the R4-capstone ladder row and a `§13` section with the measured before/after, the square names, and the residual (what splitting did NOT reach). Do NOT fork a new numbering.

- [ ] **Step 4: Commit and push**

```bash
git add REARCH.md docs/superpowers/plans/2026-08-26-allocator-splitting-restructure.md
git commit -m "perf(R4-capstone): live-range splitting — allocator restructure banked

sqlite frame str+ldr AAA -> BBB (gcc 12,721); sqlite CCC = D.DDDDx; VdbeExec
[sp,#600] 227 -> ~1; geo40 EXEC E.EEEE / INSN F.FFFF. Full gate 15/15."
git push
```

- [ ] **Step 5: If banked and at/near size parity, consider cutting rc5**

Only on explicit user instruction (RC cuts are user-gated): `git checkout main && git merge --ff-only mir-rearch && git tag -a rc5 -m "..." && git push origin main && git push origin rc5 && git checkout mir-rearch`.

---

## Self-Review

**Spec coverage:** §4.1 generalized carry → Tasks 3+2; §4.2 loop headers → Task 4; §4.3 eviction-as-split → Task 5; §4.4 fixpoint → Task 1; §5 correctness → the `same()`/`mir::verify`/provenance discipline in every task; §6 risks → Task 1 (termination), Task 6 (explosion, pressure, compile-speed); §7 scope → Task 7 quarantine; §8 testing → each task's tests + gate. All covered.

**Placeholder scan:** implementation steps give algorithm + exact real types (`Plan.wexit`, `Res`, `carried`, `Sim`, `insert_phi`, `MBlock.params`, `MTarget.args`) rather than full Rust, because the exact code is discovered against the existing spiller — this is inherent to a restructure, not a placeholder; every TEST and COMMAND is concrete and runnable.

**Type consistency:** `insert_phi(f, block, class, width, args) -> VReg` is defined in Task 2 and consumed by Tasks 3/4 with that signature; `Plan.wexit` / `Res` / `Sim` are the real spill.rs types; `entry_resident` (Task 1) feeds `carried` (Task 3). Consistent.

## Progress

(Task 0 fills this in.)
