#!/bin/sh
# abpair.sh — INTERLEAVED A/B of two zcc builds on one real program, one session.
#
# WHY THIS EXISTS, and why `realprog.sh` cannot answer the question it answers.
# `realprog.sh` reports zcc AGAINST GCC. To judge a candidate row you then need
# two of its runs and a subtraction — and the two runs are two box sessions. On
# 2026-08-28 that subtraction said a row cost 1.1298 -> 1.1590 while the GCC
# side, which cannot have changed, moved 7.6% between the same two sessions. The
# box drifts by more than any row this campaign will ever be worth.
#
# So the referee is dropped and the two CANDIDATES are compared directly, in one
# process tree, alternating A and B on every repetition so a drift that happens
# mid-run lands on both sides equally. The output is a ratio B/A: below 1.0 means
# B is faster. It says nothing about parity with gcc — `realprog.sh` is still the
# instrument for that — and it is not a correctness gate: each phase's stdout is
# compared between the two builds and a difference stops the phase.
#
# Run INSIDE the box, with the ELF built RELEASE (the §CP finding — a debug zcc
# measures rustc's debug profile, not zcc):
#
#   ZCC_REL=1 sh tests/box.sh s 'A= B=ZCC_CSBIAS=1 sh /work/zcc/tests/bench/abpair.sh'
#
# A and B are the ENVIRONMENT each side compiles under, as `VAR=value` words
# (empty = the shipped defaults). Both sides run the same binary of zcc, so the
# row under test must be reachable from an environment variable; a row that is
# already the default is measured by putting its OFF switch in A.
set -u
SQ="${SQLITE_DIR:-/suites/sqlite}"
W="${ZCC_WORK:-/work/zcc}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
N="${N:-5}"
A="${A:-}"
B="${B:-}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

[ -f "$SQ/sqlite3.c" ] || { echo "no $SQ/sqlite3.c"; exit 2; }
# The instrument is built by the REFEREE, never by the compiler under test.
$GCC -O2 -w -o "$T/maxrss" "$W/tests/bench/maxrss.c" || { echo "maxrss build failed"; exit 2; }
M="$T/maxrss"
meas() { out=$1; shift; "$M" "$@" >"$out" 2>>"$T/err" 3>"$T/m"; cat "$T/m"; }

CFLAGS="-w -DSQLITE_THREADSAFE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_DISABLE_LFS"
echo "A env: ${A:-<default>}"
echo "B env: ${B:-<default>}"
# shellcheck disable=SC2086
env $A "$ZCC" $CFLAGS -I"$SQ" -o "$T/cli_a" "$SQ/shell.c" "$SQ/sqlite3.c" -lm \
    || { echo "A build failed"; tail -5 "$T/err"; exit 1; }
# shellcheck disable=SC2086
env $B "$ZCC" $CFLAGS -I"$SQ" -o "$T/cli_b" "$SQ/shell.c" "$SQ/sqlite3.c" -lm \
    || { echo "B build failed"; tail -5 "$T/err"; exit 1; }
# Two builds that hash the same were compiled with the same options, whatever the
# environment said — a ratio taken from them measures the box and nothing else.
ha=$(md5sum "$T/cli_a" | cut -d' ' -f1); hb=$(md5sum "$T/cli_b" | cut -d' ' -f1)
echo "A md5 $ha  ($(wc -c < "$T/cli_a") bytes)"
echo "B md5 $hb  ($(wc -c < "$T/cli_b") bytes)"
[ "$ha" = "$hb" ] && { echo "abpair: A and B are the SAME BINARY — nothing to compare"; exit 3; }
echo

printf "%-14s %10s %10s %8s\n" phase A_us B_us B/A
rm -f "$T/a.db" "$T/b.db"
: > "$T/r"
diverge=0
for f in "$W"/tests/bench/sql/*.sql; do
    p=$(basename "$f" .sql)
    # correctness FIRST, on a scratch pair of databases carried across phases
    ar=$(meas "$T/ao" "$T/cli_a" "$T/a.db" -init /dev/null ".read $f")
    br=$(meas "$T/bo" "$T/cli_b" "$T/b.db" -init /dev/null ".read $f")
    if ! cmp -s "$T/ao" "$T/bo"; then
        printf "%-14s %s\n" "$p" "DIVERGE"
        diff "$T/ao" "$T/bo" | head -6
        diverge=$((diverge+1))
        continue
    fi
    am=${ar%% *}; bm=${br%% *}
    # ALTERNATE, so a drift inside the loop is paid by both sides
    i=1
    while [ "$i" -lt "$N" ]; do
        x=$(meas /dev/null "$T/cli_a" "$T/a.db" -init /dev/null ".read $f"); xm=${x%% *}
        y=$(meas /dev/null "$T/cli_b" "$T/b.db" -init /dev/null ".read $f"); ym=${y%% *}
        [ "$xm" -lt "$am" ] && am=$xm
        [ "$ym" -lt "$bm" ] && bm=$ym
        i=$((i+1))
    done
    r=$(awk "BEGIN{ if($am>0) printf \"%.4f\", $bm/$am; else print \"1.0000\" }")
    printf "%-14s %10s %10s %8s\n" "$p" "$am" "$bm" "$r"
    echo "$r" >> "$T/r"
done
echo "---"
awk '{s+=log($1); n++} END{ if(n) printf "B/A GEOMEAN over %d phases: %.4f  (<1 = B faster)\n", n, exp(s/n) }' "$T/r"
[ "$diverge" -gt 0 ] && { echo "abpair: $diverge DIVERGE — the ratio above is not a measurement"; exit 1; }
exit 0
