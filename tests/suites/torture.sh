#!/bin/sh
# GCC c-torture/execute — the largest codegen-torture suite. Oracle: each test
# self-checks via abort(), exit 0 = pass. Referee = `cc -std=c99 -w -O0` (gcc in
# the box: an independent referee, native to its own suite).
#
# 3-WAY CLASSIFICATION (2-fact invariant) — each case gets exactly 1 label:
#   PASS     zcc compiles a valid C99 program → runs, exits 0 (matches oracle).
#   NOT-IMPL zcc REJECTS CLEANLY (construct not yet implemented) or the case is
#            outside C99 scope:
#              oracle-invalid  the c99 referee cannot compile it / does not run
#                              cleanly → UB or a gcc-ext outside strict C99.
#              zcc-reject      the referee is OK but zcc prints a `zcc:…`
#                              diagnostic then exits 1, produces NO binary and does
#                              NOT crash. Honest.
#   FAIL     zcc SWALLOWS a valid C99 program then miscompiles/CRASHES — count =0:
#              runtime   zcc produces a binary (exit 0) but the binary is
#                        wrong/aborts/crashes.
#              backend   zcc exits 1 WITHOUT a `zcc:` diagnostic → as/ld chokes on
#                        the junk asm zcc emitted (swallow-then-emit-junk, not a
#                        clean reject).
#              crash     zcc panic/signal (rc=101 or >128) — the compiler died.
#
# Gate = 0 FAIL. NOT-IMPL is permitted but the manifest must NAME the specific
# reason (torture.not-impl) for auditing. Cache clone: see tests/README.md.
set -e
cd "$(dirname "$0")/../.."
if [ -z "$ZCC" ]; then
    cargo build --quiet 2>/dev/null || cargo build
    export ZCC="$PWD/target/debug/zcc"
fi
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export DIR="$C/gcc/gcc/testsuite/gcc.c-torture/execute"
[ -d "$DIR" ] || { echo "cache not found: sparse-clone gcc per tests/README.md"; exit 2; }
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT
# gcc-ext manifest: cases where the gcc REFEREE runs semantics outside strict C99
# (multi-angle proof in the file — an independent clang oracle + C99). zcc
# conforming, gcc the outlier.
export OE="$(dirname "$0")/torture.oracle-ext"

# CONSERVATION LAW (against silent skipping): the list of loaded cases is written
# firmly to $D/fed; after the run, EACH case must appear EXACTLY once in res.
# pass+not-impl+fail must = |fed|. A mismatch → a case vanished (worker died, OOM,
# zcc hung) → the harness TURNS RED, allowing no false green. A green verdict is
# meaningless if the measurement dropped anything — a reviewer verifies 1 equation,
# without having to trust any narration.
ls "$DIR"/*.c | { [ -n "${SEEK:-}" ] && grep -F -- "$SEEK" || cat; } > "$D/fed"
# An empty feed is an ERROR, not a pass: a SEEK that matches nothing must never
# be reported as a suite that found no failures. See the note in cts.sh.
[ -s "$D/fed" ] || { echo "torture: SEEK='${SEEK:-}' matched 0 of $(ls "$DIR"/*.c | wc -l | tr -d ' ') cases — nothing was tested"; exit 2; }
nfed=$(wc -l < "$D/fed" | tr -d ' ')
# each worker prints 1 TSV line: <CLASS>\t<sub>\t<case>\t<reason>
xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(basename "$f" .c); e="$D/$b.err"
    # ── gcc-ext gate: cases PROVEN to make gcc an outlier outside strict C99
    # (multi-angle proof in torture.oracle-ext) → oracle-invalid, do NOT compare
    # zcc↔gcc (gcc is off-standard).
    if [ -f "$OE" ] && why=$(grep "^$b " "$OE" 2>/dev/null | head -1); [ -n "$why" ]; then
        printf "NOTIMPL\toracle-invalid\t%s\tgcc-ext: %s\n" "$b" "$(echo "$why" | sed "s/^$b  *//")"; exit 0
    fi
    # ── referee c99 (independent referee) ──
    # -w silences warnings but REAL errors still show: capture the reject reason so
    # the manifest can name it (a reviewer distinguishes gcc-ext/target-specific vs
    # a referee quirk).
    if ! cc -std=c99 -w -O0 "$f" -o "$D/$b.cc" 2>"$D/$b.ce"; then
        why=$(grep -m1 "error:" "$D/$b.ce" | sed "s|.*/execute/||;s|\t| |g")
        printf "NOTIMPL\toracle-invalid\t%s\treferee-reject: %s\n" "$b" "${why:-compile-fail}"; exit 0
    fi
    if ! perl -e "alarm 10; exec @ARGV" "$D/$b.cc" >/dev/null 2>&1; then
        printf "NOTIMPL\toracle-invalid\t%s\treferee-run-fail(UB?)\n" "$b"; exit 0
    fi
    # ── zcc ──
    "$ZCC" "$f" -o "$D/$b.z" 2>"$e"; zrc=$?
    # the manifest must be STABLE + reveal the right construct: strip the driver
    # "zcc: <path>: ", collapse every box-specific ".../execute/" path → leaving
    # only "<case>.c:<ln>: msg"
    r=$(sed -n "1p" "$e" 2>/dev/null | sed -e "s|^zcc: [^:]*: ||" -e "s|/[^ ]*/execute/||g" | tr "\t" " ")
    [ -n "$r" ] || r=$(head -1 "$e" 2>/dev/null | tr "\t" " ")
    if [ "$zrc" -eq 0 ]; then
        if perl -e "alarm 10; exec @ARGV" "$D/$b.z" >/dev/null 2>&1; then
            printf "PASS\t-\t%s\t-\n" "$b"
        else
            printf "FAIL\truntime\t%s\tzcc-binary-wrong/crash\n" "$b"
        fi
    elif [ "$zrc" -eq 1 ]; then
        if grep -q "^zcc:" "$e"; then
            printf "NOTIMPL\tzcc-reject\t%s\t%s\n" "$b" "${r:-reject}"
        else
            # backend: zcc exit1 WITHOUT a diagnostic → as/ld chokes. DISTINGUISH the
            # source mechanically (do not hardcode names): ld "undefined reference to
            # X" where X IS in the source BUT ABSENT from the referee asm ⇒ the
            # referee has DCE-removed X (dead code); zcc -O0 (by design: no optimization
            # pass) keeps the ref. A PURELY optimization divergence is outside zcc scope →
            # oracle-invalid, NOT a miscompile. Otherwise (X is a zcc-internal mangled
            # symbol, or X really is missing) = FAIL.
            syms=$(grep -oE "undefined reference to .[A-Za-z_][A-Za-z0-9_.]*" "$e" \
                   | sed -E "s/.*to .//" | sort -u)
            optdep=0
            if [ -n "$syms" ] && cc -std=c99 -w -O0 -S "$f" -o "$D/$b.rs" 2>/dev/null; then
                optdep=1
                for sy in $syms; do
                    grep -qw "$sy" "$f" || { optdep=0; break; }        # X must be a source symbol (not mangled)
                    grep -qw "$sy" "$D/$b.rs" && { optdep=0; break; }  # and ABSENT from the referee asm (DCE-removed)
                done
            fi
            if [ "$optdep" = 1 ]; then
                printf "NOTIMPL\toracle-invalid\t%s\topt-dependent: referee DCE [%s], zcc -O0 keeps ref\n" "$b" "$(echo $syms | tr "\n" " ")"
            else
                printf "FAIL\tbackend\t%s\t%s\n" "$b" "${r:-as/ld-choke}"
            fi
        fi
    else
        printf "FAIL\tcrash\t%s\trc=%s %s\n" "$b" "$zrc" "$r"
    fi
