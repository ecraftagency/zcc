#!/bin/sh
# corpus25.sh — the PROVEN measurement corpus / knowledge base for spine #25 (nuclear
# register-primary allocator). READ-ONLY: it emits and greps assembly, it changes nothing.
#
# Every number is a mechanical catamorphism over the EMITTED .s (Law-3 "certify at the
# middle": the .s CONFIRMS, it does not estimate) — no allocator instrumentation, so no
# trust dependency. The frame-slot mem-op count ([sp,#..] loads/stores) IS the home-primary
# reload/spill floor #25 targets; the per-function concentration says WHETHER that floor is
# in a few monster functions (allocator wins big, easy) or spread broad (harder) — which is
# the single fact that turns the #25 projection band into a real target.
#
# Reproduce (inside the box):
#   ZCC=/usr/local/bin/zcc GCC=aarch64-linux-gnu-gcc sh tests/bench/corpus25.sh
# Snapshot of a run is recorded in OPT.md note ㉕-CORPUS; re-run to regression-check.
set -u
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-aarch64-linux-gnu-gcc}"
command -v "$GCC" >/dev/null 2>&1 || GCC=gcc
SQ="${SQLITE:-/suites/sqlite/sqlite3.c}"
SUITE="${SUITE:-tests/bench/suite}"
TMP=/tmp/corpus25
rm -rf "$TMP"; mkdir -p "$TMP"

# ── emitters ────────────────────────────────────────────────────────────────
# A FAILED compile must turn the measurement red, not reuse the last good .s.
# The directory used to persist across runs with stderr discarded, so a zcc that
# crashed on sqlite silently re-reported the previous session's numbers — the
# Article E "clean-input" hole: a green number with no evidence that the artifact
# it counts was produced by THIS binary. (Found when R2.2's promoted values first
# overflowed the spiller.)
emit() { # emit <compiler> <out> <src> <label>
    if ! "$1" -O1 -S -o "$2" "$3" 2>"$TMP/err"; then
        echo "!! $4 FAILED to compile $3 — measurement void:"; head -5 "$TMP/err"; exit 1
    fi
    [ -s "$2" ] || { echo "!! $4 produced an empty .s for $3 — measurement void"; exit 1; }
}
emit "$ZCC" "$TMP/z.s" "$SQ" zcc
emit "$GCC" "$TMP/g.s" "$SQ" gcc

# A mnemonic is the first token after the leading whitespace, delimited by whitespace or EOL.
# The delimiter MUST be [[:space:]] (matches BOTH gcc's tab and zcc's space) — an audit caught
# a `( |$)` variant that read 0 for every gcc column (gcc tab-delimits its operands).
insns() { grep -cE "^[[:space:]]+[a-z]" "$1"; }
mne()   { grep -cE "^[[:space:]]+$2([[:space:]]|$)" "$1"; }
memops(){ grep -cE "^[[:space:]]+(ldr|ldrb|ldrh|ldrsw|ldp|str|strb|strh|stp)[[:space:]]" "$1"; }
framem(){ grep -E "^[[:space:]]+(ldr|ldrb|ldrh|ldrsw|ldp|str|strb|strh|stp)[[:space:]]" "$1" | grep -cE "\[(sp|x29|fp)"; }
ratio() { awk "BEGIN{printf \"%.3f\", $1/$2}"; }

echo "############################################################"
echo "# CORPUS #25 — proven baseline (all numbers = grep over .s) #"
echo "############################################################"
echo
echo "== M1. sqlite whole-module: zcc -O1  vs  gcc -O1 =="
zt=$(insns "$TMP/z.s"); gt=$(insns "$TMP/g.s")
echo "insns          zcc=$zt  gcc=$gt  RATIO=$(ratio "$zt" "$gt")"
zm=$(memops "$TMP/z.s"); gm=$(memops "$TMP/g.s")
echo "mem-ops        zcc=$zm  gcc=$gm  (zcc excess=$((zm-gm)))"
zf=$(framem "$TMP/z.s")
echo "frame [sp] m-op zcc=$zf   <-- HOME reload/spill floor (#25 target)"
echo
echo "-- zcc mnemonic composition (the killable floor is mov + frame-mem) --"
for m in mov ldr ldrsw str stp ldp sxtw uxtb uxth add sub cmp bl adrp csel cset; do
  c=$(mne "$TMP/z.s" "$m"); p=$(awk "BEGIN{printf \"%.1f\", 100*$c/$zt}")
  printf "  %-6s %8s  (%s%%)\n" "$m" "$c" "$p"
