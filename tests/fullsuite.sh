#!/bin/sh
# fullsuite.sh — RUNNER DUY NHẤT, chạy 100% TRONG BOX ELF (target AUTHORITATIVE:
# aarch64 Linux musl — nhanh vì static-musl gần free; mac chỉ để clang làm oracle
# ad-hoc, KHÔNG còn runner mac). Trên mac tự build zcc-ELF (musl,release) + docker
# run zcc-box + gọi lại chính nó trong box.
#
# Dùng:  sh tests/fullsuite.sh [TARGET] [SEEK]
#   TARGET (mặc định all) — SEEK đến từng TẦNG, không cần chạy lại toàn bộ:
#     all                              — sci + corpus + app
#     sci | corpus | app | base        — nhóm  (base = run.sh cases+ext, vòng nhanh)
#     shape|cpp|decay|alg|abi          — 1 sci-gate (kiểm định lý)
#     cases|ext                        — 1 base differential
#     torture|cts|chibicc|kr|nora|tcc  — 1 corpus suite
#     musl                             — app libc
#   SEEK (tùy chọn) — chuỗi con tên case: SEEK sâu vào TỪNG UNIT trong 1 suite,
#     vd:  fullsuite.sh kr getint   |   fullsuite.sh cases float
#     (áp cho cases/ext + mọi corpus suite; gate sinh case nội bộ chưa nhận SEEK.)
# Output GỌN: 1 dòng/stage; chi tiết -> /tmp/full-<name>.log trong container.
set -u
TARGET="${1:-all}"
SEEK="${2:-}"

# ======== TRONG BOX: chạy target ========
if [ "${ZCC_IN_BOX:-}" = 1 ]; then
    export ZCC=/usr/local/bin/zcc ZCC_SUITE_CACHE=/suites SEEK
    W=/work/zcc
    pass=0; fail=0; red=""
    stage() { n=$1; shift
      if "$@" >"/tmp/full-$n.log" 2>&1; then
        printf '  %-9s PASS  %s\n' "$n" "$(tail -1 "/tmp/full-$n.log")"; pass=$((pass+1))
      else
        printf '  %-9s ĐỎ    %s\n' "$n" "$(tail -1 "/tmp/full-$n.log")"; fail=$((fail+1)); red="$red $n"
      fi
    }
    gate()   { case $1 in
        shape|cpp|decay|alg|abi) stage "$1" sh "$W/tests/$1.sh" ;;
        *) echo "gate lạ: $1"; exit 2 ;; esac ; }
    suite()  { stage "$1" sh "$W/tests/suites/$1.sh" ; }
    base_c() { stage cases sh "$W/tests/run.sh" cases "$SEEK" ; }
    base_e() { stage ext   sh "$W/tests/run.sh" ext   "$SEEK" ; }
    run_sci()    { echo "-- SCI-GATE (kiểm định lý, vét cạn cấu trúc) --"
                   for g in shape cpp decay alg abi; do gate "$g"; done ; }
    run_base()   { echo "-- BASE (differential viết tay, vòng nhanh) --"; base_c; base_e ; }
    run_corpus() { echo "-- CORPUS (chứng nghiệm thực tiễn, FAIL ⊆ known-fail) --"
                   base_c; base_e
                   for s in torture cts chibicc kr nora tcc; do suite "$s"; done ; }
    run_app()    { echo "-- APP (libc = musl, phần mềm thật cho minimal-distro) --"
                   stage musl sh "$W/tests/suites/musl-box.sh" ; }
    echo "== fullsuite (box ELF aarch64 — AUTHORITATIVE, target=$TARGET${SEEK:+ seek=$SEEK}) =="
    case "$TARGET" in
        all)                     run_sci; run_corpus; run_app ;;
        sci)                     run_sci ;;
        corpus)                  run_corpus ;;
        app)                     run_app ;;
        base)                    run_base ;;
        shape|cpp|decay|alg|abi) gate "$TARGET" ;;
        cases)                   base_c ;;
        ext)                     base_e ;;
        torture|cts|chibicc|kr|nora|tcc) suite "$TARGET" ;;
        musl)                    run_app ;;
        *) echo "target lạ: '$TARGET' (xem đầu file)"; exit 2 ;;
    esac
    echo "== $pass PASS / $fail ĐỎ =="
    [ -n "$red" ] && echo "ĐỎ:$red"
    [ "$fail" = 0 ]
    exit $?
fi

# ======== TRÊN HOST (mac): build ELF + launch box ========
cd "$(dirname "$0")/.." || exit 1
command -v docker >/dev/null 2>&1 || { echo "thiếu docker"; exit 1; }
docker image inspect zcc-box >/dev/null 2>&1 || { echo "thiếu image zcc-box"; exit 1; }
echo "== build zcc-ELF (aarch64-unknown-linux-musl, release) ..."
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build -q --release --target aarch64-unknown-linux-musl \
    || { echo "BUILD ELF ĐỎ"; exit 1; }
ELF="$PWD/target/aarch64-unknown-linux-musl/release/zcc"
SUITES="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
[ -d "$SUITES" ] || { echo "thiếu cache suite: $SUITES"; exit 1; }
exec docker run --rm -e ZCC_IN_BOX=1 \
    -v "$ELF":/usr/local/bin/zcc:ro \
    -v "$SUITES":/suites \
    -v "$PWD":/work/zcc:ro \
    zcc-box sh /work/zcc/tests/fullsuite.sh "$TARGET" "$SEEK"
