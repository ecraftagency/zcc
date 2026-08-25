#!/bin/sh
# exec-speed + static-insn scoreboard: zcc vs gcc-O1 over the taxonomy suite (geo40).
#
# Run INSIDE the box:  ZCC=/usr/local/bin/zcc sh tests/bench/exectime.sh
#
# TWO metrics per program, deliberately paired because NEITHER alone is trustworthy:
#
#   * insn  = static instruction-count ratio from `-S` (user code only, no libc).
#             DETERMINISTIC — zero measurement noise. This is the number to trust for
#             short programs; it is a catamorphism over the emitted stream, not a clock.
#             It CANNOT see memory-boundedness (an insn removed from a load-shadow costs
#             no cycles) — so it is a proxy, not the arbiter.
#   * exec  = best-of-N wall-clock ratio. The real arbiter (captures memory/cache/branch
#             effects insn-count misses) — but at ms granularity it is NOISE below ~30ms
#             (±1ms of a 4ms run is ±25%). Trust it only when exec_ms is large.
#   When the two DISAGREE: large-time program → exec wins; short program → insn wins.
#
# HONEST-MEASUREMENT PATCH (2026-08-24) — holes closed so the geomean cannot flatter zcc:
#   (a) gcc=0ms cases COUNTED, not skipped. gcc-O1 sometimes eliminates a whole loop
#       (final-value/SCEV or loop-DCE): gcc≈0ms, zcc runs it full. The old harness's
#       `gmin<5 → skip` dropped exactly zcc's worst cases (j1/c2/b4/c1/c3/f3). Now they
#       land in a named "gcc-zeroed" bucket with absolute zcc time + the deterministic
#       insn ratio — ASYMPTOTIC gaps (gcc O(1) vs zcc O(n)), kept out of the constant-
#       factor exec geomean but never hidden. Closing them is #24 (SCEV/loop-DCE).
#   (b) DISTRIBUTION reported, not just the geomean — worst program, median, count>1.1x.
#       Parity means no systematic loss on ANY class, not just on-average.
#   (c) SYMMETRY (2026-08-25) — the zcc-ZEROED bucket, the exact mirror of (a). Once zcc
#       kills a loop gcc keeps (the invariant pure-call hoist), zcc≈0ms against a gcc of
#       hundreds of ms. That is an ASYMPTOTIC win and belongs in a named bucket for the
#       same reason an asymptotic loss does: folding a ratio of 0.006 — or of 0, whose
#       log is -inf and which silently printed the geomean as 0.0000 — into a
#       CONSTANT-FACTOR geomean does not measure a constant factor. The rule is one rule
#       applied both ways: a side whose time is below GCC_FAST is unmeasurable at ms
#       granularity, so the pair leaves the geomean and is reported by name.
#
# Clean-input law: every case is correctness-gated (gcc stdout == zcc stdout) before it
# is timed; a mismatch is reported DIVERGE, never silently skipped.
set -u
SUITE="${SUITE:-/work/tests/bench/suite}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
N="${N:-5}"
GCC_FAST=5    # gcc <this ms ⟹ wall-time unmeasurable (startup-dominated)
ZCC_SLOW=15   # zcc >=this ms while gcc is fast ⟹ gcc-zeroed (real asymptotic gap)

rows=/tmp/geo40_rows.$$; zeroed=/tmp/geo40_zeroed.$$; allinsn=/tmp/geo40_insn.$$
zzero=/tmp/geo40_zzero.$$
: > "$rows"; : > "$zeroed"; : > "$allinsn"; : > "$zzero"
skip=0; diverge=0

insns() { grep -cE '^[[:space:]]+[a-z]' "$1" 2>/dev/null || echo 0; }