done
mov=$(mne "$TMP/z.s" mov)
floor=$((mov + zf))
echo "  ------"
echo "  KILLABLE FLOOR (mov + frame-mem) = $floor  = $(awk "BEGIN{printf \"%.1f\", 100*$floor/$zt}")% of zcc insns"
echo

echo "== M2. WHERE the floor lives — per-function concentration (zcc sqlite) =="
# Attribute each insn (and each frame-slot mem-op) to the nearest preceding top-level label.
awk '
  /^[A-Za-z_.][A-Za-z0-9_.$]*:$/ && $0 !~ /^\.L/ { fn=substr($0,1,length($0)-1); next }
  /^[[:space:]]+[a-z]/ {
    ins[fn]++
    if ($0 ~ /\[sp/ && $0 ~ /(ldr|ldrb|ldrh|ldrsw|ldp|str|strb|strh|stp)/) sp[fn]++
  }
  END { for (f in ins) printf "%s %d %d\n", f, ins[f], sp[f]+0 }
' "$TMP/z.s" | sort -k2 -rn > "$TMP/perfn.txt"
nfn=$(wc -l < "$TMP/perfn.txt")
echo "functions with code: $nfn"
echo "top-15 by insn   (fn  insns  frame-mem):"
head -15 "$TMP/perfn.txt" | awk '{printf "  %-34s %7d %7d\n", $1, $2, $3}'
echo "concentration (share of total insns / frame-mem held by the top-K functions):"
awk -v NF_="$nfn" '
  { ti+=$2; tm+=$3; ci[NR]=ti; cm[NR]=tm }
  END {
    for (k=1;k<=NR;k++) {
      if (k==10 || k==50 || k==200 || k==int(NR*0.01)+1) {
        printf "  top-%-4d  insns=%5.1f%%  frame-mem=%5.1f%%\n", k, 100*ci[k]/ti, 100*cm[k]/tm
      }
    }
    printf "  TOTAL     insns=%d  frame-mem=%d\n", ti, tm
  }
' "$TMP/perfn.txt"
echo

echo "== M3. geo40 per-program: static insn ratio (deterministic) =="
printf "  %-22s %8s %8s %7s\n" program zcc_ins gcc_ins ratio
sum_z=0; sum_g=0; logsum=0; n=0; skipped=""
# A program that fails to compile is NAMED, never silently dropped: a geomean
# over a shrinking population reads as an improvement (Article E, clean-input).
for c in "$SUITE"/*.c; do
  b=$(basename "$c" .c)
  "$ZCC" -O1 -S -o "$TMP/pz.s" "$c" 2>/dev/null || { skipped="$skipped $b(zcc)"; continue; }
  "$GCC" -O1 -S -o "$TMP/pg.s" "$c" 2>/dev/null || { skipped="$skipped $b(gcc)"; continue; }
  zi=$(insns "$TMP/pz.s"); gi=$(insns "$TMP/pg.s")
  [ "$gi" -gt 0 ] || { skipped="$skipped $b(empty)"; continue; }
  r=$(ratio "$zi" "$gi")
  printf "  %-22s %8s %8s %7s\n" "$b" "$zi" "$gi" "$r"
  sum_z=$((sum_z+zi)); sum_g=$((sum_g+gi))
  logsum=$(awk "BEGIN{print $logsum + log($zi/$gi)}"); n=$((n+1))
done
echo "  ------"
echo "  INSN geomean over $n programs = $(awk "BEGIN{printf \"%.4f\", exp($logsum/$n)}")"
echo "  INSN pooled (sum zcc / sum gcc) = $(ratio "$sum_z" "$sum_g")"
[ -n "$skipped" ] && echo "  !! SKIPPED (measurement incomplete):$skipped"
echo
echo "NOTE: exec (wall-clock) geo40 is in tests/bench/exectime.sh — its geomean line is"
echo "currently poisoned by g2_strlen exec_r=0.000 (zcc infinitely faster there); the #25"
echo "session must fix that reducer before trusting the exec geomean. Per-program exec is sound."
