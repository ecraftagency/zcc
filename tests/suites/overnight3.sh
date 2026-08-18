#!/bin/sh
# Gói tổng duyệt ELF 2026-08-18 — chạy TRONG zcc-box (arm64 Linux).
# Sau đợt fix 18/8: __gnuc_va_list, param-array-size, bảng nhúng ELF→glibc,
# octal pp-number, __USER_LABEL_PREFIX__, va_arg HFA (AAPCS C.3), aligned(n)
# hậu declarator. Baseline corpus đã triage bổ sung (xem tests/README.md).
# VERBOSE: giữ trọn make output, config.log, và dump per-fail cho corpus
# (zcc stderr + exit/stdout diff vs referee) vào $LOG/triage-*/.
LOG=/suites/overnight3-20260818
B=/build
mkdir -p "$LOG" "$B"
export ZCC=/usr/local/bin/zcc ZCC_SUITE_CACHE=/suites SUITES=/suites
NP=$(nproc)
echo "bắt đầu: $(date)" > "$LOG/SUMMARY.txt"

run() {
    name=$1; shift; s=$(date +%s)
    if "$@" > "$LOG/$name.log" 2>&1; then st=PASS; else st="FAIL(rc=$?)"; fi
    echo "$name: $st ($(( $(date +%s) - s ))s)" >> "$LOG/SUMMARY.txt"
}

# ---- 0. deps
deps() {
    apt-get update -qq
    apt-get install -y -qq tcl tcl-dev perl libpcre2-dev zlib1g-dev \
        libssl-dev libio-socket-ssl-perl git ca-certificates file procps
    useradd -m runner 2>/dev/null || true
}
run deps deps

# ---- 1. copy repo
repo() { cd /work && tar cf - --exclude=zcc/target --exclude=zcc/.git zcc | (cd "$B" && tar xf -); }
run repo-copy repo
Z="$B/zcc"

# ---- 2. gate khoa học ELF
run gate-abi   sh "$Z/tests/abi.sh"
run gate-alg   sh "$Z/tests/alg.sh"
run gate-cpp   sh "$Z/tests/cpp.sh"
run gate-shape sh "$Z/tests/shape.sh"
run gate-m12   sh "$Z/tests/m12.sh"

# ---- 3. corpus suite + triage verbose per-fail
# dump: nguồn, zcc stderr đầy đủ, exit code + diff stdout vs referee gcc
triage() { # $1=suite-log-name  $2=case-dir  $3=sed-transform(name→path-frag)
    t="$LOG/triage-$1"; mkdir -p "$t"
    grep '^FAIL ' "$LOG/$1.log" | awk '{print $2}' | while read -r n; do
        f=$(find "$2" -name '*.c' | while read -r p; do
                b=$(echo "$p" | sed "$3")
                [ "$b" = "$n" ] && { echo "$p"; break; }
            done)
        [ -z "$f" ] && { echo "không map được source" > "$t/$n.txt"; continue; }
        {
            echo "== source: $f"
            echo "== zcc compile stderr:"
            "$ZCC" "$f" -o "/tmp/tz.$n" 2>&1 || true
            if [ -x "/tmp/tz.$n" ]; then
                gcc -std=c89 -w -O0 "$f" -o "/tmp/tc.$n" 2>/dev/null || true
                perl -e 'alarm 10; exec @ARGV' "/tmp/tz.$n" < "$f" 2>&1 | head -c 4096 > "/tmp/oz.$n"; ez=$?
                echo "== zcc exit: $ez"
                if [ -x "/tmp/tc.$n" ]; then
                    perl -e 'alarm 10; exec @ARGV' "/tmp/tc.$n" < "$f" 2>&1 | head -c 4096 > "/tmp/oc.$n"; ec=$?
                    echo "== gcc exit: $ec"
                    echo "== diff stdout (gcc vs zcc, 4KB đầu):"
                    diff "/tmp/oc.$n" "/tmp/oz.$n" | head -60 || true
                fi
                echo "== zcc -S (100 dòng đầu):"
                "$ZCC" -S "$f" -o /dev/stdout 2>/dev/null | head -100 || true
            fi
        } > "$t/$n.txt" 2>&1
        rm -f "/tmp/tz.$n" "/tmp/tc.$n" "/tmp/oz.$n" "/tmp/oc.$n"
    done
}
run suite-torture sh "$Z/tests/suites/torture.sh"
triage suite-torture /suites/gcc/gcc/testsuite/gcc.c-torture/execute 's|.*/||; s|\.c$||'
run suite-cts     sh "$Z/tests/suites/cts.sh"
triage suite-cts /suites/c-testsuite/tests/single-exec 's|.*/||; s|\.c$||'
run suite-chibicc sh "$Z/tests/suites/chibicc.sh"
run suite-kr      sh "$Z/tests/suites/kr.sh"
triage suite-kr /suites/kr 's|.*/kr/||; s|/|_|g; s|\.c$||'
run suite-nora    sh "$Z/tests/suites/nora.sh"
triage suite-nora /suites/nora/tests 's|.*/tests/||; s|/|_|g; s|\.c$||'
echo "suite-tcc: SKIP (Darwin-only, cần xcrun)" >> "$LOG/SUMMARY.txt"

