#!/bin/sh
# Gate decay: định lý lvalue conversion C99 6.3.2.1p3 (+6.5.15, 6.5.2.2p6) —
# vét cấu trúc SOURCES × CONTEXTS sinh bởi gen_decay.py, oracle differential
# cc trên observable dẫn xuất (không in địa chỉ thô nên diff ổn định).
# Sinh từ án B 18/8: ternary string literal không decay → vararg stack cắt
# strh 2 byte → segv theo layout. Ô đó giờ có gate đứng gác vĩnh viễn.
set -e
cd "$(dirname "$0")/.."
# ZCC đặt sẵn từ env (chạy trong box Linux, không cargo) — không thì tự build
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    ZCC=target/debug/zcc
fi
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

python3 tests/gen_decay.py "$D"

"$ZCC" "$D/decay_t.c" -o "$D/dz"
cc -std=c99 -w -O0 "$D/decay_t.c" -o "$D/dc"
"$D/dz" > "$D/out_zcc.txt"
"$D/dc" > "$D/out_cc.txt"
if diff -u "$D/out_cc.txt" "$D/out_zcc.txt" > "$D/d.txt"; then
    echo "DECAY PASS ($(wc -l < "$D/out_cc.txt" | tr -d ' ') dòng khớp differential)"
else
    head -40 "$D/d.txt"
    echo "DECAY FAIL"
    exit 1
fi
