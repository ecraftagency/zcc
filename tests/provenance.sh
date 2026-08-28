#!/bin/sh
# provenance.sh — Law 1 and Law 3, made mechanical.
#
# Law 1 says every line of `src/` lies on exactly one side: an algorithm derived
# from a theorem, or a constant transcribed from a spec line. Law 3 says every
# pass ships the commuting square that certifies it. Both are claims ABOUT THE
# SOURCE, and until this script existed neither was checked — §17 carried eight
# ✔ marks that were measurably false, and `scev::compute_trips` was wrong for
# weeks under a green gate.
#
# THE THREE CHECKS, and what each can actually catch:
#
#   1. PROVENANCE. Every module cites the theorem sections it realizes; every
#      `const`/`static` cites the spec line or measured fact behind its value;
#      every pass cites its theorem. Every LOC is covered — by its module's
#      citation, unless it is a constant or a pass, which carry their own.
#      Catches: a file with no theorem at all; a constant with no provenance,
#      which is exactly Article E's "the spec's number, or my convenience's?".
#
#   2. SQUARE. Every pass names its commuting-square test, and that test exists.
#      Catches: a pass shipped without the proof Law 3 requires.
#
#   3. VACUITY. A named square must assert BOTH the equivalence and an EFFECT —
#      a count that drops, a mnemonic that appears, a branch that disappears.
#      Catches the failure mode that produced §17's false ✔ marks: a square
#      holds VACUOUSLY for a pass that never fires, so a test that only checks
#      `⟦f⟧ = ⟦P f⟧` stays green with nothing selected. This is the check with
#      teeth, and it is the reason the battery convention in this project is
#      "a square AND a count".
#
# THE CITATION IS ALSO THE READING PATH. zcc is a teaching compiler: a student
# who lands on any line should be able to read upward to the theorem it
# realizes. So a citation is written for a person, not for this script —
#
#     // src/mir/pass/ldstp.rs
#     // THEORY A6b  — MIR, the machine layer
#     // THEORY II-5 — DDI 0487 C6.2.130, the paired forms
#     // SQUARE      — adjacent_accesses_become_a_pair
#
# — and the script only checks that what a person reads is true.
#
# Run from the repo root:  sh tests/provenance.sh
set -u
cd "$(dirname "$0")/.."

DOCS="THEORY.md SEMANTICS.md MECHANISM.md"
BAD=$(mktemp)
note() { printf '  %s\n' "$1" >> "$BAD"; }

# ── the citable universe ───────────────────────────────────────────────────
# THEORY.md's Side-I sections (A1…A8, A6b, A7b, B, C, D) and Side-II sections
# (II-1…II-6); SEMANTICS.md's numbered sections; MECHANISM.md's M<n> entries.
ids=$(mktemp); trap 'rm -f "$ids" "$ids.used" "$ids.sq" "$BAD" "$squares"' EXIT
# THE FIRST TOKEN OF EVERY HEADING, and nothing cleverer. A pattern with `§` in
# it does not survive the box: `§` is two bytes, and GNU grep in the C locale
# will not match past it in that position, so `^#{2,4} (§?[0-9A-Za-z]+…)` found
# `§0` and `§0b` and MISSED `A1`…`A8` — the script passed on the host and went
# RED in the box with ten "no doc has A1" findings. Taking the first field is
# both simpler and dialect-proof.
{
    grep -ohE '^#+ [^ ]+' THEORY.md SEMANTICS.md MECHANISM.md | sed -E 's/^#+ //; s/\.$//'
} | sort -u > "$ids"

known() { grep -qxF "$1" "$ids"; }

# Every citation mentioned anywhere in src, so one cannot name a section that was
# renamed or deleted. A citation has a SHAPE — `THEORY A6b`, `THEORY II-5`,
# `MEASURED M3`, `SEMANTICS 5.5` — and prose that merely contains the word
# THEORY is not one; matching loosely reported "§7" and "IT." as broken
# citations on the first run.
: > "$ids.used"

# ── 1 + 2: the three kinds of anchor ───────────────────────────────────────
# `tests.rs` and `testutil.rs` are the PROOF, not the compiler: they are what
# the squares are written in, so they cite nothing and are exempt by name.
srcs=$(find src -name '*.rs' ! -name 'tests.rs' ! -name 'testutil.rs' | sort)
nmod=0; ncon=0; npass=0

for f in $srcs; do
    nmod=$((nmod + 1))
    # A module's citation lives in its header comment — the first block of `//`
    # lines, which is where a reader starts.
    head_block=$(awk 'NR<=40 && /^\/\//' "$f")
    case "$head_block" in
        *THEORY\ *|*MEASURED\ *) ;;
        *) note "$f: module header cites no THEORY/MEASURED section" ;;
    esac
    printf '%s\n' "$head_block" | grep -ohE '(THEORY (A[0-9]+b?|II-[0-9]+|[B-D])|MEASURED M[0-9]+|SEMANTICS [0-9]+(\.[0-9]+)*[a-z]?)' >> "$ids.used"
done

