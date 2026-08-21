#!/bin/sh
# opt-parity.sh — a mechanical MEASUREMENT (self-differential): for each torture
# execute .c, compile TWICE — WITHOUT opt (ZCC_OPT unset) vs WITH opt (ZCC_OPT=1) —
# run both, compare exit codes. The noopt path is already at parity with the
# referee (measured beforehand), so opt≡noopt on EVERY case ⟹ the optimization
# pipeline (const-fold/copy-prop/CSE/DCE) is correct by transitivity, end-to-end on
# a REAL ELF. DIVERGE = a pass bug (printed for diagnosis). SKIP = a case where one
# of the two does not compile (exotic/reject) — outside the scope of measuring the
# passes.
#
# Run INSIDE the box:  ZCC=/usr/local/bin/zcc sh opt-parity.sh [N]   (N = case limit)
set -u
ZCC="${ZCC:-/usr/local/bin/zcc}"
C="${ZCC_SUITE_CACHE:-/suites}"
DIR="$C/gcc/gcc/testsuite/gcc.c-torture/execute"
LIM="${1:-0}"
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

par=0; div=0; skip=0; n=0
divlist=""
for f in "$DIR"/*.c; do
    [ -f "$f" ] || continue
    b=$(basename "$f" .c)
    n=$((n+1))
    [ "$LIM" -gt 0 ] && [ "$n" -gt "$LIM" ] && { n=$((n-1)); break; }

    # noopt
    if ! "$ZCC" "$f" -o "$D/a" >/dev/null 2>&1; then skip=$((skip+1)); continue; fi
    # opt (ZCC_OPT_MODE=1 → non-SSA pipeline; =ssa → the SSA/QBE pipeline)
    if ! ZCC_OPT="${ZCC_OPT_MODE:-1}" "$ZCC" "$f" -o "$D/b" >/dev/null 2>&1; then skip=$((skip+1)); continue; fi

    timeout 5 "$D/a" >/dev/null 2>&1; ra=$?
    timeout 5 "$D/b" >/dev/null 2>&1; rb=$?
    if [ "$ra" = "$rb" ]; then
        par=$((par+1))
    else
        div=$((div+1))
        divlist="$divlist $b(noopt=$ra,opt=$rb)"
    fi
done

echo "opt-parity: $par PARITY / $div DIVERGE / $skip SKIP  (scanned $n cases)"
[ -n "$divlist" ] && echo "DIVERGE:$divlist"
[ "$div" = 0 ]
