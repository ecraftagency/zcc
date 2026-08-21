#!/bin/sh
# Suite 7 — musl libc, COMPILE-ONLY differential (musl = the Linux libc: syscall +
# ELF, not runnable on Darwin — a full run awaits the ELF target; the charter
# forbids designing ahead). The gate measures: the zcc frontend accepts musl
# source (parse/typecheck/codegen to .s) for every file the referee accepts.
# Referee: clang -target aarch64-linux-musl (Mach-O lacks the alias attribute that
# musl's weak_alias requires — clang's ELF target must be borrowed; same LP64).
# Gate: the FAIL set ⊆ baseline musl.known-fail.
set -e
cd "$(dirname "$0")/../.."
cargo build --quiet 2>/dev/null || cargo build
export ZCC="$PWD/target/debug/zcc"
C="${ZCC_SUITE_CACHE:-$HOME/.cache/zcc-suites}"
export SRC="$C/musl"
[ -d "$SRC" ] || git clone --depth 1 https://git.musl-libc.org/git/musl "$SRC"
export D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

# 3 generated headers (replacing configure — extracted from the musl Makefile, ARCH=aarch64)
mkdir -p "$D/obj/include/bits" "$D/obj/src/internal"
sed -f "$SRC/tools/mkalltypes.sed" "$SRC/arch/aarch64/bits/alltypes.h.in" \
    "$SRC/include/alltypes.h.in" > "$D/obj/include/bits/alltypes.h"
{ cat "$SRC/arch/aarch64/bits/syscall.h.in"
  sed -n -e 's/__NR_/SYS_/p' < "$SRC/arch/aarch64/bits/syscall.h.in"
} > "$D/obj/include/bits/syscall.h"
printf '#define VERSION "%s"\n' "$(cat "$SRC/VERSION")" > "$D/obj/src/internal/version.h"

export INC="-I$SRC/arch/aarch64 -I$SRC/arch/generic -I$D/obj/src/internal \
-I$SRC/src/include -I$SRC/src/internal -I$D/obj/include -I$SRC/include"

find "$SRC/src" -name '*.c' | xargs -n 1 -P 8 sh -c '
    f="$1"; b=$(echo "$f" | sed "s|.*/src/||; s|/|_|g; s|\.c$||")
    cc -target aarch64-unknown-linux-musl -std=c99 -nostdinc -D_XOPEN_SOURCE=700 \
        $INC -w -c "$f" -o "$D/$b.co" 2>/dev/null || { echo "skip $b"; exit 0; }
    if "$ZCC" -std=c99 -nostdinc -D_XOPEN_SOURCE=700 $INC -c "$f" -o "$D/$b.zo" 2>/dev/null; then
        echo "pass $b"
    else
        echo "FAIL $b"
    fi
' sh > "$D/res"

p=$(grep -c '^pass' "$D/res" || true); s=$(grep -c '^skip' "$D/res" || true)
grep '^FAIL' "$D/res" | sort > "$D/fails"
sort "$(dirname "$0")/musl.known-fail" > "$D/known" 2>/dev/null || : > "$D/known"
new=$(comm -23 "$D/fails" "$D/known" || true)
echo "musl (compile-only): $p pass, $s skip, $(wc -l < "$D/fails" | tr -d ' ') fail"
if [ -n "$new" ]; then
    echo "MUSL NEW FAIL (outside baseline):"; echo "$new" | head -20; exit 1
fi
echo "MUSL PASS (every fail is in the triaged baseline)"
