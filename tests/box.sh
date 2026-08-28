#!/bin/sh
# box.sh — ITERATION on zcc-ELF inside the docker box (the REAL target + 35× faster
# than the mac). macOS clang compile+exec is ~2.7s per run → torture on the mac
# ~19min; the static-musl box ~16s. Rebuild the ELF debug (~2s) then run inside an
# ephemeral container (--rm); the ELF is bind-mounted so it is always fresh, no
# docker cp. ELF-specific bugs (fall-off-main, float_h) reproduce ONLY here.
# Requires the 'zcc-box' image + the suite cache (see MECHANISM.md Part A).
# Usage:
#   tests/box.sh torture              # torture suite (16s) — ELF gate
#   tests/box.sh c FILE.c             # compile+run 1 file, print exit code + output
#   tests/box.sh s 'shell...'         # arbitrary shell; has $ZCC, /suites, /work/zcc
set -eu
cd "$(dirname "$0")/.."
# ZCC_REL=1 builds a RELEASE zcc — use it when RUNNING A SUITE (torture), where a
# release zcc compiles the corpus much faster (debug Rust is ~9x slower to
# compile the input; the gap is largest on big functions). Default stays debug
# for the fast ~2s rebuild that single-file `c FILE` / `s` iteration wants.
if [ -n "${ZCC_REL:-}" ]; then
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
        cargo build -q --release --target aarch64-unknown-linux-musl
    ELF="$PWD/target/aarch64-unknown-linux-musl/release/zcc"
else
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
        cargo build -q --target aarch64-unknown-linux-musl
    ELF="$PWD/target/aarch64-unknown-linux-musl/debug/zcc"
fi
SUITES="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
DR="docker run --rm -e ZCC=/usr/local/bin/zcc -e ZCC_SUITE_CACHE=/suites
    -v $ELF:/usr/local/bin/zcc:ro -v $SUITES:/suites -v $PWD:/work/zcc:ro zcc-box"
cmd=${1:-torture}; shift 2>/dev/null || true
case "$cmd" in
    torture) $DR sh /work/zcc/tests/suites/torture.sh ;;
    c) f=$1; $DR sh -c 'b=$(basename "'"$f"'"); cp "/work/zcc/'"$f"'" /tmp/x.c 2>/dev/null || cp "'"$f"'" /tmp/x.c;
         if zcc /tmp/x.c -o /tmp/x 2>/tmp/e; then /tmp/x; echo "[exit=$?]"; else echo "COMPILE-FAIL:"; cat /tmp/e; fi' ;;
    s) $DR sh -c "$1" ;;
    *) echo "box.sh: unknown command '$cmd' (torture|c FILE|s SHELL)"; exit 2 ;;
esac
