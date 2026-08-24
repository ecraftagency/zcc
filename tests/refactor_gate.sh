#!/bin/bash
# Byte-identical refactor gate (CLAUDE.md Article E). The correctness proof for PURE CODE
# MOTION: record md5(.s) over a determinism-exercising corpus, refactor, assert unchanged.
# Identical bytes == the commuting-square ⟦f⟧=⟦refactor f⟧. Usage:
#   tests/refactor_gate.sh baseline   # before the refactor: record sums
#   tests/refactor_gate.sh check      # after: rebuild, re-emit, diff vs baseline
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ZCC="${ZCC:-$ROOT/target/release/zcc}"
WORK="$ROOT/tests/refactor_gate"
MODE="${1:-check}"
OUT="$WORK/.out"; mkdir -p "$OUT"
SUMS="$WORK/sums.$MODE.txt"; : > "$SUMS"
md5f(){ md5 -q "$1" 2>/dev/null || md5sum "$1" | awk '{print $1}'; }
# Corpus = freestanding stress programs + every tests/cases + tests/bench .c that compiles
# with embedded headers alone (header-needing cases are skipped: they need the Linux box).
for c in "$WORK"/stress/*.c "$ROOT"/tests/cases/*.c "$ROOT"/tests/bench/*.c; do
  [ -e "$c" ] || continue
  b="$(basename "${c%.c}")"
  s="$OUT/$b.$MODE.s"
  if "$ZCC" -S -o "$s" "$c" 2>/dev/null; then
    echo "$(md5f "$s")  $b" >> "$SUMS"
  fi
done
sort -k2 "$SUMS" -o "$SUMS"
n=$(wc -l < "$SUMS" | tr -d ' ')
if [ "$MODE" = baseline ]; then echo "baseline: $n programs recorded"; exit 0; fi
if diff "$WORK/sums.baseline.txt" "$SUMS" >/dev/null 2>&1; then
  echo "✅ BYTE-IDENTICAL ($n programs)"; exit 0
else
  echo "❌ DIVERGENCE (Law-2: localize to the touched code):"
  diff "$WORK/sums.baseline.txt" "$SUMS" || true; exit 1
fi
