#!/bin/sh
# GCC c-torture/execute — suite hành hạ codegen lớn nhất. Oracle: mỗi test tự
# kiểm bằng abort(), exit 0 = pass. Referee = `cc -std=c99 -w -O0` (gcc trong
# box: trọng tài độc lập, native cho chính suite của nó).
#
# PHÂN 3 ĐƯỜNG (bất biến 2-fact, Vu 2026-08-20) — mỗi case đúng 1 nhãn:
#   PASS     zcc compile chương trình C99-hợp-lệ → chạy exit 0 (khớp oracle).
#   NOT-IMPL zcc TỪ CHỐI SẠCH (chưa cài construct) hoặc case ngoài tầm C99:
#              oracle-invalid  referee c99 không compile-nổi/không-chạy-sạch
#                              → UB hoặc gcc-ext ngoài strict-C99.
#              zcc-reject      referee OK nhưng zcc in diagnostic `zcc:…` rồi
#                              exit 1, KHÔNG đẻ binary, KHÔNG crash. Trung thực.
#   FAIL     zcc NUỐT chương trình C99-hợp-lệ rồi cho SAI/CRASH — chỉ số phải =0:
#              runtime   zcc đẻ binary (exit 0) nhưng binary sai/abort/crash.
#              backend   zcc exit 1 KHÔNG có diagnostic `zcc:` → as/ld nghẹn asm
#                        rác zcc phun (nuốt-rồi-phun-rác, không phải reject sạch).
#              crash     zcc panic/signal (rc=101 hoặc >128) — compiler tự chết.
#
# Gate = 0 FAIL. NOT-IMPL được phép nhưng manifest phải NÊU ĐÍCH DANH lý do
# (torture.not-impl) để audit. Cache clone: xem tests/README.md.
set -e
cd "$(dirname "$0")/../.."
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    export ZCC="$PWD/target/debug/zcc"
fi
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/gcc/gcc/testsuite/gcc.c-torture/execute"
[ -d "$DIR" ] || { echo "thiếu cache: clone sparse gcc theo tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

# LUẬT BẢO TOÀN (chống skip ngầm — Vu 2026-08-20): danh sách case được nạp
# ghi cứng ra $D/fed; sau vòng chạy, MỖI case phải xuất hiện ĐÚNG 1 lần trong
# res. pass+not-impl+fail phải = |fed|. Lệch → có case bốc hơi (worker chết,
# OOM, zcc treo) → harness TỰ ĐỎ, không cho xanh giả. Verdict xanh vô nghĩa nếu
# phép đo bỏ sót — reviewer/Vu chỉ cần đối chiếu 1 đẳng thức, không cần tin tao.
ls "$DIR"/*.c | { [ -n "${SEEK:-}" ] && grep -F -- "$SEEK" || cat; } > "$D/fed"
nfed=$(wc -l < "$D/fed" | tr -d ' ')
# mỗi worker in 1 dòng TSV: <CLASS>\t<sub>\t<case>\t<reason>
xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(basename "$f" .c); e="$D/$b.err"
    # ── referee c99 (trọng tài độc lập) ──
    # -w tắt warning nhưng error THẬT vẫn hiện: bắt lý do reject để manifest nêu
    # tên (reviewer phân biệt gcc-ext/target-specific vs referee-quirk).
    if ! cc -std=c99 -w -O0 "$f" -o "$D/$b.cc" 2>"$D/$b.ce"; then
        why=$(grep -m1 "error:" "$D/$b.ce" | sed "s|.*/execute/||;s|\t| |g")
        printf "NOTIMPL\toracle-invalid\t%s\treferee-reject: %s\n" "$b" "${why:-compile-fail}"; exit 0
    fi
    if ! perl -e "alarm 10; exec @ARGV" "$D/$b.cc" >/dev/null 2>&1; then
        printf "NOTIMPL\toracle-invalid\t%s\treferee-run-fail(UB?)\n" "$b"; exit 0
    fi
    # ── zcc ──
    "$ZCC" "$f" -o "$D/$b.z" 2>"$e"; zrc=$?
    # manifest phải STABLE + lộ đúng construct: bỏ "zcc: <path>: " của driver,
    # collapse mọi path box-specific ".../execute/" → chỉ còn "<case>.c:<ln>: msg"
    r=$(sed -n "1p" "$e" 2>/dev/null | sed -e "s|^zcc: [^:]*: ||" -e "s|/[^ ]*/execute/||g" | tr "\t" " ")
    [ -n "$r" ] || r=$(head -1 "$e" 2>/dev/null | tr "\t" " ")
    if [ "$zrc" -eq 0 ]; then
        if perl -e "alarm 10; exec @ARGV" "$D/$b.z" >/dev/null 2>&1; then
            printf "PASS\t-\t%s\t-\n" "$b"
        else
            printf "FAIL\truntime\t%s\tzcc-binary-sai/crash\n" "$b"
        fi
    elif [ "$zrc" -eq 1 ]; then
        if grep -q "^zcc:" "$e"; then
            printf "NOTIMPL\tzcc-reject\t%s\t%s\n" "$b" "${r:-reject}"
        else
            printf "FAIL\tbackend\t%s\t%s\n" "$b" "${r:-as/ld-choke}"
        fi
    else
        printf "FAIL\tcrash\t%s\trc=%s %s\n" "$b" "$zrc" "$r"
    fi
' sh < "$D/fed" > "$D/res"

sort "$D/res" > "$D/res.s"
p=$(  grep -c "^PASS"    "$D/res.s" || true)
oi=$( grep -c "	oracle-invalid	" "$D/res.s" || true)
zr=$( grep -c "	zcc-reject	"     "$D/res.s" || true)
ni=$((oi + zr))
k=$(  grep -c "^FAIL"    "$D/res.s" || true)

# ── KIỂM BẢO TOÀN: mỗi case nạp phải được phân loại ĐÚNG 1 lần ──
nout=$(wc -l < "$D/res.s" | tr -d ' ')
cut -f3 "$D/res.s" | sort > "$D/seen"          # tên case đã có verdict
sed "s|.*/||;s|\.c$||" "$D/fed" | sort > "$D/want"   # tên case đã nạp
miss=$(comm -23 "$D/want" "$D/seen")           # nạp mà không có verdict = bốc hơi
dup=$(cut -f3 "$D/res.s" | sort | uniq -d)     # 1 case ≥2 verdict = đếm nhiễu
if [ "$nout" -ne "$nfed" ] || [ -n "$miss" ] || [ -n "$dup" ]; then
    echo "!! BẢO TOÀN VỠ: nạp=$nfed, verdict=$nout (pass=$p not-impl=$ni fail=$k)"
    [ -n "$miss" ] && { echo "   BỐC HƠI (nạp, không verdict):"; echo "$miss" | head; }
    [ -n "$dup" ]  && { echo "   TRÙNG (≥2 verdict):"; echo "$dup" | head; }
    echo "TORTURE ĐỎ (phép đo bỏ sót/nhiễu — verdict xanh vô hiệu tới khi vá)"; exit 1
fi
[ "$((p + ni + k))" -eq "$nfed" ] || { echo "!! p+ni+k != nạp"; exit 1; }

# manifest NOT-IMPL: <bucket> <case> <reason> — nêu đích danh, audit được.
grep "^NOTIMPL" "$D/res.s" | cut -f2- | sort > "$D/not-impl"
MF="${NOTIMPL_OUT:-$(dirname "$0")/torture.not-impl}"
cp "$D/not-impl" "$MF" 2>/dev/null || :   # box mount ro → in ra dưới thay vì ghi

# work-list FAIL (chỉ số duy nhất phải về 0)
grep "^FAIL" "$D/res.s" | cut -f2- > "$D/fails"
[ -n "${LIVE_FAILS_DIR:-}" ] && cp "$D/fails" "$LIVE_FAILS_DIR/torture.fails" 2>/dev/null; :

if [ "$k" -gt 0 ]; then
    echo "── FAIL work-list ($k) — zcc nuốt C99-hợp-lệ rồi sai/crash:"
    cat "$D/fails"
fi
echo "── NOT-IMPL manifest (torture.not-impl):"
cat "$D/not-impl"
echo "torture: $p pass, $ni not-impl ($oi oracle-invalid + $zr zcc-reject), $k FAIL"
if [ "$k" -eq 0 ]; then
    echo "TORTURE PASS (0 FAIL — mọi non-pass đều NOT-IMPL nêu tên)"; exit 0
fi
echo "TORTURE ĐỎ ($k FAIL cần triage → reject sạch hoặc fix)"; exit 1
