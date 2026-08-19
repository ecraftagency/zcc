#!/bin/sh
# ============================================================================
# tests/fullsuite.sh — RUNNER FULL-SUITE overnight (bank hoá từ overnight3).
# Chạy TRONG zcc-box (arm64 ELF Linux), KHÔNG chạy trực tiếp trên mac.
# Cổng vào duy nhất: tests/run-fullsuite.sh (host launcher: build zcc-ELF +
# docker run -d detached). Runner này thay thế toàn bộ overnight{2..8}.sh.
#
# Hợp đồng môi trường (launcher dựng — xem run-fullsuite.sh):
#   $LOG   = /suites/<run>        log sống NGOÀI container (persist mọi lúc)
#   $JOBS  = mức song song (mặc định 2) — Vu: overnight ưu tiên MÁT hơn nhanh
#   /work  = repo zcc (RO)        nguồn tests/ + gen_*.py + cases
#   /build = ~/.cache/.../build-elf  volume làm việc (git/redis/nginx/sqlite sẵn)
#   /suites= ~/.cache/zcc-suites  cache nguồn suite
#   zcc    = /usr/local/bin/zcc   binary zcc ELF static (RO)
#
# 4 điều kiện (Vu 2026-08-20) — thiết kế theo từng cái:
#  [1] DETACH:    launcher docker run -d; runner ghi $LOG/SUMMARY.txt live.
#  [2] OUTPUT:    mỗi stage → $LOG/<name>.log; corpus fail → $LOG/triage-*/;
#                 SUMMARY.txt tier hoá, mỗi dòng = stage/trạng thái/giây.
#  [3] RESILIENT: KHÔNG có `set -e` ở orchestrator. Mỗi stage chạy trong
#                 tiến trình con (self-dispatch `sh $0 __label`) bọc `timeout`.
#                 Stage fail HOẶC hang → ghi FAIL/TIMEOUT, cả đêm CHẠY TIẾP.
#  [4] TUẦN TỰ:   thứ tự math → compiler-suite → compile-app → app-suite;
#                 stage không đè nhau; JOBS thấp để không nóng máy.
# ============================================================================
set -u

# ---- môi trường (áp cho CẢ orchestrator lẫn mỗi stage con) ----
export ZCC=/usr/local/bin/zcc ZCC_SUITE_CACHE=/suites SUITES=/suites
B=/build; Z="$B/zcc"; export B Z
JOBS="${JOBS:-2}"; export JOBS
NP="$JOBS"; export NP                 # tương thích tên cũ trong stage
: "${LOG:?LOG chưa set (chạy qua run-fullsuite.sh)}"; export LOG

# ---- triage: dump per-fail cho corpus suite (nguồn + zcc stderr + diff vs gcc
#      + zcc -S). Bounded: mỗi fail compile+run alarm 10. Định nghĩa TRƯỚC
#      dispatch để tiến trình con thấy được. ($1=logname $2=case-dir $3=sed) ----
triage() {
    t="$LOG/triage-$1"; mkdir -p "$t"
    grep '^FAIL ' "$LOG/$1.log" 2>/dev/null | awk '{print $2}' | while read -r n; do
        [ -z "$n" ] && continue
        f=$(find "$2" -name '*.c' 2>/dev/null | while read -r p; do
                b=$(echo "$p" | sed "$3")
                [ "$b" = "$n" ] && { echo "$p"; break; }
            done)
        [ -z "$f" ] && { echo "không map được source cho '$n'" > "$t/$n.txt"; continue; }
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
    echo "triage-$1: $(ls "$t" 2>/dev/null | wc -l | tr -d ' ') dump"
}

