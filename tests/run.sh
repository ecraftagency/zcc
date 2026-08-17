#!/bin/sh
# Diff zcc với clang trên từng case: tests/run.sh [cases|ext]
# Trọng tài = cc hệ thống -O0. cases/ (mặc định): thêm -std=c89 — lãnh thổ C89
# thuần. ext/: KHÔNG -std — phương ngữ C89+ (C99 cherry-pick + GCC extension).
cd "$(dirname "$0")" || exit 1
cargo build -q --manifest-path ../Cargo.toml || exit 1
ZCC=../target/debug/zcc
DIR=${1:-cases}
STD=-std=c89
[ "$DIR" = ext ] && STD=-std=gnu99
mkdir -p out
pass=0 fail=0
for c in "$DIR"/*.c; do
  n=$(basename "$c" .c)
  cc "$STD" -w -O0 "$c" -o "out/$n.ref" 2>/dev/null || { echo "SKIP $n (cc từ chối case)"; continue; }
  in=/dev/null; [ -f "$DIR/$n.in" ] && in="$DIR/$n.in"
  "./out/$n.ref" < "$in" > "out/$n.ref.txt"; want=$?
  if "$ZCC" "$c" -o "out/$n.bin"; then
    "./out/$n.bin" < "$in" > "out/$n.bin.txt"; got=$?
    if [ "$want" = "$got" ] && cmp -s "out/$n.ref.txt" "out/$n.bin.txt"; then
      pass=$((pass + 1)); echo "PASS $n"
    else
      fail=$((fail + 1)); echo "FAIL $n (exit want=$want got=$got)"
    fi
  else
    fail=$((fail + 1)); echo "FAIL $n (compile)"
  fi
done
echo "---- $pass pass, $fail fail"
[ "$fail" = 0 ]
