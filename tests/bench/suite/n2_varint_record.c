/* n2_varint_record — VARINT DECODE AND RECORD UNPACK, the sqlite
 * `sqlite3VdbeRecordUnpack` / `sqlite3GetVarint32` shape.
 *
 * WHY IT IS HERE.  Between the page and the interpreter sits the serial format,
 * and it is where sqlite turns bytes back into values: every row read by every
 * query passes through a varint decode and a switch on a serial type, once per
 * column.  Nothing in this suite has that shape.  n3_vdbe_loop has a big switch
 * but its operands are already integers in a register file; m3_dict_rehash
 * chases pointers but every field it touches is a word at a fixed offset.  Here
 * the *width of the next load is not known until the previous one has been
 * decoded* — the serial type says how many bytes the body occupies, so the
 * pointer bump feeding the next iteration depends on the switch arm the current
 * one took.  That is a data-dependent, unpredictable chain of short loads, and
 * Law 3c says it is judged by that chain and not by how many instructions the
 * arms contain.
 *
 * The varint itself is the other half.  Seven bits to the byte, high bit as the
 * continuation flag, big-endian, one to nine bytes — so decoding it is a loop
 * whose trip count is a property of the data, with a shift, an or and a
 * bit-test per byte and a branch that leaves the loop from the middle.  A
 * compiler that unrolls it wrongly, or that fails to keep the accumulator in a
 * register across the test, pays for it on every column of every row.
 *
 * The codec is exercised at both ends.  A small round-trip table walks the
 * boundary value of every one of the nine widths, so no encoder branch ships
 * untested; the bulk workload then uses the one- and two-byte cases that real
 * records are made of, which is where the time actually goes.
 */
#include <stdio.h>
#include <string.h>

#define NREC     220000
#define NBYTES   (15 * 1024 * 1024)
#define MAXCOL   8
#define PASSES   3

static unsigned char rec[NBYTES];
static unsigned long recn;              /* bytes of `rec` in use */
static long          nrec;

/* one decoded column, the `Mem` an unpacked record hands to the VDBE */
struct Val {
    long                 i;
    double               r;
    const unsigned char *z;
    long                 n;
    int                  type;          /* 0 null 1 int 2 real 3 blob/text */
};

static struct Val vals[MAXCOL];

/* --- the varint codec ----------------------------------------------------- */

static int put_varint(unsigned char *p, unsigned long v) {
    unsigned char buf[10];
    int i, j, n;

    if (v & (0xffUL << 56)) {           /* needs the full nine bytes */
        p[8] = (unsigned char)(v & 0xff);
        v >>= 8;
        for (i = 7; i >= 0; i--) {
            p[i] = (unsigned char)((v & 0x7f) | 0x80);
            v >>= 7;
        }
        return 9;
    }
    n = 0;
    do {
        buf[n++] = (unsigned char)(v & 0x7f);
        v >>= 7;
    } while (v != 0);
    for (i = 0, j = n - 1; j >= 0; i++, j--)
        p[i] = (unsigned char)(buf[j] | (j > 0 ? 0x80 : 0));
    return n;
}

static int get_varint(const unsigned char *p, unsigned long *out) {
    unsigned long v = 0;
    int i;
    for (i = 0; i < 8; i++) {
        v = (v << 7) | (unsigned long)(p[i] & 0x7f);
        if ((p[i] & 0x80) == 0) { *out = v; return i + 1; }
    }
    *out = (v << 8) | (unsigned long)p[8];
    return 9;
}

/* --- bodies: big-endian signed integers, and an IEEE double ---------------- */

static void put_be(unsigned char *p, unsigned long v, int w) {
    int i;
    for (i = w - 1; i >= 0; i--) {
        p[i] = (unsigned char)(v & 0xff);
        v >>= 8;
    }
}

