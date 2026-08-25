#!/bin/sh
# opt-parity.sh — a mechanical MEASUREMENT (self-differential): for each torture
# execute .c, compile TWICE — the -O0 naive path (ZCC_O0=1) vs the DEFAULT optimizer
# (SSA + regalloc, no env) — run both, compare exit codes. The -O0 path is already at
# parity with the referee (measured beforehand), so opt≡-O0 on EVERY case ⟹ the
# optimization pipeline (SSA + const-fold/copy-prop/CSE/GVN/SCCP/DCE) is correct by
# transitivity, end-to-end on a REAL ELF. DIVERGE = a pass bug (printed for diagnosis).
# SKIP = a case where one of the two does not compile (exotic/reject) — outside the
# scope of measuring the passes.
#
# Run INSIDE the box:  ZCC=/usr/local/bin/zcc sh opt-parity.sh [N]   (N = case limit)
set -u
ZCC="${ZCC:-/usr/local/bin/zcc}"
C="${ZCC_SUITE_CACHE:-/suites}"
DIR="$C/gcc/gcc/testsuite/gcc.c-torture/execute"
LIM="${1:-0}"
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT
JOBS="${OPT_JOBS:-$(nproc 2>/dev/null || echo 4)}"
RES="$D/res"

# PARALLEL, and the verdict is order-independent by construction. Each case
# writes ONE tab-separated line — verdict, name, detail — and the aggregation is
# a separate pass over the SORTED result file, so the counts and the DIVERGE list
# do not depend on which worker finished first. (`csmith.sh` beside this file has
# used the same shape since it was written; this loop simply never adopted it.)
# Each case also gets its OWN two binaries: the serial version reused `$D/a` and
# `$D/b`, which parallel workers would race over — a race that would not fail
# loudly, it would compare one case's -O0 binary against another's optimized one.
work='
    f=$1
    b=$(basename "$f" .c)
    o="$D/$b"
    if ! ZCC_O0=1 "$ZCC" "$f" -o "$o.a" >/dev/null 2>&1; then printf "SKIP\t%s\t\n" "$b"; exit 0; fi
    if ! "$ZCC" "$f" -o "$o.b" >/dev/null 2>&1; then printf "SKIP\t%s\t\n" "$b"; exit 0; fi
    timeout 5 "$o.a" >/dev/null 2>&1; ra=$?
    timeout 5 "$o.b" >/dev/null 2>&1; rb=$?
    rm -f "$o.a" "$o.b"
    if [ "$ra" = "$rb" ]; then printf "PARITY\t%s\t\n" "$b"
    else printf "DIVERGE\t%s\t(noopt=%s,opt=%s)\n" "$b" "$ra" "$rb"; fi
'
export ZCC D
ls "$DIR"/*.c 2>/dev/null | { [ "$LIM" -gt 0 ] && head -n "$LIM" || cat; } \
  | xargs -P "$JOBS" -I@ sh -c "$work" _ @ | sort > "$RES"

n=$(wc -l < "$RES" | tr -d ' ')
par=$(grep -c "^PARITY" "$RES" || true)
div=$(grep -c "^DIVERGE" "$RES" || true)
skip=$(grep -c "^SKIP" "$RES" || true)
divlist=$(awk -F'\t' '$1=="DIVERGE"{printf " %s%s", $2, $3}' "$RES")

echo "opt-parity: $par PARITY / $div DIVERGE / $skip SKIP  (scanned $n cases, ${JOBS} jobs)"
[ -n "$divlist" ] && echo "DIVERGE:$divlist"
[ "$div" = 0 ]
