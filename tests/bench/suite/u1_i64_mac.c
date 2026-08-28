/* u1_i64_mac — 64-BIT MULTIPLY-ACCUMULATE.
 * WHY: the suite's arithmetic is 32-bit. A 64x64 multiply has a different
 * latency from a 32-bit one, so bignum, hashing and checksum code lives here
 * and nowhere else in the suite. */
#include <stdio.h>
int main(void){
    unsigned long i, a = 0x123456789abcdefUL, b = 0xfedcba987654321UL, s = 0, c = 0;
    for(i=0;i<6000000UL;i++){
        unsigned long lo = a * b;
        a = lo ^ (a >> 7);
        b = b + (lo >> 13) + i;
        c += (a & 0xffffUL) * (b & 0xffffUL);
        s ^= a + b + c;
    }
    printf("%lu %lu\n", s, c);
    return 0;
}
