#!/bin/sh
# excess.sh — the sqlite excess histogram, EVERY mnemonic, ranked.
#
# §13n was decomposed from a table of hand-picked mnemonics, which can only find
# the classes someone already suspected. This ranks the WHOLE emitted alphabet by
# `zcc - gcc` so the absurd shows itself: a form zcc emits thousands of times and
# gcc never, or one gcc uses freely and zcc cannot spell at all. Both directions
# are printed, because "gcc emits 612 `ccmp` and we emit none" is as much a
# finding as "we emit 4,441 `cbnz` and gcc emits 2,435".
#
# Also breaks the excess down by ADDRESSING/OPERAND SHAPE for the memory and ALU
# classes, since "ldr" is not one thing: a frame reload and an indexed array read
# are different rows of the plan.
#
# Run INSIDE the box:  sh tests/bench/excess.sh
set -u
SQ="${SQLITE_DIR:-/suites/sqlite}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

"$ZCC" -S -o "$T/z.s" "$SQ/sqlite3.c" 2>/dev/null || { echo "zcc failed"; exit 1; }
"$GCC" -O1 -w -S -o "$T/g.s" "$SQ/sqlite3.c" 2>/dev/null || { echo "gcc failed"; exit 1; }

# The mnemonic is the first token of an instruction line — but THE TWO
# ASSEMBLERS DO NOT SPELL THE SAME INSTRUCTION THE SAME WAY, and a histogram
# that does not say so reports dialect as excess. Two cases, both of which
# appeared at the TOP of the first ranking and were pure noise:
#   * a constant materialization is `movz`/`movn` in zcc's output and `mov
#     wN, #imm` in gcc's — so gcc's `mov` column silently included its
#     immediates while zcc's did not, poisoning BOTH rows;
#   * a conditional branch is `b.eq` in one and `beq` in the other, which put
#     ~5,000 of phantom excess in eight rows and ~5,000 of phantom deficit in
#     eight more.
# Both are canonicalized here. This is Article E's clean-input law applied to the
# instrument itself: a measurement that cannot survive being read literally is
# not evidence.
mne() {
    grep -aE "^[[:space:]]+[a-z]" "$1" \
    | awk '{
        m = $1
        # `mov reg, #imm` and `movz/movn` are one thing: materialize a constant
        if (m == "movz" || m == "movn" || m == "movk") m = "mov#imm"
        # …and gcc OMITS the `#`: it writes `mov w0, 0` where zcc writes
        # `mov w0, #0`, so the immediates were still hiding in its plain `mov`
        # column after the first correction. Match the operand, not the sigil.
        else if (m == "mov" && $0 ~ /,[[:space:]]*#?-?[0-9]/) m = "mov#imm"
        else if (m == "mov" && $0 ~ /[wx]zr[[:space:]]*$/) m = "mov,zr"
        # `beq` (gcc) and `b.eq` (zcc) are one instruction
        else if (m ~ /^b\.(eq|ne|lt|le|gt|ge|lo|ls|hi|hs|mi|pl|vs|vc)$/) { sub(/^b\./, "b", m) }
        print m
      }' | sort | uniq -c | awk '{print $2, $1}'
}
mne "$T/z.s" > "$T/zm"; mne "$T/g.s" > "$T/gm"

zt=$(grep -acE "^[[:space:]]+[a-z]" "$T/z.s"); gt=$(grep -acE "^[[:space:]]+[a-z]" "$T/g.s")
echo "sqlite: zcc $zt   gcc-O1 $gt   excess $((zt-gt))   ratio $(awk "BEGIN{printf \"%.4f\", $zt/$gt}")"
echo
echo "== EVERY mnemonic, ranked by excess (zcc − gcc) =="
printf "%-12s %8s %8s %9s %7s\n" mnemonic zcc gcc excess "%gap"
awk -v gap="$((zt-gt))" '
  NR==FNR { g[$1]=$2; next }
  { z[$1]=$2 }
  END {
    for (m in z) seen[m]=1
    for (m in g) seen[m]=1
    for (m in seen) {
      d = (m in z ? z[m] : 0) - (m in g ? g[m] : 0)
      printf "%-12s %8d %8d %9d %6.1f%%\n", m, (m in z?z[m]:0), (m in g?g[m]:0), d, (gap!=0? 100*d/gap : 0)
    }
  }' "$T/gm" "$T/zm" | sort -k4,4nr | awk 'NR<=25 || $4+0 <= -50'
echo
echo "== the memory classes, split by SHAPE (zcc) =="
shape() { printf "  %-26s %8s\n" "$1" "$(grep -acE "$2" "$T/z.s")"; }
echo "-- loads --"
shape "frame [sp,#imm]"      '^[[:space:]]+ldr[a-z]*[[:space:]]+[wxdsq][0-9]+, \[sp'
shape "base+imm [xN,#imm]"   '^[[:space:]]+ldr[a-z]*[[:space:]]+[wxdsq][0-9]+, \[x[0-9]+, #'
shape "base only [xN]"       '^[[:space:]]+ldr[a-z]*[[:space:]]+[wxdsq][0-9]+, \[x[0-9]+\]'
shape "indexed [xN,xM...]"   '^[[:space:]]+ldr[a-z]*[[:space:]]+[wxdsq][0-9]+, \[x[0-9]+, [wx]'
shape "symbol :lo12:"        '^[[:space:]]+ldr[a-z]*.*:lo12:'
echo "-- stores --"
shape "frame [sp,#imm]"      '^[[:space:]]+str[a-z]*[[:space:]]+[wxdsq][0-9]+, \[sp'
shape "base+imm [xN,#imm]"   '^[[:space:]]+str[a-z]*[[:space:]]+[wxdsq][0-9]+, \[x[0-9]+, #'
shape "indexed [xN,xM...]"   '^[[:space:]]+str[a-z]*[[:space:]]+[wxdsq][0-9]+, \[x[0-9]+, [wx]'
echo "-- moves --"
shape "mov x,x"              '^[[:space:]]+mov[[:space:]]+x[0-9]+, x[0-9]+$'
shape "mov w,w"              '^[[:space:]]+mov[[:space:]]+w[0-9]+, w[0-9]+$'
shape "mov reg, zr"          '^[[:space:]]+mov[[:space:]]+[wx][0-9]+, [wx]zr$'
shape "mov reg, #imm"        '^[[:space:]]+mov[[:space:]]+[wx][0-9]+, #'
echo
echo "== the same shapes on gcc-O1, for the ones above =="
gshape() { printf "  %-26s %8s\n" "$1" "$(grep -acE "$2" "$T/g.s")"; }
gshape "frame ldr [sp,#imm]"  '^[[:space:]]+ldr[a-z]*[[:space:]]+[wxdsq][0-9]+, \[sp'
gshape "frame str [sp,#imm]"  '^[[:space:]]+str[a-z]*[[:space:]]+[wxdsq][0-9]+, \[sp'
gshape "mov x,x"              '^[[:space:]]+mov[[:space:]]+x[0-9]+, x[0-9]+$'
gshape "mov w,w"              '^[[:space:]]+mov[[:space:]]+w[0-9]+, w[0-9]+$'
