/* n4_hash_agg — GROUP BY over a hash aggregate, the shape a query planner
 * produces for `SELECT g, sum(v), count(*), min(v), max(v) FROM t GROUP BY g`.
 *
 * WHY IT IS HERE.  The suite's aggregate programs (`j1_reduction`,
 * `j2_histogram`) accumulate into a small dense array whose whole working set
 * fits in L1.  Real grouping does not: the group key must be hashed, the bucket
 * chased through a chain of dependent loads, the accumulator READ-MODIFIED-
 * WRITTEN in memory rather than held in a register, and the table is large
 * enough that the load misses.  That combination — a hash, a dependent chain,
 * and a memory accumulator under a branch — is what the localizer keeps finding
 * in sqlite, and instruction count cannot see any of it (Law 3c).
 *
 * The second half is the ordered scan out of the table, which is where a
 * grouped query spends the rest of its time.
 */
#include <stdio.h>

#define NROW   600000
#define NBUCK  8192            /* power of two: the modulo is a mask */
#define NGRP   40000           /* distinct group keys, so chains are real */

struct Group {
    long key;
    long sum;
    long count;
    long min;
    long max;
    long next;                 /* index into `pool`, -1 at the end of a chain */
};

static long col_g[NROW];
static long col_v[NROW];
static struct Group pool[NGRP + 1];
static long bucket[NBUCK];
static long pool_used;

static unsigned long mix(unsigned long k) {
    k ^= k >> 33;
    k *= 0xff51afd7ed558ccdUL;
    k ^= k >> 29;
    return k;
}

static void fill(void) {
    unsigned long s = 0x9e3779b97f4a7c15UL;
    long i;
    for (i = 0; i < NROW; i++) {
        s = s * 6364136223846793005UL + 1442695040888963407UL;
        col_g[i] = (long)((s >> 31) % NGRP);
        col_v[i] = (long)((s >> 13) & 0xffff) - 32768;
    }
}

/* the aggregate step: find or create the group, then read-modify-write it */
static void step(long g, long v) {
    unsigned long h = mix((unsigned long)g) & (NBUCK - 1);
    long e = bucket[h];
    while (e >= 0) {
        if (pool[e].key == g) {
            pool[e].sum += v;
            pool[e].count++;
            if (v < pool[e].min) pool[e].min = v;
            if (v > pool[e].max) pool[e].max = v;
            return;
        }
        e = pool[e].next;
    }
    if (pool_used >= NGRP) return;
    e = pool_used++;
    pool[e].key = g;
    pool[e].sum = v;
    pool[e].count = 1;
    pool[e].min = v;
    pool[e].max = v;
    pool[e].next = bucket[h];
    bucket[h] = e;
}

int main(void) {
    long i, chk = 0, groups = 0;

    for (i = 0; i < NBUCK; i++) bucket[i] = -1;
    fill();

    for (i = 0; i < NROW; i++) step(col_g[i], col_v[i]);

    /* the ordered scan out: bucket order is an implementation detail, so the
     * checksum is taken over the pool in creation order, which is determined by
     * the data alone */
    for (i = 0; i < pool_used; i++) {
        long avg = pool[i].count ? pool[i].sum / pool[i].count : 0;
        chk += pool[i].key * 31 + avg * 7 + pool[i].count + pool[i].min + pool[i].max;
        groups++;
    }
    printf("%ld %ld\n", chk, groups);
    return 0;
}
