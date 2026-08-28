#!/bin/sh
# Suite 7b — musl libc FULL BUILD (an upgrade of the compile-only musl.sh).
# Runs INSIDE the Linux box (zcc-box): requires $ZCC = a static ELF zcc binary, the
# musl-1.2.5 source tree + libc-test in $SUITES (default /suites = a mount of
# ~/.cache/zcc-suites). Two-tier referee: gcc+glibc for the smoke test (byte match),
# musl-gcc for libc-test (only F_zcc \ F_ref are suspects — see MECHANISM.md Part A).
set -e
: "${ZCC:?ZCC=/path/to/zcc (ELF binary)}"
S="${SUITES:-/suites}"
M="$S/musl-1.2.5"
INST="$S/musl-install"
LT="$S/libc-test"

# ---- 1. patch LDBL64 (long double in zcc = double → borrow musl's upstream arm32
# branch, 1 file, self-consistent; the original is kept at .orig)
[ -f "$M/arch/aarch64/bits/float.h.orig" ] || \
    cp "$M/arch/aarch64/bits/float.h" "$M/arch/aarch64/bits/float.h.orig"
cp "$M/arch/arm/bits/float.h" "$M/arch/aarch64/bits/float.h"

# ---- 2. clean build + install sysroot (bare AR/RANLIB: configure --target
# generates the prefix aarch64-ar which does not exist; SHARED_LIBS= disables
# libc.so — outstanding -shared debt)
cd "$M"
[ -f config.mak ] || ./configure CC="$ZCC" --target=aarch64
make clean >/dev/null
make -j"${JOBS:-$(nproc)}" CC="$ZCC" AR=ar RANLIB=ranlib \
    lib/libc.a lib/crt1.o lib/crti.o lib/crtn.o
rm -rf "$INST"
make install prefix="$INST" CC="$ZCC" AR=ar RANLIB=ranlib SHARED_LIBS= >/dev/null
echo "MUSL-BUILD-OK: $(find obj -name '*.o' | wc -l) obj, $(ls -l lib/libc.a | awk '{print $5}') byte libc.a"

# ---- 3. smoke differential: same source, zcc+musl vs gcc+glibc, byte match
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
cmp "$D/z.txt" "$D/g.txt" && echo "SMOKE-MATCH (byte-identical vs gcc+glibc)"

# ---- 4. libc-test with zcc+sysroot; result = the list of .err files (LC_ALL=C!)
cd "$LT"
find src -name '*.o' -o -name '*.exe' -o -name '*.err' -o -name '*.so' \
    -o -name '*.d' | xargs rm -f 2>/dev/null || true
printf 'CC = %s\nCFLAGS += -D_POSIX_C_SOURCE=200809L\nLDLIBS += -lpthread -lm -lrt\n' \
    "$ZCC" > config.mak
ZCC_SYSROOT="$INST" make -k -j"${JOBS:-$(nproc)}" >/dev/null 2>&1 || true
LC_ALL=C sh -c 'find src -name "*.err" -size +0c | sort' > ZCC-FAILS.txt
# CLEAN-INPUT LAW (MECHANISM.md Part A). The verdict below used to be reached by
# counting FAILURES alone, and `make -k ... || true` swallows every error — so a
# build that produced NOTHING left zero `.err` files and this gate printed
# MUSL-BOX PASS. That is the exact shape Article E forbids: a green with no
# evidence that any work happened. The positive artifact count is that evidence.
# Measured 2026-08-29 on a passing run: 479 linked test binaries from 464 source
# files, 73 non-empty `.err`. The gate refuses at ZERO — the question it answers
# is "did the compiler build anything at all", which needs no threshold and so
# carries no unprovenanced constant.
BUILT=$(find src -name '*.exe' | wc -l | tr -d ' ')
echo "LIBC-TEST: $BUILT test binaries linked, $(wc -l < ZCC-FAILS.txt) err-file (list: $LT/ZCC-FAILS.txt)"
if [ "$BUILT" -eq 0 ]; then
    echo "MUSL-BOX FAIL: zcc linked NO libc-test binary — the build did not run, so the empty failure list proves nothing"
    exit 1
fi

# ---- 5. differential vs the musl-gcc referee (if already built — see MECHANISM.md Part A)
REF="$S/libc-test-ref/REF-FAILS.txt"
if [ -f "$REF" ]; then
    LC_ALL=C sort "$REF" > "$D/ref.txt"
    echo "--- ZCC-ONLY (F_zcc \\ F_ref — suspects to triage):"
    LC_ALL=C comm -23 ZCC-FAILS.txt "$D/ref.txt"
else
    echo "(no musl-gcc referee yet: apt musl-tools + build libc-test-ref)"
fi
# The verdict CARRIES ITS EVIDENCE, as every other gate's does (`torture ... 1694
# cases`, `cts ... 220 cases`): `fullsuite` prints only this last line, so a
# count that lives anywhere else is a count nobody reads.
echo "MUSL-BOX PASS ($BUILT test binaries linked from $(find src -name '*.c' | wc -l | tr -d ' ') sources, $(wc -l < ZCC-FAILS.txt) err-file)"
