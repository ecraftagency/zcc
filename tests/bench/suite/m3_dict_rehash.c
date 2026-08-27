/* m3_dict_rehash — a REDIS-style hash table: hash, chain, and incremental
 * rehash.
 *
 * WHY IT IS HERE.  A dictionary is the data structure real server software
 * spends its time in, and its shape is absent from this suite: pointer chasing
 * through chains (dependent loads, unpredictable), a multiply-shift hash, a
 * modulo replaced by a mask, bucket-array reallocation, and the two-table
 * incremental rehash that redis runs so a resize never stalls a request.  The
 * loop bodies are short and the dependence chain is a LOAD, which is exactly
 * the case Law 3c says instruction count cannot see.
 */
#include <stdio.h>
#include <stdlib.h>

struct Entry { unsigned long key; long val; struct Entry *next; };

struct Dict {
    struct Entry **tab[2];
    unsigned long mask[2];
    long used[2];
    long rehash_idx;                    /* -1 when not rehashing */
};

static struct Entry pool[1 << 17];
static long pool_used;

static unsigned long hash(unsigned long k) {
    k ^= k >> 33;
    k *= 0xff51afd7ed558ccdUL;
    k ^= k >> 29;
    k *= 0xc4ceb9fe1a85ec53UL;
    k ^= k >> 32;
    return k;
}

static void step_rehash(struct Dict *d) {
    long moved = 0;
    if (d->rehash_idx < 0) return;
    while (moved < 10 && d->used[0] > 0) {
        struct Entry *e;
        while ((unsigned long)d->rehash_idx <= d->mask[0]
               && d->tab[0][d->rehash_idx] == 0)
            d->rehash_idx++;
        if ((unsigned long)d->rehash_idx > d->mask[0]) break;
        e = d->tab[0][d->rehash_idx];
        while (e) {
            struct Entry *nx = e->next;
            unsigned long i = hash(e->key) & d->mask[1];
            e->next = d->tab[1][i];
            d->tab[1][i] = e;
            d->used[0]--;
            d->used[1]++;
            e = nx;
            moved++;
        }
        d->tab[0][d->rehash_idx] = 0;
        d->rehash_idx++;
    }
    if (d->used[0] == 0) {
        free(d->tab[0]);
        d->tab[0] = d->tab[1];
        d->mask[0] = d->mask[1];
        d->used[0] = d->used[1];
        d->tab[1] = 0;
        d->used[1] = 0;
        d->rehash_idx = -1;
    }
}

static void expand(struct Dict *d) {
    unsigned long n = (d->mask[0] + 1) * 2;
    unsigned long i;
    d->tab[1] = malloc(n * sizeof *d->tab[1]);
    for (i = 0; i < n; i++) d->tab[1][i] = 0;
    d->mask[1] = n - 1;
    d->used[1] = 0;
    d->rehash_idx = 0;
}

static void put(struct Dict *d, unsigned long k, long v) {
    int t = (d->rehash_idx >= 0) ? 1 : 0;
    unsigned long i = hash(k) & d->mask[t];
    struct Entry *e = &pool[pool_used++ & ((1 << 17) - 1)];
    e->key = k;
    e->val = v;
    e->next = d->tab[t][i];
    d->tab[t][i] = e;
    d->used[t]++;
    if (d->rehash_idx < 0 && d->used[0] > (long)(d->mask[0] + 1)) expand(d);
    step_rehash(d);
}

static long get(struct Dict *d, unsigned long k) {
    unsigned long h = hash(k);
    int t;
    for (t = 0; t < 2; t++) {
        struct Entry *e;
        if (!d->tab[t]) continue;
        e = d->tab[t][h & d->mask[t]];
        while (e) {
            if (e->key == k) return e->val;
            e = e->next;
        }
    }
    return -1;
}

int main(void) {
    struct Dict d;
    unsigned long i;
    long s = 0, r;
    d.tab[0] = malloc(64 * sizeof *d.tab[0]);
    for (i = 0; i < 64; i++) d.tab[0][i] = 0;
    d.tab[1] = 0;
    d.mask[0] = 63;
    d.mask[1] = 0;
    d.used[0] = d.used[1] = 0;
    d.rehash_idx = -1;
    for (r = 0; r < 90000; r++) {
        unsigned long k = (unsigned long)(r * 2654435761UL) & 0xffff;
        put(&d, k + (unsigned long)(s & 1), r);
        s += get(&d, (k >> 1) ^ 0x5a5a);
        step_rehash(&d);
    }
    printf("%ld %ld\n", s, d.used[0] + d.used[1]);
    return 0;
}
