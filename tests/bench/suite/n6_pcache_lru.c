/* n6_pcache_lru — the `pcache1Fetch` shape: an open hash table and an LRU list
 * threaded through the same headers.
 *
 * WHY IT IS HERE.  Every page a database touches arrives through one function,
 * and in sqlite that function is `pcache1Fetch`.  Its shape is a pair of data
 * structures sharing one node: a bucket chain reached by a hash of the page
 * number, and a doubly-linked recency list reached by nothing at all — you are
 * already holding the node.  A hit is a short dependent-load walk followed by
 * six pointer stores to unlink and relink; a miss adds the victim's eviction,
 * which means re-deriving the OLD key's bucket and walking that chain a second
 * time to find the predecessor.
 *
 * Nothing in this suite samples that.  The loop body is almost all loads and
 * stores through pointers that alias each other in ways a compiler must assume
 * and cannot disprove — `p->lnext->lprev = p->lprev` is the canonical case —
 * so what is measured is how many of the six live pointers stay in registers
 * across the aliasing stores, not how much arithmetic is done.  Law 3c again:
 * the chain is load, load, store, and the instruction count is nearly fixed.
 *
 * The workload is the one that makes a cache a cache: a hot working set that
 * fits, plus a scanning tail that does not, so the victim path runs often
 * enough to be measured rather than predicted away.
 */
#include <stdio.h>

#define PGSZ     128            /* bytes of page payload per header      */
#define NCACHE   4096           /* headers, i.e. the cache size in pages */
#define NBUCKET  8192           /* power of two, so the modulo is a mask */
#define NHOT     3000           /* the working set — fits in the cache   */
#define NSCAN    100000         /* the scanned relation — does not       */
#define NFETCH   800000         /* fetches driven through the cache      */

struct PgHdr {
    unsigned long pgno;
    struct PgHdr *hnext;                /* bucket chain            */
    struct PgHdr *lprev, *lnext;        /* LRU list, MRU at head   */
    unsigned char data[PGSZ];
};

static struct PgHdr pool[NCACHE];
static struct PgHdr *bucket[NBUCKET];
static struct PgHdr *lru_head, *lru_tail;
static long nalloc;
static long nmiss;

static unsigned long hash(unsigned long k) {
    k *= 0x9e3779b97f4a7c15UL;
    k ^= k >> 31;
    k *= 0xbf58476d1ce4e5b9UL;
    k ^= k >> 29;
    return k;
}

static void lru_unlink(struct PgHdr *p) {
    if (p->lprev) p->lprev->lnext = p->lnext;
    else lru_head = p->lnext;
    if (p->lnext) p->lnext->lprev = p->lprev;
    else lru_tail = p->lprev;
}

static void lru_push(struct PgHdr *p) {
    p->lprev = 0;
    p->lnext = lru_head;
    if (lru_head) lru_head->lprev = p;
    else lru_tail = p;
    lru_head = p;
}

/* the "read from disk" — deterministic in the page number alone */
static void page_load(struct PgHdr *p, unsigned long pgno) {
    unsigned long st = pgno * 6364136223846793005UL + 1442695040888963407UL;
    int i;
    for (i = 0; i < PGSZ; i++) {
        st = st * 6364136223846793005UL + 1442695040888963407UL;
        p->data[i] = (unsigned char)(st >> 40);
    }
}

static struct PgHdr *fetch(unsigned long pgno) {
    unsigned long h = hash(pgno) & (NBUCKET - 1);
    struct PgHdr *p = bucket[h];

    while (p && p->pgno != pgno) p = p->hnext;
    if (p) {                            /* hit: promote to MRU */
        lru_unlink(p);
        lru_push(p);
        return p;
    }

    nmiss++;
    if (nalloc < NCACHE) {
        p = &pool[nalloc++];            /* cache not full yet  */
    } else {
        struct PgHdr **pp;
        unsigned long oh;
        p = lru_tail;                   /* the victim          */
        lru_unlink(p);
        oh = hash(p->pgno) & (NBUCKET - 1);
        pp = &bucket[oh];               /* walk to its predecessor */
        while (*pp != p) pp = &(*pp)->hnext;
        *pp = p->hnext;
    }
    p->pgno = pgno;
    p->hnext = bucket[h];               /* rehash under the new key */
    bucket[h] = p;
    lru_push(p);
    page_load(p, pgno);
    return p;
}

int main(void) {
    unsigned long st = 0x2545f4914f6cdd1dUL;
    unsigned long ck = 0;
    unsigned long scan = 0;
    long i;

    for (i = 0; i < NFETCH; i++) {
        struct PgHdr *p;
        unsigned long pgno;
        unsigned o;
        st = st * 6364136223846793005UL + 1442695040888963407UL;
        if ((st >> 40) % 100 < 78)
            pgno = (st >> 13) % NHOT;                   /* the hot set */
        else
            pgno = NHOT + (scan++ % NSCAN);             /* the scan    */

        p = fetch(pgno);

        /* touch the page, so the traffic is real and the checksum depends on
         * the contents rather than on which header happened to hold them */
        o = (unsigned)(pgno & (PGSZ - 1));
        ck += p->data[o];
        ck += (unsigned long)p->data[(o + 37) & (PGSZ - 1)] << 3;
        ck ^= (unsigned long)p->data[(o + 91) & (PGSZ - 1)] << 11;
        p->data[(o + 7) & (PGSZ - 1)] += (unsigned char)(i & 0xff);
        ck += p->pgno;
    }

    printf("%lu %ld %ld\n", ck, nmiss, nalloc);
    return 0;
}
