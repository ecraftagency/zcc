#!/bin/sh
# M14 — redis: clone source thật, make CC=zcc MALLOC=libc (kéo theo vendored
# deps: lua, hiredis, hdr_histogram, fpconv, xxhash, tre, linenoise — tất cả
# compile bằng zcc), rồi chạy thật: redis-server + redis-cli PING/SET/GET/INCR.
# Build đích danh 3 binary chính: target `all` còn kéo tests/modules vốn
# hardcode đường dẫn SDK CommandLineTools không tồn tại trên máy cài Xcode
# (tests/modules/Makefile gọi ld thẳng, không qua CC — không phải việc của zcc).
# MALLOC=libc: né jemalloc (đòi __atomic_fetch_or/and/xor — chưa ai khác đòi).
# Port 8972: kề port m13, cùng dải ít đụng.
set -e
cd "$(dirname "$0")/.."
cargo build
ZCC="$PWD/target/debug/zcc"
WORK="${1:-$(mktemp -d)}"

if [ ! -d "$WORK/redis" ]; then
    git clone --depth 1 https://github.com/redis/redis.git "$WORK/redis"
fi
cd "$WORK/redis"
make -C deps CC="$ZCC" hiredis lua hdr_histogram fpconv xxhash tre linenoise \
    > /dev/null 2>&1
make -C src CC="$ZCC" MALLOC=libc -j8 redis-server redis-cli redis-benchmark \
    > /dev/null

src/redis-server --port 8972 --save '' --appendonly no --daemonize no \
    --logfile "$WORK/redis.log" &
trap 'src/redis-cli -p 8972 SHUTDOWN NOSAVE 2>/dev/null || true' EXIT
sleep 2

[ "$(src/redis-cli -p 8972 PING)" = "PONG" ]
[ "$(src/redis-cli -p 8972 SET zcc_key gia_tri_tu_zcc)" = "OK" ]
[ "$(src/redis-cli -p 8972 GET zcc_key)" = "gia_tri_tu_zcc" ]
[ "$(src/redis-cli -p 8972 INCR dem)" = "1" ]
[ "$(src/redis-cli -p 8972 INCR dem)" = "2" ]

echo "M14 PASS: redis build bằng zcc (server+cli+benchmark, deps vendored) — PING/SET/GET/INCR đúng"
