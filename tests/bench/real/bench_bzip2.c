/* bench_bzip2.c — bzip2 1.0.8 compress and decompress as TWO ARMS, in memory.
 *
 * WHY TWO ARMS AND NOT ONE NUMBER. bzip2's two directions are not two samples of
 * one workload, they are two different programs:
 *
 *   COMPRESS is dominated by `blocksort.c` — a suffix sort (Bentley-Sedgewick
 *   ternary quicksort with a radix pre-pass, falling back to a doubling
 *   Sadakane-style sort) over a 900 KB block. That is unpredictable branches on
 *   comparison outcomes plus a scattered access pattern over a working set far
 *   larger than L2, and it is the single shape no other program on this surface
 *   has: sqlite's branches are cold, lua's are a dispatch table, zlib's matcher
 *   walks a hash chain of BOUNDED depth.
 *
 *   DECOMPRESS is dominated by `decompress.c`'s inverse BWT — one pass building
 *   a 900 K-entry permutation and then a POINTER CHASE through it, one dependent
 *   load per output byte with no locality at all. It is a pure latency workload
 *   where compress is a throughput one, and Law 3c says those are judged by
 *   different models.
 *
 * Reporting them separately is the point. A geomean of the two would hide
 * exactly the disagreement that is worth reading.
 *
 * CLEAN INPUT, and the work-pinning quantity. The line printed carries the
 * COMPRESSED LENGTH and a checksum of the decompressed bytes. The compressed
 * length pins every decision the block sorter and the Huffman coder made — two
 * builds that agree on it did the same work, not merely reached the same answer.
 * A build that agreed only on the final bytes could have sorted differently and
 * be timed on a different workload.
 */
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include "bzlib.h"

#define RAW (2u * 1024u * 1024u)

static char *raw, *cmp_buf, *out;

/* Deterministic input with the redundancy bzip2 exists for: a small dictionary
   of tokens laid down in a shifting pattern, so runs and repeats exist at
   several scales rather than at one. A random buffer would measure the sorter
   on incompressible data, which is not what anyone runs bzip2 on. */
static void make_input(void){
    static const char *w[16] = {
        "the ", "quick ", "brown ", "fox ", "jumps ", "over ", "lazy ", "dog ",
        "SELECT ", "FROM ", "WHERE ", "index_", "value=", "0123456789", "\n", "  "
    };
    unsigned i = 0, s = 12345u;
    while (i < RAW - 32) {
        const char *t;
        size_t n;
        s = s * 1103515245u + 12345u;
        t = w[(s >> 16) & 15];
        n = strlen(t);
        memcpy(raw + i, t, n);
        i += (unsigned)n;
    }
    memset(raw + i, ' ', RAW - i);
}

static unsigned long fnv(const char *p, unsigned n){
    unsigned long h = 2166136261UL; unsigned i;
    for (i = 0; i < n; i++) { h ^= (unsigned char)p[i]; h *= 16777619UL; }
    return h & 0xffffffffUL;
}

int main(int argc, char **argv){
    unsigned int clen, olen;
    int rc, reps, r;
    const char *arm = argc > 1 ? argv[1] : "both";

    raw = (char *)malloc(RAW);
    cmp_buf = (char *)malloc(RAW + RAW / 100 + 600);
    out = (char *)malloc(RAW);
    if (!raw || !cmp_buf || !out) return 2;
    make_input();

    /* one compress up front: the decompress arm needs something to read, and
       the clean-input line must be printed whichever arm is timed */
    clen = (unsigned int)(RAW + RAW / 100 + 600);
    rc = BZ2_bzBuffToBuffCompress(cmp_buf, &clen, raw, RAW, 9, 0, 30);
    if (rc != BZ_OK) { fprintf(stderr, "compress rc=%d\n", rc); return 2; }
    olen = RAW;
    rc = BZ2_bzBuffToBuffDecompress(out, &olen, cmp_buf, clen, 0, 0);
    if (rc != BZ_OK) { fprintf(stderr, "decompress rc=%d\n", rc); return 2; }
    if (olen != RAW || memcmp(out, raw, RAW) != 0) {
        fprintf(stderr, "ROUND TRIP MISMATCH\n"); return 2;
    }

    if (strcmp(arm, "compress") == 0) {
        reps = 3;
        for (r = 0; r < reps; r++) {
            unsigned int n = (unsigned int)(RAW + RAW / 100 + 600);
            rc = BZ2_bzBuffToBuffCompress(cmp_buf, &n, raw, RAW, 9, 0, 30);
            if (rc != BZ_OK) return 2;
        }
    } else if (strcmp(arm, "decompress") == 0) {
        reps = 8;
        for (r = 0; r < reps; r++) {
            unsigned int n = RAW;
            rc = BZ2_bzBuffToBuffDecompress(out, &n, cmp_buf, clen, 0, 0);
            if (rc != BZ_OK) return 2;
        }
    }
    printf("raw=%u comp=%u fnv=%lu\n", RAW, clen, fnv(out, olen));
    return 0;
}
