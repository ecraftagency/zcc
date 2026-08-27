#!/bin/sh
# localize.sh — WHICH FUNCTIONS CARRY THE GAP, answered by the linker.
#
# THE PROBLEM. sqlite runs 1.65x gcc -O1 and its instruction count is only
# 1.11x, so the gap is cycles rather than instructions — and this box's kernel
# exposes no PMU (`/sys/bus/event_source/devices` has software events only), so
# there is no profiler. Every optimization row shipped against sqlite so far was
# aimed by STATIC instruction counts, which is the one instrument that provably
# cannot see the thing being chased.
#
# THE INSTRUMENT. Compile the same source with both compilers and let the LINKER
# choose per function: take function `f` from gcc and everything else from zcc,
# then time the workload. The difference from the all-zcc build is exactly what
# `f` costs. Bisect, and 1.65x resolves into a list of names.
#
# HOW IT WORKS, and it is only symbol binding:
#   * `-DSQLITE_PRIVATE=` turns sqlite's 1,260 internal functions from `static`
#     into externals, so they HAVE symbols to select by. (The amalgamation
#     guards it with `#ifndef`, so this needs no patch.)
#   * every global in the DONOR object is weakened EXCEPT the chosen ones;
#   * the chosen ones are weakened in the RECIPIENT object;
#   * the recipient is linked first. A strong definition beats a weak one, so
#     the chosen names come from the donor and every other name from the
#     recipient.
#
# VERIFIED on a three-way fixture before use: with two functions differing
# between the builds, the hybrid landed distinct from BOTH pure builds and
# differed from the donor build by exactly the second function's contribution.
# That check matters — an earlier version of this idea "passed" while actually
# measuring nothing, because the toy's callee was small enough for gcc to inline
# and there was no call left to redirect.
#
# ⚠️ WHAT IT CHANGES. `-DSQLITE_PRIVATE=` makes every internal function extern,
# which costs BOTH compilers their static-function inlining. The hybrid is
# therefore a slightly different program from the shipping build, so the
# baselines printed below are taken under the same flag: read the numbers
# against THOSE, never against realprog.sh's.
#
# Run INSIDE the box:
#   sh tests/bench/localize.sh                        # baselines only
#   sh tests/bench/localize.sh f1 [f2 ...]            # ONE hybrid, all of them from gcc
#   sh tests/bench/localize.sh -scan f1 f2 f3 ...     # one hybrid EACH, ranked
#
# The two objects are compiled once and cached under /suites/.localize, keyed to
# the zcc binary's checksum, so the first question costs ~45s and every question
# after it costs a link and a run. `-rebuild` forces a recompile.
# `SQL=` selects the workload (default p01_insert, the worst phase); `N=` the
# number of timed runs.
set -u
SQ="${SQLITE_DIR:-/suites/sqlite}"
W="${ZCC_WORK:-/work/zcc}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
N="${N:-7}"
SQL="${SQL:-$W/tests/bench/sql/p01_insert.sql}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
CF="-w -DSQLITE_PRIVATE= -DSQLITE_THREADSAFE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_DISABLE_LFS"
C="${LOCALIZE_CACHE:-/suites/.localize}"
mkdir -p "$C"

