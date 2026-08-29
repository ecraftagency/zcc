#!/bin/sh
# libpng.sh — libpng 1.6.43 over zlib: an encode/decode round trip on the clock.
#
# WHY THIS ONE. The measured surface had sqlite (branchy, cold, integer), lua (a
# dispatch loop with FP and GC) and zlib (one hot loop over a sliding window).
# What none of them has is a BYTE-ORIENTED PIPELINE: libpng's row filters are
# five predictors (sub, up, average, paeth) run over every scanline, each a short
# loop of byte loads, adds and absolute differences, and the encoder runs all of
# them and picks per row. That is unsigned-char arithmetic with narrow types and
# constant strides — the shape `t3_base64` and `h2_revbits` sample at kernel
# scale and nothing samples at program scale. The deflate underneath is zlib's,
# so this program is libpng's filters and Huffman driving zlib's matcher, which
# is a different mix from zlib's own workload.
#
# TWO `-D`s, FORCED ON BOTH SIDES, and neither favours either compiler:
#   -DPNG_ARM_NEON_OPT=0 — libpng compiles `arm/filter_neon_intrinsics.c` and
#     `arm/palette_neon_intrinsics.c` by default on aarch64, and those are ARM
#     NEON INTRINSICS, which zcc does not implement. Left alone the two builds
#     would not be the same program — gcc would run hand-vectorized filters and
#     zcc could not build at all. Forcing 0 gives both the C filters, which is
#     also the code this benchmark exists to measure. A compiler is not being
#     asked to match hand-written SIMD here; it is being asked to compile C.
#   -DHAVE_UNISTD_H — what zlib's configure would have written into zconf.h; a
#     Side-II fact about the platform, not a knob. See `zlib.sh`.
#
# CLEAN-INPUT LAW, and the sharp end of it. The driver prints the raw size, the
# ENCODED size and an FNV checksum of the decoded pixels. The encoded size is the
# one that matters: it is a statement about every filter the encoder chose and
# every Huffman tree it built, so two builds that agree on it agree about the
# whole pipeline rather than merely about the final image. A divergence stops the
# run before any time is printed.
#
# WHY A DRIVER AND NOT `pngtest`. libpng ships `pngtest`, which reads a file,
# writes it back and compares — a genuine oracle, and it is run in the
# CORRECTNESS half of this script because that is what it is good for. It is not
# a benchmark: it runs for a few milliseconds and its clock is a filesystem's.
# The timed workload is `bench_png.c`, in memory, long enough to measure.
#
# THE REFEREE IS `gcc -O2` (MEASURED M48); `GCC_OPT=-O1` restores the old column,
# and a number taken at one level does not transfer to the other.
#
# THE COMPILER MUST BE THE RELEASE BUILD (the §CP finding: a debug zcc is ~9x
# slower to compile and holds a debug allocator's memory).
#
# Run on the AWS Graviton box, natively, from the repo root there:
#   PNG_DIR=/suites/libpng ZLIB_DIR=/suites/zlib ZCC_WORK=$PWD \
#     ZCC=$PWD/target/release/zcc sh tests/bench/real/libpng.sh
#
# The source tree is NOT in the repo. Fetch it once into the suite cache:
#   mkdir -p /suites/libpng && cd /suites/libpng \
#     && curl -sSLO https://download.sourceforge.net/libpng/libpng-1.6.43.tar.gz \
#     && tar xzf libpng-1.6.43.tar.gz
# `bench_png.c` lives beside it in /suites/libpng/.
set -u
PNG="${PNG_DIR:-/suites/libpng}"
SRC="$PNG/libpng-1.6.43"
ZL="${ZLIB_DIR:-/suites/zlib}"
W="${ZCC_WORK:-/work/zcc}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
GCCO="${GCC_OPT:--O2}"   # MEASURED M48 — the referee level; see the header
N="${N:-3}"              # timeit reps inside one round
ROUNDS="${ROUNDS:-5}"    # interleaved rounds; the box's drift is why this is not 1
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