' sh < "$D/fed" > "$D/res"

sort "$D/res" > "$D/res.s"
p=$(  grep -c "^PASS"    "$D/res.s" || true)
oi=$( grep -c "	oracle-invalid	" "$D/res.s" || true)
zr=$( grep -c "	zcc-reject	"     "$D/res.s" || true)
ni=$((oi + zr))
k=$(  grep -c "^FAIL"    "$D/res.s" || true)

# ── CONSERVATION CHECK: each loaded case must be classified EXACTLY once ──
nout=$(wc -l < "$D/res.s" | tr -d ' ')
cut -f3 "$D/res.s" | sort > "$D/seen"          # case names that have a verdict
sed "s|.*/||;s|\.c$||" "$D/fed" | sort > "$D/want"   # case names that were loaded
miss=$(comm -23 "$D/want" "$D/seen")           # loaded but no verdict = vanished
dup=$(cut -f3 "$D/res.s" | sort | uniq -d)     # 1 case with ≥2 verdicts = noisy count
if [ "$nout" -ne "$nfed" ] || [ -n "$miss" ] || [ -n "$dup" ]; then
    echo "!! CONSERVATION BROKEN: loaded=$nfed, verdicts=$nout (pass=$p not-impl=$ni fail=$k)"
    [ -n "$miss" ] && { echo "   VANISHED (loaded, no verdict):"; echo "$miss" | head; }
    [ -n "$dup" ]  && { echo "   DUPLICATE (≥2 verdicts):"; echo "$dup" | head; }
    echo "TORTURE RED (measurement dropped/noisy — green verdict void until fixed)"; exit 1
fi
[ "$((p + ni + k))" -eq "$nfed" ] || { echo "!! p+ni+k != loaded"; exit 1; }

# NOT-IMPL manifest: <bucket> <case> <reason> — named, auditable.
grep "^NOTIMPL" "$D/res.s" | cut -f2- | sort > "$D/not-impl"
MF="${NOTIMPL_OUT:-$(dirname "$0")/torture.not-impl}"
cp "$D/not-impl" "$MF" 2>/dev/null || :   # box mount ro → print below instead of writing

# FAIL work-list (the one count that must reach 0)
grep "^FAIL" "$D/res.s" | cut -f2- > "$D/fails"
[ -n "${LIVE_FAILS_DIR:-}" ] && cp "$D/fails" "$LIVE_FAILS_DIR/torture.fails" 2>/dev/null; :

if [ "$k" -gt 0 ]; then
    echo "── FAIL work-list ($k) — zcc swallowed valid C99 then miscompiled/crashed:"
    cat "$D/fails"
fi
echo "── NOT-IMPL manifest (torture.not-impl):"
cat "$D/not-impl"
echo "torture: $p pass, $ni not-impl ($oi oracle-invalid + $zr zcc-reject), $k FAIL"
if [ "$k" -eq 0 ]; then
    # The COUNT is part of the verdict, not decoration. Guarding only against an
    # EMPTY feed is not enough: `SEEK=300` matches a handful of torture names, so
    # a three-case run printed the same "TORTURE PASS (0 FAIL)" as a 1471-case
    # one. A verdict that does not say how much it examined cannot be audited.
    echo "TORTURE PASS ($nfed cases, 0 FAIL — every non-pass is a named NOT-IMPL)"; exit 0
fi
echo "TORTURE RED ($nfed cases, $k FAIL to triage → clean reject or fix)"; exit 1