case "${1:-}" in -rebuild) rm -f "$C"/*.o "$C/stamp"; shift ;; esac
SCAN=0
case "${1:-}" in -scan) SCAN=1; shift ;; esac

$GCC -O2 -w -o "$T/timeit" "$W/tests/bench/timeit.c" || exit 2

# The cache is keyed to the COMPILER, not to the source: sqlite3.c does not
# change and gcc does not change, but zcc changes every time a row ships, and a
# stale z.o would attribute the previous build's gap.
STAMP=$(md5sum "$ZCC" | cut -d' ' -f1)
[ -f "$C/stamp" ] && [ "$(cat "$C/stamp")" = "$STAMP" ] || rm -f "$C/z.o"
[ -f "$C/g.o" ] || { echo "compiling gcc side (once)";
  $GCC -O1 $CF -I"$SQ" -c "$SQ/sqlite3.c" -o "$C/g.o" || { echo "gcc failed"; exit 1; }; }
[ -f "$C/sh_g.o" ] || $GCC -O1 $CF -I"$SQ" -c "$SQ/shell.c" -o "$C/sh_g.o" || exit 1
[ -f "$C/z.o" ] || { echo "compiling zcc side (once per zcc build)";
  $ZCC $CF -I"$SQ" -c "$SQ/sqlite3.c" -o "$C/z.o" || { echo "zcc failed"; exit 1; }
  echo "$STAMP" > "$C/stamp"; }
cp "$C/g.o" "$T/g.o"; cp "$C/z.o" "$T/z.o"; cp "$C/sh_g.o" "$T/sh_g.o"

# The workload runs against :memory: — a FRESH database in every process, which
# is what makes best-of-N honest here. A file database would be created by the
# first run and then make every later run fail on `table already exists`, and
# `min` would happily report the error path as the fastest time.
run() { # binary -> microseconds, best of N
    "$T/timeit" "$N" "$1" ":memory:" -init /dev/null ".read $SQL" | awk '{print $2}'
}
out() { "$1" ":memory:" -init /dev/null ".read $SQL" 2>&1; }

$GCC -o "$T/pure_g" "$T/g.o" "$T/sh_g.o" -lm || exit 1
$GCC -o "$T/pure_z" "$T/z.o" "$T/sh_g.o" -lm || exit 1
GT=$(run "$T/pure_g"); ZT=$(run "$T/pure_z")
REF=$(out "$T/pure_g")
printf "%-34s %9s us\n" "gcc -O1 (all)" "$GT"
printf "%-34s %9s us   ratio %s\n" "zcc (all)" "$ZT" \
  "$(awk "BEGIN{printf \"%.3f\", $ZT/$GT}")"
[ "$(out "$T/pure_z")" = "$REF" ] || { echo "PURE BUILDS DIVERGE — stop"; exit 1; }
[ $# -eq 0 ] && exit 0

# ONE HYBRID PER FUNCTION, ranked — the ordinary way to use this. Each line is
# an independent experiment: that function from gcc, all the others from zcc.
if [ "$SCAN" = 1 ]; then
    for f in "$@"; do
        sh "$0" "$f" 2>/dev/null | tail -1
    done | sed 's/.*closes \([-0-9.]*\)%.*/\1\t&/' | sort -rn | cut -f2-
    exit 0
fi

# Every donor global except the chosen ones gets weakened — DATA included.
# `-DSQLITE_PRIVATE=` externalizes sqlite's tables as well as its functions
# (sqlite3OpcodeProperty, sqlite3CtypeMap, sqlite3Config...), so a text-only
# list leaves twenty duplicate definitions and the link dies.
nm -g --defined-only "$T/g.o" | awk 'NF==3 {print $3}' | sort -u > "$T/all"
printf '%s\n' "$@" | sort > "$T/keep"
comm -23 "$T/all" "$T/keep" > "$T/weaken"
objcopy --weaken-symbols="$T/weaken" "$T/g.o" "$T/gw.o"
: > "$T/wz"
for f in "$@"; do echo "$f" >> "$T/wz"; done
objcopy --weaken-symbols="$T/wz" "$T/z.o" "$T/zw.o"
$GCC -o "$T/hy" "$T/zw.o" "$T/gw.o" "$T/sh_g.o" -lm || { echo "hybrid link failed"; exit 1; }
if [ "$(out "$T/hy")" != "$REF" ]; then echo "HYBRID DIVERGES — result discarded"; exit 1; fi
HT=$(run "$T/hy")
printf "%-34s %9s us   ratio %s   closes %s%% of the gap\n" \
  "hybrid: $* from gcc" "$HT" \
  "$(awk "BEGIN{printf \"%.3f\", $HT/$GT}")" \
  "$(awk "BEGIN{ g=$ZT-$GT; if(g>0) printf \"%.1f\", 100*($ZT-$HT)/g; else print \"n/a\" }")"
