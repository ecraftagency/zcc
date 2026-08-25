#!/bin/sh
# realprog.sh — REARCH.md §19: a REAL PROGRAM on the clock, both axes, both sides.
#
# WHY THIS EXISTS. The 35-program taxonomy suite times 18 of its 35 programs and
# is structurally blind to three things: instruction-cache pressure (every kernel
# fits in L1i, so zcc's instruction excess costs zero cycles there), register
# pressure (§13n: no function in the suite spills, so the allocator rows — over
# half the measured size gap — cannot move any exec number the project has ever
# taken), and working sets past L2. sqlite was compiled and COUNTED, never run.
#
# WHAT IS MEASURED, on both sides of the `bl`, per the spine recorded in §19:
#   compile — wall time, PEAK RSS of the compiler, size of the binary produced
#   runtime — wall time and PEAK RSS per workload phase
# Peak RSS is not decoration: a compiler that reaches parity on time by spending
# unbounded memory has not reached parity, and neither has a program.
#
# CLEAN-INPUT LAW. Every phase's stdout is compared between the two builds
# BEFORE any timing is reported. A divergence is printed and the run stops; a
# number taken from a binary that computes something else is not a measurement.
#
# THE COMPILER MUST BE THE RELEASE BUILD. A debug zcc is ~9x slower to compile
# and holds a debug allocator's memory, so timing it measures rustc's debug
# profile rather than zcc (the §CP finding). `box.sh ZCC_REL=1` builds it; this
# script refuses to guess.
#
# Run INSIDE the box:
#   ZCC_REL=1 sh tests/box.sh s 'sh /work/zcc/tests/bench/realprog.sh'
set -u
SQ="${SQLITE_DIR:-/suites/sqlite}"
W="${ZCC_WORK:-/work/zcc}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
N="${N:-3}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

[ -f "$SQ/sqlite3.c" ] || { echo "no $SQ/sqlite3.c"; exit 2; }
[ -f "$SQ/shell.c" ]   || { echo "no $SQ/shell.c";   exit 2; }

# The instrument is built by the REFEREE, never by the compiler under test.
$GCC -O2 -w -o "$T/maxrss" "$W/tests/bench/maxrss.c" || { echo "maxrss build failed"; exit 2; }
M="$T/maxrss"

# `<ms> <peak_kb> <rc>` on fd 3; the child's own output is left alone.
meas() { out=$1; shift; "$M" "$@" >"$out" 2>>"$T/err" 3>"$T/m"; cat "$T/m"; }

CFLAGS="-w -DSQLITE_THREADSAFE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_DISABLE_LFS"

echo "############################################################"
echo "# §19 REAL PROGRAM — sqlite CLI, built and RUN by both      #"
echo "############################################################"
echo
echo "== BUILD (compile wall · compiler peak RSS · binary bytes) =="
printf "%-10s %10s %12s %12s\n" compiler wall_ms peak_rss_kb bytes

build() { # name  cc-command...
    nm=$1; shift
    best_ms=; best_kb=
    i=0
    while [ "$i" -lt "$N" ]; do
        set -- "$@"
        r=$(meas /dev/null "$@") || { echo "$nm: COMPILE FAILED"; tail -5 "$T/err"; exit 1; }
        ms=$(echo "$r" | awk '{print $1}'); kb=$(echo "$r" | awk '{print $2}')
        { [ -z "$best_ms" ] || [ "$ms" -lt "$best_ms" ]; } && best_ms=$ms
        { [ -z "$best_kb" ] || [ "$kb" -gt "$best_kb" ]; } && best_kb=$kb
        i=$((i+1))
    done
    echo "$best_ms $best_kb"
}

