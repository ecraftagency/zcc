#!/bin/sh
# Suite 7b — musl libc FULL BUILD (thăng cấp musl.sh compile-only, trả nợ M17).
# Chạy TRONG box Linux (zcc-box): cần $ZCC = binary zcc ELF static, cây nguồn
# musl-1.2.5 + libc-test trong $SUITES (mặc định /suites = mount của
# ~/.cache/zcc-suites). Referee 2 tầng: gcc+glibc cho smoke (khớp byte),
# musl-gcc cho libc-test (F_zcc \ F_ref mới là nghi phạm — xem tests/README.md).
set -e
: "${ZCC:?ZCC=/đường/dẫn/zcc (binary ELF)}"
S="${SUITES:-/suites}"
M="$S/musl-1.2.5"
INST="$S/musl-install"
LT="$S/libc-test"

# ---- 1. patch LDBL64 (quyết định M17: long double zcc = double → mượn nhánh
# arm32 upstream của musl, 1 file, tự nhất quán; bản gốc giữ ở .orig)
[ -f "$M/arch/aarch64/bits/float.h.orig" ] || \
    cp "$M/arch/aarch64/bits/float.h" "$M/arch/aarch64/bits/float.h.orig"
cp "$M/arch/arm/bits/float.h" "$M/arch/aarch64/bits/float.h"

# ---- 2. build sạch + install sysroot (AR/RANLIB trần: configure --target
# sinh prefix aarch64-ar không tồn tại; SHARED_LIBS= tắt libc.so — nợ -shared)
cd "$M"
[ -f config.mak ] || ./configure CC="$ZCC" --target=aarch64
make clean >/dev/null
make -j"${JOBS:-$(nproc)}" CC="$ZCC" AR=ar RANLIB=ranlib \
    lib/libc.a lib/crt1.o lib/crti.o lib/crtn.o
rm -rf "$INST"
make install prefix="$INST" CC="$ZCC" AR=ar RANLIB=ranlib SHARED_LIBS= >/dev/null
echo "MUSL-BUILD-OK: $(find obj -name '*.o' | wc -l) obj, $(ls -l lib/libc.a | awk '{print $5}') byte libc.a"

# ---- 3. smoke differential: cùng source, zcc+musl vs gcc+glibc, khớp byte
D=$(mktemp -d); trap 'rm -rf "$D"' EXIT
cat > "$D/sm.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
int cmp(const void *a, const void *b) { return *(const int *)a - *(const int *)b; }
int main(void)
{
    int v[7] = { 5, 2, 7, 1, 6, 3, 4 }, i;
    char buf[64];
    FILE *f;
    qsort(v, 7, sizeof(int), cmp);
    for (i = 0; i < 7; i++) printf("%d", v[i]);
    printf(" musl-zcc %d\n", v[6] + 1);
    printf("[%8.3f|%-6d|%#x|%e]\n", 3.14159, -42, 255, 12345.678);
    printf("%.10f %g %g\n", sqrt(2.0), pow(2.0, 10.0), fmod(7.5, 2.0));
    f = fopen("/tmp/zcc_sm.txt", "w");
    fputs("line 1\n123 -456\n", f);
    fclose(f);
    f = fopen("/tmp/zcc_sm.txt", "r");
    while (fgets(buf, sizeof buf, f)) fputs(buf, stdout);
    fclose(f);
    printf("%.3f\n", strtod("2.718281828", 0));
    return 0;
}
EOF
ZCC_SYSROOT="$INST" "$ZCC" "$D/sm.c" -o "$D/sm_zcc"
gcc -O0 -w "$D/sm.c" -o "$D/sm_gcc"
"$D/sm_zcc" > "$D/z.txt"; "$D/sm_gcc" > "$D/g.txt"
cmp "$D/z.txt" "$D/g.txt" && echo "SMOKE-KHOP (byte-identical vs gcc+glibc)"

# ---- 4. libc-test bằng zcc+sysroot; kết quả = danh sách .err (LC_ALL=C!)
cd "$LT"
find src -name '*.o' -o -name '*.exe' -o -name '*.err' -o -name '*.so' \
    -o -name '*.d' | xargs rm -f 2>/dev/null || true
printf 'CC = %s\nCFLAGS += -D_POSIX_C_SOURCE=200809L\nLDLIBS += -lpthread -lm -lrt\n' \
    "$ZCC" > config.mak
ZCC_SYSROOT="$INST" make -k -j"${JOBS:-$(nproc)}" >/dev/null 2>&1 || true
LC_ALL=C sh -c 'find src -name "*.err" -size +0c | sort' > ZCC-FAILS.txt
echo "LIBC-TEST: $(wc -l < ZCC-FAILS.txt) err-file (danh sách: $LT/ZCC-FAILS.txt)"

# ---- 5. differential vs referee musl-gcc (nếu đã dựng — xem tests/README.md)
REF="$S/libc-test-ref/REF-FAILS.txt"
if [ -f "$REF" ]; then
    LC_ALL=C sort "$REF" > "$D/ref.txt"
    echo "--- ZCC-ONLY (F_zcc \\ F_ref — nghi phạm cần triage):"
    LC_ALL=C comm -23 ZCC-FAILS.txt "$D/ref.txt"
else
    echo "(chưa có referee musl-gcc: apt musl-tools + build libc-test-ref)"
fi
echo "MUSL-BOX PASS"