# A constant carries its own provenance: the doc comment or comment block
# immediately above it must cite. `awk` walks each file once, remembering the
# comment lines it has just seen.
for f in $srcs; do
    awk -v F="$f" '
        /^[[:space:]]*(\/\/|\/\*|\*)/ { blk = blk "\n" $0; next }
        /^[[:space:]]*$/              { next }
        # A `static X: OnceLock<bool>` is a memoized environment lookup — an
        # INSTRUMENT, not a value the compiler computes with. Law 1 is about the
        # latter, so these are exempt by shape rather than by a list of names.
        /^[[:space:]]*static [A-Z_0-9]+: *(std::sync::)?(OnceLock|AtomicBool)/ { blk=""; next }
        /^[[:space:]]*(pub )?(const|static) [A-Z]/ {
            name = $0
            sub(/^[[:space:]]*(pub )?(const|static) /, "", name)
            sub(/[:[:space:]].*$/, "", name)
            # A CONTIGUOUS RUN of constants shares the citation above it. The
            # twelve `TypeId`s in ast.rs are one table with one provenance, and
            # repeating the same line twelve times would be noise a reader
            # learns to skip — which is how a citation stops being read.
            cited = (blk ~ /THEORY [0-9A-Za-z§.-]/ || blk ~ /MEASURED M[0-9]/) || (run && blk == "")
            if (!cited) printf "%s: const %s has no THEORY/MEASURED citation\n", F, name
            run = cited
            print "CONST" > "/dev/stderr"
            blk = ""
            next
        }
        { blk = ""; run = 0 }
    ' "$f" 2>>"$ids.sq" | while read -r l; do note "$l"; done
    grep -ohE '(THEORY (A[0-9]+b?|II-[0-9]+|[B-D])|MEASURED M[0-9]+|SEMANTICS [0-9]+(\.[0-9]+)*[a-z]?)' "$f" >> "$ids.used"
done
ncon=$(grep -c CONST "$ids.sq" 2>/dev/null); ncon=${ncon:-0}

# A PASS is a `pub fn run` in hir/pass, mir/pass or a top-level pipeline stage.
# It must cite a theorem AND name its square.
squares=$(mktemp); : > "$squares"
for f in $(printf '%s\n' $srcs | grep -E 'pass/|regalloc/|isel/|hir/build|emit\.rs'); do
    awk -v F="$f" '
        /^[[:space:]]*(\/\/\/|\/\/|\/\*|\*)/ { blk = blk "\n" $0; next }
        /^[[:space:]]*$/                     { next }
        /^pub fn run\(/ {
            if (blk !~ /THEORY [0-9A-Za-z§.-]/)
                printf "MISS\t%s: `pub fn run` cites no THEORY section\n", F
            if (match(blk, /SQUARE[[:space:]]+[-—[:space:]]*[a-z_0-9]+/)) {
                s = substr(blk, RSTART, RLENGTH)
                sub(/^SQUARE[[:space:]]+[-—[:space:]]*/, "", s)
                printf "SQ\t%s\t%s\n", F, s
            } else {
                printf "MISS\t%s: `pub fn run` names no SQUARE\n", F
            }
        }
        { blk = "" }
    ' "$f" >> "$squares"
done
npass=$(grep -c '^SQ' "$squares" 2>/dev/null); npass=${npass:-0}
grep '^MISS' "$squares" | cut -f2- | while read -r l; do note "$l"; done

# ── 2: the named square exists ─────────────────────────────────────────────
# ── 3: and it is not vacuous ───────────────────────────────────────────────
grep '^SQ' "$squares" | while IFS="$(printf '\t')" read -r _ f s; do
    # The body is read from the battery files only, one at a time: concatenating
    # every source and taking the first match found a DIFFERENT function of the
    # same name in another file.
    body=$(for tf in $(find src -name 'tests.rs'); do
               awk -v s="fn $s(" 'index($0,s){on=1} on{print} on && /^}/{exit}' "$tf"
           done)
    if [ -z "$body" ]; then
        note "$f: SQUARE $s — no such test"
        continue
    fi
    # THE EQUIVALENCE HALF — one of the battery helpers, which run both
    # interpreters and compare, or `new_machine` doing it by hand.
    # …either by calling a battery helper, which runs both interpreters and
    # compares, or by naming the denotation itself: every hand-rolled square in
    # this codebase says `⟦f⟧ = ⟦P f⟧` in its own failure message, and a test
    # that never mentions ⟦·⟧ is not claiming to preserve a meaning.
    case "$body" in
        *square\(*|*square_all\(*|*same\(*|*same_all\(*|*equiv\(*|*equiv_all\(*|*new_machine\(*|*⟦*) ;;
        *) note "$f: SQUARE $s — asserts no equivalence: no battery helper and no ⟦·⟧" ;;
    esac
    # THE EFFECT HALF, and the rule is simpler than enumerating idioms. The
    # helpers assert the equivalence INTERNALLY, so a body that calls one and
    # asserts nothing else has checked exactly one thing: that the pass did not
    # break the program. That is precisely the vacuous square — it stays green
    # for a pass that never fires, which is how §17 acquired eight false ✔
    # marks. A real square also says what the pass DID: a count that drops, a
    # form that appears, a block that disappears. So: at least one assertion of
    # its own.
    if [ "$(printf '%s' "$body" | grep -c 'assert')" -eq 0 ]; then
        note "$f: SQUARE $s — no assertion of its own; a vacuous square stays green for a pass that never fires"
    fi
done

# ── every cited id exists ──────────────────────────────────────────────────
nused=0
for c in $(sed -E 's/^(THEORY|MEASURED|SEMANTICS) //' "$ids.used" | sort -u); do
    nused=$((nused + 1))
    known "$c" || note "citation names a section no doc has: $c"
done

nbad=$(wc -l < "$BAD" | tr -d ' ')
[ "$nbad" -gt 0 ] && { echo "-- findings --"; cat "$BAD"; }
echo "provenance: $nmod modules, $ncon constants, $npass passes; $nused distinct citations over $DOCS"
if [ "$nbad" = 0 ]; then
    echo "PROVENANCE PASS (every LOC in theory ∪ fact; every pass squared, none vacuous)"
    exit 0
fi
echo "PROVENANCE RED ($nbad findings)"
exit 1