r=$(build gcc $GCC -O1 $CFLAGS -I"$SQ" -o "$T/cli_gcc" "$SQ/shell.c" "$SQ/sqlite3.c" -lm)
g_ms=${r% *}; g_kb=${r#* }; g_sz=$(wc -c < "$T/cli_gcc")
printf "%-10s %10s %12s %12s\n" "gcc -O1" "$g_ms" "$g_kb" "$g_sz"

r=$(build zcc $ZCC $CFLAGS -I"$SQ" -o "$T/cli_zcc" "$SQ/shell.c" "$SQ/sqlite3.c" -lm)
z_ms=${r% *}; z_kb=${r#* }; z_sz=$(wc -c < "$T/cli_zcc")
printf "%-10s %10s %12s %12s\n" "zcc" "$z_ms" "$z_kb" "$z_sz"
printf "%-10s %10s %12s %12s\n" "RATIO" \
  "$(awk "BEGIN{printf \"%.3f\", $z_ms/$g_ms}")" \
  "$(awk "BEGIN{printf \"%.3f\", $z_kb/$g_kb}")" \
  "$(awk "BEGIN{printf \"%.3f\", $z_sz/$g_sz}")"
echo

echo "== RUN (per phase: wall · peak RSS; output differentially checked first) =="
printf "%-14s %9s %9s %7s %11s %11s %7s\n" phase gcc_ms zcc_ms t_ratio gcc_rss_kb zcc_rss_kb r_ratio

SQLDIR="$W/tests/bench/sql"
rm -f "$T/g.db" "$T/z.db"
tot_g=0; tot_z=0; peak_g=0; peak_z=0; diverge=0
for f in "$SQLDIR"/*.sql; do
    p=$(basename "$f" .sql)
    # correctness FIRST, on a scratch pair of databases carried across phases
    gr=$(meas "$T/go" "$T/cli_gcc" "$T/g.db" -init /dev/null ".read $f")
    zr=$(meas "$T/zo" "$T/cli_zcc" "$T/z.db" -init /dev/null ".read $f")
    if ! cmp -s "$T/go" "$T/zo"; then
        printf "%-14s %s\n" "$p" "DIVERGE"
        diff "$T/go" "$T/zo" | head -6
        diverge=$((diverge+1))
        continue
    fi
    gm=${gr%% *}; rest=${gr#* }; gk=${rest%% *}
    zm=${zr%% *}; rest=${zr#* }; zk=${rest%% *}
    # best-of-N on the timing only; the databases are already in their post state
    i=1
    while [ "$i" -lt "$N" ]; do
        a=$(meas /dev/null "$T/cli_gcc" "$T/g.db" -init /dev/null ".read $f"); am=${a%% *}
        b=$(meas /dev/null "$T/cli_zcc" "$T/z.db" -init /dev/null ".read $f"); bm=${b%% *}
        [ "$am" -lt "$gm" ] && gm=$am
        [ "$bm" -lt "$zm" ] && zm=$bm
        i=$((i+1))
    done
    tot_g=$((tot_g+gm)); tot_z=$((tot_z+zm))
    [ "$gk" -gt "$peak_g" ] && peak_g=$gk
    [ "$zk" -gt "$peak_z" ] && peak_z=$zk
    tr=$(awk "BEGIN{ if($gm>0) printf \"%.3f\", $zm/$gm; else print \"-\" }")
    rr=$(awk "BEGIN{ if($gk>0) printf \"%.3f\", $zk/$gk; else print \"-\" }")
    printf "%-14s %9s %9s %7s %11s %11s %7s\n" "$p" "$gm" "$zm" "$tr" "$gk" "$zk" "$rr"
done

echo "---"
printf "%-14s %9s %9s %7s %11s %11s %7s\n" TOTAL "$tot_g" "$tot_z" \
  "$(awk "BEGIN{ if($tot_g>0) printf \"%.3f\", $tot_z/$tot_g; else print \"-\" }")" \
  "$peak_g" "$peak_z" \
  "$(awk "BEGIN{ if($peak_g>0) printf \"%.3f\", $peak_z/$peak_g; else print \"-\" }")"
echo
echo "PARITY on this axis = build ratios ≈1.0 AND total run ratio ≈1.0 AND peak RSS ≈1.0,"
echo "with 0 DIVERGE. Unlike the taxonomy suite this program is large enough to"
echo "feel instruction-cache pressure and to make the allocator spill."
[ "$diverge" = 0 ] || { echo "REALPROG RED ($diverge phases DIVERGE)"; exit 1; }
echo "REALPROG OK (0 DIVERGE)"
