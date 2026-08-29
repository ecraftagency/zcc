/* bench_png.c — libpng encode/decode round trip, in memory, deterministic.
 *
 * WHY A DRIVER AND NOT `pngtest`. pngtest reads and writes files, so its clock
 * is a filesystem's as much as a compiler's, and it runs for a few milliseconds
 * — below the resolution of any wall-clock comparison. This drives the same
 * code paths (row filtering, the Huffman/deflate encode, the inflate and
 * unfilter on the way back) against memory buffers, for long enough to measure.
 *
 * CLEAN INPUT. One line is printed: the raw size, the encoded size, and an FNV
 * checksum of the decoded pixels. The ENCODED SIZE is the sharp end — it is a
 * statement about every filter choice and every Huffman tree the encoder built,
 * so two builds that agree on it agree about the whole pipeline and not merely
 * about the final image.
 */
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include "png.h"

#define W 384
#define H 384
#define REPS 6

static unsigned char *img;
static unsigned char *enc;
static size_t enc_len, enc_cap;

static void wr(png_structp p, png_bytep d, png_size_t n){
    (void)p;
    if (enc_len + n > enc_cap) { fprintf(stderr, "encode overflow\n"); exit(2); }
    memcpy(enc + enc_len, d, n); enc_len += n;
}
static void fl(png_structp p){ (void)p; }

struct rd { const unsigned char *p; size_t off, len; };
static void rd_fn(png_structp p, png_bytep d, png_size_t n){
    struct rd *s = (struct rd *)png_get_io_ptr(p);
    if (s->off + n > s->len) { fprintf(stderr, "decode underflow\n"); exit(2); }
    memcpy(d, s->p + s->off, n); s->off += n;
}

/* A pattern with real structure: smooth gradients the up/paeth filters like,
   plus a high-frequency term they do not, so neither filter choice is free. */
static void make_image(void){
    int y, x;
    for (y = 0; y < H; y++)
        for (x = 0; x < W; x++) {
            unsigned char *p = img + (size_t)y * W * 4 + (size_t)x * 4;
            p[0] = (unsigned char)(x * 2 + y);
            p[1] = (unsigned char)(y * 3 - x);
            p[2] = (unsigned char)(((x * x) >> 5) ^ (y << 1));
            p[3] = (unsigned char)(255 - ((x + y) & 63));
        }
}

static void encode(void){
    png_structp p; png_infop i; png_bytep rows[H]; int y;
    enc_len = 0;
    p = png_create_write_struct(PNG_LIBPNG_VER_STRING, 0, 0, 0);
    i = png_create_info_struct(p);
    if (setjmp(png_jmpbuf(p))) { fprintf(stderr, "encode failed\n"); exit(2); }
    png_set_write_fn(p, 0, wr, fl);
    png_set_IHDR(p, i, W, H, 8, PNG_COLOR_TYPE_RGBA, PNG_INTERLACE_NONE,
                 PNG_COMPRESSION_TYPE_DEFAULT, PNG_FILTER_TYPE_DEFAULT);
    png_set_compression_level(p, 6);
    for (y = 0; y < H; y++) rows[y] = img + (size_t)y * W * 4;
    png_set_rows(p, i, rows);
    png_write_png(p, i, PNG_TRANSFORM_IDENTITY, 0);
    png_destroy_write_struct(&p, &i);
}

static unsigned long decode(void){
    png_structp p; png_infop i; struct rd s; unsigned long h = 2166136261UL;
    png_bytep *rows; int y; size_t x, rb;
    s.p = enc; s.off = 0; s.len = enc_len;
    p = png_create_read_struct(PNG_LIBPNG_VER_STRING, 0, 0, 0);
    i = png_create_info_struct(p);
    if (setjmp(png_jmpbuf(p))) { fprintf(stderr, "decode failed\n"); exit(2); }
    png_set_read_fn(p, &s, rd_fn);
    png_read_png(p, i, PNG_TRANSFORM_IDENTITY, 0);
    rows = png_get_rows(p, i);
    rb = png_get_rowbytes(p, i);
    for (y = 0; y < H; y++)
        for (x = 0; x < rb; x++) { h ^= rows[y][x]; h *= 16777619UL; }
    png_destroy_read_struct(&p, &i, 0);
    return h & 0xffffffffUL;
}

int main(void){
    int r; unsigned long h = 0;
    img = (unsigned char *)malloc((size_t)W * H * 4);
    enc_cap = (size_t)W * H * 8; enc = (unsigned char *)malloc(enc_cap);
    if (!img || !enc) return 2;
    make_image();
    for (r = 0; r < REPS; r++) { encode(); h = decode(); }
    printf("raw=%lu enc=%lu fnv=%lu\n",
           (unsigned long)((size_t)W * H * 4), (unsigned long)enc_len, h);
    return 0;
}