printf "%-22s %8s %8s %8s %8s\n" program insn_r gcc_ms zcc_ms exec_r
for c in "$SUITE"/*.c; do
  b=$(basename "$c" .c)
  "$GCC" -O1 -w -o /tmp/g "$c" 2>/dev/null || { skip=$((skip+1)); continue; }
  "$ZCC" -o /tmp/z "$c" 2>/dev/null || { skip=$((skip+1)); continue; }
  /tmp/g > /tmp/go 2>/dev/null; /tmp/z > /tmp/zo 2>/dev/null
  if ! cmp -s /tmp/go /tmp/zo; then
    printf "%-22s %8s %8s %8s %8s\n" "$b" - DIVERGE - -; diverge=$((diverge+1)); continue
  fi
  # DETERMINISTIC static insn ratio (user code only, via -S).
  "$GCC" -O1 -w -S -o /tmp/g.s "$c" 2>/dev/null; "$ZCC" -S -o /tmp/z.s "$c" 2>/dev/null
  gi=$(insns /tmp/g.s); zi=$(insns /tmp/z.s)
  ir=$(awk "BEGIN{ if($gi>0) printf \"%.3f\", $zi/$gi; else print \"-\" }")
  [ "$gi" -gt 0 ] && echo "$b $ir" >> "$allinsn"
  # NOISY wall-clock ratio.
  gmin=; zmin=
  i=0; while [ "$i" -lt "$N" ]; do
    t0=$(date +%s%N); /tmp/g >/dev/null 2>&1; t1=$(date +%s%N); d=$(( (t1-t0)/1000000 ))
    { [ -z "$gmin" ] || [ "$d" -lt "$gmin" ]; } && gmin=$d
    t0=$(date +%s%N); /tmp/z >/dev/null 2>&1; t1=$(date +%s%N); d=$(( (t1-t0)/1000000 ))
    { [ -z "$zmin" ] || [ "$d" -lt "$zmin" ]; } && zmin=$d
    i=$((i+1))
  done

  if [ "$gmin" -lt "$GCC_FAST" ]; then
    if [ "$zmin" -ge "$ZCC_SLOW" ]; then
      printf "%-22s %8s %8s %8d %8s\n" "$b" "$ir" "~0" "$zmin" "ZEROED"
      echo "$b $zmin $ir" >> "$zeroed"
    else
      printf "%-22s %8s %8d %8d %8s\n" "$b" "$ir" "$gmin" "$zmin" "fast"
      skip=$((skip+1))
    fi
    continue
  fi
  if [ "$zmin" -lt "$GCC_FAST" ]; then
    # The mirror of the gcc-ZEROED case: zcc killed work gcc still performs.
    printf "%-22s %8s %8d %8s %8s\n" "$b" "$ir" "$gmin" "~0" "ZCC-ZEROED"
    echo "$b $gmin $ir" >> "$zzero"
    continue
  fi
  er=$(awk "BEGIN{printf \"%.3f\", $zmin/$gmin}")
  printf "%-22s %8s %8d %8d %8s\n" "$b" "$ir" "$gmin" "$zmin" "$er"
  echo "$b $er" >> "$rows"
done

echo "---"
# EXEC (arbiter): constant-factor geomean + distribution, measurable-both set only.
awk '{n++; r=$2; s+=log(r); a[n]=r; if(r>worst){worst=r; wn=$1}; if(r>1.1)hi++}
     END{ if(n==0){print "EXEC: no timed programs"; exit}
       for(i=1;i<=n;i++)for(j=i+1;j<=n;j++)if(a[j]<a[i]){t=a[i];a[i]=a[j];a[j]=t}
       med=(n%2)?a[(n+1)/2]:(a[n/2]+a[n/2+1])/2;
       printf "EXEC geomean (arbiter, noisy): %.4f over %d | median %.3f | worst %s %.3f | %d>1.1x\n",
              exp(s/n), n, med, wn, worst, hi }' "$rows"
# INSN (deterministic proxy): geomean + distribution over ALL matched programs (incl. zeroed).
awk '{n++; r=$2; s+=log(r); a[n]=r; if(r>worst){worst=r; wn=$1}; if(r>1.1)hi++}
     END{ if(n==0){print "INSN: none"; exit}
       for(i=1;i<=n;i++)for(j=i+1;j<=n;j++)if(a[j]<a[i]){t=a[i];a[i]=a[j];a[j]=t}
       med=(n%2)?a[(n+1)/2]:(a[n/2]+a[n/2+1])/2;
       printf "INSN geomean (determ., all %d): %.4f | median %.3f | worst %s %.3f | %d>1.1x\n",
              n, exp(s/n), med, wn, worst, hi }' "$allinsn"

zc=$(wc -l < "$zeroed" | tr -d ' ')
if [ "$zc" -gt 0 ]; then
  echo "gcc-ZEROED (gcc-O1 killed the loop; asymptotic = #24 SCEV/loop-DCE) — insn ratio still deterministic:"
  while read -r nm ms ir; do printf "  %-20s zcc=%sms vs gcc≈0  (insn %s)\n" "$nm" "$ms" "$ir"; done < "$zeroed"
fi
zz=$(wc -l < "$zzero" | tr -d ' ')
if [ "$zz" -gt 0 ]; then
  echo "zcc-ZEROED (zcc killed the loop gcc-O1 keeps; asymptotic WIN, kept out of the geomean):"
  while read -r nm ms ir; do printf "  %-20s gcc=%sms vs zcc≈0  (insn %s)\n" "$nm" "$ms" "$ir"; done < "$zzero"
fi
echo "(skipped $skip trivial, $diverge DIVERGE)"
echo "PARITY = exec≈1.0 AND insn≈1.0 AND flat distribution AND gcc-zeroed bucket EMPTY."
rm -f "$rows" "$zeroed" "$allinsn" "$zzero"