/* sign-extend a w-byte big-endian field without ever overflowing a signed type */
static long get_be(const unsigned char *p, int w) {
    unsigned long v = 0;
    int i;
    for (i = 0; i < w; i++) v = (v << 8) | (unsigned long)p[i];
    if (w == 8) {
        if (v <= 0x7fffffffffffffffUL) return (long)v;
        return -(long)(~v) - 1;
    }
    {
        unsigned long sign = 1UL << (w * 8 - 1);
        if (v >= sign) return (long)(v - sign) - (long)sign;
        return (long)v;
    }
}

static void put_double(unsigned char *p, double d) {
    unsigned char b[8];
    int i;
    memcpy(b, &d, 8);
    for (i = 0; i < 8; i++) p[i] = b[7 - i];
}

static double get_double(const unsigned char *p) {
    unsigned char b[8];
    double d;
    int i;
    for (i = 0; i < 8; i++) b[i] = p[7 - i];
    memcpy(&d, b, 8);
    return d;
}

/* --- sqlite3VdbeRecordUnpack: the hot loop -------------------------------- */

static unsigned long unpack(const unsigned char *p, int *ncol) {
    const unsigned char *hp, *hend, *bp;
    unsigned long hdrlen, t;
    int nc = 0;

    hp = p + get_varint(p, &hdrlen);
    hend = p + hdrlen;
    bp = hend;

    while (hp < hend && nc < MAXCOL) {
        hp += get_varint(hp, &t);
        switch ((int)t) {
        case 0:                                     /* NULL */
            vals[nc].type = 0;
            vals[nc].i = 0;
            break;
        case 1: case 2: case 3: case 4: {
            int w = (int)t;
            vals[nc].type = 1;
            vals[nc].i = get_be(bp, w);
            bp += w;
            break;
        }
        case 5:
            vals[nc].type = 1;
            vals[nc].i = get_be(bp, 6);
            bp += 6;
            break;
        case 6:
            vals[nc].type = 1;
            vals[nc].i = get_be(bp, 8);
            bp += 8;
            break;
        case 7:
            vals[nc].type = 2;
            vals[nc].r = get_double(bp);
            bp += 8;
            break;
        case 8:                                     /* the constant 0 */
            vals[nc].type = 1;
            vals[nc].i = 0;
            break;
        case 9:                                     /* the constant 1 */
            vals[nc].type = 1;
            vals[nc].i = 1;
            break;
        default: {
            /* >=12 even is a blob, >=13 odd is text; the parity picks the base */
            unsigned long n = (t & 1) ? (t - 13) / 2 : (t - 12) / 2;
            vals[nc].type = 3;
            vals[nc].n = (long)n;
            vals[nc].z = bp;
            bp += n;
            break;
        }
        }
        nc++;
    }
    *ncol = nc;
    return (unsigned long)(bp - p);
}

/* --- building the corpus -------------------------------------------------- */