# ============================================================================
# DISPATCH — mỗi stage là một nhánh. Chạy dưới `set -e` (fail-fast an toàn vì
# đã cách ly bởi timeout+tiến-trình-con của orchestrator). Xong thì exit, KHÔNG
# rơi xuống orchestrator.
# ============================================================================
if [ "${1:-}" != "" ]; then
    set -e
    case "$1" in
    # ---- prep ----
    __deps)
        # Phần lớn deps đã bake vào zcc-box; nhưng git thường thiếu. Thử apt-get
        # BEST-EFFORT (timeout, non-fatal) cho tool còn thiếu — có mạng thì cài,
        # không thì stage phụ thuộc FAIL (được bắt), KHÔNG treo cả đêm.
        need=""
        for tprog in gcc git make perl prove python3 tclsh openssl; do
            command -v "$tprog" >/dev/null 2>&1 || need="$need $tprog"
        done
        if [ -n "$need" ]; then
            echo "thiếu:$need → thử apt-get (best-effort, ≤4p)"
            timeout 240 apt-get update -qq 2>&1 | tail -2 || true
            timeout 300 apt-get install -y -qq git tcl tcl-dev perl libpcre2-dev \
                zlib1g-dev libssl-dev libio-socket-ssl-perl ca-certificates file procps \
                2>&1 | tail -3 || true
        fi
        for tprog in gcc git make perl prove python3 tclsh openssl timeout; do
            printf '%-10s %s\n' "$tprog" "$(command -v "$tprog" 2>/dev/null || echo MISSING)"
        done
        id runner >/dev/null 2>&1 || useradd -m runner 2>/dev/null || true
        echo "zcc: $("$ZCC" --version 2>&1 | head -1 || echo '(không có --version)')"
        ;;
    __repo_copy)
        cd /work && tar cf - --exclude=zcc/target --exclude=zcc/.git zcc | (cd "$B" && tar xf -)
        echo "repo → $B/zcc: $(find "$Z/tests" -name '*.sh' | wc -l | tr -d ' ') script"
        ;;

    # ---- TIER 1: math / sci gate + base differential ----
    __gate_abi)   sh "$Z/tests/abi.sh" ;;
    __gate_alg)   sh "$Z/tests/alg.sh" ;;
    __gate_cpp)   sh "$Z/tests/cpp.sh" ;;
    __gate_shape) sh "$Z/tests/shape.sh" ;;
    __gate_decay) sh "$Z/tests/decay.sh" ;;
    __gate_m12)   sh "$Z/tests/m12.sh" ;;
    __base_cases) sh "$Z/tests/run.sh" cases ;;
    __base_ext)   sh "$Z/tests/run.sh" ext ;;

    # ---- TIER 2: compiler test suite (corpus) ----
    __suite_torture) sh "$Z/tests/suites/torture.sh" ;;
    __suite_cts)     sh "$Z/tests/suites/cts.sh" ;;
    __suite_chibicc) sh "$Z/tests/suites/chibicc.sh" ;;
    __suite_kr)      sh "$Z/tests/suites/kr.sh" ;;
    __suite_nora)    sh "$Z/tests/suites/nora.sh" ;;
    __suite_tcc)     sh "$Z/tests/suites/tcc.sh" ;;
    __triage)        triage "$2" "$3" "$4" ;;

    # ---- TIER 3: compile app (native build lớn) ----
    # git/nginx build trong WB=/root/wb (container-local, root sở hữu) — KHÔNG
    # dùng $B (bind-mount): stage test chạy `su runner` để lại file uid 1000,
    # root-in-container không rm nổi qua VirtioFS macOS → build kế chết. WB sống
    # trọn 1 lần chạy container nên stage test tìm lại được.
    __app_git_build)
        WB=/root/wb; mkdir -p "$WB"; rm -rf "$WB/git"
        cp -r /suites/git "$WB/git" && cd "$WB/git"
        make distclean >/dev/null 2>&1 || true
        make -j"$JOBS" V=1 CC="$ZCC" NO_RUST=1 NO_GETTEXT=1 NO_TCLTK=1 NO_CURL=1 \
            NO_EXPAT=1 FSMONITOR_DAEMON_BACKEND= FSMONITOR_OS_SETTINGS=
        ./git version
        ;;
    __app_redis_build)
        cd "$B/redis"
        make distclean >/dev/null 2>&1 || true
        make -j"$JOBS" V=1 CC="$ZCC" MALLOC=libc
        ./src/redis-server --version
        ;;
    __app_nginx_build)
        WB=/root/wb; mkdir -p "$WB"; rm -rf "$WB/nginx"
        cp -r /suites/nginx "$WB/nginx" && cd "$WB/nginx"
        rm -rf objs Makefile
        CC="$ZCC" ./auto/configure --with-http_ssl_module --with-http_v2_module \
            --with-stream --with-stream_ssl_module --with-mail --with-mail_ssl_module \
            || { cp objs/autoconf.err "$LOG/nginx-autoconf.err" 2>/dev/null || true; exit 1; }
        make -j"$JOBS"
        ./objs/nginx -v
        ;;

    # ---- TIER 4: chạy test suite của app đã compile ----
    __suite_sqlite_diff)
        mkdir -p "$B/sqlite" && cd "$B/sqlite"
        cp /suites/sqlite/sqlite3.c /suites/sqlite/sqlite3.h /suites/sqlite/shell.c .
        "$ZCC" -c sqlite3.c -o sqlite3_zcc.o
        "$ZCC" shell.c sqlite3_zcc.o -o sq_zcc -lpthread -ldl -lm
        gcc -O0 -w sqlite3.c shell.c -o sq_gcc -lpthread -ldl -lm
        cat > w.sql <<'SQL'
CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT, c REAL);
WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<5000)
INSERT INTO t SELECT x, printf('r%d',x%97), x*1.5 FROM n;
CREATE INDEX ib ON t(b);
SELECT count(*), sum(c), avg(a) FROM t;
SELECT b, count(*) FROM t GROUP BY b ORDER BY b LIMIT 7;
SELECT a,b FROM t WHERE b LIKE 'r1%' ORDER BY a DESC LIMIT 5;
UPDATE t SET c=c*2 WHERE a%3=0; DELETE FROM t WHERE a%7=0;
SELECT total(c) FROM t; VACUUM; PRAGMA integrity_check;
SQL
        ./sq_zcc :memory: < w.sql > z.txt
        ./sq_gcc :memory: < w.sql > g.txt
        cmp z.txt g.txt && echo "SQLITE-ELF-KHOP ($(wc -l < z.txt | tr -d ' ') dòng byte-identical)"
        ;;
    __suite_sqlite_tcl)
        cd "$B/sqlite-src"
        CC="$ZCC" ./configure || { cp config.log "$LOG/sqlite-tcl-config.log" 2>/dev/null || true; exit 1; }
        cp config.log "$LOG/sqlite-tcl-config.log" 2>/dev/null || true
        make -j"$JOBS" testfixture
        ./testfixture test/veryquick.test
        ;;
    __suite_musl)
        sh "$Z/tests/suites/musl-box.sh"
        cp /suites/libc-test/ZCC-FAILS.txt "$LOG/" 2>/dev/null || true
        cp /suites/libc-test-ref/REF-FAILS.txt "$LOG/" 2>/dev/null || true
        ;;
    __suite_git_t)
        WB=/root/wb
        id runner >/dev/null 2>&1 || useradd -m runner 2>/dev/null || true
        chown -R runner "$WB/git"
        su runner -c "cd '$WB/git/t' && prove -j$JOBS t[0-9]*.sh"
        ;;
    __suite_redis_runtest)
        cd "$B/redis" && ./runtest --clients "$JOBS"
        ;;
    __suite_nginx_tests)
        WB=/root/wb
        id runner >/dev/null 2>&1 || useradd -m runner 2>/dev/null || true
        rm -rf "$WB/nginx-tests" && cp -r /suites/nginx-tests "$WB/nginx-tests"
        chown -R runner "$WB/nginx" "$WB/nginx-tests"
        su runner -c "cd '$WB/nginx-tests' && TEST_NGINX_BINARY='$WB/nginx/objs/nginx' prove -j$JOBS ."
        ;;
    __suite_nginx_ref)   # trọng tài gcc — CHỈ chạy khi nginx-tests fail (phân định flake vs bug)
        WB=/root/wb
        id runner >/dev/null 2>&1 || useradd -m runner 2>/dev/null || true
        rm -rf "$WB/nginx-gcc" && cp -r /suites/nginx "$WB/nginx-gcc" && cd "$WB/nginx-gcc"
        rm -rf objs Makefile
        ./auto/configure --with-http_ssl_module --with-http_v2_module \
            --with-stream --with-stream_ssl_module --with-mail --with-mail_ssl_module
        make -j"$JOBS"
        rm -rf "$WB/nginx-tests-ref" && cp -r /suites/nginx-tests "$WB/nginx-tests-ref"
        chown -R runner "$WB/nginx-gcc" "$WB/nginx-tests-ref"
        su runner -c "cd '$WB/nginx-tests-ref' && TEST_NGINX_BINARY='$WB/nginx-gcc/objs/nginx' prove -j$JOBS ."
        ;;
    *) echo "fullsuite: stage lạ '$1'" >&2; exit 99 ;;
    esac
    exit 0
fi

# ============================================================================
# ORCHESTRATOR — KHÔNG set -e. Chạy tuần tự, mỗi stage bọc timeout+catch.
# ============================================================================
SUM="$LOG/SUMMARY.txt"
mkdir -p "$LOG"

