#!/bin/sh
# chibicc test/ — mỗi test tự kiểm bằng ASSERT (in "... => v", exit 1 nếu sai),
# link kèm test/common. Phần lớn test đòi C11/gcc-ext (alignof, atomic,
# bitfield...) → referee-filter cc -std=c89 skip chúng; phần C89-được thì
# zcc phải chạy khớp stdout + exit với referee. Referee KHÔNG -std=c89 (test
# chibicc là đất C11/gcc-ext — cùng triết lý tests/ext/): c89-referee skip
# sạch 41 file → gate rỗng, vô nghĩa.
# Gate: tập FAIL ⊆ baseline chibicc.known-fail.
set -e
cd "$(dirname "$0")/../.."
# ZCC đặt sẵn từ env (chạy trong box Linux, không cargo) — không thì tự build
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    export ZCC="$PWD/target/debug/zcc"
fi
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/chibicc/test"
[ -d "$DIR" ] || { echo "thiếu cache: clone chibicc theo tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

cp "$DIR/common" "$D/zcommon.c"   # common không đuôi .c — cc coi là linker input
ls "$DIR"/*.c | xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(basename "$f" .c)
    cc -w -O0 -I"$DIR" "$f" "$D/zcommon.c" -o "$D/$b.cc" 2>/dev/null \
        || { echo "skip $b"; exit 0; }
    "$D/$b.cc" > "$D/$b.cout" 2>/dev/null; ec=$?
    # referee tự fail test của chính nó (đặc thù arm64) → oracle vô hiệu → skip
    [ "$ec" = 0 ] || { echo "skip $b"; exit 0; }
    if ! "$ZCC" -I"$DIR" "$f" "$D/zcommon.c" -o "$D/$b.z" 2>/dev/null; then
        echo "FAIL $b (compile)"; exit 0
    fi
    "$D/$b.z" > "$D/$b.zout" 2>/dev/null; ez=$?
    if [ "$ec" = "$ez" ] && cmp -s "$D/$b.cout" "$D/$b.zout"; then
        echo "pass $b"
    else
        echo "FAIL $b (exit $ec vs $ez)"
    fi
' sh > "$D/res"

p=$(grep -c '^pass' "$D/res" || true); s=$(grep -c '^skip' "$D/res" || true)
grep '^FAIL' "$D/res" | cut -d' ' -f1-2 | sort > "$D/fails"   # bỏ suffix exit-code: UB program rác stack đổi theo run
[ -n "${LIVE_FAILS_DIR:-}" ] && cp "$D/fails" "$LIVE_FAILS_DIR/chibicc.fails" 2>/dev/null; :
sort "$(dirname "$0")/chibicc.known-fail" > "$D/known" 2>/dev/null || : > "$D/known"
new=$(comm -23 "$D/fails" "$D/known" || true)
echo "chibicc: $p pass, $s skip, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "CHIBICC FAIL MỚI (ngoài baseline):"; echo "$new" | head -20; exit 1
fi
echo "CHIBICC PASS (mọi fail đều trong baseline đã triage)"
