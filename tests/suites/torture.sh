#!/bin/sh
# GCC c-torture/execute — suite hành hạ codegen lớn nhất còn sống.
# Oracle: mỗi test tự kiểm bằng abort(), exit 0 = pass. Referee-filter:
# case nào `cc -std=c89 -w -O0` không compile/chạy sạch = ngoài scope C89
# (builtin gcc, C99+...) → skip; còn lại zcc phải compile + exit 0.
# Gate: tập FAIL ⊆ baseline torture.known-fail (các fail đã triage: toàn
# extension ngoài scope mà clang vẫn nuốt — vector_size, _Complex, VLA...).
# Cache clone (dựng lại nếu mất): xem tests/README.md.
set -e
cd "$(dirname "$0")/../.."
# ZCC đặt sẵn từ env (chạy trong box Linux, không cargo) — không thì tự build
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    export ZCC="$PWD/target/debug/zcc"
fi
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/gcc/gcc/testsuite/gcc.c-torture/execute"
[ -d "$DIR" ] || { echo "thiếu cache: clone sparse gcc theo tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

ls "$DIR"/*.c | xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(basename "$f" .c)
    cc -std=c89 -w -O0 "$f" -o "$D/$b.cc" 2>/dev/null || { echo "skip $b"; exit 0; }
    perl -e "alarm 10; exec @ARGV" "$D/$b.cc" >/dev/null 2>&1 || { echo "skip $b"; exit 0; }
    if "$ZCC" "$f" -o "$D/$b.z" 2>/dev/null \
       && perl -e "alarm 10; exec @ARGV" "$D/$b.z" >/dev/null 2>&1; then
        echo "pass $b"
    else
        echo "FAIL $b"
    fi
' sh > "$D/res"

p=$(grep -c '^pass' "$D/res" || true); s=$(grep -c '^skip' "$D/res" || true)
grep '^FAIL' "$D/res" | sort > "$D/fails"
sort "$(dirname "$0")/torture.known-fail" > "$D/known" 2>/dev/null || : > "$D/known"
new=$(comm -23 "$D/fails" "$D/known" || true)
echo "torture: $p pass, $s skip, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "TORTURE FAIL MỚI (ngoài baseline):"; echo "$new" | head -20; exit 1
fi
echo "TORTURE PASS (mọi fail đều trong baseline đã triage)"
