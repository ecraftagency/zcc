#!/bin/sh
# probe.sh — runner CHUẨN cho mọi probe/round trong box (SOP: "compile quy trình").
# Thi hành các luật bake cứng một lần, vĩnh viễn:
#   - deps ĐÃ BAKE vào image zcc-box (docker commit 18/8) — probe cấm apt-get
#   - log sống ngoài container ($SUITES/<tên>/, container --rm không mất gì)
#   - proof-of-run: script PHẢI để lại $LOG/SUMMARY.txt không rỗng, thiếu = BROKEN
# Dùng:  tests/probe.sh <script.sh> [tên-run]
#   Script chạy trong box với: $LOG=/suites/<tên> (đã mkdir), /build = build-elf,
#   /suites = cache suite, /probe = thư mục chứa script (RO), zcc tại /usr/local/bin/zcc.
SCRIPT=$1
[ -f "$SCRIPT" ] || { echo "probe.sh: không có script: $SCRIPT"; exit 2; }
NAME=${2:-$(basename "${SCRIPT%.sh}")-$(date +%Y%m%d-%H%M%S)}
SUITES=${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}
ZCC_BIN=${ZCC_ELF_BIN:-$(cd "$(dirname "$0")/.." && pwd)/target/aarch64-unknown-linux-musl/release/zcc}
[ -f "$ZCC_BIN" ] || { echo "probe.sh: không có zcc ELF: $ZCC_BIN (rebuild: CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld cargo build --release --target aarch64-unknown-linux-musl)"; exit 2; }
mkdir -p "$SUITES/$NAME"
SDIR=$(cd "$(dirname "$SCRIPT")" && pwd)
docker run --rm --name "zcc-$NAME" \
    -v "$ZCC_BIN":/usr/local/bin/zcc:ro \
    -v "$SUITES/build-elf":/build \
    -v "$SUITES":/suites \
    -v "$SDIR":/probe:ro \
    -e LOG="/suites/$NAME" \
    zcc-box sh -c "
        mkdir -p \"\$LOG\"
        sh /probe/$(basename "$SCRIPT")"
rc=$?
if [ ! -s "$SUITES/$NAME/SUMMARY.txt" ]; then
    echo "probe.sh: BROKEN — '$NAME' không để lại SUMMARY.txt (proof-of-run vỡ, rc=$rc)"
    exit 3
fi
echo "== $NAME (rc=$rc) SUMMARY:"
cat "$SUITES/$NAME/SUMMARY.txt"
exit $rc
