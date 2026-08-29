#!/bin/sh
# bzip2.sh — bzip2 1.0.8, compress and decompress as TWO ARMS on the clock.
#
# WHY THIS ONE, AND WHY IT IS NOT "ANOTHER COMPRESSOR". zlib is already on the
# surface, and the two share nothing that matters here. zlib's matcher walks a
# hash chain of BOUNDED depth over a 32 KB window; bzip2 sorts every rotation of
# a 900 KB block. The two directions are two different programs and are reported
# separately for that reason:
#
#   COMPRESS — `blocksort.c`: a suffix sort (Bentley-Sedgewick ternary quicksort
#     over a radix pre-pass, with a Sadakane-style doubling fallback for
#     repetitive input) followed by MTF and a multi-table Huffman coder. The
#     branches are data-dependent comparison outcomes, so they are genuinely
#     unpredictable — unlike sqlite's cold branches or lua's dispatch table —
#     and the working set is far past L2.
#   DECOMPRESS — `decompress.c`'s inverse BWT: build a 900 K-entry permutation,
#     then chase it, one DEPENDENT load per output byte with no locality. A pure
#     latency workload where compress is a throughput one. Law 3c judges those by
#     different models, so a geomean over the two would hide the finding.
#
# A NOTE ON MEASURING THE DECOMPRESS ARM. It is the most drift-prone workload on
# this surface: a pointer chase through a permutation larger than the TLB reach
# swings by a factor of two between single readings on an OTHERWISE IDLE box —
# gcc's own time was seen at 604 ms and 1143 ms in consecutive samples. That is
# the workload, not the machine and not the compiler. It is why this module takes
# `ROUNDS` interleaved pairs of `N` repetitions and keeps the minimum per side,
# and why a single reading of this arm means nothing.
#
# CLEAN INPUT, and the work-pinning quantity. The driver prints the raw size, the
# COMPRESSED LENGTH and an FNV checksum of the decompressed bytes, and the two
# builds' lines are compared before any time is printed. The compressed length is
# the sharp end: it pins every decision the block sorter and the Huffman coder
# made. Two builds agreeing only on the final bytes could have sorted differently
# and be timed on different work.
#
# THE UPSTREAM ORACLE runs too, and it is stronger than anything this harness
# would invent: bzip2 ships `sample1..3.ref` with their `.bz2` counterparts, so
# each build must reproduce the distributed compressed file BYTE FOR BYTE and
# decompress it back to the reference. That is upstream's own answer to "did this
# build work".
#
# THE REFEREE IS `gcc -O2` (MEASURED M48). Bench is -O2 only from 2026-08-29.
#
# Run on the AWS Graviton box, natively, from the repo root there:
#   BZIP2_DIR=/suites/_fetch/bzip2-1.0.8 ZCC_WORK=$PWD \
#     ZCC=$PWD/target/release/zcc sh tests/bench/real/bzip2.sh
#
# Fetch once into the suite cache:
#   cd /suites/_fetch && curl -sSLO https://sourceware.org/pub/bzip2/bzip2-1.0.8.tar.gz \
#     && tar xzf bzip2-1.0.8.tar.gz
# `bench_bzip2.c` lives in /suites/_fetch/.
set -u
BZ="${BZIP2_DIR:-/suites/_fetch/bzip2-1.0.8}"
DRV=""   # set below, once $W is known
W="${ZCC_WORK:-/work/zcc}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
GCCO="${GCC_OPT:--O2}"
N="${N:-3}"
ROUNDS="${ROUNDS:-5}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
# THE DRIVER IS THIS PROJECT'S OWN CODE AND LIVES IN THE REPO, not in the suite
# cache, so a measurement is reproducible at a commit.
DRV="${BZIP2_DRIVER:-$W/tests/bench/real/bench_bzip2.c}"

LIB="blocksort.c huffman.c crctable.c randtable.c compress.c decompress.c bzlib.c"
[ -f "$BZ/blocksort.c" ] || { echo "no $BZ/blocksort.c — see the fetch command in this file's header"; exit 2; }
[ -f "$DRV" ]            || { echo "no $DRV — the bench driver is missing"; exit 2; }

srcs=""; for f in $LIB; do srcs="$srcs $BZ/$f"; done
D="-w -I$BZ"

echo "############################################################"
echo "# REAL PROGRAM — bzip2 1.0.8 block sort + inverse BWT       #"
echo "############################################################"
echo

echo "== BUILD =="
# shellcheck disable=SC2086
$GCC $GCCO $D -o "$T/bb_gcc" "$DRV" $srcs 2>"$T/gerr" || {
    echo "gcc $GCCO BUILD FAILED"; head -20 "$T/gerr"; exit 2; }
# shellcheck disable=SC2086
$ZCC $D -o "$T/bb_zcc" "$DRV" $srcs 2>"$T/zerr" || {
    echo "zcc BUILD FAILED — this is a FINDING, not a harness error:"
    head -30 "$T/zerr"; exit 1; }
# and the CLI, for the upstream oracle below
# shellcheck disable=SC2086
$GCC $GCCO $D -o "$T/bz_gcc" $srcs "$BZ/bzip2.c" 2>/dev/null || exit 2
# shellcheck disable=SC2086
$ZCC $D -o "$T/bz_zcc" $srcs "$BZ/bzip2.c" 2>/dev/null || {
    echo "zcc CLI BUILD FAILED — a FINDING"; exit 1; }
