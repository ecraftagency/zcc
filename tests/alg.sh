#!/bin/sh
# Gate đại số biểu thức: vét cạn (op × kiểu × kiểu × corner) sinh bởi gen_alg.py.
# 3 phép so: run zcc↔cc (trọng tài runtime), fold zcc↔cc (trọng tài const-eval),
# fold zcc ↔ fri zcc (biểu đồ giao hoán fold/runtime NỘI BỘ một compiler).
set -e
cd "$(dirname "$0")/.."
cargo build --quiet 2>/dev/null || cargo build
ZCC=target/debug/zcc
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

python3 tests/gen_alg.py "$D"

collect() { # $1=compiler-cmd-prefix  $2=stem  → output gộp $D/$2_$3.out
    : > "$D/$2_$3.out"
    for f in "$D"/alg_"$2"_*.c; do
        case "$3" in
            zcc) "$ZCC" "$f" -o "$D/x" ;;
            cc)  cc -std=c89 -w -O0 "$f" -o "$D/x" ;;
        esac
        "$D/x" >> "$D/$2_$3.out"
    done
}

fail=0
for stem in run fold fri; do
    collect _ "$stem" zcc
    collect _ "$stem" cc
    if ! diff -q "$D/${stem}_zcc.out" "$D/${stem}_cc.out" > /dev/null; then
        echo "ALG FAIL: $stem zcc≠cc"
        diff "$D/${stem}_zcc.out" "$D/${stem}_cc.out" | head -10
        fail=1
    fi
done
if ! diff -q "$D/fold_zcc.out" "$D/fri_zcc.out" > /dev/null; then
    echo "ALG FAIL: fold≠runtime nội bộ zcc (biểu đồ giao hoán vỡ)"
    diff "$D/fold_zcc.out" "$D/fri_zcc.out" | head -10
    fail=1
fi
n=$(wc -l < "$D/run_zcc.out" | tr -d ' ')
[ "$fail" = 0 ] && echo "ALG PASS ($n điểm runtime + $(wc -l < "$D/fold_zcc.out" | tr -d ' ') điểm fold, 4 phép so)" || exit 1
