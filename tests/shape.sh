#!/bin/sh
# Gate "hình dạng" — 3 lớp không gian hữu hạn còn lại của bộ 6 lớp:
#   lex    (lớp 1): bảng phân loại literal 3.1.3.2 + escape 3.1.3.4 + maximal
#                   munch — gen_lex.py
#   decl   (lớp 3): đại số declarator sâu ≤3 (ptr/array/fn-ptr, spiral rule),
#                   gọi thật qua function pointer — gen_decl.py
#   layout (lớp 5): layout struct/union/bitfield, offset từng member —
#                   gen_layout.py (sai ở đây = hindenbug interop SDK)
# Oracle: differential cc -std=c89 -w -O0, cùng source, diff stdout + exit.
# Lý thuyết từng lớp: đọc đầu mỗi gen_*.py. Chạy: tests/shape.sh [lex|decl|layout]
set -e
cd "$(dirname "$0")/.."
# ZCC đặt sẵn từ env (chạy trong box Linux, không cargo) — không thì tự build
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    ZCC=target/debug/zcc
fi
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

fail=0
for g in ${1:-lex decl layout}; do
    python3 "tests/gen_$g.py" "$D"
    f="$D/${g}_cases.c"
    "$ZCC" "$f" -o "$D/${g}_zcc" || { echo "SHAPE FAIL: zcc không compile ${g}_cases.c"; exit 1; }
    cc -std=c89 -w -O0 "$f" -o "$D/${g}_cc"
    z=0; "$D/${g}_zcc" > "$D/${g}_zcc.out" || z=$?
    c=0; "$D/${g}_cc" > "$D/${g}_cc.out" || c=$?
    [ "$z" = "$c" ] || { echo "SHAPE FAIL: $g exit zcc=$z cc=$c"; fail=1; }
    if ! diff -q "$D/${g}_zcc.out" "$D/${g}_cc.out" > /dev/null; then
        echo "SHAPE FAIL: $g stdout zcc≠cc"
        diff "$D/${g}_zcc.out" "$D/${g}_cc.out" | head -15
        fail=1
    else
        echo "$g: $(wc -l < "$D/${g}_zcc.out" | tr -d ' ') dòng khớp"
    fi
done
[ "$fail" = 0 ] && echo "SHAPE PASS" || exit 1