printf "%-10s %12s\n" compiler bytes
printf "%-10s %12s\n" "gcc $GCCO" "$(wc -c < "$T/bb_gcc")"
printf "%-10s %12s\n" "zcc"       "$(wc -c < "$T/bb_zcc")"
echo

# ------------------------------------------- upstream's own oracle, untimed
echo "== UPSTREAM ORACLE (bzip2's distributed sample1..3, byte for byte) =="
fail=0
for s in 1 2 3; do
    [ -f "$BZ/sample$s.ref" ] || { echo "sample$s.ref missing — skipped"; continue; }
    lvl=$(awk "BEGIN{print ($s==1)?\"-1\":(($s==2)?\"-2\":\"-3\")}")
    for c in gcc zcc; do
        "$T/bz_$c" $lvl -c "$BZ/sample$s.ref" > "$T/s$s.$c.bz2" 2>/dev/null
        "$T/bz_$c" -d -c "$BZ/sample$s.bz2"   > "$T/s$s.$c.out" 2>/dev/null
        cmp -s "$T/s$s.$c.bz2" "$BZ/sample$s.bz2" || { echo "sample$s $c: COMPRESS != distributed .bz2"; fail=1; }
        cmp -s "$T/s$s.$c.out" "$BZ/sample$s.ref" || { echo "sample$s $c: DECOMPRESS != .ref"; fail=1; }
    done
done
[ "$fail" = 0 ] && echo "all samples reproduce upstream's bytes on BOTH builds." || exit 1
echo

echo "== CLEAN INPUT (raw · COMPRESSED LENGTH · FNV of decompressed) =="
"$T/bb_gcc" > "$T/go" 2>&1 || { echo "gcc driver nonzero"; cat "$T/go"; exit 1; }
"$T/bb_zcc" > "$T/zo" 2>&1 || { echo "zcc driver nonzero"; cat "$T/zo"; exit 1; }
echo "gcc: $(cat "$T/go")"
echo "zcc: $(cat "$T/zo")"
cmp -s "$T/go" "$T/zo" || { echo "DIVERGE"; diff "$T/go" "$T/zo"; exit 1; }
echo "IDENTICAL — timing may proceed."
echo

TIMEIT="$T/timeit"
$GCC -O2 -w -o "$TIMEIT" "$W/tests/bench/timeit.c" 2>/dev/null || { echo "cannot build timeit.c"; exit 2; }
FLOOR=$("$TIMEIT" 20 /bin/true | awk '{print $2}')
echo "== EXEC per ARM (best of ${ROUNDS}x${N}, INTERLEAVED; floor ${FLOOR}us subtracted) =="
tmin() { "$TIMEIT" "$N" "$1" "$2" | awk -v f="$FLOOR" '{ d=$2-f; print (d>0?d:0) }'; }
printf "%-12s %12s %12s %8s\n" arm gcc_us zcc_us ratio
prod=1; narm=0
for arm in compress decompress; do
    gbest=; zbest=; r=1
    while [ "$r" -le "$ROUNDS" ]; do
        gm=$(tmin "$T/bb_gcc" "$arm")
        zm=$(tmin "$T/bb_zcc" "$arm")
        { [ -z "$gbest" ] || [ "$gm" -lt "$gbest" ]; } && gbest=$gm
        { [ -z "$zbest" ] || [ "$zm" -lt "$zbest" ]; } && zbest=$zm
        r=$((r+1))
    done
    ratio=$(awk "BEGIN{printf \"%.3f\", $zbest/$gbest}")
    printf "%-12s %12s %12s %8s\n" "$arm" "$gbest" "$zbest" "$ratio"
    prod=$(awk "BEGIN{print $prod*$ratio}"); narm=$((narm+1))
done
echo "---"
EXEC_R=$(awk "BEGIN{printf \"%.3f\", $prod^(1.0/$narm)}")
printf "%-12s %12s %12s %8s\n" GEOMEAN - - "$EXEC_R"
echo

echo "== INSN (static, from -S, BZIP2 LIBRARY UNITS ONLY — driver excluded) =="
# `grep -c` PRINTS 0 and EXITS 1 when it matches nothing, so `|| echo 0` appends
# a SECOND line and the caller's `$((tot+gi))` dies with "Illegal number: 0 0".
# bzip2's `crctable.c` and `randtable.c` are pure data and have no instructions
# at all, which is the case that found it. Take stdout, default it if empty.
insns() { c=$(grep -cE '^[[:space:]]+[a-z]' "$1" 2>/dev/null); echo "${c:-0}"; }
printf "%-14s %9s %9s %8s\n" unit gcc zcc ratio
tot_g=0; tot_z=0
for f in $LIB; do
    b=$(basename "$f" .c)
    # shellcheck disable=SC2086
    $GCC $GCCO -S $D -o "$T/g.s" "$BZ/$f" 2>/dev/null || continue
    # shellcheck disable=SC2086
    $ZCC -S $D -o "$T/z.s" "$BZ/$f" 2>/dev/null || {
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
echo "SUMMARY  exec ${EXEC_R}x (see the ARM rows, not this)  ·  insn ${INSN_R}x  ·  upstream samples reproduce  ·  0 DIVERGE"
echo "READ THE ARMS. compress is throughput over unpredictable branches;"
echo "decompress is one dependent load per byte. A compiler can be at parity on"
echo "one and not the other, and that disagreement is the finding."
