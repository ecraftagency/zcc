#!/bin/sh
# csmith.sh — RANDOM-DIFFERENTIAL correctness proof (bằng chứng ĐẮT: chục compiler
# cùng cỡ vẫn trượt). csmith sinh chương trình C UB-FREE by-construction; trọng tài
# ĐỘC LẬP = gcc (oracle ELF cùng arch trong box). Với mỗi .c cache sẵn ở
# /suites/csmith/*.c:
#   gcc  compile+run  → checksum tham chiếu (nếu gcc fail/timeout → SKIP: mẫu rác)
#   zcc  compile      → COMPILE-FAIL = NOT-IMPL (nêu tên, ngoài phạm vi — completeness)
#   zcc  run (noopt)  + zcc ZCC_OPT=1 run  → so stdout với gcc
# PARITY = gcc==zcc==zcc-opt (chuỗi checksum khớp). DIVERGE = compile+run được mà KHÁC
# gcc ⟹ MISCOMPILE (0 DIVERGE là bất biến; in ra để trị theo phân rã). Vì UB-free nên
# "diff tại UB" KHÔNG viện được — mọi DIVERGE là tội compiler tới khi proof ngược.
#
# Evidence trail (LUẬT INPUT SẠCH): in số binary thật + tổng byte để chống no-op xanh.
# Chạy TRONG box:  ZCC=/usr/local/bin/zcc sh csmith.sh [N]   (N = giới hạn)
set -u
ZCC="${ZCC:-/usr/local/bin/zcc}"
INC=/suites/csmith/include
DIR=/suites/csmith
LIM="${1:-0}"
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

par=0; div=0; ni=0; skip=0; n=0; bytes=0
divlist=""; nilist=""
for f in "$DIR"/*.c; do
    [ -f "$f" ] || continue
    b=$(basename "$f" .c)
    n=$((n+1))
    [ "$LIM" -gt 0 ] && [ "$n" -gt "$LIM" ] && { n=$((n-1)); break; }

    # oracle gcc — mẫu rác (gcc fail/timeout/UB-runtime) → SKIP, KHÔNG tính vào tội zcc
    if ! gcc -w -I"$INC" "$f" -o "$D/g" 2>/dev/null; then skip=$((skip+1)); continue; fi
    timeout 15 "$D/g" > "$D/go" 2>&1; grc=$?
    [ "$grc" -gt 127 ] && { skip=$((skip+1)); continue; } # gcc-binary crash/timeout: mẫu loại

    # zcc noopt — compile-fail = NOT-IMPL (completeness gap, nêu tên)
    if ! "$ZCC" -I"$INC" "$f" -o "$D/z" 2>"$D/ze"; then
        ni=$((ni+1)); nilist="$nilist $b:$(head -1 "$D/ze" | cut -c1-40)"; continue
    fi
    bytes=$((bytes + $(wc -c < "$D/z")))
    timeout 15 "$D/z" > "$D/zo" 2>&1; zrc=$?

    # zcc opt
    if ! ZCC_OPT=1 "$ZCC" -I"$INC" "$f" -o "$D/zp" 2>/dev/null; then
        div=$((div+1)); divlist="$divlist $b(OPT-COMPILE-FAIL)"; continue
    fi
    timeout 15 "$D/zp" > "$D/zpo" 2>&1; prc=$?

    if [ "$grc" = "$zrc" ] && [ "$grc" = "$prc" ] \
       && cmp -s "$D/go" "$D/zo" && cmp -s "$D/go" "$D/zpo"; then
        par=$((par+1))
    else
        div=$((div+1))
        divlist="$divlist $b(g=$grc,z=$zrc,o=$prc)"
    fi
done

echo "csmith: $par PARITY / $div DIVERGE / $ni NOT-IMPL / $skip SKIP  (quét $n; zcc-ELF ${bytes}B)"
[ -n "$divlist" ] && echo "DIVERGE:$divlist"
[ -n "$nilist" ] && echo "NOT-IMPL:$nilist" | cut -c1-400
[ "$div" = 0 ]
