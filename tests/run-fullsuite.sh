#!/bin/sh
# ============================================================================
# tests/run-fullsuite.sh — HOST LAUNCHER cho tests/fullsuite.sh (chạy trên mac).
# Quy trình 1 lệnh (Vu 2026-08-20): compile zcc-ELF → prepare box → execute
# DETACHED. Chạy xong đi ngủ; mai đọc $LOG/SUMMARY.txt. KHÔNG cần recovery:
# mọi stage trong box đã bọc timeout + catch (xem fullsuite.sh điều kiện [3]).
#
# Dùng:  sh tests/run-fullsuite.sh [--jobs N] [--no-build] [--name TÊN]
#   --jobs N     mức song song (mặc định 2 — mát máy). 1 = tuần tự tuyệt đối.
#   --no-build   bỏ qua rebuild zcc-ELF (dùng binary hiện có).
#   --name TÊN   tên run (mặc định fullsuite-<timestamp>).
# ============================================================================
set -u

REPO=$(cd "$(dirname "$0")/.." && pwd)
JOBS=2
BUILD=1
NAME=""
while [ $# -gt 0 ]; do
    case "$1" in
        --jobs) JOBS=$2; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        --name) NAME=$2; shift 2 ;;
        *) echo "tham số lạ: $1"; exit 2 ;;
    esac
done

SUITES="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
ELF="$REPO/target/aarch64-unknown-linux-musl/release/zcc"
IMG=zcc-box
CONT=zcc-fullsuite
[ -n "$NAME" ] || NAME="fullsuite-$(date +%Y%m%d-%H%M%S)"
LOGDIR="$SUITES/$NAME"

# ---- tiền kiểm: mọi thứ phải sẵn TRƯỚC khi detach (không có recovery) ----
fail() { echo "run-fullsuite: $1" >&2; exit 1; }
command -v docker >/dev/null 2>&1 || fail "không có docker"
docker image inspect "$IMG" >/dev/null 2>&1 || fail "thiếu image '$IMG' (docker commit box)"
[ -d "$SUITES/build-elf" ] || fail "thiếu $SUITES/build-elf (volume làm việc)"
[ -d "$SUITES/git" ] && [ -d "$SUITES/nginx" ] && [ -d "$SUITES/gcc" ] \
    || fail "thiếu nguồn suite trong $SUITES (git/nginx/gcc)"
[ -d "$SUITES/build-elf/redis" ] || fail "thiếu $SUITES/build-elf/redis"

# ---- compile zcc-ELF (trừ khi --no-build) ----
if [ "$BUILD" = 1 ]; then
    echo "== build zcc-ELF (aarch64-unknown-linux-musl, release) ..."
    ( cd "$REPO" && CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
        cargo build --release --target aarch64-unknown-linux-musl ) \
        || fail "build zcc-ELF THẤT BẠI — dừng, KHÔNG launch"
fi
[ -f "$ELF" ] || fail "không có binary zcc-ELF: $ELF"
file "$ELF" | grep -q 'ARM aarch64' || fail "binary không phải ARM aarch64 ELF: $(file "$ELF")"

# ---- dọn container cũ cùng tên (nếu còn) ----
docker rm -f "$CONT" >/dev/null 2>&1 || true
mkdir -p "$LOGDIR"

# ---- LAUNCH DETACHED ----
CID=$(docker run -d --name "$CONT" \
    -v "$ELF":/usr/local/bin/zcc:ro \
    -v "$SUITES/build-elf":/build \
    -v "$SUITES":/suites \
    -v "$REPO":/work/zcc:ro \
    -e LOG="/suites/$NAME" -e JOBS="$JOBS" \
    "$IMG" sh -c 'mkdir -p "$LOG"; sh /work/zcc/tests/fullsuite.sh') \
    || fail "docker run thất bại"

cat <<INFO

== ĐÃ LAUNCH DETACHED ==
  container : $CONT ($(echo "$CID" | cut -c1-12))
  JOBS      : $JOBS
  log dir   : $LOGDIR   (sống ngoài container — an toàn khi --rm/crash)

Mai kiểm tra:
  cat $LOGDIR/SUMMARY.txt                 # bảng tier/trạng thái/giây
  ls  $LOGDIR/                            # log từng stage + triage-*/
  docker ps -a --filter name=$CONT        # còn chạy? exit code?
  docker logs -f $CONT                    # theo dõi trực tiếp (nếu muốn)

Dừng khẩn nếu cần:
  docker rm -f $CONT
INFO
