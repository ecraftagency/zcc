/* u4_popcnt64 — POPCOUNT AND LEADING-ZERO COUNT on 64-bit words.
 * WHY: h1 counts bits one at a time in 32 bits. This is the word-parallel form
 * plus `clz`, the pair that every bitmap index and every allocator uses, and
 * neither has been timed at 64 bits. */
#include <stdio.h>
static int pc64(unsigned long x){
    x = x - ((x >> 1) & 0x5555555555555555UL);
    x = (x & 0x3333333333333333UL) + ((x >> 2) & 0x3333333333333333UL);
    x = (x + (x >> 4)) & 0x0f0f0f0f0f0f0f0fUL;
    return (int)((x * 0x0101010101010101UL) >> 56);
}
static int clz64(unsigned long x){ int n = 0; if(!x) return 64; while(!(x & 0x8000000000000000UL)){ x <<= 1; n++; } return n; }
int main(void){
    unsigned long i, s = 0, x = 0x9e3779b97f4a7c15UL;
    for(i=0;i<3000000UL;i++){
        x = x * 6364136223846793005UL + 1442695040888963407UL;
        s += (unsigned long)pc64(x) + (unsigned long)clz64(x | 1UL) * 3UL;
    }
    printf("%lu\n", s);
    return 0;
}
