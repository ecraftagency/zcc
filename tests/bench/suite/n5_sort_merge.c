/* n5_sort_merge — SORT the two relations, then MERGE-JOIN them on the key.
 *
 * WHY IT IS HERE.  A database spends its time in two shapes this suite has
 * never sampled.  The first is a general-purpose sort: a quicksort whose every
 * comparison goes through a FUNCTION POINTER, because a real engine reaches its
 * collation indirectly and cannot inline it — so the inner loop is a call whose
 * target the compiler must assume it does not know, with the row struct live
 * across it and the partition pointers live across all of it.  That is a
 * register-residency question of exactly the kind `sqlite3VdbeSorterCompare`
 * poses, and instruction count cannot see it (Law 3c): the chain runs
 * load-key, call, compare, branch.
 *
 * The second is the merge itself — two cursors walking forward, duplicate runs
 * on both sides producing a cross product, and an unpredictable branch choosing
 * which cursor advances.  Nothing here is arithmetic-bound; it is bound by the
 * branch, the dependent load, and where the comparator's arguments live.
 *
 * The join below is, in SQL,
 *
 *     SELECT sum(...) FROM r, s WHERE r.key = s.key;
 *
 * planned the way a planner plans it when neither side carries an index.
 */
#include <stdio.h>

#define NROW  240000            /* rows per relation                     */
#define NKEY  (1 << 18)         /* key domain — small enough to duplicate */
#define CUT   12                /* below this, insertion sort            */

struct Row {
    long key;
    long v1;
    long v2;
};

static struct Row r[NROW];
static struct Row s[NROW];

typedef int (*cmpfn)(const struct Row *, const struct Row *);

/* The collation.  Reached only through a pointer, never by name — that
 * indirection is half of what this program is here to measure.  It is a total
 * order on the whole row so the sorted sequence, and therefore the checksum,
 * does not depend on the quicksort's instability. */
static int cmp_key(const struct Row *a, const struct Row *b) {
    if (a->key < b->key) return -1;
    if (a->key > b->key) return 1;
    if (a->v1 < b->v1) return -1;
    if (a->v1 > b->v1) return 1;
    if (a->v2 < b->v2) return -1;
    if (a->v2 > b->v2) return 1;
    return 0;
}

static void fill(struct Row *a, unsigned long seed) {
    unsigned long st = seed;
    long i;
    for (i = 0; i < NROW; i++) {
        st = st * 6364136223846793005UL + 1442695040888963407UL;
        a[i].key = (long)((st >> 33) & (NKEY - 1));
        a[i].v1 = (long)((st >> 17) & 0xffff) - 32768;
        a[i].v2 = (long)((st >> 5) & 0xfff);
    }
}

static void swap(struct Row *a, struct Row *b) {
    struct Row t = *a;
    *a = *b;
    *b = t;
}

static void insertion(struct Row *a, long lo, long hi, cmpfn cmp) {
    long i;
    for (i = lo + 1; i <= hi; i++) {
        struct Row t = a[i];
        long j = i - 1;
        while (j >= lo && cmp(&a[j], &t) > 0) {
            a[j + 1] = a[j];
            j--;
        }
        a[j + 1] = t;
    }
}

/* Hoare partition with a median-of-three pivot, recursing on the smaller side
 * and looping on the larger so the stack stays O(log n).  The median-of-three
 * leaves a[lo] <= pivot <= a[hi], which is what keeps both scans inside the
 * range without a bounds test in the inner loop. */
static void sort_rows(struct Row *a, long lo, long hi, cmpfn cmp) {
    while (hi - lo > CUT) {
        struct Row p;
        long i, j, m = lo + (hi - lo) / 2;
        if (cmp(&a[m], &a[lo]) < 0) swap(&a[m], &a[lo]);
        if (cmp(&a[hi], &a[lo]) < 0) swap(&a[hi], &a[lo]);
        if (cmp(&a[hi], &a[m]) < 0) swap(&a[hi], &a[m]);
        p = a[m];
        i = lo - 1;
        j = hi + 1;
        for (;;) {
            do i++; while (cmp(&a[i], &p) < 0);
            do j--; while (cmp(&a[j], &p) > 0);
            if (i >= j) break;
            swap(&a[i], &a[j]);
        }
        if (j - lo < hi - j) {
            sort_rows(a, lo, j, cmp);
            lo = j + 1;
        } else {
            sort_rows(a, j + 1, hi, cmp);
            hi = j;
        }
    }
    insertion(a, lo, hi, cmp);
}

/* The merge.  Both sides carry duplicate runs, so an equal key produces the
 * cross product of the two runs; the accumulator is a sum, hence commutative,
 * so the checksum cannot depend on the order the pairs come out in. */
static unsigned long merge_join(long *npair) {
    unsigned long sum = 0;
    long i = 0, j = 0, n = 0;
    while (i < NROW && j < NROW) {
        if (r[i].key < s[j].key) {
            i++;
        } else if (r[i].key > s[j].key) {
            j++;
        } else {
            long k = r[i].key, i2 = i, j2 = j, x, y;
            while (i2 < NROW && r[i2].key == k) i2++;
            while (j2 < NROW && s[j2].key == k) j2++;
            for (x = i; x < i2; x++) {
                for (y = j; y < j2; y++) {
                    unsigned long a = (unsigned long)(r[x].v1 + s[y].v1);
                    unsigned long b = (unsigned long)(r[x].v2 ^ s[y].v2);
                    sum += a * 0x9e3779b97f4a7c15UL + b + (unsigned long)k;
                    n++;
                }
            }
            i = i2;
            j = j2;
        }
    }
    *npair = n;
    return sum;
}

int main(void) {
    cmpfn cmp = cmp_key;
    unsigned long sum;
    long npair = 0, i;
    unsigned long ck = 0;

    fill(r, 0x2545f4914f6cdd1dUL);
    fill(s, 0x853c49e6748fea9bUL);

    sort_rows(r, 0, NROW - 1, cmp);
    sort_rows(s, 0, NROW - 1, cmp);

    /* the sort is part of the answer, not just a preparation for the join */
    for (i = 0; i < NROW; i += 997)
        ck += (unsigned long)(r[i].key * 3 + s[i].key);

    sum = merge_join(&npair);
    printf("%lu %ld %lu\n", sum, npair, ck);
    return 0;
}
