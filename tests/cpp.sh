#!/bin/sh
# Gate lớp 2 — preprocessor như hệ viết lại hạng (lý thuyết + exclusion list:
# đọc đầu tests/gen_cpp.py). Hai chương trình sinh ra:
#   cpp_mech.c — ma trận tương tác cơ chế expansion, dạng chuẩn reify qua "#"
#                thành string in ra runtime → diff stdout zcc↔cc
#   cpp_if.c   — vét cạn số học #if (long/ulong, 3.8.1), oracle kép:
#                zcc==cc VÀ cc==python (FAIL nội dòng nếu lệch python)
set -e
cd "$(dirname "$0")/.."
cargo build --quiet 2>/dev/null || cargo build
ZCC=target/debug/zcc
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

python3 tests/gen_cpp.py "$D"

fail=0
for f in cpp_mech cpp_if; do
    "$ZCC" "$D/$f.c" -o "$D/${f}_zcc" || { echo "CPP FAIL: zcc không compile $f.c"; exit 1; }
    cc -std=c89 -w -O0 "$D/$f.c" -o "$D/${f}_cc"
    z=0; "$D/${f}_zcc" > "$D/${f}_zcc.out" || z=$?
    c=0; "$D/${f}_cc" > "$D/${f}_cc.out" || c=$?
    [ "$z" = "$c" ] || { echo "CPP FAIL: $f exit zcc=$z cc=$c"; fail=1; }
    if ! diff -q "$D/${f}_zcc.out" "$D/${f}_cc.out" > /dev/null; then
        echo "CPP FAIL: $f stdout zcc≠cc"
        diff "$D/${f}_zcc.out" "$D/${f}_cc.out" | head -20
        fail=1
    fi
    if grep -q FAIL "$D/${f}_cc.out"; then
        echo "CPP FAIL: generator lệch cc (python sai luật?)"
        grep FAIL "$D/${f}_cc.out" | head -5
        fail=1
    fi
    if grep -q FAIL "$D/${f}_zcc.out"; then
        echo "CPP FAIL: zcc lệch python:"
        grep FAIL "$D/${f}_zcc.out" | head -5
        fail=1
    fi
done
if [ "$fail" = 0 ]; then
    echo "CPP PASS: mech $(wc -l < "$D/cpp_mech_zcc.out" | tr -d ' ') dòng khớp + if: $(cat "$D/cpp_if_zcc.out")"
else
    exit 1
fi
