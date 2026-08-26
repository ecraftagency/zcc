#!/bin/sh
# Diff zcc against clang on each case: tests/run.sh [cases|ext]
# Referee = the system cc at -O0. cases/ (default): -std=c99 — ISO C99 territory
# (C89 is a subset). ext/: -std=gnu99 — vendor dialects (GCC/clang/Apple
# extensions).
cd "$(dirname "$0")" || exit 1
# ZCC preset from the env (running in the Linux box, no cargo) — otherwise self-build
if [ -z "${ZCC:-}" ]; then
  cargo build -q --manifest-path ../Cargo.toml || exit 1
  ZCC=../target/debug/zcc
fi
DIR=${1:-cases}
SEEK="${2:-${SEEK:-}}"   # seek 1 unit: run only cases whose name contains this substring
STD=-std=c99
[ "$DIR" = ext ] && STD=-std=gnu99
# writable out-dir: default ./out; if tests/ is RO (running in a RO-mounted box) -> tmp
OUT=out
mkdir -p "$OUT" 2>/dev/null && [ -w "$OUT" ] || OUT=$(mktemp -d)
pass=0 fail=0
for c in "$DIR"/*.c; do
  n=$(basename "$c" .c)
  [ -n "$SEEK" ] && case "$n" in *"$SEEK"*) ;; *) continue;; esac
  cc "$STD" -w -O0 "$c" -o "$OUT/$n.ref" 2>/dev/null || { echo "SKIP $n (cc rejected the case)"; continue; }
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
# ZERO CASES IS AN ERROR, NEVER A VERDICT. A SEEK that matches nothing printed
# "---- 0 pass, 0 fail" and the gate scored it PASS — which is how `ext` reported
# a clean run three times while testing not one case (`fullsuite.sh all 300`
# passes "300" as SEEK, not as the fuzz count). A suite that was not run must not
# be indistinguishable from a suite that found nothing wrong.
if [ $((pass + fail)) -eq 0 ]; then
  echo "$DIR: SEEK='$SEEK' matched 0 of $(ls "$DIR"/*.c 2>/dev/null | wc -l | tr -d ' ') cases — nothing was tested"
  exit 2
fi
echo "---- $((pass + fail)) cases, $pass pass, $fail fail"
# Gate: FAIL ⊆ known-fail (if <DIR>.known-fail exists); default requires 0 fail.
kf="$DIR.known-fail"
if [ -f "$kf" ]; then
    new=""
    for n in $fails; do grep -qx "$n" "$kf" || new="$new $n"; done
    if [ -n "$new" ]; then echo "NEW FAIL (outside baseline):$new"; exit 1; fi
    echo "OK ($DIR: every fail ⊆ $kf)"; exit 0
fi
[ "$fail" = 0 ]
