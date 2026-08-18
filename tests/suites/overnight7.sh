#!/bin/sh
# Vòng targeted 18/8 đêm (overnight7 — lệnh Vu: "cái nào pass rồi đừng test nữa,
# seek tới unit bị fail"). CHỈ chạy lại án fail của overnight6, không lặp suite
# đã PASS. Seed mang chùm fix pr92904 (3 bug ABI thật, verify tay vs gcc trong
# box: PR92904-PASS):
#   1. AAPCS C.9: composite align≥16 → NGRN tròn chẵn (call + spill + va_arg
#      runtime gr_offs tròn 16, __stack tròn 16)
#   2. composite va_arg không split reg/stack: consume offs trước, chỉ đi reg
#      khi offs mới ≤ 0 (án f2(6): gr_offs=-8 vắt qua 0 phải rơi nguyên stack)
#   3. anonymous HFA >16B đi BY VALUE trên ELF (B.4, gcc pass d0-d3) — parser
#      bỏ nhánh gián tiếp cho ELF; Darwin giữ (khớp quy ước va_arg macro cũ)
#   + cap align composite arg tại 16 (B.4), SA16 vào gen_abi.py (automaton mở rộng)
# Án theo dõi: sqlite-tcl segv env-dependent, git merge SEGV (ăn 3 cụm
# t7600/t8002/t8012), t7112 logic, redis unit/networking "Out of memory".
LOG=/suites/overnight7-20260818
O6=/suites/overnight6-20260818
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

# ---- 0. deps (+ strace cho án segv)
deps() {
    apt-get update -qq
    apt-get install -y -qq tcl tcl-dev perl libpcre2-dev zlib1g-dev \
        libssl-dev libio-socket-ssl-perl git ca-certificates file procps strace gdb
    useradd -m runner 2>/dev/null || true
}
run deps deps

# ---- 1. copy repo
repo() { cd /work && tar cf - --exclude=zcc/target --exclude=zcc/.git zcc | (cd "$B" && tar xf -); }
run repo-copy repo
Z="$B/zcc"

# ---- 2. gate-abi: automaton ABI vừa đổi (3 site + SA16) — luật "sửa 1 = sửa 3 + abi.sh"
run gate-abi sh "$Z/tests/abi.sh"

# ---- 3. torture: đúng 1 case fail (pr92904)
tor() {
    mkdir -p /tmp/tor && cd /tmp/tor
    cp /suites/gcc/gcc/testsuite/gcc.c-torture/execute/pr92904.c .
    "$ZCC" pr92904.c -o pz && ./pz && echo PR92904-PASS
}
run torture-pr92904 tor

# ---- 4. sqlite-tcl: rerun; nếu segv → thu bằng chứng (deterministic? strace lệnh chết)
sqlite_tcl() {
    set -e
    [ -d "$B/sqlite-src" ] || git clone --depth 1 https://github.com/sqlite/sqlite.git "$B/sqlite-src"
    cd "$B/sqlite-src"
    CC="$ZCC" ./configure || { cp config.log "$LOG/sqlite-tcl-config.log" 2>/dev/null; exit 1; }
    make -j"$NP" testfixture
    ./testfixture test/veryquick.test
}
run sqlite-tcl sqlite_tcl
if ! grep -q "sqlite-tcl: PASS" "$LOG/SUMMARY.txt"; then
    sqlite_seek() {
        cd "$B/sqlite-src" || return 1
        # make lần 2: nếu qua được → segv không deterministic trong env này
        if make testfixture > "$LOG/sqlite-tcl-retry.log" 2>&1; then
            echo "RETRY-OK: make lần 2 qua — segv flaky/env"
            ./testfixture test/veryquick.test
        else
            # bắt đúng lệnh chết dưới strace: rerun make in lệnh, chạy verbatim
            make -n testfixture 2>/dev/null | tail -5 > "$LOG/sqlite-tcl-cmd.txt"
            strace -f -o "$LOG/sqlite-tcl-strace.txt" make testfixture 2>&1 | tail -20
            tail -40 "$LOG/sqlite-tcl-strace.txt" || true
            return 1
        fi
    }
    run sqlite-tcl-seek sqlite_seek
