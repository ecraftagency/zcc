/* z1_crc32 — TABLE-DRIVEN CRC32, the checksum every protocol carries.
 * WHY: one dependent table lookup per byte, where the index is built from the
 * running value XOR the input — a two-instruction chain in front of a load,
 * repeated for the length of the message. It is h3_fnv's shape with a memory
 * dependence added, and the pair separates chain cost from table cost. */
#include <stdio.h>
static unsigned tab[256];
#define N (1<<18)
static unsigned char buf[N];
int main(void){
    unsigned i, j, c; unsigned long s = 0; long r;
    for(i=0;i<256u;i++){ c = i; for(j=0;j<8u;j++) c = (c & 1u) ? (0xEDB88320u ^ (c>>1)) : (c>>1); tab[i] = c; }
    for(i=0;i<N;i++) buf[i] = (unsigned char)((i*73u) & 255u);
    for(r=0;r<180;r++){
        unsigned crc = 0xFFFFFFFFu;
        for(i=0;i<N;i++) crc = tab[(crc ^ buf[i]) & 255u] ^ (crc >> 8);
        s += crc ^ 0xFFFFFFFFu;
    }
    printf("%lu\n", s);
    return 0;
}
