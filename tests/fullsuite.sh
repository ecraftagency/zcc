#!/bin/sh
# fullsuite.sh — SOLE RUNNER, runs 100% INSIDE the ELF BOX (AUTHORITATIVE target:
# aarch64 Linux musl — fast because static-musl is nearly free; the mac is only an
# ad-hoc clang oracle, there is NO mac runner). On the mac it self-builds zcc-ELF
# (musl,release) + docker run zcc-box + re-invokes itself inside the box.
#
# Usage:  sh tests/fullsuite.sh [TARGET] [SEEK]
#   TARGET (default all) — SEEK reaches an individual LAYER, no full re-run needed:
#     all                              — sci + corpus + app
#     sci | corpus | app | base        — group (base = run.sh cases+ext, fast loop)
#     shape|cpp|decay|alg|abi          — 1 sci-gate (theorem verification)
#     cases|ext                        — 1 base differential
#     torture|cts                      — 1 corpus suite
#     musl                             — app libc
#   SEEK (optional) — a case-name substring: seek deep into an INDIVIDUAL UNIT of a
#     suite, e.g.:  fullsuite.sh torture pr22061   |   fullsuite.sh cases float
#     (applies to cases/ext + every corpus suite; internally-generated gate cases
#     do not yet accept SEEK.)
# COMPACT output: 1 line per stage; details -> /tmp/full-<name>.log in the container.
set -u
TARGET="${1:-all}"
SEEK="${2:-}"

# ======== INSIDE THE BOX: run the target ========
if [ "${ZCC_IN_BOX:-}" = 1 ]; then
    export ZCC=/usr/local/bin/zcc ZCC_SUITE_CACHE=/suites SEEK
    W=/work/zcc
    pass=0; fail=0; red=""
    stage() { n=$1; shift
      if "$@" >"/tmp/full-$n.log" 2>&1; then
        printf '  %-9s PASS  %s\n' "$n" "$(tail -1 "/tmp/full-$n.log")"; pass=$((pass+1))
      else
        printf '  %-9s RED   %s\n' "$n" "$(tail -1 "/tmp/full-$n.log")"; fail=$((fail+1)); red="$red $n"
      fi
    }
    gate()   { case $1 in
        shape|cpp|decay|alg|abi) stage "$1" sh "$W/tests/$1.sh" ;;
        *) echo "unknown gate: $1"; exit 2 ;; esac ; }
    suite()  { stage "$1" sh "$W/tests/suites/$1.sh" ; }
    base_c() { stage cases sh "$W/tests/run.sh" cases "$SEEK" ; }
    base_e() { stage ext   sh "$W/tests/run.sh" ext   "$SEEK" ; }
    run_sci()    { echo "-- SCI-GATE (theorem verification, structural exhaustion) --"
                   for g in shape cpp decay alg abi; do gate "$g"; done ; }
    run_base()   { echo "-- BASE (hand-written differential, fast loop) --"; base_c; base_e ; }
    run_corpus() { echo "-- CORPUS (practical corroboration, FAIL ⊆ known-fail) --"
                   base_c; base_e
                   for s in torture cts; do suite "$s"; done ; }
    run_app()    { echo "-- APP (libc = musl, real software for the minimal-distro) --"
                   stage musl sh "$W/tests/suites/musl-box.sh" ; }
    echo "== fullsuite (ELF box aarch64 — AUTHORITATIVE, target=$TARGET${SEEK:+ seek=$SEEK}) =="
    case "$TARGET" in
        all)                     run_sci; run_corpus; run_app ;;
        sci)                     run_sci ;;
        corpus)                  run_corpus ;;
        app)                     run_app ;;
        base)                    run_base ;;
        shape|cpp|decay|alg|abi) gate "$TARGET" ;;
        cases)                   base_c ;;
        ext)                     base_e ;;
        torture|cts)             suite "$TARGET" ;;
        musl)                    run_app ;;
        *) echo "unknown target: '$TARGET' (see the file header)"; exit 2 ;;
    esac
    echo "== $pass PASS / $fail RED =="
    [ -n "$red" ] && echo "RED:$red"
    [ "$fail" = 0 ]
    exit $?
fi

# ======== ON THE HOST (mac): build ELF + launch box ========
cd "$(dirname "$0")/.." || exit 1
command -v docker >/dev/null 2>&1 || { echo "docker not found"; exit 1; }
docker image inspect zcc-box >/dev/null 2>&1 || { echo "image zcc-box not found"; exit 1; }
echo "== build zcc-ELF (aarch64-unknown-linux-musl, release) ..."
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build -q --release --target aarch64-unknown-linux-musl \
    || { echo "ELF BUILD FAILED"; exit 1; }
ELF="$PWD/target/aarch64-unknown-linux-musl/release/zcc"
SUITES="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
[ -d "$SUITES" ] || { echo "suite cache not found: $SUITES"; exit 1; }
exec docker run --rm -e ZCC_IN_BOX=1 \
    -v "$ELF":/usr/local/bin/zcc:ro \
    -v "$SUITES":/suites \
    -v "$PWD":/work/zcc:ro \
    zcc-box sh /work/zcc/tests/fullsuite.sh "$TARGET" "$SEEK"