static void build(void) {
    unsigned long s = 0x2545f4914f6cdd1dUL;
    unsigned char hdr[2 * MAXCOL + 2];
    unsigned char body[1024];
    long r;

    for (r = 0; r < NREC; r++) {
        unsigned long t[MAXCOL];
        int nc, c, bn = 0, hn, hlen;

        if (recn + 1024 > NBYTES) break;

        s = s * 6364136223846793005UL + 1442695040888963407UL;
        nc = 4 + (int)((s >> 45) % 4);              /* 4..7 columns */

        for (c = 0; c < nc; c++) {
            unsigned long v;
            s = s * 6364136223846793005UL + 1442695040888963407UL;
            v = s >> 11;
            switch ((int)(s & 15)) {
            case 0: case 1: case 2:
                t[c] = 1; put_be(body + bn, v, 1); bn += 1; break;
            case 3: case 4:
                t[c] = 2; put_be(body + bn, v, 2); bn += 2; break;
            case 5:
                t[c] = 3; put_be(body + bn, v, 3); bn += 3; break;
            case 6: case 7:
                t[c] = 4; put_be(body + bn, v, 4); bn += 4; break;
            case 8:
                t[c] = 5; put_be(body + bn, v, 6); bn += 6; break;
            case 9:
                t[c] = 6; put_be(body + bn, v, 8); bn += 8; break;
            case 10: {
                /* an exact multiple of 1/4 in [-2048,2048), so that summing
                 * them is exact and the checksum cannot move with FP flags */
                double d = (double)((long)(v & 0x3fff) - 8192) / 4.0;
                t[c] = 7; put_double(body + bn, d); bn += 8; break;
            }
            case 11:
                t[c] = 0; break;                    /* NULL, no body */
            case 12: case 13: case 14: {
                unsigned long n = 4 + (v % 24);
                unsigned long k;
                t[c] = 13 + 2 * n;                  /* text */
                for (k = 0; k < n; k++)
                    body[bn + k] = (unsigned char)(0x41 + ((v >> k) & 15));
                bn += (int)n;
                break;
            }
            default: {
                /* long enough that the serial type needs a two-byte varint */
                unsigned long n = 58 + (v % 40);
                unsigned long k;
                t[c] = 12 + 2 * n;                  /* blob */
                for (k = 0; k < n; k++)
                    body[bn + k] = (unsigned char)((v >> (k & 7)) ^ k);
                bn += (int)n;
                break;
            }
            }
        }

        /* the header: its own length first, then one serial type per column */
        hn = 0;
        for (c = 0; c < nc; c++) hn += put_varint(hdr + hn, t[c]);
        hlen = hn + 1;                              /* the length varint fits in one byte */
        put_varint(rec + recn, (unsigned long)hlen);
        memcpy(rec + recn + 1, hdr, (size_t)hn);
        memcpy(rec + recn + 1 + hn, body, (size_t)bn);
        recn += (unsigned long)(hlen + bn);
        nrec++;
    }
}

/* --- the nine-width round trip -------------------------------------------- */

static unsigned long codec_check(void) {
    static const unsigned long probe[] = {
        0UL, 1UL,
        0x7fUL, 0x80UL,
        0x3fffUL, 0x4000UL,
        0x1fffffUL, 0x200000UL,
        0xfffffffUL, 0x10000000UL,
        0x7ffffffffUL, 0x800000000UL,
        0x3ffffffffffUL, 0x40000000000UL,
        0x1ffffffffffffUL, 0x2000000000000UL,
        0xffffffffffffffUL, 0x100000000000000UL,
        0xffffffffffffffffUL
    };
    unsigned char buf[10];
    unsigned long acc = 0, got;
    unsigned i;
    for (i = 0; i < sizeof probe / sizeof probe[0]; i++) {
        int n = put_varint(buf, probe[i]);
        int m = get_varint(buf, &got);
        acc = acc * 1000003UL + (unsigned long)n;
        acc = acc * 1000003UL + (unsigned long)m;
        acc = acc * 1000003UL + (got == probe[i] ? 1UL : 0UL);
    }
    return acc;
}

int main(void) {
    unsigned long sum = codec_check();
    unsigned long nblob = 0;
    double dsum = 0.0;
    long ncols = 0;
    int pass;

    build();

    for (pass = 0; pass < PASSES; pass++) {
        unsigned long off = 0;
        long r;
        for (r = 0; r < nrec; r++) {
            int nc, c;
            off += unpack(rec + off, &nc);
            ncols += nc;
            for (c = 0; c < nc; c++) {
                switch (vals[c].type) {
                case 0:
                    sum = sum * 31UL + 0x9e37UL;
                    break;
                case 1:
                    sum = sum * 31UL + (unsigned long)vals[c].i;
                    break;
                case 2:
                    dsum += vals[c].r;
                    break;
                default: {
                    long k;
                    unsigned long b = 0;
                    for (k = 0; k < vals[c].n; k++) b += vals[c].z[k];
                    sum = sum * 31UL + b + (unsigned long)vals[c].n;
                    nblob++;
                    break;
                }
                }
            }
        }
    }

    printf("%lu %ld %ld %lu %ld\n",
           sum, nrec, ncols, nblob, (long)(dsum * 4.0));
    return 0;
}
