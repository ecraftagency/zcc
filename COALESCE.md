# COALESCE — the register copy that is half the gap

The plan of record for one campaign. Boot here, read §0 for the number that
justifies it, §1 for what has already been refuted, and start at §3 — which is
measurement, not code.

---

## §0 THE FINDING (`MEASURED M26`, 2026-08-28, commit `5e03858`)

The 49-program taxonomy suite, compiled by both compilers, every mnemonic
counted and the spellings combined:

```
zcc 7,551 instructions   gcc -O1 6,598      +953   (+14.4%)

mov reg,reg          1006   339   +667   ← 70% of the whole excess
  · at a block edge   519    56   +463   ← HALF the entire gap
  · in the body       312   206   +106
  · placing an argument 175   77    +98
load/store slots     1107   967   +140
cmp + subs            484   359   +125
mul + madd + msub     187    70   +117
sxtw + sbfiz          161    93    +68
mov reg,#imm          608   790   −182   ← zcc materializes FEWER constants
```

The same shape was measured independently on the sqlite amalgamation:
register-to-register moves are +10,464 of a 20,264-instruction gap, 52%. Two
different corpora, one answer.

**A block-edge copy is what SSA destruction leaves behind and coalescing does not
remove.** It is not a missing optimization row, which is why three rows built on
the strength of reading one inner loop (`MEASURED M25` and the refutations beside
it) each addressed a family worth 12%, 0% and 12% and each measured a loss.

## §1 WHAT IS ALREADY KNOWN, AND WHAT IS ALREADY REFUTED

Read this before proposing anything; the obvious moves have been made.

- **The hints are asked for and REFUSED.** On sqlite the coalescing hint hit rate
  is 56.5%, and **14,615 hints were refused because the register was already
  OCCUPIED** — not because the hint was absent or badly ordered. The conclusion
  recorded there is that this needs EVICTION or priority colouring, and that
  three ordering fixes were tried and refuted.
- **ABI argument placement is 40% of sqlite's size gap** on its own (x0–x7
  traffic 22,813 against gcc's 14,626), and §0's census puts +98 of the suite's
  copies there too. It is a second front, not the same one.
- **`evict_params` strips `has_def`**, so a loop-header phi can never carry an
  accumulator; the recorded next lever there was to split the PARAMETER at the
  terminator rather than the whole web.
- **Reconstruction is Braun-2013 at joins and headers**, and eviction is already
  a regional split rather than a whole-web one — the whole-web model was wrong
  for 96% of spilled values.

The surface: `regalloc/color.rs` (952 lines, where a hint is honoured or
refused), `regalloc/destruct.rs` (715, where the edge copy is created),
`regalloc/reconstruct.rs` (124), `regalloc/spill.rs` (2,862),
`regalloc/promote.rs` (341).

## §2 WHERE A COPY COMES FROM IN ZCC — the three sources, and they need separating

The census counts what reaches the assembler; it does not say which mechanism
minted each one. Nothing should be built until each of the 519 edge copies is
attributed to one of:

1. **SSA destruction** — a phi whose argument and result were coloured
   differently, so `destruct` places a copy on the edge.
2. **A parallel copy that is genuinely a permutation** — a swap or a cycle, which
   costs copies no matter how it is coloured, and is NOT a coalescing failure.
3. **A `Copy` minted by an earlier pass and never removed** — `mir/pass/ext.rs`
   turns a redundant extension into a `Copy` and expects colouring to erase it.
   67 of these survive with the SAME register at both ends (gcc: none);
   `k1_dispatch` ends every switch arm with the identical `mov w10, w10` behind
   an `and` that already zeroed the top half.

These want opposite fixes, and the ratio between them decides the whole campaign.

## §3 THE FIRST STEP IS A COUNT, NOT A PATCH

The rule this campaign exists because of: measure the mechanism, then fix it.
Three rows were lost this session by inverting that order.

Instrument `destruct.rs` and `color.rs` with counters printed under `ZCC_TIME`,
and report, per function and summed over the suite AND over sqlite:

- edge copies minted, split by phi-argument-vs-result mismatch, permutation, and
  pass-minted `Copy` survivors;
- for each mismatch: whether a hint existed, and if it was refused, WHICH value
  held the register and whether that value was live across the edge at all;
- copies that survive to `emit` with both ends on the same register.

The output is a table of causes with counts. Nothing is written until it exists.

## §4 CANDIDATE ROWS, ranked by the census and gated

Each ships a commuting square and is judged on BOTH axes (`THE ULTIMATUM`), and
on EXEC before size (Law 0). A row that wins size and loses exec does not ship.

| # | row | what it addresses | gate |
|---|---|---|---|
| C1 | the identity `Copy` that reaches `emit` with one register at both ends | 67 instructions, suite | must be sound: a `w`-form write zeroes bits 63:32, and at `Width::W32` `ext.rs`'s lattice proves a fact about the LOW half only. The fix is to restate the fact at full width in `ext.rs`, not a peephole in `emit.rs` |
| C2 | priority / eviction colouring: a refused hint may displace the occupant when the occupant is cheaper to hold elsewhere | the 14,615 refusals | needs C0's attribution first — a refusal whose occupant is itself a hinted phi is a different problem from one whose occupant is a spilled reload |
| C3 | splitting the PARAMETER at the terminator rather than the web | the loop-header accumulator that `evict_params` makes unreachable | named already in the spill ladder; measure against the 1.10× floor, not 1.0 |
| C4 | ABI argument placement | +98 suite, 40% of sqlite's size gap | a separate front; do not fold it into C2's measurement |

## §5 TRAPS, all of them paid for once already

- **Combine the spellings or the table lies.** gcc writes `mov w7, 18725` where
  zcc writes `movz`, and `bne` where zcc writes `b.ne`. Raw counts read as +432
  and +195 against a gcc that never emits either mnemonic.
- **The EXEC geomean has a ±0.007 spread across sessions.** Only interleaved
  pairs inside one box session compare; a single reading has already dismissed
  one row wrongly and promoted another wrongly.
- **INSN geomean is deterministic** and is the axis to trust for a size claim.
- **Never chain a baseline to an earlier candidate.** Six rows drifted unnoticed
  that way.
- **The byte-identical gate has no oracle** (it is zcc against zcc) and is scoped
  to what it compiles. A row that fires on none of the corpus is invisible to it;
  measure the row's coverage before trusting a green.
- **A permutation is not a coalescing failure.** Counting it as one will make a
  fix look like it did nothing.

## §6 HOW TO MEASURE

**The census** (one box command, the source of §0): for each `.c`, emit both
`.s`; `grep -oE '^[[:space:]]+[a-z][a-z0-9._]*'` for mnemonics; `uniq -c`; `join`
the two tables; sort by difference. Classify a `mov` by scanning forward up to
seven instructions — a `bl` first means argument placement, a branch or a label
first means a block edge, anything else means body. Split `mov` by whether its
second operand begins with `#` or a digit (constant) or not (register), and by
whether its two register operands are equal.

**The scoreboard**: `SUITE=/work/zcc/tests/bench/suite sh tests/bench/exectime.sh`
inside the box (its default `SUITE` path is wrong and it then reports "no timed
programs" in silence). Interleaved pairs only.

**The gate**: `sh tests/fullsuite.sh all` — 15 stages, about six minutes with
inlining on. Batch two or three rows per full gate; per row use `cargo test`,
`tests/fullsuite.sh provenance` and `tests/bench/localize.sh`.
