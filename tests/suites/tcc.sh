#!/bin/sh
# Suite 6 — test suite CỦA CHÍNH tcc (tests2: ~130 case .c + .expect), chạy trên
# tcc DO ZCC BUILD. Đây là kiểm chứng bắc cầu mức suite: sai codegen zcc →
# tcc(zcc) hành xử khác tcc(cc) → lệch .expect.
# Trọng tài kép: build tcc HAI LẦN (CC=cc và CC=zcc) từ cùng source, chạy cùng
# runner. Case mà tcc(cc) không tái tạo được .expect (cần args/stdin đặc thù
# Makefile tcc) = ngoài scope runner → skip. Còn lại tcc(zcc) phải khớp .expect.
# Gate: tập FAIL ⊆ baseline tcc.known-fail.
set -e
cd "$(dirname "$0")/../.."
# ZCC đặt sẵn từ env — không thì tự build (LƯU Ý: suite này Darwin-only, cần xcrun/SDK)
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    ZCC="$PWD/target/debug/zcc"
fi
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
SRC="$C/tinycc"
[ -d "$SRC" ] || git clone --depth 1 https://github.com/TinyCC/tinycc.git "$SRC"
export SDK=$(xcrun -sdk macosx --show-sdk-path)
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

# hai bản build từ cùng snapshot source (m9 đã chứng minh make CC=zcc chạy)
for cc_ in cc zcc; do
    cp -R "$SRC" "$D/$cc_"
    cd "$D/$cc_"
    ./configure > /dev/null
    make tccdefs_.h > /dev/null
    if [ "$cc_" = zcc ]; then MK="$ZCC"; else MK=cc; fi
    make CC="$MK" CFLAGS="-DCONFIG_TCC_BACKTRACE=0 -DCONFIG_TCC_SEMLOCK=0" tcc > /dev/null 2>&1
    make libtcc1.a > /dev/null 2>&1 || true
    [ -f tcc ] && [ -f libtcc1.a ]
    cd - > /dev/null
done

export REF="$D/cc/tcc" ZTCC="$D/zcc/tcc"
ls "$SRC"/tests/tests2/*.c | xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(basename "$f" .c); e="${f%.c}.expect"
    [ -f "$e" ] || { echo "skip $b"; exit 0; }
    perl -e "alarm 15; exec @ARGV" "$REF" -B "$(dirname "$REF")" \
        -L"$SDK/usr/lib" -run "$f" > "$D/$b.ref" 2>&1 || true
    cmp -s "$D/$b.ref" "$e" || { echo "skip $b"; exit 0; }   # trọng tài không tái tạo được .expect
    perl -e "alarm 15; exec @ARGV" "$ZTCC" -B "$(dirname "$ZTCC")" \
        -L"$SDK/usr/lib" -run "$f" > "$D/$b.z" 2>&1 || true
    if cmp -s "$D/$b.z" "$e"; then echo "pass $b"; else echo "FAIL $b"; fi
' sh > "$D/res"

p=$(grep -c '^pass' "$D/res" || true); s=$(grep -c '^skip' "$D/res" || true)
grep '^FAIL' "$D/res" | sort > "$D/fails"
[ -n "${LIVE_FAILS_DIR:-}" ] && cp "$D/fails" "$LIVE_FAILS_DIR/tcc.fails" 2>/dev/null; :
sort "$(dirname "$0")/tcc.known-fail" > "$D/known" 2>/dev/null || : > "$D/known"
new=$(comm -23 "$D/fails" "$D/known" || true)
echo "tcc: $p pass, $s skip, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "TCC FAIL MỚI (ngoài baseline):"; echo "$new" | head -20; exit 1
fi
echo "TCC PASS (mọi fail đều trong baseline đã triage)"
