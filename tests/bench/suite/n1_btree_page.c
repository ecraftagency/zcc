/* n1_btree_page — a B-TREE PAGE: the sqlite `allocateSpace` / `insertCell` /
 * `sqlite3BtreeMovetoUnpacked` shape.
 *
 * WHY IT IS HERE.  Everything else in this suite reaches memory through a typed
 * pointer, and so every load it issues is a load the compiler already knows the
 * width and alignment of.  A btree page is the opposite: 4096 raw bytes whose
 * entire structure lives in arithmetic — a 2-byte big-endian cell count, a
 * 2-byte big-endian free-space pointer, an array of 2-byte big-endian cell
 * offsets growing up from the header, and variable-length cells growing down
 * from the end of the page to meet it.  Every one of those fields is read a
 * byte at a time and reassembled with a shift and an or, because a `short *`
 * cast onto a page buffer would be both an alignment and an aliasing violation,
 * and sqlite spends a large fraction of its page time in exactly that code.
 *
 * Three shapes live here that zcc has nowhere else.  The first is the byte-pair
 * load/store itself (`get2`/`put2`): two byte loads, a shift, an or, and the
 * whole question is whether the backend keeps that to the two instructions it
 * is worth.  The second is a binary search whose comparison is an *indirect*,
 * variable-length byte compare — the offset array is loaded, the offset is
 * reassembled, the cell is loaded, and only then does the compare happen, so
 * the loop's dependence chain runs load -> reassemble -> compare -> branch with
 * nothing to overlap it with, which is the Law 3c case in its purest form.  The
 * third is defragmentation: a copy loop over every live cell, fired every few
 * dozen inserts once deletions have left the free space in pieces.
 *
 * There is no split here.  A real btree would balance into a sibling page; this
 * keeps a fixed set of pages at a high-water mark so the interesting path — run
 * out of contiguous room, reclaim the fragments, carry on — is the one taken
 * over and over.
 */
#include <stdio.h>
#include <string.h>

#define PAGESZ  4096
#define HDRSZ   4               /* ncell(2) + cell-content top(2) */
#define NPAGE   16
#define WATER   100             /* cells kept per page */
#define MAXKEY  12
#define MAXPAY  32
#define NRING   64              /* recently inserted keys, re-searched later */
#define ROUNDS  190000

static unsigned char pages[NPAGE][PAGESZ];
static unsigned      frag[NPAGE];       /* bytes freed but not yet reclaimed */
static unsigned char scratch[PAGESZ];

static unsigned char ring[NRING][MAXKEY];
static unsigned      ringlen[NRING];
static int           ringpg[NRING];

/* --- the two-byte big-endian accessors; never a short* cast --------------- */

static unsigned get2(const unsigned char *p) {
    return ((unsigned)p[0] << 8) | (unsigned)p[1];
}

static void put2(unsigned char *p, unsigned v) {
    p[0] = (unsigned char)((v >> 8) & 0xff);
    p[1] = (unsigned char)(v & 0xff);
}

/* A cell is: size(2) | keylen(2) | key bytes | payload bytes. */
#define CELLHDR 4

static void page_init(int p) {
    put2(pages[p], 0);
    put2(pages[p] + 2, PAGESZ);
    frag[p] = 0;
}

/* --- the comparison the binary search runs -------------------------------- */

static int cell_cmp(const unsigned char *pg, unsigned off,
                    const unsigned char *key, unsigned klen) {
    const unsigned char *ck = pg + off + CELLHDR;
    unsigned cklen = get2(pg + off + 2);
    unsigned n = cklen < klen ? cklen : klen;
    unsigned i;
    for (i = 0; i < n; i++) {
        if (ck[i] != key[i]) return ck[i] < key[i] ? -1 : 1;
    }
    if (cklen == klen) return 0;
    return cklen < klen ? -1 : 1;
}

/* sqlite3BtreeMovetoUnpacked: returns the slot, and whether it was a hit. */
static int page_search(const unsigned char *pg,
                       const unsigned char *key, unsigned klen, int *found) {
    int lo = 0;
    int hi = (int)get2(pg) - 1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        unsigned off = get2(pg + HDRSZ + 2 * mid);
        int c = cell_cmp(pg, off, key, klen);
        if (c == 0) { *found = 1; return mid; }
        if (c < 0) lo = mid + 1; else hi = mid - 1;
    }
    *found = 0;
    return lo;
}

/* --- defragmentPage: live cells packed back against the end of the page ---- */

static void page_defragment(int p) {
    unsigned char *pg = pages[p];
    int n = (int)get2(pg);
    unsigned top = PAGESZ;
    int i;
    put2(scratch, (unsigned)n);
    for (i = 0; i < n; i++) {
        unsigned off = get2(pg + HDRSZ + 2 * i);
        unsigned sz = get2(pg + off);
        top -= sz;
        memcpy(scratch + top, pg + off, sz);
        put2(scratch + HDRSZ + 2 * i, top);
    }
    put2(scratch + 2, top);
    memcpy(pg, scratch, PAGESZ);
    frag[p] = 0;
}