# run <name> <timeout-giây> <__label> [args...]
run() {
    name=$1; to=$2; shift 2; s=$(date +%s)
    if timeout -k 30 "$to" sh "$0" "$@" > "$LOG/$name.log" 2>&1; then
        st=PASS
    else
        rc=$?
        if [ "$rc" = 124 ] || [ "$rc" = 137 ]; then st="TIMEOUT(${to}s)"; else st="FAIL(rc=$rc)"; fi
    fi
    printf '  %-22s %-16s %ss\n' "$name" "$st" "$(( $(date +%s) - s ))" | tee -a "$SUM"
}
tier() { printf '\n=== %s ===\n' "$1" | tee -a "$SUM"; }

{
    echo "================ zcc FULL SUITE (ELF box) ================"
    echo "bắt đầu : $(date)"
    echo "JOBS    : $JOBS (tuần tự, ưu tiên mát máy)"
    echo "zcc     : $("$ZCC" --version 2>&1 | head -1 || echo '?')"
    echo "nproc   : $(nproc)"
} > "$SUM"

# ---- TIER 0: prep ----
tier "TIER 0 — prep"
run deps        300  __deps
run repo-copy   300  __repo_copy

# ---- TIER 1: math / sci gate + base differential ----
tier "TIER 1 — MATH/SCI GATE + base differential"
run gate-abi    1800 __gate_abi
run gate-alg    3600 __gate_alg
run gate-cpp    1800 __gate_cpp
run gate-shape  1800 __gate_shape
run gate-decay  600  __gate_decay
run gate-m12    600  __gate_m12
run base-cases  1200 __base_cases
run base-ext    1200 __base_ext

# ---- TIER 2: compiler test suite (corpus) ----
tier "TIER 2 — COMPILER SUITE (corpus, differential ⊆ baseline)"
run suite-torture 3000 __suite_torture
run tri-torture   1800 __triage suite-torture /suites/gcc/gcc/testsuite/gcc.c-torture/execute 's|.*/||; s|\.c$||'
run suite-cts     900  __suite_cts
run tri-cts       600  __triage suite-cts /suites/c-testsuite/tests/single-exec 's|.*/||; s|\.c$||'
run suite-chibicc 900  __suite_chibicc
run suite-kr      900  __suite_kr
run tri-kr        600  __triage suite-kr /suites/kr 's|.*/kr/||; s|/|_|g; s|\.c$||'
run suite-nora    1200 __suite_nora
run tri-nora      600  __triage suite-nora /suites/nora/tests 's|.*/tests/||; s|/|_|g; s|\.c$||'
run suite-tcc     1500 __suite_tcc

# ---- TIER 3: compile app (native build) ----
tier "TIER 3 — COMPILE APP (CC=zcc, build system gốc)"
run app-git-build   3600 __app_git_build
run app-redis-build 2400 __app_redis_build
run app-nginx-build 3600 __app_nginx_build

# ---- TIER 4: test suite của app đã compile ----
tier "TIER 4 — COMPILED-APP SUITE (phần mềm tự kiểm chính nó)"
run suite-sqlite-diff   900   __suite_sqlite_diff
run suite-sqlite-tcl    5400  __suite_sqlite_tcl
run suite-musl          7200  __suite_musl
run suite-git-t         10800 __suite_git_t
run suite-redis-runtest 7200  __suite_redis_runtest
run suite-nginx-tests   7200  __suite_nginx_tests
if ! grep -q 'suite-nginx-tests .*PASS' "$SUM"; then
    run suite-nginx-ref 7200 __suite_nginx_ref
fi

# ---- tổng kết ----
tier "TÓM TẮT"
{
    echo "--- trích số liệu (dòng cuối mỗi log):"
    for f in gate-abi gate-alg gate-cpp gate-shape gate-decay gate-m12 \
             base-cases base-ext suite-torture suite-cts suite-chibicc \
             suite-kr suite-nora suite-tcc suite-sqlite-diff suite-musl; do
        [ -f "$LOG/$f.log" ] && printf '%-22s %s\n' "$f:" "$(tail -1 "$LOG/$f.log" 2>/dev/null)"
    done
    echo "--- tail log dài (app build/test):"
    for f in app-git-build app-redis-build app-nginx-build suite-sqlite-tcl \
             suite-git-t suite-redis-runtest suite-nginx-tests suite-nginx-ref; do
        [ -f "$LOG/$f.log" ] && { echo "== $f (8 dòng cuối):"; tail -8 "$LOG/$f.log" 2>/dev/null; }
    done
    echo "--- triage: $(ls -d "$LOG"/triage-* 2>/dev/null | wc -l | tr -d ' ') suite, $(find "$LOG" -path '*triage*' -name '*.txt' 2>/dev/null | wc -l | tr -d ' ') case dump"
    echo "kết thúc: $(date)"
    echo "===================== HẾT ====================="
} >> "$SUM"
