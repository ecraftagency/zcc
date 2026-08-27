#!/bin/sh
# xray.sh — PER-FUNCTION X-RAY of ONE translation unit (sqlite by default): which functions are fat, and what they are fat WITH.
#
# WHY IT PARSES `.s` AND NOT ELF. We are the compiler's author, so we have `-S`
# from both sides. Assembly TEXT carries the function boundaries, the labels and
# the mnemonics already; disassembling an ELF would spend a dependency
# (`Article A`: zero external crates) to recover information we never lost.
#
# TWO MODES, and they are the two questions in order:
#
#   sh tests/bench/xray.sh                  WHICH function is fat
#       every function present in both builds, ranked by zcc/gcc instruction
#       ratio, with the absolute excess beside it — a 3x ratio on a 12-instruction
#       function is noise, a 1.8x on a 10,000-instruction one is the campaign.
#
#   sh tests/bench/xray.sh <function>       WHAT it is fat with
#       the mnemonic histogram of that one function, both compilers, ranked by
#       difference. This is the line that names the next hack: `str` +110 means
#       spill traffic, `mov` +280 means marshalling, `ldp` -300 means pairing.
#
# NOT perfn.sh, which is a different instrument: that one walks the 42 suite
# programs with a correctness gate and ranks (program, function) pairs. This one
# takes a SINGLE large translation unit — the case perfn.sh cannot serve, because
# sqlite is one file with 1,260 functions and no suite to iterate.
#
# ⚠️ DIALECT. The two assemblers do not spell the same instruction the same way
# (`movz`/`mov #imm`, `b.ne`/`bne`), so the histogram folds the known synonyms —
# uncombined they read as excess that is not there (`excess.sh` records the same
# trap file-wide).
set -u
SQ="${SQLITE_DIR:-/suites/sqlite}"
SRC="${SRC:-$SQ/sqlite3.c}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

"$ZCC" -S -o "$T/z.s" "$SRC" 2>/dev/null || { echo "zcc failed"; exit 1; }
"$GCC" -O1 -w -S -o "$T/g.s" "$SRC" 2>/dev/null || { echo "gcc failed"; exit 1; }

# name<TAB>mnemonic, one line per instruction, for whichever compiler's syntax
# A label starts a new function only if it can BE one. zcc emits C `goto`
# targets as top-level symbols — `lg_sqlite3VdbeExec.next_tail:` — and a C
# identifier cannot contain a dot, so the dot is the discriminator. Without it
# `sqlite3VdbeExec` reports 4,214 instructions instead of 10,763, its body split
# across its own labels, and the fattest function in the program vanishes from
# the ranking entirely. (gcc's `.L` labels are already excluded by the leading
# character.)
split() { awk '
  /^[A-Za-z_][A-Za-z0-9_$]*:[ \t]*$/ { f=substr($0,1,index($0,":")-1); next }
  /^[ \t]+[a-z]/ {
      if (f=="") next
      m=$1
      sub(/^[ \t]+/,"",m)
      # fold the dialects: a constant materialization and a conditional branch
      if (m=="movz"||m=="movn"||m=="movk") m="mov#imm"
      if (m ~ /^b\./) { sub(/^b\./,"b",m) }
      print f "\t" m
  }' "$1"
}
split "$T/z.s" > "$T/z.fm"
split "$T/g.s" > "$T/g.fm"

if [ $# -eq 0 ]; then
    awk -F'\t' '{c[$1]++} END{for(k in c) print k"\t"c[k]}' "$T/z.fm" | LC_ALL=C sort > "$T/zc"
    awk -F'\t' '{c[$1]++} END{for(k in c) print k"\t"c[k]}' "$T/g.fm" | LC_ALL=C sort > "$T/gc"
    LC_ALL=C join -t'	' "$T/zc" "$T/gc" \
      | awk -F'\t' '$3>0 {printf "%.3f\t%d\t%s\t%s\t%s\n", $2/$3, $2-$3, $1, $2, $3}' \
      | sort -rn -k2 \
      | awk -F'\t' 'BEGIN{printf "%-34s %8s %8s %8s %8s\n","function","zcc","gcc","excess","ratio"}
                    NR<=25{printf "%-34s %8s %8s %8d %8s\n",$3,$4,$5,$2,$1}'
    echo "---"
    LC_ALL=C join -t'	' "$T/zc" "$T/gc" | awk -F'\t' '{z+=$2; g+=$3} END{
        printf "functions in both: %d   zcc %d   gcc %d   excess %d   ratio %.4f\n", NR, z, g, z-g, z/g}'
    exit 0
fi

F=$1
echo "== $F — mnemonic histogram, ranked by difference"
awk -F'\t' -v f="$F" '$1==f {c[$2]++} END{for(k in c) print k"\t"c[k]}' "$T/z.fm" | LC_ALL=C sort > "$T/zm"
awk -F'\t' -v f="$F" '$1==f {c[$2]++} END{for(k in c) print k"\t"c[k]}' "$T/g.fm" | LC_ALL=C sort > "$T/gm"
[ -s "$T/zm" ] || { echo "no such function in the zcc build"; exit 1; }
LC_ALL=C join -t'	' -a1 -a2 -e 0 -o 0,1.2,2.2 "$T/zm" "$T/gm" \
  | awk -F'\t' 'BEGIN{printf "%-12s %8s %8s %9s\n","mnemonic","zcc","gcc","diff"}
                {printf "%-12s %8d %8d %+9d\n", $1,$2,$3,$2-$3}' \
  | sort -k4 -rn
echo "---"
printf "total   zcc=%s gcc=%s\n" \
  "$(awk -F'\t' -v f="$F" '$1==f' "$T/z.fm" | wc -l)" \
  "$(awk -F'\t' -v f="$F" '$1==f' "$T/g.fm" | wc -l)"
