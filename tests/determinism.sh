#!/bin/bash
# Emission determinism seal (CLAUDE.md Article E, "Byte-identical gate" + MECHANISM.md §G9).
#
# The refactor gate proves md5(.s) is unchanged ACROSS A REFACTOR. This proves
# something the repo did not check at all: that ONE binary, run repeatedly on
# ONE input, emits identical bytes. It is a different failure mode — iteration
# order of a hash container, an address-derived tie-break, anything seeded per
# process — and it is invisible to every other gate, because each of them
# compiles each program exactly once.
#
# Each run is a FRESH PROCESS, so Rust's per-process HashMap seed differs; a
# single hash-order dependence anywhere in the backend shows up immediately.
#
#   tests/determinism.sh [N]        # N runs per program, default 8
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ZCC="${ZCC:-$ROOT/target/release/zcc}"
N="${1:-8}"
OUT="$(mktemp -d)"
md5f(){ md5 -q "$1" 2>/dev/null || md5sum "$1" | awk '{print $1}'; }
progs=0 bad=0
# `tests/bench/suite` was NOT in this list until 2026-08-29, so the ninety-six
# programs the speed scoreboard is taken from — the largest and most varied C in
# the repository, and the only functions here big enough to make the allocator
# spill — were never checked for emission determinism. A gate that omits the
# hardest inputs proves the least about them (Article E: the science gate is
# EXPANDED, never contracted).
for c in "$ROOT"/tests/cases/*.c "$ROOT"/tests/bench/*.c "$ROOT"/tests/bench/suite/*.c "$ROOT"/tests/refactor_gate/stress/*.c; do
  [ -e "$c" ] || continue
  b="$(basename "${c%.c}")"
  first=""
  ok=1
  for i in $(seq 1 "$N"); do
    if ! "$ZCC" -S -o "$OUT/$b.$i.s" "$c" 2>/dev/null; then ok=0; break; fi
    m="$(md5f "$OUT/$b.$i.s")"
    if [ -z "$first" ]; then first="$m"
    elif [ "$m" != "$first" ]; then
      echo "❌ NONDETERMINISTIC $b (run $i: $m != $first)"; bad=$((bad+1)); ok=0; break
    fi
  done
  [ "$ok" = 1 ] && [ -n "$first" ] && progs=$((progs+1))
done
rm -rf "$OUT"
# Article E (clean-input): a green verdict is valid only with a mechanical
# evidence trail. Zero programs compiled means the compiler was never run — a
# missing $ZCC, not a determinism result — so it is a FAILURE, not a pass.
if [ "$progs" = 0 ]; then
  echo "❌ determinism.sh compiled NO program (ZCC=$ZCC missing or rejecting every input)"; exit 1
fi
if [ "$bad" = 0 ]; then
  echo "✅ DETERMINISTIC ($progs programs x $N fresh processes)"; exit 0
fi
echo "$bad program(s) emitted differing bytes across runs"; exit 1
