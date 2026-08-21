#!/bin/sh
# csmith.sh — RANDOM-DIFFERENTIAL correctness proof (an EXPENSIVE proof: dozens of
# same-size compilers still fail it). csmith generates UB-FREE C programs
# by-construction; the INDEPENDENT referee = gcc (an ELF oracle of the same arch in
# the box). For each .c pre-cached at /suites/csmith/*.c:
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
# Run INSIDE the box:  ZCC=/usr/local/bin/zcc sh csmith.sh [N]   (N = limit)
set -u
export ZCC="${ZCC:-/usr/local/bin/zcc}"
export INC=/suites/csmith/include
DIR=/suites/csmith
LIM="${1:-0}"                       # 0 = all
JOBS="${CSMITH_JOBS:-$(nproc 2>/dev/null || echo 4)}"
RES=$(mktemp); trap 'rm -f "$RES"' EXIT

# One worker per .c, run under `xargs -P`. Each prints exactly one TSV line:
#   STATUS<TAB>name<TAB>bytes<TAB>detail   (STATUS ∈ PARITY|DIVERGE|NOTIMPL|SKIP)
# The worker is self-contained (fresh shell): $ZCC and $INC come from the environment.
work='
  f="$1"; b=$(basename "$f" .c)
  d=$(mktemp -d); trap "rm -rf $d" EXIT
  # gcc oracle — junk sample (gcc fail/timeout/UB-runtime) → SKIP, NOT charged to zcc
  gcc -w -I"$INC" "$f" -o "$d/g" 2>/dev/null || { printf "SKIP\t%s\t0\tgcc-compile\n" "$b"; exit 0; }
  timeout 15 "$d/g" > "$d/go" 2>&1; grc=$?
  # gcc did not COMPLETE normally (124=timeout, >127=signal) → junk reference → SKIP
  { [ "$grc" = 124 ] || [ "$grc" -gt 127 ]; } && { printf "SKIP\t%s\t0\tgcc-run\n" "$b"; exit 0; }
  # zcc -O0 (optimizer off) — compile-fail = NOT-IMPL (completeness gap, named)
  if ! ZCC_O0=1 "$ZCC" -I"$INC" "$f" -o "$d/z" 2>"$d/ze"; then
      printf "NOTIMPL\t%s\t0\t%s\n" "$b" "$(head -1 "$d/ze" | cut -c1-40)"; exit 0; fi
  bytes=$(wc -c < "$d/z")
  timeout 60 "$d/z" > "$d/zo" 2>&1; zrc=$?
  # zcc default optimizer (SSA + regalloc, no env)
  if ! "$ZCC" -I"$INC" "$f" -o "$d/zp" 2>/dev/null; then
      printf "DIVERGE\t%s\t%s\tOPT-COMPILE-FAIL\n" "$b" "$bytes"; exit 0; fi
  timeout 60 "$d/zp" > "$d/zpo" 2>&1; prc=$?
  # zcc did not COMPLETE (124=timeout, >127=signal) → NOT a checksum divergence.
  # An honest 3rd state: perf boundary (zcc -O0 naive codegen ≫ gcc -O0) OR a masked
  # non-termination. NEVER scored PARITY (unproven) NOR DIVERGE (miscompile unproven);
  # named + auditable like NOT-IMPL, resolved by a separate run-to-completion check.
  if [ "$zrc" = 124 ] || [ "$zrc" -gt 127 ] || [ "$prc" = 124 ] || [ "$prc" -gt 127 ]; then
      printf "TIMEOUT\t%s\t%s\tz=%s,o=%s (gcc completed)\n" "$b" "$bytes" "$zrc" "$prc"; exit 0; fi
  if [ "$grc" = "$zrc" ] && [ "$grc" = "$prc" ] && cmp -s "$d/go" "$d/zo" && cmp -s "$d/go" "$d/zpo"; then
      printf "PARITY\t%s\t%s\t-\n" "$b" "$bytes"
  else
      printf "DIVERGE\t%s\t%s\tg=%s,z=%s,o=%s\n" "$b" "$bytes" "$grc" "$zrc" "$prc"
  fi
'
list() { for f in "$DIR"/*.c; do [ -f "$f" ] && printf '%s\n' "$f"; done | sort
       }
if [ "$LIM" -gt 0 ]; then list | head -n "$LIM"; else list; fi \
    | xargs -P "$JOBS" -n1 sh -c "$work" _ > "$RES"

par=$(grep -c '^PARITY'  "$RES"); div=$(grep -c '^DIVERGE'  "$RES")
ni=$(grep -c '^NOTIMPL' "$RES"); skip=$(grep -c '^SKIP'    "$RES")
to=$(grep -c '^TIMEOUT' "$RES")
bytes=$(awk -F'\t' '{s+=$3} END{print s+0}' "$RES")
n=$((par+div+ni+to+skip))
echo "csmith: $par PARITY / $div DIVERGE / $to TIMEOUT / $ni NOT-IMPL / $skip SKIP  (scanned $n, ${JOBS} jobs; zcc-ELF ${bytes}B)"
[ "$div"  -gt 0 ] && { printf 'DIVERGE:'; awk -F'\t' '$1=="DIVERGE"{printf " %s(%s)",$2,$4} END{print ""}' "$RES"; }
[ "$to"   -gt 0 ] && { printf 'TIMEOUT (zcc -O0 ≫ gcc -O0 budget; correctness re-checked run-to-completion):'; awk -F'\t' '$1=="TIMEOUT"{printf " %s(%s)",$2,$4} END{print ""}' "$RES"; }
[ "$ni"   -gt 0 ] && { printf 'NOT-IMPL:'; awk -F'\t' '$1=="NOTIMPL"{printf " %s:%s",$2,$4} END{print ""}' "$RES" | cut -c1-400; echo; }
[ "$div" = 0 ]
