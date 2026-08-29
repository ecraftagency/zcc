#!/bin/sh
# ubscan.sh — every TIMED program must be DEFINED, or its number is not a measurement.
#
# WHY THIS GATE EXISTS (`MEASURED M54`). On 2026-08-29 four of the ninety-six
# programs in `tests/bench/suite` were undefined — three signed-overflow, one
# shift-by-width — and nothing in the project could see it. `exectime.sh` gates
# zcc's output against the referee's, which is the right gate for a MISCOMPILE and
# is silent about a program that has no defined answer for either compiler to get
# right: at `-O1` the two happened to agree and the row was timed; at `-O2` gcc
# exploited the overflow, the two disagreed, and the harness correctly reported
# DIVERGE for a defect that was in the BENCHMARK.
#
# Article E: "A diff at an undefined point is meaningless: filter by spec, never
# by hand-waving." UBSan is that filter. It is the referee's own sanitizer, so
# this asks gcc about the C standard rather than asking zcc about itself.
#
# The suite is compiled at -O0 on purpose: an optimizer may DELETE the undefined
# operation before the sanitizer sees it, which would make the gate quieter and
# not the corpus cleaner.
set -u
SUITE="${SUITE:-$(dirname "$0")/bench/suite}"
GCC="${GCC:-gcc}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
bad=0; n=0
for c in "$SUITE"/*.c; do
    b=$(basename "$c" .c); n=$((n + 1))
    if ! $GCC -O0 -w -fsanitize=undefined -fno-sanitize-recover=all "$c" -o "$T/ub" 2>/dev/null; then
        echo "  BUILD-FAIL $b"; bad=$((bad + 1)); continue
    fi
    if ! "$T/ub" >/dev/null 2>"$T/log"; then
        bad=$((bad + 1))
        printf '  UNDEFINED %-20s %s\n' "$b" "$(sed -n 's/.*runtime error: //p' "$T/log" | head -1)"
    fi
done
# A gate that scans nothing must not pass (the `M39` law).
[ "$n" -gt 0 ] || { echo "UBSCAN RED: no programs found under $SUITE"; exit 1; }
[ "$bad" -eq 0 ] || { echo "UBSCAN RED ($bad undefined of $n)"; exit 1; }
echo "UBSCAN PASS ($n programs, every one defined under UBSan)"
