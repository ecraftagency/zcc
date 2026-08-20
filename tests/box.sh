#!/bin/sh
# box.sh — ITERATION zcc-ELF trong docker box (target THẬT + nhanh 35× so mac).
# macOS clang compile+exec ~2.7s/lần → torture mac ~19ph; box static-musl ~16s.
# Rebuild ELF debug (~2s) rồi chạy trong container ephemeral (--rm), ELF bind-mount
# nên luôn tươi, không docker cp. Bug ELF-specific (fall-off-main, float_h) CHỈ
# tái hiện ở đây. Cần image 'zcc-box' + cache suite (xem tests/README.md).
# Dùng:
#   tests/box.sh torture              # suite torture (16s) — gate ELF
#   tests/box.sh c FILE.c             # compile+run 1 file, in exit code + output
#   tests/box.sh s 'shell...'         # shell tùy ý; có $ZCC, /suites, /work/zcc
set -eu
cd "$(dirname "$0")/.."
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build -q --target aarch64-unknown-linux-musl
ELF="$PWD/target/aarch64-unknown-linux-musl/debug/zcc"
SUITES="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
DR="docker run --rm -e ZCC=/usr/local/bin/zcc -e ZCC_SUITE_CACHE=/suites
    -v $ELF:/usr/local/bin/zcc:ro -v $SUITES:/suites -v $PWD:/work/zcc:ro zcc-box"
cmd=${1:-torture}; shift 2>/dev/null || true
case "$cmd" in
    torture) $DR sh /work/zcc/tests/suites/torture.sh ;;
    c) f=$1; $DR sh -c 'b=$(basename "'"$f"'"); cp "/work/zcc/'"$f"'" /tmp/x.c 2>/dev/null || cp "'"$f"'" /tmp/x.c;
         if zcc /tmp/x.c -o /tmp/x 2>/tmp/e; then /tmp/x; echo "[exit=$?]"; else echo "COMPILE-FAIL:"; cat /tmp/e; fi' ;;
    s) $DR sh -c "$1" ;;
    *) echo "box.sh: lệnh lạ '$cmd' (torture|c FILE|s SHELL)"; exit 2 ;;
esac
