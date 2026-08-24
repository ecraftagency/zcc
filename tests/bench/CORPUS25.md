# CORPUS #25 — the proven knowledge base for the nuclear allocator

> Purpose: everything the #25 session needs, measured (not estimated) and provenance-tagged, so
> execution starts from data instead of guesswork. Every number here is a mechanical `grep`/`awk`
> catamorphism over emitted `.s` (Law-3 "certify at the middle": the `.s` **confirms**, it does not
> estimate). Reproduce: `ZCC=/usr/local/bin/zcc GCC=aarch64-linux-gnu-gcc SQLITE=/suites/sqlite/sqlite3.c sh tests/bench/corpus25.sh`
> (inside the box). Snapshot below is HEAD `2af2702` (post-#24), sqlite3.c amalgamation, `-O1` both.

---

## 0. The two numbers the user cares most about (baseline, PROVEN)

| axis | zcc | gcc | ratio | provenance |
|---|---|---|---|---|
| **sqlite SIZE** (static insns) | 286,129 | 157,883 | **1.812×** | `grep -cE '^\s+[a-z]'` over `-S` output |
| **geo40 EXEC** | — | — | **~1.55×** | `tests/bench/exectime.sh` (19 timeable progs, box best-of-3) |
| geo40 INSN (static proxy) | — | — | **1.610×** geomean / 1.618 pooled | corpus25.sh M3, 35 progs |

The exec geomean line in `exectime.sh` currently prints 0.0000 — **poisoned** by `g2_strlen exec_r=0.000`
(zcc "infinitely faster" artifact). Per-program exec is sound; the reducer must be fixed before trusting
the aggregate. **#25 session: fix that reducer first (drop-zero / clamp), it is a display bug not a result.**

---

## 1. The gap is ENTIRELY in the backend layer (gcc-normalized excess histogram)

Total static gap = 286,129 − 157,883 = **128,246 insns**. Decomposed by mnemonic (zcc − gcc count):

| mnem | zcc | gcc | **excess** | %gap | who kills it |
|---|---|---|---|---|---|
| mov  | 70,326 | 33,555 | **36,771** | 28.7% | **allocator** (home↔reg funnels vanish) |
| ldr  | 37,250 | 23,000 | **14,250** | 11.1% | **allocator** (reloads) + addr-mode fold |
| sub  | 12,944 |  2,153 | **10,791** |  8.4% | **allocator** (9,337 are `sub xN,xN,#N` = home-address arith) + stack-adjust-combine |
| ldp  | 13,779 |  6,698 |  7,081 |  5.5% | shrink-wrap + fewer callee-saves |
| sxtw |  6,863 |    666 |  6,197 |  4.8% | *partly fundamental* (real widening); type-tracking residual |
| add  | 17,632 | 11,654 |  5,978 |  4.7% | addressing-mode fold (`[base,off,lsl]`) + allocator |
| cmp  | 12,428 |  6,999 |  5,429 |  4.2% | **compare-elim** (reuse flags from prior adds/subs) |
| str  | 14,862 | 10,653 |  4,209 |  3.3% | **allocator** (spills) |
| stp  |  5,824 |  4,857 |    967 |  0.8% | shrink-wrap |
| bl   | 12,521 | 11,918 |    603 |  0.5% | near-parity (**#24 closed this**) |

Top-10 mnemonics = **72% of the gap**. The remaining ~28% is spread over csel/cset/adrp/ldrsw/uxtb/uxth/b/ret.

**The whole story in one line:** zcc's mem-op excess over gcc is 30,609, and its frame-slot ("home") mem-ops
are 33,836 — i.e. **zcc's entire load/store excess IS home-slot traffic.** The gap is not algorithmic; it
is the home-primary value model. gcc keeps values in registers; zcc keeps them in stack homes and reloads.

### Killable floor (audited)
`mov (70,326) + frame-mem (33,836) = 104,162 = 36.4%` of zcc insns *touch* the home model. But not all
of that is **excess** — the honest allocator-addressable excess is:
`mov-excess 36,771 + mem-excess (ldr 14,250 + str 4,209 + ldp 7,081 + stp 967) 26,507 + home-address sub ~9,337`
`≈ 72,600 insns` directly attackable by real allocation = **~57% of the 128k gap.**

---

## 2. WHERE the floor lives — concentration (per-function, sqlite, 3,398 functions)

| top-K functions | share of insns | share of frame-mem |
|---|---|---|
| top-10  |  9.4% | 19.6% |
| top-50  | 24.2% | 42.5% |
| top-200 | 46.4% | 61.1% |

**The floor is MODERATELY BROAD, not concentrated.** Half the frame-mem is in ~70 functions; the long tail
(3,398 fns) holds ~40%. Consequence: **the allocator cannot win from a handful of monster functions** — it
must be correct and effective across the whole corpus. One pathological outlier exists (`L989`: 2,060
frame-mem in 3,149 insns = **65% spill**) — worth a case-study, but not representative.

*(Caveat: non-`.L` local labels like `L989`/`L1018` are miscounted as top-level functions by the awk
attributor, so a few per-function splits are imperfect. The concentration conclusion is robust to this.)*

---

## 3. The SEAM — #25 is a strategy swap, NOT a rewrite (inheritance map)

zcc **already has** a partial register allocator. The value model:

```
opt/regalloc.rs::abi_alloc(tt, f, gp, fp, coalesce) -> Vec<AbiHome>     // the ONE seam
    type AbiHome = Option<(bool /*is_fp*/, u32 /*color*/)>              // Some=in reg, None=SPILL
    interference(f, lv) -> Vec<HashSet<Tmp>>   (regalloc.rs:105)        // liveness→interference graph
    color_abi(...)      (regalloc.rs:184)                               // the coloring
    verify_abi(...)     (regalloc.rs:599)  ← CORRECTNESS GATE (renaming-bisimulation, interference-edge distinct-color)

codegen/arm64_elf/mod.rs::talloc: Vec<AbiHome>                          // emit consumes it verbatim
codegen/arm64_elf/emit.rs::ld_val (emit.rs:120)  ← the ONE choke point (46 call-sites)
    Val::Tmp(t) => talloc[t]==Some((false,idx)) ? read gpp(idx) : tmp_load from [sp,#home]
```

**What #25 changes:** the *algorithm inside* `abi_alloc` (home-primary/narrow linear-scan → global
graph-coloring + **live-range splitting** so a temp is not spilled for its whole life). It keeps the SAME
`Vec<AbiHome>` interface, the SAME `verify_abi` gate, the SAME `Val`/`ld_val`/`tmp_load`/`gpp` machinery.
Correctness inheritance is free: `verify_abi`'s renaming-bisimulation already proves any coloring correct,
so #25 ships under the existing gate — no new proof infra.

**What SHRINKS:** `codegen/arm64_elf/peephole.rs` (2,034 LOC) is largely a patch layer compensating for
home-primary (redundant-mov elimination). Much of it becomes dead once allocation is real.

**Blast radius (NOT an erase):**
- REWRITE: `opt/regalloc.rs` (660 LOC) — the coloring/spilling strategy.
- SURGERY: `emit.rs` value-residency (`ld_val`/`tmp_load`/home-address emit) — the `sub xN,xN,#N` sites.
- SHRINK: `peephole.rs` — redundant-mov passes retire as they stop firing.
- INHERITED INTACT: frontend, `ssa.rs` (790), `loops.rs` (2,236), `mem.rs`, `fold.rs`, `scalar.rs`,
  `inline.rs`, all SSA/analysis passes — they produce the SSA IR the allocator consumes, unchanged.

---

## 4. INHERITANCE LEVERS already sitting in the tree (cheap, high-yield, low-risk)

1. **Activate `GP_BUDGET_WIDE` (k:10→18).** `encoding.rs:22` uses `GP_BUDGET = {k:10, ncaller:0}` = only
   x19–x28 (10 callee-saved). `encoding.rs:34` defines `GP_BUDGET_WIDE = {k:18, ncaller:8, narg:8}` =
   x0–x7 caller-saved + x19–x28 — **already written, UNUSED.** This is the charter's own Article-E worked
   example: k=10 is a *convenience truncation posing as a Side-II constant* (AAPCS64 has ~18 leaf-usable
   GPRs). The crossing-confinement infra to use caller-saved x0–x7 safely is already coded (WIDE's
   `ncaller=8`, `crossing[]` confines call-crossers to callee-saved). **Lever #1 = flip narrow→wide,
   measure. Near-zero new code, potentially the single largest spill-floor cut.** (Why it was left narrow:
   presumably measured-negative on the naive backend, or unproven — re-measure under the real allocator.)

2. **Turn ON `licm` + `strength_reduce`.** Both are BUILT and proven-correct but defaulted OFF
   (`mod.rs:120-121`) because "the naive-slot backend spills their results" / "the accumulator φ costs
   spill." **Once #25 gives registers, these flip positive for free** — the allocator UNLOCKS passes zcc
   already owns. Re-measure both under the real allocator before shipping.

3. **`remat` (rematerialize) is already ON** — pure operand-free defs recomputed under pressure. A real
   allocator changes the pressure profile; re-tune, don't rebuild.

---

## 5. What O1 techniques zcc does / does NOT have (coverage audit)

**zcc's pass set (`opt/mod.rs::Passes`) is NOT "more than O1" — it matches the tree-SSA HALF and is
missing the backend/RTL half.** Present and ON: `sccp, const_fold, copy_prop, gvn, cse, load_elim, dce,
cfg_simplify, pointer_iv (SR+LFTR), coalesce, peephole, ldst_pair, if_convert (csel), inline, remat, sroa,
hoist_const, rotate (#17), hoist_calls (#24)`. Built but OFF: `licm, strength_reduce`. This genuinely
≈ covers O1's `-ftree-*` layer (ccp, copy-prop, FRE/GVN, DSE-ish, DCE, dominator opts, SRA, SCEV, ch).

**O1 techniques zcc does NOT touch (all in the backend/RTL layer — exactly where the measured gap lives):**

| missing O1 technique | gcc flag | targets which excess |
|---|---|---|
| **Global graph-coloring allocation (IRA/LRA)** | (always on) | mov 36.8k + mem 26.5k + home-sub 9.3k = **#25 core** |
| **Shrink-wrapping** | `-fshrink-wrap[-separate]` | ldp 7.1k + stp 0.97k (save callee-saves only on paths that use them) |
| **Compare-elimination** | `-fcompare-elim` | cmp 5.4k (reuse condition flags from a prior adds/subs) |
| **Auto-inc/dec addressing** | `-fauto-inc-dec` | ptr-walk loops (`ldr x,[p],#8`) — b-series progs |
| **Addressing-mode folding** | (combine/forwprop) | add 6.0k + some ldr (`[base,idx,lsl#3]` vs separate add) |
| **Combine-stack-adjustments** | `-fcombine-stack-adjustments` | `sub sp,sp` (2,239 sites vs gcc's 1-per-frame) |
| **Code sinking** | `-ftree-sink` | dead-path work on cold branches |
| **Omit-frame-pointer** | `-fomit-frame-pointer` | (zcc already uses `[sp,#off]` for slots — mostly OK) |

**Honest conclusion to "is our set more than O1?":** No. We have the front half (tree-SSA) at rough parity
and are missing the back half (RTL/backend). The 1.81× gap is 100% in the missing back half — the corpus
proves it (every excess mnemonic is a backend concern: allocation, frame-management, flag-reuse, addressing).
**#25 should be scoped as "the backend optimization layer," graph-coloring as the spine with shrink-wrap /
compare-elim / stack-adjust-combine as co-shipped siblings** — attacking allocation alone leaves the
sub/ldp/stp/cmp excess (≈24k) on the table.

---

## 6. HONEST projection band (recalibrated against documented realize-rate)

**No bold single number.** Documented realize-rate on structural levers: sxtw lever realized **11.5%** of its
ceiling; peephole levers 5–15%. Allocation is structural (not peephole), so realize should be **higher** —
40–70% is defensible for a real graph-coloring allocator — but it is NOT proven until measured A/B.

- **Ceiling** (perfect allocation, 0 spills, 0 home-movs): kill ~72,600 allocator-addressable →
  286k − 72.6k = 213.5k vs 158k = **1.35×** sqlite. This is the FLOOR OF THE PROJECTION, the best case.
- **Honest band** (realize 40–70% of ceiling): kill 29k–51k → 235k–257k → **sqlite 1.49×–1.63×.**
- **Do NOT claim 1.3× and NEVER 1.0× from #25 alone.** 1.0× needs #25 **plus** the siblings (shrink-wrap,
  compare-elim, addressing-fold) **plus** the isel long-tail — a chain of small proof-carrying levers, each
  5–15% realize. The corpus is the instrument that makes that tail deterministic (see §7).

**Method over projection (standing user directive):** ship #25 behind a toggle, measure A/B on sqlite +
geo40, bank the real number. The projection band is a sanity rail, not a promise.

---

## 7. Is there a PROVABLE path to 1×1×? (strategic answer)

**Yes in METHOD, no as a single lever.** To reach gcc-parity, zcc must emit gcc's instruction count. The
corpus §1 histogram IS the exact worklist: every excess mnemonic class is a proof-carrying lever
(commuting-square correctness + cost-square Δinsn). The deterministic procedure:

```
loop:
  1. measure gcc-normalized excess-per-mnemonic (corpus25.sh §1)      # the worklist, ranked
  2. attack the largest excess class with ONE proof-carrying lever    # allocator, then siblings
  3. classify residual (Law-4): fundamental (real widening/ABI) vs convenience-truncation
  4. repeat until every excess class is residual=fundamental
```

After #25, **re-run corpus25.sh** — the new excess histogram tells you EXACTLY how far 1× is and whether
the tail is worth it. Decide "worth" THEN, on data.

**Is 1.3×/1.3× worth it?** Grounded honesty: 1.81→~1.5 (one structural win, #25) is the cheap big step;
1.5→1.3 is the siblings; 1.3→1.0 is a long tail of small isel levers (addressing-mode parity, flag-reuse,
widening-elision), each with 5–15% realize and diminishing returns. **1.3×/1.3× would already beat every
known toy compiler and is the realistic engineering optimum.** 1.0×/1.0× is *provably approachable* (the
excess histogram enumerates every remaining insn) but asymptotically expensive. **Recommendation: do not
pre-redefine "done" at 1.3×, and do not promise 1.0×. Fire #25, re-measure, let the post-#25 histogram
decide whether the tail is worth grinding — it will be a data decision, not a feeling.**

---

## 8. Execution loop for #25 (mechanism)

1. **Split emit first (optional, byte-identical).** `emit.rs` (1,892 LOC) mixes value-residency with
   instruction-emit. If context pressure demands, split along the Law-1 seam (residency/home-lowering vs
   insn-emit vs directives) and prove `md5(.s)` identical over the fixed corpus (Article E / `refactor_gate.sh`)
   BEFORE any allocator change. This is a refactor, not an opt — it ships zero Δinsn.
2. **Lever #1 = activate `GP_BUDGET_WIDE`** (§4.1). Toggle-gated, A/B on sqlite+geo40, `verify_abi` must stay
   green, full gate (cargo + torture + opt-parity + csmith300 + yarpgen300). Bank the real number.
3. **Then the real allocator** inside `abi_alloc`: global graph-coloring + live-range splitting, same
   interface, same `verify_abi` gate. Predict Δinsn on the cost-model first (Law-3), then measure.
4. **Then the siblings** (shrink-wrap, compare-elim, addressing-fold) as separate proof-carrying levers,
   ranked by the re-measured excess histogram.
5. **Turn ON licm + strength_reduce** (§4.2) once registers exist; re-measure each.
6. Bank every ≥0.5% positive; correctness gate never traded for a number (Law-3). Re-run corpus25.sh after
   each bank to regression-check the excess histogram.

Gate command crib (box): `ZCC_SUITE_CACHE=/suites` is required for torture/csmith/yarpgen. Rebuild zcc in
the box (`cargo build --release`), re-emit, re-run corpus25.sh.
