#!/bin/sh
# zlib.sh — PLAN.md, second real program: zlib 1.3.1 on the clock.
#
# WHY A SECOND ONE. sqlite is the only member of the measured surface with real
# register pressure, and Law 3c's note on what may be claimed names the gaps in
# that surface by hand: no deep call graphs, one input size, one texture of
# work. zlib closes a different one than sqlite does. It is a small library —
# sixteen translation units, no configure needed for a static build — whose hot
# path is two tight loops over a 32 KB sliding window: `longest_match` in
# deflate.c and `inflate_fast` in inffast.c. Those loops carry pointer-chasing
# recurrences through a hash chain, which is exactly the shape Law 3c says an
# instruction count cannot see. sqlite is branchy and cold; zlib is a hot loop
# with a working set that does not fit in L1d. Neither stands in for the other.
#
# CLEAN-INPUT LAW. The driver prints one line — input size, adler32 of the
# input, the compressed length, and an independent FNV checksum of the
# decompressed bytes — and the two builds' lines are compared BEFORE any time is
# reported. The compressed length is the sharp end of that comparison: it is a
# statement about every decision the match finder made, so two builds that agree
# on it agree about the hash chains and the tree construction, not merely about
# the final answer. A divergence stops the run. A number taken from a binary
# that computes something else is not a measurement.
#
# THE INTERLEAVE. This box drifts about 5% run to run, so a single reading of
# each side is worth nothing: whichever side happened to be sampled during a
# quiet stretch wins. Both builds are therefore timed in EVERY round, adjacent
# in time, and the minimum is taken across all rounds — the same discipline the
# R5 measurement note records after one reading dismissed the scheduler and
# promoted a hoist that was a regression. `timeit` reports its own fork+exec
# floor and that floor is subtracted from both sides before the ratio.
#
# WHAT IS AND IS NOT COUNTED. The instruction count covers the ZLIB sources
# only; the driver is excluded, because it is this project's own code and would
# be measuring the harness. It covers all sixteen units including the gz*
# file-I/O ones, which the workload never calls: they are compiled, so they are
# a fair statement about zlib's static size, but they contribute nothing to the
# exec ratio and should not be read as if they did.
#
# Run INSIDE the box, from the repo root on the host:
#   ZCC_REL=1 sh tests/box.sh s 'sh /work/zcc/tests/bench/real/zlib.sh'
set -u
ZL="${ZLIB_DIR:-/suites/zlib}"
W="${ZCC_WORK:-/work/zcc}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
N="${N:-5}"            # timeit reps inside one round
ROUNDS="${ROUNDS:-5}"  # interleaved rounds; the drift is why this is not 1
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

# HAVE_UNISTD_H is what zlib's configure would have written into zconf.h. It is
# a Side-II fact about the platform, not a tuning knob: without it the gz*
# units call read/write/lseek with no declaration in scope, which is a
# constraint violation in C99 and is rejected by both compilers.
CFLAGS="-w -DHAVE_UNISTD_H -I$ZL"
ZSRC="adler32.c compress.c crc32.c deflate.c gzclose.c gzlib.c gzread.c
      gzwrite.c infback.c inffast.c inflate.c inftrees.c trees.c uncompr.c
      zutil.c"
DRIVER="bench_zlib.c"

[ -f "$ZL/deflate.c" ]  || { echo "no $ZL/deflate.c — unpack zlib 1.3.1 there"; exit 2; }
[ -f "$ZL/$DRIVER" ]    || { echo "no $ZL/$DRIVER — the bench driver is missing"; exit 2; }

echo "############################################################"
echo "# §19 REAL PROGRAM — zlib 1.3.1 deflate/inflate round trip  #"
echo "############################################################"
echo

# ---------------------------------------------------------------- build
srcs=""
for f in $DRIVER $ZSRC; do srcs="$srcs $ZL/$f"; done

echo "== BUILD =="
# shellcheck disable=SC2086
$GCC -O1 $CFLAGS -o "$T/bench_gcc" $srcs 2>"$T/gerr" || {
    echo "gcc -O1 BUILD FAILED"; head -20 "$T/gerr"; exit 2; }
# shellcheck disable=SC2086
$ZCC $CFLAGS -o "$T/bench_zcc" $srcs 2>"$T/zerr" || {
    echo "zcc BUILD FAILED — this is a FINDING, not a harness error:"
    head -30 "$T/zerr"; exit 1; }
g_sz=$(wc -c < "$T/bench_gcc"); z_sz=$(wc -c < "$T/bench_zcc")
# READ THIS ROW WITH CARE, AND PREFER THE INSN TOTAL BELOW. gcc links a PIE and
# zcc links a fixed EXEC on this box, and both drag in a different slice of
# static musl, so the file byte count compares two link modes as much as two
# compilers — measured here as gcc text 81,379 / data 1,296 against zcc text
# 63,605 / data 36,344, which is why zcc's file is SMALLER while its zlib code
# is 17% LARGER. The instruction count over the zlib units is the code-size
# statement; this row is context.
printf "%-10s %12s\n" compiler bytes
printf "%-10s %12s\n" "gcc -O1" "$g_sz"
printf "%-10s %12s\n" "zcc"     "$z_sz"
printf "%-10s %12s\n" "RATIO"   "$(awk "BEGIN{printf \"%.3f\", $z_sz/$g_sz}")"
echo

