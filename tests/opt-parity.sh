#!/bin/sh
# opt-parity.sh — ĐO cơ học (differential tự thân): với mỗi torture execute .c,
# compile HAI lần — KHÔNG opt (ZCC_OPT unset) vs CÓ opt (ZCC_OPT=1) —
# chạy cả hai, so exit code. Đường noopt đã ở parity với referee (đo trước),
# nên opt≡noopt trên MỌI case ⟹ pipeline pass (const-fold/copy-prop/CSE/DCE) đúng
# bắc-cầu, end-to-end trên ELF THẬT. DIVERGE = bug pass (in ra để trị). SKIP = case
# một trong hai không compile (exotic/reject) — ngoài phạm vi đo passes.
#
# Chạy TRONG box:  ZCC=/usr/local/bin/zcc sh opt-parity.sh [N]   (N = giới hạn số case)
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
    # opt
    if ! ZCC_OPT=1 "$ZCC" "$f" -o "$D/b" >/dev/null 2>&1; then skip=$((skip+1)); continue; fi

    timeout 5 "$D/a" >/dev/null 2>&1; ra=$?
    timeout 5 "$D/b" >/dev/null 2>&1; rb=$?
    if [ "$ra" = "$rb" ]; then
        par=$((par+1))
    else
        div=$((div+1))
        divlist="$divlist $b(noopt=$ra,opt=$rb)"
    fi
done

echo "opt-parity: $par PARITY / $div DIVERGE / $skip SKIP  (quét $n case)"
[ -n "$divlist" ] && echo "DIVERGE:$divlist"
[ "$div" = 0 ]