# ---- 4. musl full + libc-test
run musl-full sh "$Z/tests/suites/musl-box.sh"
cp /suites/libc-test/ZCC-FAILS.txt "$LOG/" 2>/dev/null || true
cp /suites/libc-test-ref/REF-FAILS.txt "$LOG/" 2>/dev/null || true

# ---- 5. sqlite differential ELF
sqlite_diff() {
    set -e
    mkdir -p "$B/sqlite" && cd "$B/sqlite"
    cp /suites/sqlite/sqlite3.c /suites/sqlite/sqlite3.h /suites/sqlite/shell.c .
    "$ZCC" -c sqlite3.c -o sqlite3_zcc.o && "$ZCC" shell.c sqlite3_zcc.o -o sq_zcc -lpthread -ldl -lm
    gcc -O0 -w sqlite3.c shell.c -o sq_gcc -lpthread -ldl -lm
    cat > w.sql <<'EOF'
CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT, c REAL);
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<5000)
INSERT INTO t SELECT x, printf('r%d',x%97), x*1.5 FROM n;
CREATE INDEX ib ON t(b);
SELECT count(*), sum(c), avg(a) FROM t;
SELECT b, count(*) FROM t GROUP BY b ORDER BY b LIMIT 7;
SELECT a,b FROM t WHERE b LIKE 'r1%' ORDER BY a DESC LIMIT 5;
UPDATE t SET c=c*2 WHERE a%3=0; DELETE FROM t WHERE a%7=0;
SELECT total(c) FROM t; VACUUM; PRAGMA integrity_check;
EOF
    ./sq_zcc :memory: < w.sql > z.txt
    ./sq_gcc :memory: < w.sql > g.txt
    cmp z.txt g.txt && echo "SQLITE-ELF-KHOP ($(wc -l < z.txt) dòng byte-identical)"
}
run sqlite-diff sqlite_diff

# ---- 6. sqlite TCL veryquick (giữ config.log để triage)
sqlite_tcl() {
    set -e
    [ -d "$B/sqlite-src" ] || git clone --depth 1 https://github.com/sqlite/sqlite.git "$B/sqlite-src"
    cd "$B/sqlite-src"
    CC="$ZCC" ./configure || { cp config.log "$LOG/sqlite-tcl-config.log" 2>/dev/null; exit 1; }
    cp config.log "$LOG/sqlite-tcl-config.log" 2>/dev/null || true
    make -j"$NP" testfixture
    ./testfixture test/veryquick.test
}
run sqlite-tcl sqlite_tcl

