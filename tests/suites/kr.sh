#!/bin/sh
# K&R 2nd ed (ANSI C89 nguyên bản) — bài tập + ví dụ sách, repo caisah.
# Nhiều bài đọc stdin (copy/count/word): cho mỗi binary đọc CHÍNH SOURCE
# của nó làm stdin — deterministic và không tầm thường. Oracle differential
# cc -std=c99 (diff exit + stdout); referee không compile được → skip.
# Referee c99 (KHÔNG c89): zcc target C99 (charter 2026-08-18) + clang -std=c89
# strict từ chối //-comment/C99-ism trong đáp án caisah (compile-ok 66 vs c99
# 109) + C99 định nghĩa fall-off-main=return 0 (C89 unspecified) khớp zcc.
# Gate: tập FAIL ⊆ baseline kr.known-fail.
set -e
cd "$(dirname "$0")/../.."
# ZCC đặt sẵn từ env (chạy trong box Linux, không cargo) — không thì tự build
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    export ZCC="$PWD/target/debug/zcc"
fi
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/kr"
[ -d "$DIR" ] || { echo "thiếu cache: clone K-and-R-exercises-and-examples (caisah) theo tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

find "$DIR" -name '*.c' | { [ -n "${SEEK:-}" ] && grep -F -- "$SEEK" || cat; } | xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(echo "$f" | sed "s|.*/kr/||; s|/|_|g; s|\.c$||")
    cc -std=c99 -w -O0 "$f" -o "$D/$b.cc" 2>/dev/null || { echo "skip $b"; exit 0; }
    perl -e "alarm 10; exec @ARGV" "$D/$b.cc" < "$f" > "$D/$b.cout" 2>/dev/null; ec=$?
    if ! "$ZCC" "$f" -o "$D/$b.z" 2>/dev/null; then echo "FAIL $b (compile)"; exit 0; fi
    perl -e "alarm 10; exec @ARGV" "$D/$b.z" < "$f" > "$D/$b.zout" 2>/dev/null; ez=$?
    if [ "$ec" = "$ez" ] && cmp -s "$D/$b.cout" "$D/$b.zout"; then
        echo "pass $b"
    else
        echo "FAIL $b (exit $ec vs $ez)"
    fi
' sh > "$D/res"

p=$(grep -c '^pass' "$D/res" || true); s=$(grep -c '^skip' "$D/res" || true)
grep '^FAIL' "$D/res" | cut -d' ' -f1-2 | sort > "$D/fails"   # bỏ suffix exit-code: UB program rác stack đổi theo run
[ -n "${LIVE_FAILS_DIR:-}" ] && cp "$D/fails" "$LIVE_FAILS_DIR/kr.fails" 2>/dev/null; :
sort "$(dirname "$0")/kr.known-fail" > "$D/known" 2>/dev/null || : > "$D/known"
new=$(comm -23 "$D/fails" "$D/known" || true)
echo "kr: $p pass, $s skip, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "KR FAIL MỚI (ngoài baseline):"; echo "$new" | head -20; exit 1
fi
echo "KR PASS (mọi fail đều trong baseline đã triage)"
