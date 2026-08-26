#!/bin/sh
# c-testsuite (single-exec) — a first-party oracle: each case ships a .expected
# file, pass = exit 0 + stdout matching byte-for-byte. No referee needed.
# Gate: the FAIL set ⊆ baseline cts.known-fail (triaged: outside C89 scope).
set -e
cd "$(dirname "$0")/../.."
# ZCC preset from the env (running in the Linux box, no cargo) — otherwise self-build
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    export ZCC="$PWD/target/debug/zcc"
fi
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/c-testsuite/tests/single-exec"
[ -d "$DIR" ] || { echo "cache not found: clone c-testsuite per tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

# A SEEK that matches NOTHING must be an ERROR, never a verdict on an empty set.
# `fullsuite.sh all 300` puts "300" in SEEK — the second positional is SEEK, not
# the fuzz count — and this suite then reported "0 pass, 1 fail" from a single
# empty xargs argument, which read as a RED gate for three runs in a row.
ls "$DIR"/*.c | { [ -n "${SEEK:-}" ] && grep -F -- "$SEEK" || cat; } > "$D/fed" || :
[ -s "$D/fed" ] || { echo "cts: SEEK='${SEEK:-}' matched 0 of $(ls "$DIR"/*.c | wc -l | tr -d ' ') cases — nothing was tested"; exit 2; }
xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(basename "$f" .c)
    # CWD = $D (writable): some cases write files to CWD (00187 fopen "fred.txt","w")
    # — running at the repo root would litter it (mac) or fail on a read-only mount
    # (box).
    if "$ZCC" "$f" -o "$D/$b" 2>/dev/null \
       && ( cd "$D" && perl -e "alarm 10; exec @ARGV" "./$b" ) > "$D/$b.out" 2>/dev/null \
       && cmp -s "$D/$b.out" "$f.expected"; then
        echo "pass $b"
    else
        echo "FAIL $b"
    fi
' sh < "$D/fed" > "$D/res"

p=$(grep -c '^pass' "$D/res" || true)
grep '^FAIL' "$D/res" | sort > "$D/fails"
[ -n "${LIVE_FAILS_DIR:-}" ] && cp "$D/fails" "$LIVE_FAILS_DIR/cts.fails" 2>/dev/null; :
sort "$(dirname "$0")/cts.known-fail" > "$D/known" 2>/dev/null || : > "$D/known"
new=$(comm -23 "$D/fails" "$D/known" || true)
echo "c-testsuite: $(wc -l < "$D/fed" | tr -d ' ') cases, $p pass, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "CTS NEW FAIL (outside baseline):"; echo "$new" | head -20; exit 1
fi
echo "CTS PASS ($(wc -l < "$D/fed" | tr -d ' ') cases, every fail is in the triaged baseline)"
