#!/bin/sh
# quickapp.sh — a REAL application on the clock in 2-3 minutes.
#
# `realprog.sh` is the thorough version: 11 phases, peak RSS, compile time, both
# sides of the `bl`. This is the short one — build the sqlite CLI with each
# compiler and run ONE heavy workload (bulk insert, then a nested join) — for
# when you want an application number without waiting for the full spine.
#
# WHY AN APPLICATION AT ALL. The taxonomy suite is 18 timed kernels that fit in
# L1i, and it reports zcc at 0.9494x gcc -O1. The sqlite CLI reports 1.69-1.74x.
# Both are honest and they measure different things: instruction FOOTPRINT is a
# performance term in a 173k-instruction program with a 10.8k-instruction
# interpreter loop, and free in a kernel that fits in cache. When the two
# disagree, the application is the one a user feels.
#
# CLEAN-INPUT LAW. Both builds run the same SQL and their output is compared
# BEFORE any time is reported. A number from a binary that computed something
# else is not a measurement — and note that `realprog.sh`'s join phase currently
# DIVERGES on zcc (a pre-existing miscompile, present before 2026-08-27), so
# this script checks rather than assumes.
#
# Run in the box:  sh tests/bench/quickapp.sh
# THE REFEREE IS `gcc -O2`, and the level is a decision rather than a default.
# Real software is built at -O2: it is the level every distribution, every
# `./configure` and every `Makefile` reaches for, so it is the only level a claim
# about zcc's generated code can be read against without misleading someone. The
# project scored against -O1 until 2026-08-29 because -O1 is the fair comparison
# for a compiler with no loop or vector passes — that reasoning is sound and it
# answers a question about the COMPILER, not about the code a user would get.
# Both are available: `GCC_OPT=-O1 sh <this>` restores the old column.
#
set -u
GCCO="${GCC_OPT:--O2}"   # MEASURED M48 — the referee level; see the header
SQ="${SQLITE_DIR:-/suites/sqlite}"
ZCC="${ZCC:-/usr/local/bin/zcc}"
GCC="${GCC:-gcc}"
N="${N:-3}"
ROWS="${ROWS:-200000}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
CF="-w -DSQLITE_THREADSAFE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_DISABLE_LFS"

[ -f "$SQ/sqlite3.c" ] || { echo "no $SQ/sqlite3.c"; exit 2; }

echo "== building the sqlite CLI both ways =="
$GCC $GCCO $CF -I"$SQ" -o "$T/cli_gcc" "$SQ/shell.c" "$SQ/sqlite3.c" -lm 2>/dev/null || { echo "gcc build failed"; exit 2; }
$ZCC     $CF -I"$SQ" -o "$T/cli_zcc" "$SQ/shell.c" "$SQ/sqlite3.c" -lm 2>/dev/null || { echo "zcc build failed"; exit 2; }
echo "   gcc $(wc -c < "$T/cli_gcc") bytes   zcc $(wc -c < "$T/cli_zcc") bytes"

# Bulk insert, then a NESTED JOIN over the two tables — the shape that stresses
# the interpreter loop rather than the parser.
cat > "$T/w.sql" <<SQL
PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
CREATE TABLE a(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER);
CREATE TABLE b(id INTEGER PRIMARY KEY, k INTEGER, w INTEGER);
BEGIN;
WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i<$ROWS)
INSERT INTO a SELECT i, i%997, i*7 FROM s;
WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i<$ROWS)
INSERT INTO b SELECT i, i%991, i*13 FROM s;
COMMIT;
SELECT count(*), sum(a.v+b.w) FROM a JOIN b ON a.k=b.k WHERE a.id<b.id AND a.k<40;
SELECT count(*) FROM a JOIN b ON a.v=b.w;
SQL

run() { rm -f "$T/db"; "$1" "$T/db" < "$T/w.sql" 2>&1; }

echo "== correctness first =="
og=$(run "$T/cli_gcc"); oz=$(run "$T/cli_zcc")
if [ "$og" != "$oz" ]; then
    echo "   DIVERGE — not timing a binary that computes something else"
    echo "   gcc: $og"
    echo "   zcc: $oz"
    exit 1
fi
echo "   identical: $og"

echo "== $ROWS rows x2, bulk insert + nested join, best of $N =="
for c in cli_gcc cli_zcc; do
    best=999999
    i=0
    while [ "$i" -lt "$N" ]; do
        rm -f "$T/db"
        t0=$(date +%s%N); "$T/$c" "$T/db" < "$T/w.sql" >/dev/null 2>&1; t1=$(date +%s%N)
        d=$(( (t1 - t0) / 1000000 ))
        [ "$d" -lt "$best" ] && best=$d
        i=$((i + 1))
    done
    echo "$c: ${best}ms"
    eval "t_$c=$best"
done
awk "BEGIN{ printf \"zcc / gcc$GCCO = %.3fx\n\", $t_cli_zcc / $t_cli_gcc }"