PSRC="png.c pngerror.c pngget.c pngmem.c pngpread.c pngread.c pngrio.c
      pngrtran.c pngrutil.c pngset.c pngtrans.c pngwio.c pngwrite.c
      pngwtran.c pngwutil.c"
ZSRC="adler32.c compress.c crc32.c deflate.c infback.c inffast.c inflate.c
      inftrees.c trees.c uncompr.c zutil.c"

[ -f "$SRC/png.c" ]      || { echo "no $SRC/png.c — see the fetch command in this file's header"; exit 2; }
[ -f "$ZL/deflate.c" ]   || { echo "no $ZL/deflate.c — zlib is a dependency, unpack it there"; exit 2; }
# THE DRIVER IS THIS PROJECT'S OWN CODE AND LIVES IN THE REPO, not in the suite
# cache. `bench_zlib.c` set the other precedent and it was the wrong one: a
# driver that is not version-controlled makes a measurement unreproducible at a
# commit. `PNG_DRIVER` overrides for an experiment.
DRV="${PNG_DRIVER:-$W/tests/bench/real/bench_png.c}"
[ -f "$DRV" ] || { echo "no $DRV — the bench driver is missing"; exit 2; }

# `pnglibconf.h` is what libpng's configure would generate. The tree ships the
# prebuilt one for exactly this case; it is a Side-II input, not a choice.
cp "$SRC/scripts/pnglibconf.h.prebuilt" "$T/pnglibconf.h" || exit 2

D="-w -DPNG_ARM_NEON_OPT=0 -DHAVE_UNISTD_H -I$T -I$SRC -I$ZL"
psrcs=""; for f in $PSRC; do psrcs="$psrcs $SRC/$f"; done
zsrcs=""; for f in $ZSRC; do zsrcs="$zsrcs $ZL/$f"; done

echo "############################################################"
echo "# REAL PROGRAM — libpng 1.6.43 filters + zlib deflate       #"
echo "############################################################"
echo

# ------------------------------------------------------------------- build
echo "== BUILD =="
# shellcheck disable=SC2086
$GCC $GCCO $D -o "$T/bp_gcc" "$DRV" $psrcs $zsrcs -lm 2>"$T/gerr" || {
    echo "gcc $GCCO BUILD FAILED"; head -20 "$T/gerr"; exit 2; }
# shellcheck disable=SC2086
$ZCC $D -o "$T/bp_zcc" "$DRV" $psrcs $zsrcs 2>"$T/zerr" || {
    echo "zcc BUILD FAILED — this is a FINDING, not a harness error:"
    head -30 "$T/zerr"; exit 1; }
g_sz=$(wc -c < "$T/bp_gcc"); z_sz=$(wc -c < "$T/bp_zcc")
# The file byte count compares two LINK MODES as much as two compilers on this
# box; prefer the INSN total below as the code-size statement. See `zlib.sh`.
printf "%-10s %12s\n" compiler bytes
printf "%-10s %12s\n" "gcc $GCCO" "$g_sz"
printf "%-10s %12s\n" "zcc"       "$z_sz"
printf "%-10s %12s\n" "RATIO"     "$(awk "BEGIN{printf \"%.3f\", $z_sz/$g_sz}")"
echo

# ------------------------------------- libpng's own oracle, before any timing
#
# `pngtest` reads a PNG, writes it back and compares the two byte for byte. It is
# libpng's own answer to "did this build work", it is stronger than anything this
# harness would invent, and it costs a few milliseconds — so it is run for
# CORRECTNESS and is not what gets timed.
echo "== PNGTEST (libpng's own read/write/compare oracle) =="
# shellcheck disable=SC2086
$GCC $GCCO $D -o "$T/pt_gcc" $psrcs "$SRC/pngtest.c" $zsrcs -lm 2>/dev/null || {
    echo "gcc pngtest BUILD FAILED"; exit 2; }
