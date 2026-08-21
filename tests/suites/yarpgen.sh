#!/bin/sh
# yarpgen.sh — RANDOM-DIFFERENTIAL correctness proof, arithmetic/loop axis (the
# complement of csmith.sh, whose axis is pointers/aggregates). yarpgen generates
# UB-FREE C programs by-construction that stress scalar computation, integer
# promotion/conversion, and loop nests — precisely the surface the optimization
# passes act on. The INDEPENDENT referee = gcc (an ELF oracle of the same arch in
# the box). A yarpgen test is THREE files compiled together:
#   driver.c  — global inputs, main(), a hash-based checksum(), prints "%llu\n"
#   func.c    — the function under test (#include "init.h")
#   init.h    — extern declarations shared by the two translation units
# For each pre-cached test dir at /suites/yarpgen/<id>/:
#   gcc  compile+run  → reference checksum (if gcc fails/timeouts → SKIP: junk sample)
#   zcc  compile      → COMPILE-FAIL = NOT-IMPL (named, out of scope — completeness)
#   zcc  run (noopt)  + zcc ZCC_OPT=1 run  → compare stdout against gcc
# PARITY = gcc==zcc==zcc-opt (checksum strings match). DIVERGE = compiles+runs but
# DIFFERS from gcc ⟹ MISCOMPILE (0 DIVERGE is the invariant; printed for diagnosis by
# decomposition). Being UB-free, "diff at UB" CANNOT be invoked — every DIVERGE is a
# compiler fault until proven otherwise.
#
# Evidence trail (clean-input rule): print the real binary count + total bytes to
# guard against a green no-op.
# Run INSIDE the box:  ZCC=/usr/local/bin/zcc sh yarpgen.sh [N]   (N = limit)
#
# Cache generation (host, one-off; the .c is portable so the generator need not be
# in the box). Build yarpgen 2.0 from source, then for each seed:
#   yarpgen --std=c --emit-align-attr=none --emit-pragmas=none --max-array-dims=3 \
#           -s <seed> -o /suites/yarpgen/s<seed>/
# --std=c is the only C standard yarpgen accepts; align-attr/pragmas are disabled to
# stay within the supported surface; max-array-dims=3 caps global size so a test runs
# in well under a second (the default admits 7-D arrays → hundreds of MB per global).
set -u
ZCC="${ZCC:-/usr/local/bin/zcc}"
DIR=/suites/yarpgen
LIM="${1:-0}"
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

par=0; div=0; ni=0; skip=0; n=0; bytes=0
divlist=""; nilist=""
for t in "$DIR"/*/; do
    [ -f "$t/driver.c" ] && [ -f "$t/func.c" ] || continue
    b=$(basename "$t")
    n=$((n+1))
    [ "$LIM" -gt 0 ] && [ "$n" -gt "$LIM" ] && { n=$((n-1)); break; }

    # gcc oracle — junk sample (gcc fail/timeout/UB-runtime) → SKIP, NOT charged to zcc
    if ! gcc -w -I"$t" "$t/driver.c" "$t/func.c" -o "$D/g" 2>/dev/null; then skip=$((skip+1)); continue; fi
    timeout 20 "$D/g" > "$D/go" 2>&1; grc=$?
    [ "$grc" -gt 127 ] && { skip=$((skip+1)); continue; } # gcc-binary crash/timeout: reject sample

    # zcc noopt — compile-fail = NOT-IMPL (completeness gap, named)
    if ! "$ZCC" -I"$t" "$t/driver.c" "$t/func.c" -o "$D/z" 2>"$D/ze"; then
        ni=$((ni+1)); nilist="$nilist $b:$(head -1 "$D/ze" | cut -c1-40)"; continue
    fi
    bytes=$((bytes + $(wc -c < "$D/z")))
    timeout 20 "$D/z" > "$D/zo" 2>&1; zrc=$?

    # zcc opt
    if ! ZCC_OPT=1 "$ZCC" -I"$t" "$t/driver.c" "$t/func.c" -o "$D/zp" 2>/dev/null; then
        div=$((div+1)); divlist="$divlist $b(OPT-COMPILE-FAIL)"; continue
    fi
    timeout 20 "$D/zp" > "$D/zpo" 2>&1; prc=$?

    if [ "$grc" = "$zrc" ] && [ "$grc" = "$prc" ] \
       && cmp -s "$D/go" "$D/zo" && cmp -s "$D/go" "$D/zpo"; then
        par=$((par+1))
    else
        div=$((div+1))
        divlist="$divlist $b(g=$grc,z=$zrc,o=$prc)"
    fi
done

echo "yarpgen: $par PARITY / $div DIVERGE / $ni NOT-IMPL / $skip SKIP  (scanned $n; zcc-ELF ${bytes}B)"
[ -n "$divlist" ] && echo "DIVERGE:$divlist"
[ -n "$nilist" ] && echo "NOT-IMPL:$nilist" | cut -c1-400
[ "$div" = 0 ]
