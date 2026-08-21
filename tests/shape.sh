#!/bin/sh
# "Shape" gate — the 3 remaining finite-space layers of the 6-layer set:
#   lex    (layer 1): literal classification table 3.1.3.2 + escape 3.1.3.4 +
#                     maximal munch — gen_lex.py
#   decl   (layer 3): declarator algebra of depth ≤3 (ptr/array/fn-ptr, spiral
#                     rule), with real calls through a function pointer — gen_decl.py
#   layout (layer 5): struct/union/bitfield layout, per-member offset —
#                     gen_layout.py (an error here = an SDK interop heisenbug)
# Oracle: differential cc -std=c89 -w -O0, same source, diff stdout + exit.
# Per-layer theory: read the head of each gen_*.py. Run: tests/shape.sh [lex|decl|layout]
set -e
cd "$(dirname "$0")/.."
# ZCC preset from the env (running in the Linux box, no cargo) — otherwise self-build
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
    "$ZCC" "$f" -o "$D/${g}_zcc" || { echo "SHAPE FAIL: zcc did not compile ${g}_cases.c"; exit 1; }
    cc -std=c89 -w -O0 "$f" -o "$D/${g}_cc"
    z=0; "$D/${g}_zcc" > "$D/${g}_zcc.out" || z=$?
    c=0; "$D/${g}_cc" > "$D/${g}_cc.out" || c=$?
    [ "$z" = "$c" ] || { echo "SHAPE FAIL: $g exit zcc=$z cc=$c"; fail=1; }
    if ! diff -q "$D/${g}_zcc.out" "$D/${g}_cc.out" > /dev/null; then
        echo "SHAPE FAIL: $g stdout zcc≠cc"
        diff "$D/${g}_zcc.out" "$D/${g}_cc.out" | head -15
        fail=1
    else
        echo "$g: $(wc -l < "$D/${g}_zcc.out" | tr -d ' ') lines match"
    fi
done
[ "$fail" = 0 ] && echo "SHAPE PASS" || exit 1