# ---- 7. git: build (NO_RUST=1 — box không có cargo; 2.55 có phần Rust) + FULL t/
git_build() {
    set -e
    rm -rf "$B/git" && cp -r /suites/git "$B/git" && cd "$B/git"
    make distclean >/dev/null 2>&1 || true
    make -j"$NP" V=1 CC="$ZCC" NO_RUST=1 NO_GETTEXT=1 NO_TCLTK=1 NO_CURL=1 \
        NO_EXPAT=1 FSMONITOR_DAEMON_BACKEND= FSMONITOR_OS_SETTINGS=
    ./git version
}
run git-build git_build
git_t() {
    cd "$B/git/t" || return 1
    chown -R runner "$B/git"
    su runner -c "cd '$B/git/t' && prove -j$NP t[0-9]*.sh" || {
        # verbose: chạy lại đúng các file fail, log -v
        su runner -c "cd '$B/git/t' && prove -j$NP t[0-9]*.sh 2>&1 | grep -B1 'Wstat' " > "$LOG/git-t-refails.txt" 2>&1 || true
        return 1
    }
}
run git-t-full git_t

# ---- 8. redis build + runtest
redis_build() {
    set -e
    cd "$B/redis"
    make distclean >/dev/null 2>&1 || true
    make -j"$NP" V=1 CC="$ZCC" MALLOC=libc
    ./src/redis-server --version
}
run redis-build redis_build
redis_test() { cd "$B/redis" && ./runtest --clients "$NP"; }
run redis-runtest redis_test

# ---- 9. nginx full build + tests (+ referee gcc chỉ khi zcc fail)
nginx_build() {
    set -e
    rm -rf "$B/nginx" && cp -r /suites/nginx "$B/nginx" && cd "$B/nginx"
    rm -rf objs Makefile
    CC="$ZCC" ./auto/configure --with-http_ssl_module --with-http_v2_module \
        --with-stream --with-stream_ssl_module --with-mail --with-mail_ssl_module \
        || { cp objs/autoconf.err "$LOG/nginx-autoconf.err" 2>/dev/null; exit 1; }
    make -j"$NP"
    ./objs/nginx -v
}
run nginx-build nginx_build
nginx_test() {
    rm -rf "$B/nginx-tests" && cp -r /suites/nginx-tests "$B/nginx-tests"
    chown -R runner "$B/nginx" "$B/nginx-tests"
    su runner -c "cd '$B/nginx-tests' && TEST_NGINX_BINARY='$B/nginx/objs/nginx' prove -j4 ."
}
run nginx-tests nginx_test
if ! grep -q "nginx-tests: PASS" "$LOG/SUMMARY.txt"; then
    nginx_ref() {
        set -e
        rm -rf "$B/nginx-gcc" && cp -r /suites/nginx "$B/nginx-gcc" && cd "$B/nginx-gcc"
        rm -rf objs Makefile
        ./auto/configure --with-http_ssl_module --with-http_v2_module \
            --with-stream --with-stream_ssl_module --with-mail --with-mail_ssl_module
        make -j"$NP"
        rm -rf "$B/nginx-tests-ref" && cp -r /suites/nginx-tests "$B/nginx-tests-ref"
        chown -R runner "$B/nginx-gcc" "$B/nginx-tests-ref"
        su runner -c "cd '$B/nginx-tests-ref' && TEST_NGINX_BINARY='$B/nginx-gcc/objs/nginx' prove -j4 ."
    }
    run nginx-tests-referee-gcc nginx_ref
fi

# ---- tổng kết
{
    echo "--- trích số liệu:"
    for f in gate-abi gate-alg gate-cpp gate-shape gate-m12 suite-torture \
             suite-cts suite-chibicc suite-kr suite-nora musl-full sqlite-diff; do
        [ -f "$LOG/$f.log" ] && printf '%s: %s\n' "$f" "$(tail -1 "$LOG/$f.log")"
    done
    for f in sqlite-tcl git-t-full redis-runtest nginx-tests nginx-tests-referee-gcc; do
        [ -f "$LOG/$f.log" ] && { echo "== $f (tail):"; tail -8 "$LOG/$f.log"; }
    done
    echo "--- triage dumps: $(ls -d "$LOG"/triage-* 2>/dev/null | wc -l) suite, $(find "$LOG" -path '*triage*' -name '*.txt' 2>/dev/null | wc -l) case"
    echo "--- kết thúc: $(date)"
} >> "$LOG/SUMMARY.txt"
