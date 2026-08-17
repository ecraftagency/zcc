#!/bin/sh
# c-testsuite (single-exec) — oracle CHÍNH CHỦ: mỗi case kèm file .expected,
# pass = exit 0 + stdout khớp từng byte. Không cần referee.
# Gate: tập FAIL ⊆ baseline cts.known-fail (đã triage: ngoài scope C89).
set -e
cd "$(dirname "$0")/../.."
cargo build --quiet 2>/dev/null || cargo build
export ZCC="$PWD/target/debug/zcc"
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/c-testsuite/tests/single-exec"
[ -d "$DIR" ] || { echo "thiếu cache: clone c-testsuite theo tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

ls "$DIR"/*.c | xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(basename "$f" .c)
    if "$ZCC" "$f" -o "$D/$b" 2>/dev/null \
       && perl -e "alarm 10; exec @ARGV" "$D/$b" > "$D/$b.out" 2>/dev/null \
       && cmp -s "$D/$b.out" "$f.expected"; then
        echo "pass $b"
    else
        echo "FAIL $b"
    fi
' sh > "$D/res"

p=$(grep -c '^pass' "$D/res" || true)
grep '^FAIL' "$D/res" | sort > "$D/fails"
sort "$(dirname "$0")/cts.known-fail" > "$D/known" 2>/dev/null || : > "$D/known"
new=$(comm -23 "$D/fails" "$D/known" || true)
echo "c-testsuite: $p pass, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "CTS FAIL MỚI (ngoài baseline):"; echo "$new" | head -20; exit 1
fi
echo "CTS PASS (mọi fail đều trong baseline đã triage)"