/* --- allocateSpace + insertCell ------------------------------------------- */

static int page_insert(int p, const unsigned char *key, unsigned klen,
                       const unsigned char *pay, unsigned plen, int idx) {
    unsigned char *pg = pages[p];
    unsigned n = get2(pg);
    unsigned sz = CELLHDR + klen + plen;
    unsigned need = sz + 2;             /* the cell, and its offset slot */
    unsigned top = get2(pg + 2);
    unsigned gap = top - (HDRSZ + 2 * n);

    if (gap < need) {
        if (gap + frag[p] < need) return 0;         /* genuinely full */
        page_defragment(p);
        top = get2(pg + 2);
        gap = top - (HDRSZ + 2 * n);
    }

    top -= sz;
    put2(pg + 2, top);
    put2(pg + top, sz);
    put2(pg + top + 2, klen);
    memcpy(pg + top + CELLHDR, key, klen);
    memcpy(pg + top + CELLHDR + klen, pay, plen);

    memmove(pg + HDRSZ + 2 * (idx + 1), pg + HDRSZ + 2 * idx,
            2 * (n - (unsigned)idx));
    put2(pg + HDRSZ + 2 * idx, top);
    put2(pg, n + 1);
    return 1;
}

/* dropCell: the slot goes, the bytes become fragments until the next defrag. */
static unsigned page_delete(int p, int idx) {
    unsigned char *pg = pages[p];
    unsigned n = get2(pg);
    unsigned off = get2(pg + HDRSZ + 2 * idx);
    unsigned sz = get2(pg + off);
    frag[p] += sz;
    memmove(pg + HDRSZ + 2 * idx, pg + HDRSZ + 2 * (idx + 1),
            2 * (n - (unsigned)idx - 1));
    put2(pg, n - 1);
    return sz;
}

int main(void) {
    unsigned char key[MAXKEY], pay[MAXPAY];
    unsigned long s = 0x2545f4914f6cdd1dUL;
    unsigned long probe = 0, freed = 0;
    long hits = 0, ins = 0, full = 0;
    long r;
    int p;

    for (p = 0; p < NPAGE; p++) page_init(p);

    for (r = 0; r < ROUNDS; r++) {
        unsigned klen, plen, i;
        int idx, found;
        unsigned char *pg;

        s = s * 6364136223846793005UL + 1442695040888963407UL;
        klen = 4 + (unsigned)((s >> 40) % (MAXKEY - 3));
        plen = 8 + (unsigned)((s >> 24) % (MAXPAY - 7));
        p = (int)((s >> 17) & (NPAGE - 1));
        pg = pages[p];

        for (i = 0; i < klen; i++)
            key[i] = (unsigned char)(((s >> ((i & 7) * 8)) & 0xff)
                                     ^ (unsigned char)(i * 37));
        for (i = 0; i < plen; i++)
            pay[i] = (unsigned char)(key[i % klen] ^ (unsigned char)i);

        idx = page_search(pg, key, klen, &found);
        probe += (unsigned long)idx;
        if (found) {
            hits++;
        } else if (page_insert(p, key, klen, pay, plen, idx)) {
            unsigned slot = (unsigned)ins & (NRING - 1);
            memcpy(ring[slot], key, klen);
            ringlen[slot] = klen;
            ringpg[slot] = p;
            ins++;
        } else {
            full++;
            page_init(p);
        }

        /* re-search a key that really was inserted: this is the lookup half of
         * the workload, and the half where the byte compare runs to the end of
         * the key instead of failing on its first byte */
        {
            unsigned slot = (unsigned)((r * 7 + 3) & (NRING - 1));
            if (ringlen[slot]) {
                int j = page_search(pages[ringpg[slot]], ring[slot],
                                    ringlen[slot], &found);
                probe += (unsigned long)j;
                hits += found;
            }
        }

        /* hold the page at its high-water mark, which is what keeps the free
         * space fragmented and the defragmenter busy */
        while (get2(pg) > WATER) {
            int victim;
            s = s * 6364136223846793005UL + 1442695040888963407UL;
            victim = (int)((s >> 33) % get2(pg));
            freed += page_delete(p, victim);
        }
    }

    for (p = 0; p < NPAGE; p++) {
        unsigned n = get2(pages[p]);
        unsigned i;
        probe = probe * 1000003UL + n;
        for (i = 0; i < n; i++) probe += get2(pages[p] + HDRSZ + 2 * i);
        probe += get2(pages[p] + 2) + frag[p];
    }

    printf("%lu %ld %ld %ld %lu\n", probe, ins, hits, full, freed);
    return 0;
}
