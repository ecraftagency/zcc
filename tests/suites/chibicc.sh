#!/bin/sh
# chibicc test/ — mỗi test tự kiểm bằng ASSERT (in "... => v", exit 1 nếu sai),
# link kèm test/common. Phần lớn test đòi C11/gcc-ext (alignof, atomic,
# bitfield...) → referee-filter cc -std=c89 skip chúng; phần C89-được thì
# zcc phải chạy khớp stdout + exit với referee.
# Gate: tập FAIL ⊆ baseline chibicc.known-fail.
set -e
cd "$(dirname "$0")/../.."
cargo build --quiet 2>/dev/null || cargo build
export ZCC="$PWD/target/debug/zcc"
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/chibicc/test"
[ -d "$DIR" ] || { echo "thiếu cache: clone chibicc theo tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

ls "$DIR"/*.c | xargs -P 8 -I{} sh -c '
    f="{}"; b=$(basename "$f" .c)
    cc -std=c89 -w -O0 -I"$DIR" "$f" "$DIR/common" -o "$D/$b.cc" 2>/dev/null \
        || { echo "skip $b"; exit 0; }
    "$D/$b.cc" > "$D/$b.cout" 2>/dev/null; ec=$?
    if ! "$ZCC" -I"$DIR" "$f" "$DIR/common" -o "$D/$b.z" 2>/dev/null; then
        echo "FAIL $b (compile)"; exit 0
    fi
    "$D/$b.z" > "$D/$b.zout" 2>/dev/null; ez=$?
    if [ "$ec" = "$ez" ] && cmp -s "$D/$b.cout" "$D/$b.zout"; then
        echo "pass $b"
    else
        echo "FAIL $b (exit $ec vs $ez)"
    fi
' > "$D/res"

p=$(grep -c '^pass' "$D/res" || true); s=$(grep -c '^skip' "$D/res" || true)
grep '^FAIL' "$D/res" | sort > "$D/fails"
new=$(comm -23 "$D/fails" <(sort "$(dirname "$0")/chibicc.known-fail" 2>/dev/null || :) || true)
echo "chibicc: $p pass, $s skip, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "CHIBICC FAIL MỚI (ngoài baseline):"; echo "$new" | head -20; exit 1
fi
echo "CHIBICC PASS (mọi fail đều trong baseline đã triage)"
