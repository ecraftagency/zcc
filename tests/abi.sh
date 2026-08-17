#!/bin/sh
# Gate ABI: phủ transition automaton (kiểu × trạng thái GPR/FPR/stack) sinh bởi
# gen_abi.py, link CHÉO cc↔zcc cả hai chiều + hai control cùng-compiler.
# Lỗi ABI cùng-compiler tự triệt tiêu — chỉ link chéo mới phơi ra.
set -e
cd "$(dirname "$0")/.."
cargo build --quiet 2>/dev/null || cargo build
ZCC=target/debug/zcc
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

python3 tests/gen_abi.py "$D"

"$ZCC" -c "$D/abi_callee.c" -o "$D/callee_zcc.o"
"$ZCC" -c "$D/abi_caller.c" -o "$D/caller_zcc.o"
cc -std=c89 -w -O0 -c "$D/abi_callee.c" -o "$D/callee_cc.o"
cc -std=c89 -w -O0 -c "$D/abi_caller.c" -o "$D/caller_cc.o"

fail=0
for pair in "cc zcc" "zcc cc" "zcc zcc" "cc cc"; do
    set -- $pair
    cc "$D/callee_$1.o" "$D/caller_$2.o" -o "$D/run" 2>/dev/null
    out=$("$D/run") || { echo "ABI FAIL callee=$1 caller=$2"; echo "$out" | grep FAIL | head -20; fail=1; }
    echo "callee=$1 caller=$2: $out" | tail -1
done
[ "$fail" = 0 ] && echo "ABI PASS (4 hướng link)" || exit 1
