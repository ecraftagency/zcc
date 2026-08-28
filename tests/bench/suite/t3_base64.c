/* t3_base64 — BASE64 ENCODE, three bytes in and four out.
 * WHY: a byte-shuffling loop with a table lookup per output — the encoder shape
 * every serializer shares. Narrow loads, shifts, masks and a table index, in a
 * loop with no branches at all. */
#include <stdio.h>
#define N (1<<18)
static unsigned char in[N];
static unsigned char out[N/3*4+8];
static const char tbl[65] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
int main(void){
    long i, r; unsigned long s = 0;
    for(i=0;i<N;i++) in[i] = (unsigned char)((i*31) & 255);
    for(r=0;r<70;r++){
        long o = 0;
        for(i=0;i+2<N;i+=3){
            unsigned v = ((unsigned)in[i]<<16) | ((unsigned)in[i+1]<<8) | (unsigned)in[i+2];
            out[o++] = (unsigned char)tbl[(v>>18)&63];
            out[o++] = (unsigned char)tbl[(v>>12)&63];
            out[o++] = (unsigned char)tbl[(v>>6)&63];
            out[o++] = (unsigned char)tbl[v&63];
        }
        s += (unsigned long)out[o-1] + (unsigned long)o;
    }
    printf("%lu\n", s);
    return 0;
}
