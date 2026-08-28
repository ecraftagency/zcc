#!/bin/bash
# Byte-identical refactor gate (CLAUDE.md Article E). The correctness proof for PURE CODE
# MOTION: record md5(.s) over a determinism-exercising corpus, refactor, assert unchanged.
# Identical bytes == the commuting-square ⟦f⟧=⟦refactor f⟧. Usage:
#   tests/refactor_gate.sh baseline   # before the refactor: record sums
#   tests/refactor_gate.sh check      # after: rebuild, re-emit, diff vs baseline
#
# TWO WITNESSES, AND THE SECOND ONE IS NOT OPTIONAL.
#
# The corpus below is 58 small programs. It cannot see a pass whose behaviour
# scales with the size of a function — inlining, spilling, anything that reruns
# an analysis per rewrite — because none of its members is big enough to make
# the pass work. That is not a hypothetical: on 2026-08-28 six inline rows passed
# this gate, green every time, while changing the assembly of the sqlite
# amalgamation. The corpus was never wrong; it was answering a smaller question
# than the one being asked of it.
#
# So the gate also compiles ONE large translation unit — sqlite3.c, a quarter of
# a million lines with one function of several thousand blocks — and compares its
# md5 against a recorded reference. It costs one compile.
#
# The large witness needs the Linux box (an aarch64-musl zcc, docker, and the
# suite cache), so it cannot always run. When it cannot, this script says so and
# FAILS. A gate that quietly proves less than it claims is the thing that went
# wrong here, and silence is exactly how it went wrong. To run the corpus alone —
# for a quick loop, knowing the proof is partial — pass `ZCC_GATE_LARGE=0`, which
# prints the waiver in place of the result.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ZCC="${ZCC:-$ROOT/target/release/zcc}"
WORK="$ROOT/tests/refactor_gate"
MODE="${1:-check}"
OUT="$WORK/.out"; mkdir -p "$OUT"
SUMS="$WORK/sums.$MODE.txt"; : > "$SUMS"
md5f(){ md5 -q "$1" 2>/dev/null || md5sum "$1" | awk '{print $1}'; }
# Corpus = freestanding stress programs + every tests/cases, tests/bench and
# tests/bench/suite .c that compiles with embedded headers alone (header-needing
# cases are skipped: they need the Linux box).
#
# THE SUITE PROGRAMS ARE IN THE CORPUS BECAUSE OF WHAT THEY REACH, not because
# they are benchmarks. A gate program earns its place by making a pass FIRE:
# a corpus that never exercises a row cannot notice a change to it, and on
# 2026-08-28 exactly that happened — `iv`'s consumer-blind row fired on 0 of the
# 59 programs then in the corpus, so a change to it passed this gate, passed the
# sqlite witness, and crashed on musl. `k1_dispatch` and `k2_live_pressure` are
# the two programs in the tree that fire that row. The rule the episode leaves:
# when a defect escapes this gate, the shape that escaped joins the corpus.
for c in "$WORK"/stress/*.c "$ROOT"/tests/cases/*.c "$ROOT"/tests/bench/*.c "$ROOT"/tests/bench/suite/*.c; do
  [ -e "$c" ] || continue
  b="$(basename "${c%.c}")"
  s="$OUT/$b.$MODE.s"
  if "$ZCC" -S -o "$s" "$c" 2>/dev/null; then
    echo "$(md5f "$s")  $b" >> "$SUMS"
  fi
done
sort -k2 "$SUMS" -o "$SUMS"
n=$(wc -l < "$SUMS" | tr -d ' ')

# ── the large witness ──────────────────────────────────────────────────────
# The flags are part of the recorded fact: a different set compiles a different
# program and its md5 means nothing against this one.
LARGE_FLAGS="-w -DSQLITE_THREADSAFE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_DISABLE_LFS"
LARGE_REF="$WORK/sums.large.txt"
SUITES="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
MUSL="$ROOT/target/aarch64-unknown-linux-musl/release/zcc"

large_md5() { # prints the md5, or nothing when the box is unavailable
  [ -f "$SUITES/sqlite/sqlite3.c" ] || return 1
  command -v docker >/dev/null 2>&1 || return 1
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build -q --release --target aarch64-unknown-linux-musl 2>/dev/null || return 1
  docker run --rm \
    -v "$MUSL:/usr/local/bin/zcc:ro" -v "$SUITES:/suites:ro" zcc-box \
    sh -c "zcc $LARGE_FLAGS -I/suites/sqlite -S -o /tmp/l.s /suites/sqlite/sqlite3.c \
           && md5sum /tmp/l.s | cut -d' ' -f1" 2>/dev/null
}

if [ "${ZCC_GATE_LARGE:-1}" = 0 ]; then
  LARGE_STATE=waived
else
  if LARGE_NOW="$(large_md5)" && [ -n "$LARGE_NOW" ]; then
    LARGE_STATE=ran
  else
    LARGE_STATE=unavailable
  fi
fi

if [ "$MODE" = baseline ]; then
  echo "baseline: $n programs recorded"
  case "$LARGE_STATE" in
    ran) printf '%s  sqlite3.c %s\n' "$LARGE_NOW" "$LARGE_FLAGS" > "$LARGE_REF"
         echo "baseline: large witness recorded ($LARGE_NOW)" ;;
    waived) echo "baseline: large witness WAIVED — sums.large.txt left as it was" ;;
    unavailable) echo "baseline: large witness UNAVAILABLE (needs docker + zcc-box + the suite cache)"; exit 2 ;;
  esac
  exit 0
fi

small_ok=0
if diff "$WORK/sums.baseline.txt" "$SUMS" >/dev/null 2>&1; then
  small_ok=1
fi

case "$LARGE_STATE" in
  ran)
    want="$(awk '{print $1}' "$LARGE_REF" 2>/dev/null || true)"
    if [ -z "$want" ]; then
      echo "❌ no large-witness reference recorded — run 'refactor_gate.sh baseline' on a tree you trust"
      exit 2
    fi
    if [ "$LARGE_NOW" = "$want" ]; then large_ok=1; else large_ok=0; fi ;;
  waived) large_ok=waived ;;
  unavailable) large_ok=missing ;;
esac

if [ "$small_ok" = 0 ]; then
  echo "❌ DIVERGENCE on the corpus (Law-2: localize to the touched code):"
  diff "$WORK/sums.baseline.txt" "$SUMS" || true
  exit 1
fi

case "$large_ok" in
  1) echo "✅ BYTE-IDENTICAL ($n programs + sqlite3.c)"; exit 0 ;;
  0) echo "❌ DIVERGENCE on sqlite3.c — the corpus did NOT see this."
     echo "   recorded $want"
     echo "   now      $LARGE_NOW"
     echo "   The touched pass behaves differently at scale; the corpus is too small to show it."
     exit 1 ;;
  waived)
     echo "⚠️  $n programs identical — sqlite3.c NOT CHECKED (ZCC_GATE_LARGE=0)."
     echo "   This proves nothing about a pass whose behaviour scales with function size."
     exit 0 ;;
  missing)
     echo "⚠️  $n programs identical — sqlite3.c COULD NOT BE CHECKED."
     echo "   Needs docker, the zcc-box image, and \$ZCC_SUITE_CACHE/sqlite/sqlite3.c."
     echo "   Refusing to report a pass: the corpus alone has already missed a real divergence."
     exit 2 ;;
esac