fi

# ---- 5. git: rebuild bằng zcc mới → seek 4 cluster verbose → prove ĐÚNG các file fail của o6
git_build() {
    set -e
    chmod -R u+rwX "$B/git" 2>/dev/null || true
    rm -rf "$B/git" && cp -r /suites/git "$B/git" && cd "$B/git"
    make distclean >/dev/null 2>&1 || true
    make -j"$NP" V=1 CC="$ZCC" NO_RUST=1 NO_GETTEXT=1 NO_TCLTK=1 NO_CURL=1 \
        NO_EXPAT=1 FSMONITOR_DAEMON_BACKEND= FSMONITOR_OS_SETTINGS=
    ./git version
}
run git-build git_build
git_seek() {
    chown -R runner "$B/git"
    ok=0
    for tt in t7600-merge t8002-blame t8012-blame-colors t7112-reset-submodule; do
        su runner -c "cd '$B/git/t' && ./$tt.sh -v" > "$LOG/git-zcc-$tt.log" 2>&1 || true
        zf=$(grep -c '^not ok' "$LOG/git-zcc-$tt.log" || true)
        echo "$tt: zcc-fail=$zf (o6 ref-fail=0)" >> "$LOG/git-cluster-verdict.txt"
        [ "$zf" -eq 0 ] || ok=1
    done
    cat "$LOG/git-cluster-verdict.txt"
    return $ok
}
run git-cluster git_seek
git_refail() {
    # prove lại ĐÚNG danh sách file fail của o6 (không full 1056 file)
    list=$(grep -oE '^t[0-9][^ ]*\.sh' "$O6/git-t-full.log" | sort -u | tr '\n' ' ')
    [ -n "$list" ] || return 0
    echo "prove lại $(echo $list | wc -w) file fail của o6"
    chown -R runner "$B/git"
    su runner -c "cd '$B/git/t' && prove -j$NP $list"
}
run git-refail git_refail

# ---- 6. redis: build lại bằng zcc mới, seek đúng unit chết (networking)
redis_build() {
    set -e
    cd "$B/redis"
    make distclean >/dev/null 2>&1 || true
    make -j"$NP" CC="$ZCC" MALLOC=libc >/dev/null 2>&1
    ./src/redis-server --version
}
run redis-build redis_build
redis_seek() { cd "$B/redis" && ./runtest --single unit/networking; }
run redis-networking redis_seek
if grep -q "redis-networking: PASS" "$LOG/SUMMARY.txt"; then
    # unit chết đã xanh → full runtest đóng sổ (chạy 1 lần cuối, luật charter)
    redis_full() { cd "$B/redis" && ./runtest --clients "$NP"; }
    run redis-runtest redis_full
fi

# ---- nginx: KHÔNG chạy lại nếu o6 đã PASS (đọc sổ o6 lúc này)
if grep -q "nginx-tests: PASS" "$O6/SUMMARY.txt" 2>/dev/null; then
    echo "nginx-tests: SKIP (o6 PASS)" >> "$LOG/SUMMARY.txt"
else
    echo "nginx-tests: o6 chưa xanh — xem $O6/nginx-tests.log, chưa rerun (chờ triage)" >> "$LOG/SUMMARY.txt"
fi

# ---- tổng kết
{
    echo "--- trích:"
    for f in gate-abi torture-pr92904 git-cluster; do
        [ -f "$LOG/$f.log" ] && printf '%s: %s\n' "$f" "$(tail -1 "$LOG/$f.log")"
    done
    for f in sqlite-tcl sqlite-tcl-seek git-refail redis-networking redis-runtest; do
        [ -f "$LOG/$f.log" ] && { echo "== $f (tail):"; tail -6 "$LOG/$f.log"; }
    done
    echo "--- kết thúc: $(date)"
} >> "$LOG/SUMMARY.txt"
