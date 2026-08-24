#!/bin/sh
# perfn.sh — PER-FUNCTION instruction-count differential vs gcc-O1 (the discovery
# engine; OPT.md §0 re-plan 2026-08-24). For each program in suite/:
#   (1) CORRECTNESS GATE: zcc stdout == gcc-O1 stdout (else MISCOMPILE, case dropped).
#   (2) count instruction lines per FUNCTION in each .s (a fn spans its `.type X,%function`
#       label .. its `.size X`), attribute to the fn name (identical between compilers).
#   (3) emit one row per (program,function): gcc / zcc / delta / ratio.
# Ranked by delta desc = the worklist. Instruction-count is the size+speed proxy the
# plan optimizes (user decision 2026-08-24); wall-clock is confirmation only.
#   Run in box:  ZCC=/usr/local/bin/zcc GCC=aarch64-linux-gnu-gcc sh tests/bench/perfn.sh
set -u
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
DIR="$(dirname "$0")/suite"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
# awk: emit "fn<TAB>count" for real functions in a .s
perfn_awk='
  /%function/ { l=$0; sub(/^[ \t]*\.type[ \t]+/,"",l); sub(/[ \t,].*/,"",l); pend=l }
  /^[A-Za-z_.$][A-Za-z0-9_.$]*:/ { lbl=$0; sub(/:.*/,"",lbl); if(lbl==pend){ if(fn!="")print fn"\t"c; fn=lbl; c=0; pend="" } }
  /^\t[a-z]/ && !/^\t\./ { if(fn!="") c++ }
  /^[ \t]*\.size/ { if(fn!=""){ print fn"\t"c; fn=""; c=0 } }
  END { if(fn!="") print fn"\t"c }
'
ROWS="$TMP/rows"; : > "$ROWS"
ndiv=0; nmis=0; nok=0
for c in "$DIR"/*.c; do
  b=$(basename "$c" .c)
  $GCC -O1 -w -S -o "$TMP/g.s" "$c" 2>"$TMP/ge" || { echo "GCC-COMPILE-FAIL $b"; continue; }
  if ! $ZCC -S -o "$TMP/z.s" "$c" 2>"$TMP/ze"; then echo "ZCC-COMPILE-FAIL $b: $(head -1 "$TMP/ze")"; continue; fi
  # correctness gate (execute both)
  $GCC -O1 -w -o "$TMP/g" "$c" 2>/dev/null; $ZCC -o "$TMP/z" "$c" 2>/dev/null
  "$TMP/g" > "$TMP/go" 2>&1; "$TMP/z" > "$TMP/zo" 2>&1
  if ! cmp -s "$TMP/go" "$TMP/zo"; then echo "MISCOMPILE $b: gcc=$(cat "$TMP/go") zcc=$(cat "$TMP/zo")"; nmis=$((nmis+1)); continue; fi
  nok=$((nok+1))
  awk "$perfn_awk" "$TMP/g.s" | sort > "$TMP/gf"
  awk "$perfn_awk" "$TMP/z.s" | sort > "$TMP/zf"
  join "$TMP/gf" "$TMP/zf" | while read fn gc zc; do
    d=$((zc-gc)); printf '%s\t%s\t%s\t%s\t%s\n' "$b" "$fn" "$gc" "$zc" "$d" >> "$ROWS"
  done
done
echo "gate: $nok CLEAN / $nmis MISCOMPILE"
echo
printf '%-22s %-16s %6s %6s %6s %7s\n' PROGRAM FUNCTION gcc zcc delta ratio
echo "------------------------------------------------------------------------------"
sort -t"$(printf '\t')" -k5 -nr "$ROWS" | while IFS="$(printf '\t')" read b fn gc zc d; do
  r=$(awk "BEGIN{ if($gc>0) printf \"%.2f\", $zc/$gc; else printf \"inf\" }")
  printf '%-22s %-16s %6s %6s %6s %7s\n' "$b" "$fn" "$gc" "$zc" "$d" "$r"
done
echo
# totals: sum gcc, sum zcc over all matched functions → aggregate insn ratio
awk -F"$(printf '\t')" '{ G+=$3; Z+=$4 } END{ printf "TOTAL matched-fn insns: gcc=%d zcc=%d  ratio=%.3f\n", G, Z, Z/G }' "$ROWS"