# shellcheck disable=SC2086
$ZCC $D -o "$T/pt_zcc" $psrcs "$SRC/pngtest.c" $zsrcs 2>/dev/null || {
    echo "zcc pngtest BUILD FAILED — a FINDING"; exit 1; }
( cd "$T" && "$T/pt_gcc" "$SRC/pngtest.png" > pt_g.out 2>&1 ); grc=$?
( cd "$T" && "$T/pt_zcc" "$SRC/pngtest.png" > pt_z.out 2>&1 ); zrc=$?
echo "gcc: $(grep -c 'PASS' "$T/pt_g.out") PASS lines [exit=$grc]"
echo "zcc: $(grep -c 'PASS' "$T/pt_z.out") PASS lines [exit=$zrc]"
if [ "$grc" != 0 ] || [ "$zrc" != 0 ]; then
    echo "PNGTEST NONZERO EXIT:"; tail -5 "$T/pt_g.out" "$T/pt_z.out"; exit 1
fi
if ! cmp -s "$T/pt_g.out" "$T/pt_z.out"; then
    echo "PNGTEST DIVERGE:"; diff "$T/pt_g.out" "$T/pt_z.out" | head -10; exit 1
fi
grep -q "libpng passes test" "$T/pt_g.out" || { echo "pngtest did not pass"; exit 1; }
echo "IDENTICAL, and libpng passes its own test on both builds."
echo

# --------------------------------------------- clean input for the timed run
echo "== CLEAN INPUT (raw size · ENCODED size · FNV of decoded pixels) =="
"$T/bp_gcc" > "$T/go" 2>&1; grc=$?
"$T/bp_zcc" > "$T/zo" 2>&1; zrc=$?
echo "gcc: $(cat "$T/go") [exit=$grc]"
echo "zcc: $(cat "$T/zo") [exit=$zrc]"
if [ "$grc" != 0 ] || [ "$zrc" != 0 ]; then echo "NONZERO EXIT"; exit 1; fi
if ! cmp -s "$T/go" "$T/zo"; then
    echo "DIVERGE — the two builds do not encode the same bytes the same way."
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
    gm=$(tmin "$T/bp_gcc")
    zm=$(tmin "$T/bp_zcc")
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

# ------------------------------------------- static instruction counts
# Deterministic, zero measurement noise — the size half of the pair. LIBPNG
# UNITS ONLY: zlib has its own module and counting it here would report the same
# code twice, and the driver is this project's own and would measure the harness.
echo "== INSN (static, from -S, LIBPNG UNITS ONLY — zlib and driver excluded) =="
# `grep -c` PRINTS 0 and EXITS 1 when it matches nothing, so `|| echo 0` appends
# a SECOND line and the caller's `$((tot+gi))` dies with "Illegal number: 0 0".
# bzip2's `crctable.c` and `randtable.c` are pure data and have no instructions
# at all, which is the case that found it. Take stdout, default it if empty.
insns() { c=$(grep -cE '^[[:space:]]+[a-z]' "$1" 2>/dev/null); echo "${c:-0}"; }
printf "%-14s %9s %9s %8s\n" unit gcc zcc ratio
tot_g=0; tot_z=0
for f in $PSRC; do
    b=$(basename "$f" .c)
    # shellcheck disable=SC2086
    $GCC $GCCO -S $D -o "$T/g.s" "$SRC/$f" 2>/dev/null || continue
    # shellcheck disable=SC2086
    $ZCC -S $D -o "$T/z.s" "$SRC/$f" 2>/dev/null || {
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

echo "SUMMARY  exec ${EXEC_R}x  ·  insn ${INSN_R}x  ·  pngtest PASS both  ·  0 DIVERGE"
echo "Law 3c's margin applies to the claim, not to the number: this is one"
echo "program on one core at one image, and it is evidence about libpng first."
