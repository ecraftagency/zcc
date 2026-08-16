#!/bin/sh
# Diff zcc với clang trên từng case: tests/run.sh
# Trọng tài = cc hệ thống (-std=c89 -O0). So exit code; từ M5 so thêm stdout.
cd "$(dirname "$0")" || exit 1
cargo build -q --manifest-path ../Cargo.toml || exit 1
ZCC=../target/debug/zcc
mkdir -p out
pass=0 fail=0
for c in cases/*.c; do
  n=$(basename "$c" .c)
  cc -std=c89 -w -O0 "$c" -o "out/$n.ref" 2>/dev/null || { echo "SKIP $n (cc từ chối case)"; continue; }
  in=/dev/null; [ -f "cases/$n.in" ] && in="cases/$n.in"
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
