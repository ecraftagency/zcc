#!/bin/sh
# M12 — atomics __sync_*: N thread cùng băm counter chung, kết quả phải CHÍNH XÁC
# N*M (atomic hỏng = mất update = sai số ngay). Suy ra từ tiền đề:
#   N=4  — M1 có ≥4 performance core, đủ tranh chấp thật sự;
#   M=100000 — đủ lớn để non-atomic gần như chắc chắn mất update, đủ nhỏ chạy <1s.
# Phần 2: trích NGUYÊN VĂN họ macro __sync trong atomicvar.h của redis
# (path HAVE_ATOMIC) — đúng gate "atomicvar.h chọn được path compile sạch".
# So stdout với trọng tài cc, chạy 3 lần (bẫy ASLR + bẫy scheduling).
set -e
cd "$(dirname "$0")/.."
# ZCC đặt sẵn từ env (box Linux) — không thì tự build
if [ -z "$ZCC" ]; then
    cargo build
    ZCC="$PWD/target/debug/zcc"
fi
WORK="${1:-$(mktemp -d)}"

cat > "$WORK/gate.c" <<'EOF'
#include <stdio.h>
#include <pthread.h>

#define NTH 4
#define M 100000

/* trích nguyên văn atomicvar.h (redis, path HAVE_ATOMIC dùng __sync builtins) */
#define atomicIncr(var,count) __sync_add_and_fetch(&var,(count))
#define atomicGetIncr(var,oldvalue_var,count) do { \
    oldvalue_var = __sync_fetch_and_add(&var,(count)); \
} while(0)
#define atomicDecr(var,count) __sync_sub_and_fetch(&var,(count))
#define atomicGet(var,dstvar) do { \
    dstvar = __sync_sub_and_fetch(&var,0); \
} while(0)
#define atomicSet(var,value) while(!__sync_bool_compare_and_swap(&var,var,value))

static long counter;  /* fetch_add trần */
static int lock;      /* spinlock: CAS lấy, lock_release trả */
static long guarded;  /* biến THƯỜNG được spinlock bảo vệ */
static long rcounter; /* đi qua macro redis */

static void *worker(void *arg) {
    int k;
    long old;
    for (k = 0; k < M; k++) {
        __sync_fetch_and_add(&counter, 1);
        atomicGetIncr(rcounter, old, 1);
        while (!__sync_bool_compare_and_swap(&lock, 0, 1))
            ;
        guarded++;
        __sync_synchronize();
        __sync_lock_release(&lock);
    }
    (void)old;
    return arg;
}

int main(void) {
    pthread_t th[NTH];
    int k;
    long got;
    for (k = 0; k < NTH; k++) pthread_create(&th[k], 0, worker, 0);
    for (k = 0; k < NTH; k++) pthread_join(th[k], 0);
    atomicIncr(rcounter, 7);
    atomicDecr(rcounter, 7);
    atomicSet(counter, counter);
    atomicGet(rcounter, got);
    printf("counter=%ld guarded=%ld redis=%ld\n", counter, guarded, got);
    return 0;
}
EOF

cc -w -O0 "$WORK/gate.c" -o "$WORK/gate.ref"
"$WORK/gate.ref" > "$WORK/ref.txt"
"$ZCC" "$WORK/gate.c" -o "$WORK/gate.zcc"
for i in 1 2 3; do
    "$WORK/gate.zcc" > "$WORK/out.txt"
    cmp "$WORK/ref.txt" "$WORK/out.txt"
done
echo "M12 PASS: 4 thread x 100000 — counter/spinlock/macro redis đều chính xác, 3/3"
