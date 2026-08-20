#!/bin/sh
# halfsuite.sh — HALF SUITE RUNNER: vòng fix correctness (mac, không docker).
# Scope Option-1 (Vu 2026-08-20): sci-gate + base differential + corpus
# compiler-suite. BỎ real-software (musl/redis/nginx/git/sqlite/tcc) — vô nghĩa
# khi corpus/gate chưa xanh; những thứ đó thuộc FULL SUITE RUNNER (fullsuite.sh,
# chạy trong docker ELF box). Half = xanh trước, rồi mới tới full.
# Resilient: 1 stage đỏ KHÔNG chặn stage sau. Fast: build 1 lần, chạy tuần tự.
# Live-fail worklist: LIVE_FAILS_DIR=<dir> để mỗi corpus ghi <name>.fails.
# Dùng:  tests/halfsuite.sh            (chạy hết, in SUMMARY)
#        LIVE_FAILS_DIR=/tmp/lf tests/halfsuite.sh    (kèm dump fail từng case)
set -u
cd "$(dirname "$0")/.." || exit 1
cargo build -q 2>/dev/null || { echo "BUILD ĐỎ — dừng"; exit 1; }
export ZCC="$PWD/target/debug/zcc"
[ -n "${LIVE_FAILS_DIR:-}" ] && mkdir -p "$LIVE_FAILS_DIR"

pass=0; fail=0; redlist=""
stage() {  # stage <nhãn> <lệnh...>
    name=$1; shift
    if ( "$@" ) >"/tmp/subset-$name.log" 2>&1; then
        printf '  %-22s PASS\n' "$name"; pass=$((pass+1))
    else
        printf '  %-22s ĐỎ   (log:/tmp/subset-%s.log — %s)\n' "$name" "$name" "$(tail -1 /tmp/subset-$name.log)"
        fail=$((fail+1)); redlist="$redlist $name"
    fi
}

echo "===== SUBSET (mac): sci-gate + base + corpus ====="
echo "-- sci-gate --"
stage gate-shape sh tests/shape.sh
stage gate-cpp   sh tests/cpp.sh
stage gate-alg   sh tests/alg.sh
stage gate-abi   sh tests/abi.sh
stage gate-decay sh tests/decay.sh
echo "-- base differential --"
stage base-cases sh tests/run.sh cases
stage base-ext   sh tests/run.sh ext
echo "-- corpus compiler-suite --"
stage torture  sh tests/suites/torture.sh
stage cts      sh tests/suites/cts.sh
stage chibicc  sh tests/suites/chibicc.sh
stage kr       sh tests/suites/kr.sh
stage nora     sh tests/suites/nora.sh

echo "===== SUMMARY: $pass PASS / $fail ĐỎ ====="
[ -n "$redlist" ] && echo "ĐỎ:$redlist"
[ "$fail" = 0 ]
