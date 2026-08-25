# PURITY — the one goal, and how it is checked

**Purity is the precondition, not a phase.** The ULTIMATUM names 1× against
gcc-O1 on both axes as the stopping point; purity is what may not be spent to
get there. Standing order, 2026-08-26:

```
purity  ≫  exec  >  size  >  compile speed
```

No number is banked at the cost of a citation. A row that would reach parity by
removing a proof does not ship.

---

## What purity means, exactly

Two of the three Laws are claims **about the source**, and until 2026-08-26
neither was checked by anything:

| | the claim | the check |
|---|---|---|
| **Law 1** | every line of `src/` is a theorem (Side I) or a constant transcribed from a spec line (Side II) | every module, constant and pass carries a citation |
| **Law 3** | every pass ships the commuting square that certifies it | every pass names its square, and the square is not vacuous |
| — | compilation is a FUNCTION: identical input, identical bytes | `tests/determinism.sh` |

`tests/provenance.sh` is the first two. It runs in the sci gate.

### The three documents

* **`THEORY.md`** — Side I theorems and Side II **citations**: a section number
  in ISO 9899, AAPCS64, DDI 0487 or the ELF ABI that a reader can look up.
* **`MEASURED.md`** — facts with **no spec to cite**. Apple publishes no
  Software Optimization Guide for the M1, so a latency, or whether a transform
  pays here, can only be measured. Keeping these out of `THEORY.md` is what lets
  Law 1's two-side claim stay literally true.
* **`SEMANTICS.md`** — the ⟦·⟧ every square is stated against.

`REARCH.md` is **not** one of these. It is the execution plan — R4 is unfinished
(R4.15, R4.12, and the tracked residuals) and it stays until that ladder closes.
What comes OUT of it as purity work proceeds is the durable half: theorems into
`THEORY.md`, spec constants into II-*, measured facts into `MEASURED.md`, ⟦·⟧
definitions into `SEMANTICS.md`. It gets thinner, not deleted.

### A citation is a reading path, not a lint

zcc is a teaching compiler. A student who lands on any line should be able to
read upward to the theorem it realizes, so a citation is written for a person:

```rust
// src/mir/pass/ldstp.rs
// THEORY A6b — MIR, the machine layer
// THEORY II-5 — DDI 0487 C6.2.130, the paired forms
// SQUARE      — a_pair_replaces_two_adjacent_accesses
```

The script only checks that what a person reads is true.

### Why the vacuity check is the one with teeth

A commuting square holds **vacuously** for a pass that never fires. A test that
checks only `⟦f⟧ = ⟦P f⟧` therefore stays green when the pass does nothing at
all — which is how §17 came to carry eight ✔ marks that were measurably false,
and how `scev::compute_trips` was wrong for weeks under a green gate.

So a square must assert **both** halves: the equivalence (a battery helper, or
`⟦·⟧` named in its own failure message) **and an effect** — at least one
assertion of its own about what the pass DID. A body that calls `same_all([…])`
and asserts nothing else has checked exactly one thing: that the pass did not
break the program.

---

## State — 2026-08-26

```
provenance: 55 modules, 58 constants, 21 passes; 25 distinct citations
PROVENANCE PASS (every LOC in theory ∪ fact; every pass squared, none vacuous)
```

### What the audit found

Six real defects, none of which the full gate could see:

1. **`mir/pass/cmpelim.rs` shipped with NO commuting square.** A pass in the
   default pipeline since R3, with no proof at all. Written:
   `an_arithmetic_result_needs_no_second_compare`, which also pins the condition
   rewrite (`lt` → `mi` once `subs` sets V from the arithmetic).
2. **Four vacuous squares** — `frame`, `layout`, `ldstp`, `legalize` — each
   calling a battery helper and asserting nothing else. All four would have
   stayed green for a pass that did nothing. Each now asserts its effect.
3. **`ladder_is_idempotent_at_the_fixpoint` had the effect half and not the
   equivalence half.** It proved the ladder reaches a fixpoint without ever
   proving the fixpoint means what the source meant — a ladder that miscompiled
   identically on both runs satisfied it.
4. **`layout_preserves_every_edge` is misnamed**, and the effect half is what
   exposed it: layout THREADS empty blocks, so a predecessor's successor changes
   (bb2: `[11,12]` → `[2,12]`). The edge set is not the invariant; the run is.
5. **Constants with no provenance**, now each carrying one — and four that have
   no spec to cite are labelled honestly in `MEASURED.md` rather than given
   invented citations: `MIN_CASES` (M4, measured INCONCLUSIVE), `WINDOW` (M5),
   `MAX_HEADER_INSTS` (M7, gcc's `--param`, not a spec), `ARM_LIMIT` (M8,
   reasoned, never swept).
6. **A missed pairing, found while writing the square.** `p->a + p->b` emits two
   adjacent loads off one base and does not pair: `ldstp::fuse` refuses when a
   destination equals the base register. DDI 0487 C6.2.130 constrains that only
   for the **writeback** forms — plain `ldp x1, x0, [x0]` reads the base once to
   form the address and is well defined. Recorded, not fixed: a correctness-
   sensitive ISA change does not belong at the end of a long session.

### Open

| | what | where |
|---|---|---|
| ⬜ | harvest REARCH's durable theorems into THEORY.md / SEMANTICS.md | REARCH §13n, §13o, the R4.x sections |
| ⬜ | `ldp` may write its own base (finding 6) — a missed pair, measured | `mir/pass/ldstp.rs::fuse` |
| ⬜ | `M4` unsettled: the jump-table crossover is not a function of case count | `isel/lower.rs::MIN_CASES` |
| ⬜ | `M7`, `M8` never swept on this corpus | `rotate.rs`, `ifconv.rs` |
| ⬜ | a square asserting an effect is still not a square asserting the RIGHT effect — the check cannot see that | `tests/provenance.sh` |

The last one is the honest limit of the mechanism: `provenance.sh` proves that
every pass claims a theorem and that every claim is non-vacuous. It cannot prove
the theorem is the right one. That remains a reading, and it is why the citations
are written to be read.
