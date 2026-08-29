#!/bin/sh
# fullsuite.sh — SOLE RUNNER, runs 100% INSIDE the ELF BOX (AUTHORITATIVE target:
# aarch64 Linux musl — fast because static-musl is nearly free; the mac is only an
# ad-hoc clang oracle, there is NO mac runner). On the mac it self-builds zcc-ELF
# (musl,release) + docker run zcc-box + re-invokes itself inside the box.
#
# Usage:  sh tests/fullsuite.sh [TARGET] [SEEK]
#   TARGET (default all) — SEEK reaches an individual LAYER, no full re-run needed:
#     all                              — sci + corpus + fuzz + app (THE gate)
#     sci | corpus | fuzz | app | base  — group (base = run.sh cases+ext, fast loop)
#     provenance | determ | optpar | csmith | yarpgen — one stage
#   FUZZ_N (default 300) sizes BOTH random generators; 1000 for a seal. It is an
#     ENV VAR, not a positional — `fullsuite.sh all 300` does NOT set it, it sets
#     SEEK=300, which filters every corpus suite down to case names containing
#     "300" and silently shrinks the gate. Write `FUZZ_N=1000 fullsuite.sh all`.
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
    # WHICH COMPILER IS ON TRIAL, and it is PRINTED rather than assumed.
    #
    # This line used to read `export ZCC=/usr/local/bin/zcc` unconditionally.
    # That is right in the docker box, where the path is a bind mount of the ELF
    # just built on the host. It is WRONG on the Graviton box, where it is a
    # symlink into a build tree the gate does not own — and on 2026-08-29 it
    # silently gated a compiler FOUR HOURS OLD while the tree under test carried
    # a lexer fix the binary had never seen. Fifteen stages reported PASS about
    # code none of them had run. That is the `trusting-trust` failure shape:
    # green while checking nothing.
    #
    # An explicitly-set ZCC now wins, provided it exists and is executable, and
    # the resolved path plus its mtime is printed at the top of every run so the
    # question "what did this gate actually test" has an answer in the log.
    if [ -n "${ZCC:-}" ] && [ -x "${ZCC:-}" ]; then :; else ZCC=/usr/local/bin/zcc; fi
    export ZCC ZCC_SUITE_CACHE=/suites SEEK
    W="${ZCC_WORK:-/work/zcc}"
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
    suite()  { stage "$1" sh "$W/tests/suites/$1.sh" ${SUITE_N:+"$SUITE_N"} ; }
    base_c() { stage cases sh "$W/tests/run.sh" cases "$SEEK" ; }
    base_e() { stage ext   sh "$W/tests/run.sh" ext   "$SEEK" ; }
    run_sci()    { echo "-- SCI-GATE (theorem verification, structural exhaustion) --"
                   stage provenance sh "$W/tests/provenance.sh"
                   stage ubscan sh "$W/tests/ubscan.sh"
                   for g in shape cpp decay alg abi; do gate "$g"; done
                   stage determ sh "$W/tests/determinism.sh" ; }
    run_base()   { echo "-- BASE (hand-written differential, fast loop) --"; base_c; base_e ; }
    run_corpus() { echo "-- CORPUS (practical corroboration, FAIL ⊆ known-fail) --"
                   base_c; base_e
                   for s in torture cts; do suite "$s"; done
                   stage optpar sh "$W/tests/opt-parity.sh" ; }
    # The two random differential generators. They are the slowest stages and the
    # only ones whose sample size is a dial, so they take one: FUZZ_N (default 300)
    # for iteration, 1000 for a seal. Both are `xargs -P` internally.
    run_fuzz()   { echo "-- FUZZ (random differential; FUZZ_N=${FUZZ_N:-300} per generator) --"
                   stage csmith  sh "$W/tests/suites/csmith.sh"  "${FUZZ_N:-300}"
                   stage yarpgen sh "$W/tests/suites/yarpgen.sh" "${FUZZ_N:-300}" ; }
    run_app()    { echo "-- APP (libc = musl, real software for the minimal-distro) --"
                   stage musl sh "$W/tests/suites/musl-box.sh" ; }
    echo "== fullsuite (ELF box aarch64 — AUTHORITATIVE, target=$TARGET${SEEK:+ seek=$SEEK}) =="
    # A gate that does not name what it gated cannot be trusted the next morning.
    echo "   compiler on trial: $(readlink -f "$ZCC" 2>/dev/null || echo "$ZCC")  built $(date -r "$ZCC" '+%Y-%m-%d %H:%M' 2>/dev/null || echo '?')"
    case "$TARGET" in
        all)                     run_sci; run_corpus; run_fuzz; run_app ;;
        sci)                     run_sci ;;
        corpus)                  run_corpus ;;
        fuzz)                    run_fuzz ;;
        app)                     run_app ;;
        base)                    run_base ;;
        shape|cpp|decay|alg|abi) gate "$TARGET" ;;
        determ)                  stage determ sh "$W/tests/determinism.sh" ;;
        provenance)              stage provenance sh "$W/tests/provenance.sh" ;;
        optpar)                  stage optpar sh "$W/tests/opt-parity.sh" ;;
        cases)                   base_c ;;
        ext)                     base_e ;;
        torture|cts)             suite "$TARGET" ;;
        csmith|yarpgen)          stage "$TARGET" sh "$W/tests/suites/$TARGET.sh" "${FUZZ_N:-300}" ;;
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
# `docker image inspect` is the obvious probe and it FAILS on this machine while
# `docker run zcc-box` works — Desktop's CLI plugin answers "No such image" for a
# bare repository name that `docker images` lists. A gate that cannot start is
# indistinguishable from a gate that found nothing, so the probe is the listing.
[ -n "$(docker image ls -q zcc-box 2>/dev/null)" ] || { echo "image zcc-box not found"; exit 1; }
echo "== build zcc-ELF (aarch64-unknown-linux-musl, release) ..."
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build -q --release --target aarch64-unknown-linux-musl \
    || { echo "ELF BUILD FAILED"; exit 1; }
ELF="$PWD/target/aarch64-unknown-linux-musl/release/zcc"
SUITES="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
[ -d "$SUITES" ] || { echo "suite cache not found: $SUITES"; exit 1; }
# EVERY ZCC_* THE HOST SET GOES IN WITH IT. A row shipped behind a toggle is
# tested by the gate only if the gate can SEE the toggle: without this, running
# `ZCC_VRP=1 fullsuite.sh all` builds the compiler with the row present, runs it
# with the row off, and reports green for something it never exercised. FUZZ_N
# is forwarded for the same reason — a seal asked for on the host that arrives
# inside as the default 300 is a seal in name only.
ENVS=""
for v in $(env | sed -n 's/^\(ZCC_[A-Z_0-9]*\)=.*/\1/p'); do
    [ "$v" = ZCC_IN_BOX ] && continue
    [ "$v" = ZCC_SUITE_CACHE ] && continue
    ENVS="$ENVS -e $v"
done
[ -n "${FUZZ_N:-}" ] && ENVS="$ENVS -e FUZZ_N"
# shellcheck disable=SC2086
exec docker run --rm -e ZCC_IN_BOX=1 $ENVS \
    -v "$ELF":/usr/local/bin/zcc:ro \
    -v "$SUITES":/suites \
    -v "$PWD":/work/zcc:ro \
    zcc-box sh /work/zcc/tests/fullsuite.sh "$TARGET" "$SEEK"
