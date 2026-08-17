#!/bin/sh
# Nora Sandler (writing-a-c-compiler-tests) — toàn bộ case valid/, oracle
# differential: referee cc -std=c89 (skip case referee không nhận — subset
# sách vượt C89 ở vài chương), diff exit code + stdout.
# Gate: tập FAIL ⊆ baseline nora.known-fail.
set -e
cd "$(dirname "$0")/../.."
cargo build --quiet 2>/dev/null || cargo build
export ZCC="$PWD/target/debug/zcc"
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/nora/tests"
[ -d "$DIR" ] || { echo "thiếu cache: clone nora theo tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

find "$DIR" -path '*/valid/*' -name '*.c' | xargs -P 8 -I{} sh -c '
    f="{}"; b=$(echo "$f" | sed "s|.*/tests/||; s|/|_|g; s|\.c$||")
    cc -std=c89 -w -O0 "$f" -o "$D/$b.cc" 2>/dev/null || { echo "skip $b"; exit 0; }
    "$D/$b.cc" > "$D/$b.cout" 2>/dev/null; ec=$?
    if ! "$ZCC" "$f" -o "$D/$b.z" 2>/dev/null; then echo "FAIL $b (compile)"; exit 0; fi
    "$D/$b.z" > "$D/$b.zout" 2>/dev/null; ez=$?
    if [ "$ec" = "$ez" ] && cmp -s "$D/$b.cout" "$D/$b.zout"; then
        echo "pass $b"
    else
        echo "FAIL $b (exit $ec vs $ez)"
    fi
' > "$D/res"

p=$(grep -c '^pass' "$D/res" || true); s=$(grep -c '^skip' "$D/res" || true)
grep '^FAIL' "$D/res" | sort > "$D/fails"
new=$(comm -23 "$D/fails" <(sort "$(dirname "$0")/nora.known-fail" 2>/dev/null || :) || true)
echo "nora: $p pass, $s skip, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "NORA FAIL MỚI (ngoài baseline):"; echo "$new" | head -20; exit 1
fi
echo "NORA PASS (mọi fail đều trong baseline đã triage)"
