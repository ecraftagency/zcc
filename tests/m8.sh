#!/bin/sh
# M8 — kiểm chứng bắc cầu: zcc compile tcc (mob), tcc đó compile hello world
# (và tự compile chính nó), binary chạy đúng stdout + exit code.
# Dùng: tests/m8.sh [thư mục làm việc]  (mặc định: mktemp -d)
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

# 1. zcc compile tcc (tắt semlock/backtrace: né dispatch/ucontext ngoài scope)
"$ZCC" -Dinline=__inline -DONE_SOURCE=1 -DCONFIG_TCC_BACKTRACE=0 \
    -DCONFIG_TCC_SEMLOCK=0 -I. tcc.c -o ./tcc
./tcc -v

# 2. tcc (zcc-built) tự build runtime libtcc1.a của nó
make libtcc1.a > /dev/null 2>&1 || true
[ -f libtcc1.a ]

# 3. tcc compile hello world
cat > "$WORK/hello.c" <<'EOF'
#include <stdio.h>
int main(void) { printf("hello via zcc->tcc\n"); return 42; }
EOF
./tcc -B. -L"$SDK/usr/lib" "$WORK/hello.c" -o "$WORK/hello"
rc=0
"$WORK/hello" > "$WORK/hello.out" || rc=$?
cat "$WORK/hello.out"; echo "exit=$rc"
grep -q 'hello via zcc->tcc' "$WORK/hello.out" && [ "$rc" -eq 42 ]

# 4. vòng mạnh hơn: tcc tự compile tcc, tcc mới compile hello lần nữa
./tcc -B. -L"$SDK/usr/lib" -DONE_SOURCE=1 tcc.c -o "$WORK/tcc2"
"$WORK/tcc2" -B. -L"$SDK/usr/lib" "$WORK/hello.c" -o "$WORK/hello2"
rc2=0
"$WORK/hello2" > /dev/null || rc2=$?
[ "$rc2" -eq 42 ]
echo "M8 PASS: zcc -> tcc -> tcc -> hello (stdout + exit code đúng)"
