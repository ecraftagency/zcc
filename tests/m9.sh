#!/bin/sh
# M9 — driver đúng chuẩn cc: build tcc bằng CHÍNH Makefile gốc của nó
# (không ONE_SOURCE: 12 file .c compile riêng → ar libtcc.a → link -lm -ldl
# -lpthread). Chứng minh driver sống được với make thật: -c/-o từng file,
# nhận .a input, forward -l/-L cho ld.
# Dùng: tests/m9.sh [thư mục làm việc]  (mặc định: mktemp -d)
set -e
cd "$(dirname "$0")/.."
cargo build
ZCC="$PWD/target/debug/zcc"
WORK="${1:-$(mktemp -d)}"
SDK=$(xcrun -sdk macosx --show-sdk-path)

[ -d "$WORK/tcc" ] || git clone --depth 1 https://github.com/TinyCC/tinycc.git "$WORK/tcc"
cd "$WORK/tcc"
[ -f config.h ] || ./configure > /dev/null
[ -f tccdefs_.h ] || make tccdefs_.h > /dev/null

# 1. make với CC=zcc — Makefile gốc, chỉ tắt semlock/backtrace (dispatch/ucontext
# ngoài scope). KHÔNG cần -Dinline: parser coi inline là no-op từ M10.
make CC="$ZCC" CFLAGS="-DCONFIG_TCC_BACKTRACE=0 -DCONFIG_TCC_SEMLOCK=0" tcc
./tcc -v

# 2. tcc (zcc-built) build runtime rồi compile hello — chạy 3 lần (bẫy ASLR)
make libtcc1.a > /dev/null 2>&1 || true
[ -f libtcc1.a ]
cat > "$WORK/hello.c" <<'EOF'
#include <stdio.h>
int main(void) { printf("hello via makefile-tcc\n"); return 42; }
EOF
./tcc -B. -L"$SDK/usr/lib" "$WORK/hello.c" -o "$WORK/hello"
for i in 1 2 3; do
    rc=0
    "$WORK/hello" > "$WORK/hello.out" || rc=$?
    grep -q 'hello via makefile-tcc' "$WORK/hello.out"
    [ "$rc" -eq 42 ]
done
echo "M9 PASS: make CC=zcc (Makefile gốc tcc) -> tcc -> hello (3/3 lần đúng)"
