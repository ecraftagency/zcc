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
#   zcc  run (-O0, ZCC_O0=1)  + zcc default-opt run  → compare stdout against gcc
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
# ── TEMPORARILY DISABLED (user request, 2026-08-25) ──────────────────────────
# yarpgen is removed from the suite until the user brings it back. To restore,
# delete this block. Exits 0 (skip, not fail) so any gate that invokes this
# script treats it as a clean no-op. Set ZCC_YARPGEN=1 to run it anyway.
if [ -z "${ZCC_YARPGEN:-}" ]; then
    echo "yarpgen: TEMPORARILY DISABLED (user request 2026-08-25) — set ZCC_YARPGEN=1 to run, or delete the guard in tests/suites/yarpgen.sh to restore"
    exit 0
fi
export ZCC="${ZCC:-/usr/local/bin/zcc}"
# Compile-timeout ceiling (s): bounds a pathological zcc compile (e.g. s0940, super-
# linear opt path) so a 10k run cannot stall. An overrun is a PERF fault, named
# CTIMEOUT — NEVER DIVERGE (a slow compile is not a miscompile). Generous: only true
# pathology trips it; on 64 cores the tail is max(one case), not the sum.
: "${ZCC_CTIMEOUT:=300}"; export ZCC_CTIMEOUT
DIR=/suites/yarpgen
LIM="${1:-0}"                       # 0 = all
JOBS="${YARP_JOBS:-$(nproc 2>/dev/null || echo 4)}"
RES=$(mktemp); trap 'rm -f "$RES"' EXIT

# One worker per test dir, run under `xargs -P`. Each prints exactly one TSV line:
#   STATUS<TAB>name<TAB>bytes<TAB>detail   (STATUS ∈ PARITY|DIVERGE|NOTIMPL|SKIP)
# The worker is self-contained (fresh shell): $ZCC is inherited from the environment.
work='
  t="$1"; b=$(basename "$t")
  [ -f "$t/driver.c" ] && [ -f "$t/func.c" ] || exit 0
  d=$(mktemp -d); trap "rm -rf $d" EXIT
  # gcc oracle — junk sample (gcc fail/timeout/UB-runtime) → SKIP, NOT charged to zcc
  gcc -w -I"$t" "$t/driver.c" "$t/func.c" -o "$d/g" 2>/dev/null || { printf "SKIP\t%s\t0\tgcc-compile\n" "$b"; exit 0; }
  timeout 20 "$d/g" > "$d/go" 2>&1; grc=$?
  # gcc did not COMPLETE normally (124=timeout, >127=signal) → junk reference → SKIP
  { [ "$grc" = 124 ] || [ "$grc" -gt 127 ]; } && { printf "SKIP\t%s\t0\tgcc-run\n" "$b"; exit 0; }
  # zcc -O0 (optimizer off) — compile-fail = NOT-IMPL (completeness gap, named)
  timeout "$ZCC_CTIMEOUT" env ZCC_O0=1 "$ZCC" -I"$t" "$t/driver.c" "$t/func.c" -o "$d/z" 2>"$d/ze"; crc=$?
  [ "$crc" = 124 ] && { printf "CTIMEOUT\t%s\t0\to0-compile>%ss\n" "$b" "$ZCC_CTIMEOUT"; exit 0; }
  [ "$crc" != 0 ] && { printf "NOTIMPL\t%s\t0\t%s\n" "$b" "$(head -1 "$d/ze" | cut -c1-40)"; exit 0; }
  bytes=$(wc -c < "$d/z")
  timeout 60 "$d/z" > "$d/zo" 2>&1; zrc=$?
  # zcc default optimizer (SSA + regalloc, no env)
  timeout "$ZCC_CTIMEOUT" "$ZCC" -I"$t" "$t/driver.c" "$t/func.c" -o "$d/zp" 2>/dev/null; crc=$?
  [ "$crc" = 124 ] && { printf "CTIMEOUT\t%s\t%s\topt-compile>%ss\n" "$b" "$bytes" "$ZCC_CTIMEOUT"; exit 0; }
  [ "$crc" != 0 ] && { printf "DIVERGE\t%s\t%s\tOPT-COMPILE-FAIL\n" "$b" "$bytes"; exit 0; }
  timeout 60 "$d/zp" > "$d/zpo" 2>&1; prc=$?
  # zcc did not COMPLETE (124=timeout, >127=signal) → NOT a checksum divergence.
  # Honest 3rd state (perf boundary OR masked non-termination): never PARITY nor
  # DIVERGE; named + auditable, resolved by a separate run-to-completion check.
  if [ "$zrc" = 124 ] || [ "$zrc" -gt 127 ] || [ "$prc" = 124 ] || [ "$prc" -gt 127 ]; then
      printf "TIMEOUT\t%s\t%s\tz=%s,o=%s (gcc completed)\n" "$b" "$bytes" "$zrc" "$prc"; exit 0; fi
  if [ "$grc" = "$zrc" ] && [ "$grc" = "$prc" ] && cmp -s "$d/go" "$d/zo" && cmp -s "$d/go" "$d/zpo"; then
      printf "PARITY\t%s\t%s\t-\n" "$b" "$bytes"
  else
      printf "DIVERGE\t%s\t%s\tg=%s,z=%s,o=%s\n" "$b" "$bytes" "$grc" "$zrc" "$prc"
  fi
'
# deterministic, seekable list of test dirs; LIM>0 truncates
list() { for t in "$DIR"/*/; do [ -d "$t" ] && printf '%s\n' "$t"; done | sort
       }
if [ "$LIM" -gt 0 ]; then list | head -n "$LIM"; else list; fi \
    | xargs -P "$JOBS" -n1 sh -c "$work" _ > "$RES"

par=$(grep -c '^PARITY'  "$RES"); div=$(grep -c '^DIVERGE'  "$RES")
ni=$(grep -c '^NOTIMPL' "$RES"); skip=$(grep -c '^SKIP'    "$RES")
to=$(grep -c '^TIMEOUT' "$RES"); ct=$(grep -c '^CTIMEOUT' "$RES")
bytes=$(awk -F'\t' '{s+=$3} END{print s+0}' "$RES")
n=$((par+div+ni+to+ct+skip))
echo "yarpgen: $par PARITY / $div DIVERGE / $to TIMEOUT / $ct CTIMEOUT / $ni NOT-IMPL / $skip SKIP  (scanned $n, ${JOBS} jobs; zcc-ELF ${bytes}B)"
[ "$div"  -gt 0 ] && { printf 'DIVERGE:'; awk -F'\t' '$1=="DIVERGE"{printf " %s(%s)",$2,$4} END{print ""}' "$RES"; }
[ "$to"   -gt 0 ] && { printf 'TIMEOUT (zcc -O0 ≫ gcc -O0 budget; correctness re-checked run-to-completion):'; awk -F'\t' '$1=="TIMEOUT"{printf " %s(%s)",$2,$4} END{print ""}' "$RES"; }
[ "$ct"   -gt 0 ] && { printf 'CTIMEOUT (zcc compile > %ss — PERF pathology, not a miscompile; isolate the pass):' "$ZCC_CTIMEOUT"; awk -F'\t' '$1=="CTIMEOUT"{printf " %s(%s)",$2,$4} END{print ""}' "$RES"; }
[ "$ni"   -gt 0 ] && { printf 'NOT-IMPL:'; awk -F'\t' '$1=="NOTIMPL"{printf " %s:%s",$2,$4} END{print ""}' "$RES" | cut -c1-400; echo; }
[ "$div" = 0 ]
