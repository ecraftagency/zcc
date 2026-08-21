#!/bin/sh
# bench.sh — EXECUTION-SPEED differential: zcc (SSA opt + Stage-5b regalloc) vs the
# gcc -O0 reference, on compute-bound single-file programs. Run INSIDE the box (musl
# is nearly free; the mac oracle is retired). gcc -O0 is the fair referee: zcc has no
# loop/strength/vectorize passes, so comparing against -O2 would measure absent passes,
# not codegen quality. The honest question is "how close to a naive-but-correct
# compiler's -O0 is zcc's -O0-plus-SSA?".
#
# Clean-input law: a timing is worthless if the two binaries compute DIFFERENT things.
# Each case is CORRECTNESS-GATED first (gcc stdout == zcc stdout) — a mismatch aborts
# the case as MISCOMPILE, never silently timed. Evidence trail: binary bytes printed.
#
# Method: best-of-3 wall-clock (min = least noisy), nanosecond clock. Ratio = zcc/gcc
# (>1 ⟹ zcc slower). Geomean across cases = the single headline number.
#   Usage (in box):  ZCC=/usr/local/bin/zcc sh bench.sh
set -u
export ZCC="${ZCC:-/usr/local/bin/zcc}"
DIR="$(dirname "$0")"
# zcc optimization is DEFAULT-ON (SSA + Stage-5b regalloc). BENCH_O0=1 times the -O0
# baseline instead (ZCC_O0 disables the optimizer) — the A/B "what did opt buy" run.
Z0="${BENCH_O0:+ZCC_O0=1}"; LBL="$([ -n "${BENCH_O0:-}" ] && echo O0 || echo ssa+ra)"

ns() { date +%s%N; }
best3() {                             # $1 = binary → echoes min ns of 3 runs
    m=""
    for _ in 1 2 3; do
        s=$(ns); "$1" >/dev/null 2>&1; e=$(ns); d=$((e - s))
        [ -z "$m" ] || [ "$d" -lt "$m" ] && m=$d
    done
    echo "$m"
}

printf '%-10s %12s %12s %8s %10s %10s\n' CASE gcc_ms zcc_ms ratio gcc_bytes zcc_bytes
prod=1; n=0
for c in "$DIR"/*.c; do
    b=$(basename "$c" .c)
    d=$(mktemp -d)
    gcc -O0 -w "$c" -o "$d/g" 2>/dev/null || { printf '%-10s gcc-compile-FAIL\n' "$b"; rm -rf "$d"; continue; }
    if ! env $Z0 "$ZCC" "$c" -o "$d/z" 2>"$d/ze"; then
        printf '%-10s zcc-compile-FAIL: %s\n' "$b" "$(head -1 "$d/ze")"; rm -rf "$d"; continue; fi
    # CORRECTNESS GATE — identical stdout, else abort this case (miscompile, not a timing)
    "$d/g" >"$d/go" 2>&1; "$d/z" >"$d/zo" 2>&1
    if ! cmp -s "$d/go" "$d/zo"; then
        printf '%-10s DIVERGE gcc=%s zcc=%s\n' "$b" "$(cat "$d/go")" "$(cat "$d/zo")"; rm -rf "$d"; continue; fi
    gb=$(wc -c < "$d/g"); zb=$(wc -c < "$d/z")
    gt=$(best3 "$d/g"); zt=$(best3 "$d/z")
    gms=$((gt / 1000000)); zms=$((zt / 1000000))
    ratio=$(awk "BEGIN{printf \"%.2f\", $zt/$gt}")
    printf '%-10s %12d %12d %8sx %10d %10d\n' "$b" "$gms" "$zms" "$ratio" "$gb" "$zb"
    prod=$(awk "BEGIN{print $prod * $zt/$gt}"); n=$((n + 1))
    rm -rf "$d"
done
[ "$n" -gt 0 ] && awk "BEGIN{printf \"GEOMEAN zcc/gcc-O0 (zcc=$LBL, n=$n): %.2fx\n\", exp(log($prod)/$n)}"
