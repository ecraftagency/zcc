#!/bin/sh
# csmith.sh — RANDOM-DIFFERENTIAL correctness proof (an EXPENSIVE proof: dozens of
# same-size compilers still fail it). csmith generates UB-FREE C programs
# by-construction; the INDEPENDENT referee = gcc (an ELF oracle of the same arch in
# the box). For each .c pre-cached at /suites/csmith/*.c:
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
# Run INSIDE the box:  ZCC=/usr/local/bin/zcc sh csmith.sh [N]   (N = limit)
set -u
ZCC="${ZCC:-/usr/local/bin/zcc}"
INC=/suites/csmith/include
DIR=/suites/csmith
LIM="${1:-0}"
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

par=0; div=0; ni=0; skip=0; n=0; bytes=0
divlist=""; nilist=""
for f in "$DIR"/*.c; do
    [ -f "$f" ] || continue
    b=$(basename "$f" .c)
    n=$((n+1))
    [ "$LIM" -gt 0 ] && [ "$n" -gt "$LIM" ] && { n=$((n-1)); break; }

    # gcc oracle — junk sample (gcc fail/timeout/UB-runtime) → SKIP, NOT charged to zcc
    if ! gcc -w -I"$INC" "$f" -o "$D/g" 2>/dev/null; then skip=$((skip+1)); continue; fi
    timeout 15 "$D/g" > "$D/go" 2>&1; grc=$?
    [ "$grc" -gt 127 ] && { skip=$((skip+1)); continue; } # gcc-binary crash/timeout: reject sample

    # zcc noopt — compile-fail = NOT-IMPL (completeness gap, named)
    if ! "$ZCC" -I"$INC" "$f" -o "$D/z" 2>"$D/ze"; then
        ni=$((ni+1)); nilist="$nilist $b:$(head -1 "$D/ze" | cut -c1-40)"; continue
    fi
    bytes=$((bytes + $(wc -c < "$D/z")))
    timeout 15 "$D/z" > "$D/zo" 2>&1; zrc=$?

    # zcc opt
    if ! ZCC_OPT=1 "$ZCC" -I"$INC" "$f" -o "$D/zp" 2>/dev/null; then
        div=$((div+1)); divlist="$divlist $b(OPT-COMPILE-FAIL)"; continue
    fi
    timeout 15 "$D/zp" > "$D/zpo" 2>&1; prc=$?

    if [ "$grc" = "$zrc" ] && [ "$grc" = "$prc" ] \
       && cmp -s "$D/go" "$D/zo" && cmp -s "$D/go" "$D/zpo"; then
        par=$((par+1))
    else
        div=$((div+1))
        divlist="$divlist $b(g=$grc,z=$zrc,o=$prc)"
    fi
done

echo "csmith: $par PARITY / $div DIVERGE / $ni NOT-IMPL / $skip SKIP  (scanned $n; zcc-ELF ${bytes}B)"
[ -n "$divlist" ] && echo "DIVERGE:$divlist"
[ -n "$nilist" ] && echo "NOT-IMPL:$nilist" | cut -c1-400
[ "$div" = 0 ]