# ------------------------------------------------- clean input, before timing
echo "== CLEAN INPUT (both builds' one output line, compared first) =="
"$T/bench_gcc" > "$T/go" 2>"$T/goe"; grc=$?
"$T/bench_zcc" > "$T/zo" 2>"$T/zoe"; zrc=$?
echo "gcc: $(cat "$T/go") [exit=$grc]"
echo "zcc: $(cat "$T/zo") [exit=$zrc]"
if [ "$grc" != 0 ] || [ "$zrc" != 0 ]; then
    echo "NONZERO EXIT — the round trip failed inside the program:"
    head -5 "$T/goe" "$T/zoe"; exit 1
fi
if ! cmp -s "$T/go" "$T/zo"; then
    echo "DIVERGE — the two builds do not compress the same bytes the same way."
    diff "$T/go" "$T/zo"; exit 1
fi
echo "IDENTICAL — timing may proceed."
echo

# ------------------------------------------------------------------ timing
TIMEIT="$T/timeit"
$GCC -O2 -w -o "$TIMEIT" "$W/tests/bench/timeit.c" 2>/dev/null || {
    echo "cannot build timeit.c"; exit 2; }
FLOOR=$("$TIMEIT" 20 /bin/true | awk '{print $2}')
echo "== EXEC (best of ${ROUNDS}x${N}, INTERLEAVED; fork+exec floor ${FLOOR}us subtracted) =="
tmin() { "$TIMEIT" "$N" "$1" | awk -v f="$FLOOR" '{ d=$2-f; print (d>0?d:0) }'; }

printf "%-8s %12s %12s %8s\n" round gcc_us zcc_us ratio
gbest=; zbest=
r=1
while [ "$r" -le "$ROUNDS" ]; do
    # adjacent in time, gcc then zcc, so a slow stretch of the machine taxes
    # both sides rather than whichever one it happened to land on
    gm=$(tmin "$T/bench_gcc")
    zm=$(tmin "$T/bench_zcc")
    { [ -z "$gbest" ] || [ "$gm" -lt "$gbest" ]; } && gbest=$gm
    { [ -z "$zbest" ] || [ "$zm" -lt "$zbest" ]; } && zbest=$zm
    printf "%-8s %12s %12s %8s\n" "$r" "$gm" "$zm" \
      "$(awk "BEGIN{printf \"%.3f\", $zm/$gm}")"
    r=$((r+1))
done
echo "---"
EXEC_R=$(awk "BEGIN{printf \"%.3f\", $zbest/$gbest}")
printf "%-8s %12s %12s %8s\n" BEST "$gbest" "$zbest" "$EXEC_R"
echo

# ------------------------------------------------- static instruction counts
# Deterministic, zero measurement noise — the size half of the pair. Law 3c is
# the reason both are printed: fewest instructions is not fastest code, and a
# disagreement between these two columns is the finding, not an error.
echo "== INSN (static, from -S, ZLIB SOURCES ONLY — driver excluded) =="
insns() { grep -cE '^[[:space:]]+[a-z]' "$1" 2>/dev/null || echo 0; }
printf "%-14s %9s %9s %8s\n" unit gcc zcc ratio
tot_g=0; tot_z=0
for f in $ZSRC; do
    b=$(basename "$f" .c)
    $GCC -O1 -S $CFLAGS -o "$T/g.s" "$ZL/$f" 2>/dev/null || continue
    $ZCC    -S $CFLAGS -o "$T/z.s" "$ZL/$f" 2>/dev/null || {
        printf "%-14s %9s %9s %8s\n" "$b" - "ZCC -S FAIL" -; continue; }
    gi=$(insns "$T/g.s"); zi=$(insns "$T/z.s")
    tot_g=$((tot_g+gi)); tot_z=$((tot_z+zi))
    printf "%-14s %9s %9s %8s\n" "$b" "$gi" "$zi" \
      "$(awk "BEGIN{ if($gi>0) printf \"%.3f\", $zi/$gi; else print \"-\" }")"
done
echo "---"
INSN_R=$(awk "BEGIN{ if($tot_g>0) printf \"%.3f\", $tot_z/$tot_g; else print \"-\" }")
printf "%-14s %9s %9s %8s\n" TOTAL "$tot_g" "$tot_z" "$INSN_R"
echo

echo "SUMMARY  exec ${EXEC_R}x  ·  insn ${INSN_R}x  ·  0 DIVERGE  (file bytes: see the BUILD caveat)"
echo "PARITY on this program = all three ≈1.0. Law 3c's margin applies to the"
echo "claim, not to the number: this is one program on one core at one input"
echo "size, and it is evidence about zlib first."
