#!/bin/sh
# Vòng targeted 18/8 khuya (overnight8) — seed mang 2 chùm fix sau overnight7:
#   1. pr92904 CHỐT LẠI theo trọng tài: composite va_arg không split reg/stack
#      (blk consume-first) + anonymous HFA >16B by value; REVERT NGRN rounding
#      align16 (gcc arm64 BỎ QUA over-alignment composite — verify asm: named
#      x3,x4 / anon x4,x5 / stack [sp,8]); composite stack align = 8.
#      Verify throwaway: pr92904 PASS, abi ELF 312/312 cả 4 hướng (có SA16).
#   2. ÁN (B) root-cause CHỐT: ternary 2 string literal không decay array→ptr
#      (C99 6.5.15) → arg variadic thứ 9+ tràn stack bị store_narrow cắt strh
#      2 byte → glibc vsnprintf đọc con trỏ rác → segv THEO LAYOUT (giải thích
#      trọn: git merge segv 24/30 bare nhưng 0 fail dưới gdb-tắt-ASLR, t8012
#      lúc 23 lúc 0 fail, testfixture segv, nghi cả redis "Out of memory").
#      Fix parser: arr_decay ở cond_expr (cả elvis) + default promotion
#      finish_call (C99 6.5.2.2p6). Mini repro 0/20 fail, byte-khớp gcc.
#      Case mới: tests/cases/c99_ternary_decay_vararg.c (run.sh 68/68 Darwin).
# Án còn mở theo dõi ở đây: (A) zcc TỰ segv flaky khi compile dưới make -j
# (0/150 serial standalone — chỉ nổ song song/memory pressure; sqlite 88-TU,
# git unix-socket.o ×3 cùng lúc) — seek strace giữ nguyên.
# LỆNH VU: chỉ chạy cái đang fail, seek tới unit fail; mọi án xanh hết mới mở
# 1 vòng full suite toàn diện cuối cùng.
LOG=/suites/overnight8-20260818
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

deps() {
    apt-get update -qq
    apt-get install -y -qq tcl tcl-dev perl libpcre2-dev zlib1g-dev \
        libssl-dev libio-socket-ssl-perl git ca-certificates file procps strace gdb
    useradd -m runner 2>/dev/null || true
}
run deps deps
repo() { cd /work && tar cf - --exclude=zcc/target --exclude=zcc/.git zcc | (cd "$B" && tar xf -); }
run repo-copy repo
Z="$B/zcc"

# ---- gate nhanh chốt sổ 2 fix (1s mỗi cái)
run gate-abi sh "$Z/tests/abi.sh"
tor() {
    mkdir -p /tmp/tor && cd /tmp/tor
    cp /suites/gcc/gcc/testsuite/gcc.c-torture/execute/pr92904.c .
    "$ZCC" pr92904.c -o pz && ./pz && echo PR92904-PASS
}
run torture-pr92904 tor
decay() {
    cd /tmp && "$ZCC" "$Z/tests/cases/c99_ternary_decay_vararg.c" -o dz && ./dz > z.txt
    gcc -O0 -w "$Z/tests/cases/c99_ternary_decay_vararg.c" -o dg && ./dg > g.txt
    cmp z.txt g.txt && echo DECAY-KHOP
}
run case-decay decay

# ---- git: rebuild bằng seed mới → 4 cụm → prove lại đúng list fail o6
git_build() {
    set -e
    chmod -R u+rwX "$B/git" 2>/dev/null || true
    rm -rf "$B/git" && cp -r /suites/git "$B/git" && cd "$B/git"
    make distclean >/dev/null 2>&1 || true
    # án (A): make -j hay segv zcc flaky — retry 3 lượt trước khi chịu thua
    make -j"$NP" V=1 CC="$ZCC" NO_RUST=1 NO_GETTEXT=1 NO_TCLTK=1 NO_CURL=1 \
        NO_EXPAT=1 FSMONITOR_DAEMON_BACKEND= FSMONITOR_OS_SETTINGS= \
      || make -j"$NP" CC="$ZCC" NO_RUST=1 NO_GETTEXT=1 NO_TCLTK=1 NO_CURL=1 \
        NO_EXPAT=1 FSMONITOR_DAEMON_BACKEND= FSMONITOR_OS_SETTINGS= \
      || make -j"$NP" CC="$ZCC" NO_RUST=1 NO_GETTEXT=1 NO_TCLTK=1 NO_CURL=1 \
        NO_EXPAT=1 FSMONITOR_DAEMON_BACKEND= FSMONITOR_OS_SETTINGS=
    ./git version
}
run git-build git_build
git_seek() {
    chown -R runner "$B/git"
    ok=0
    for tt in t7600-merge t8002-blame t8012-blame-colors t7112-reset-submodule; do
        su runner -c "cd '$B/git/t' && ./$tt.sh" > "$LOG/git-zcc-$tt.log" 2>&1 || true
        zf=$(grep -c '^not ok' "$LOG/git-zcc-$tt.log" || true)
        echo "$tt: zcc-fail=$zf" >> "$LOG/git-cluster-verdict.txt"
        [ "$zf" -eq 0 ] || ok=1
    done
    cat "$LOG/git-cluster-verdict.txt"
    return $ok
}
run git-cluster git_seek
git_refail() {
    list=$(grep -oE '^t[0-9][^ ]*\.sh' "$O6/git-t-full.log" | sort -u | tr '\n' ' ')
    [ -n "$list" ] || return 0
    echo "prove lại $(echo $list | wc -w) file fail của o6"
    su runner -c "cd '$B/git/t' && prove -j$NP $list"
}
run git-refail git_refail

# ---- sqlite-tcl (án A theo dõi + testfixture runtime giờ có fix decay)
sqlite_tcl() {
    set -e
    [ -d "$B/sqlite-src" ] || git clone --depth 1 https://github.com/sqlite/sqlite.git "$B/sqlite-src"
    cd "$B/sqlite-src"
    CC="$ZCC" ./configure || { cp config.log "$LOG/sqlite-tcl-config.log" 2>/dev/null; exit 1; }
    # án (A): retry make (segv flaky chỉ dưới song song/memory pressure)
    make -j"$NP" testfixture || make testfixture || make testfixture
    ./testfixture test/veryquick.test
}
run sqlite-tcl sqlite_tcl

# ---- redis: seek đúng unit chết → xanh thì full (đóng sổ)
redis_build() {
    set -e
    cd "$B/redis"
    make distclean >/dev/null 2>&1 || true
    make -j"$NP" CC="$ZCC" MALLOC=libc >/dev/null 2>&1 || make -j"$NP" CC="$ZCC" MALLOC=libc >/dev/null 2>&1
    ./src/redis-server --version
}
run redis-build redis_build
redis_seek() { cd "$B/redis" && ./runtest --single unit/networking; }
run redis-networking redis_seek
if grep -q "redis-networking: PASS" "$LOG/SUMMARY.txt"; then
    redis_full() { cd "$B/redis" && ./runtest --clients "$NP"; }
    run redis-runtest redis_full
fi

{
    echo "--- trích:"
    for f in gate-abi torture-pr92904 case-decay git-cluster; do
        [ -f "$LOG/$f.log" ] && printf '%s: %s\n' "$f" "$(tail -1 "$LOG/$f.log")"
    done
    for f in git-refail sqlite-tcl redis-networking redis-runtest; do
        [ -f "$LOG/$f.log" ] && { echo "== $f (tail):"; tail -6 "$LOG/$f.log"; }
    done
    echo "--- kết thúc: $(date)"
} >> "$LOG/SUMMARY.txt"
