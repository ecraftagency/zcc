#!/bin/sh
# Diff zcc với clang trên từng case: tests/run.sh [cases|ext]
# Trọng tài = cc hệ thống -O0. cases/ (mặc định): -std=c99 — lãnh thổ ISO C99
# (chuẩn gốc từ 2026-08-18, C89 là tập con). ext/: -std=gnu99 — phương ngữ
# vendor (GCC/clang/Apple extension).
cd "$(dirname "$0")" || exit 1
# ZCC đặt sẵn từ env (chạy trong box Linux, không cargo) — không thì tự build
if [ -z "${ZCC:-}" ]; then
  cargo build -q --manifest-path ../Cargo.toml || exit 1
  ZCC=../target/debug/zcc
fi
DIR=${1:-cases}
SEEK="${2:-${SEEK:-}}"   # seek 1 unit: chỉ chạy case có tên chứa chuỗi này
STD=-std=c99
[ "$DIR" = ext ] && STD=-std=gnu99
# out-dir ghi được: mặc định ./out; nếu tests/ RO (chạy trong box mount RO) -> tmp
OUT=out
mkdir -p "$OUT" 2>/dev/null && [ -w "$OUT" ] || OUT=$(mktemp -d)
pass=0 fail=0
for c in "$DIR"/*.c; do
  n=$(basename "$c" .c)
  [ -n "$SEEK" ] && case "$n" in *"$SEEK"*) ;; *) continue;; esac
  cc "$STD" -w -O0 "$c" -o "$OUT/$n.ref" 2>/dev/null || { echo "SKIP $n (cc từ chối case)"; continue; }
  in=/dev/null; [ -f "$DIR/$n.in" ] && in="$DIR/$n.in"
  "$OUT/$n.ref" < "$in" > "$OUT/$n.ref.txt"; want=$?
  if "$ZCC" "$c" -o "$OUT/$n.bin"; then
    "$OUT/$n.bin" < "$in" > "$OUT/$n.bin.txt"; got=$?
    if [ "$want" = "$got" ] && cmp -s "$OUT/$n.ref.txt" "$OUT/$n.bin.txt"; then
      pass=$((pass + 1)); echo "PASS $n"
    else
      fail=$((fail + 1)); fails="$fails $n"; echo "FAIL $n (exit want=$want got=$got)"
    fi
  else
    fail=$((fail + 1)); fails="$fails $n"; echo "FAIL $n (compile)"
  fi
done
echo "---- $pass pass, $fail fail"
# Gate: FAIL ⊆ known-fail (nếu có <DIR>.known-fail); mặc định đòi 0 fail.
kf="$DIR.known-fail"
if [ -f "$kf" ]; then
    new=""
    for n in $fails; do grep -qx "$n" "$kf" || new="$new $n"; done
    if [ -n "$new" ]; then echo "FAIL MỚI (ngoài baseline):$new"; exit 1; fi
    echo "OK ($DIR: mọi fail ⊆ $kf)"; exit 0
fi
[ "$fail" = 0 ]
