set -u
SQ=/suites/sqlite/sqlite3.c
T=/tmp/mne; mkdir -p $T
zcc -S -o $T/z.s $SQ 2>/dev/null || echo ZCC-FAIL
gcc -O1 -w -S -o $T/g.s $SQ 2>/dev/null || echo GCC-FAIL
n(){ grep -cE "^[[:space:]]+$2([[:space:]]|$)" "$1"; }
printf "%-10s %8s %8s\n" mnemonic zcc gcc
for m in ldrsb ldrsh ldrsw sxtb sxth sxtw uxtb uxth tst ands cmn tbz tbnz cbz cbnz sbfiz ubfiz sbfx ubfx bfi csneg csinc csinv cset csel fmov madd mul orr eor bic orn neg ccmp ccmn; do
  printf "%-10s %8s %8s\n" "$m" "$(n $T/z.s $m)" "$(n $T/g.s $m)"
done
echo "TOTAL zcc=$(grep -cE '^[[:space:]]+[a-z]' $T/z.s) gcc=$(grep -cE '^[[:space:]]+[a-z]' $T/g.s)"
echo "--- shifted-operand ALU (zcc vs gcc) ---"
for p in 'lsl #' 'lsr #' 'asr #'; do printf "%-8s zcc=%s gcc=%s\n" "$p" "$(grep -cE "^[[:space:]]+(add|sub|orr|eor|and|cmp|bic) .*$p" $T/z.s)" "$(grep -cE "^[[:space:]]+(add|sub|orr|eor|and|cmp|bic) .*$p" $T/g.s)"; done
