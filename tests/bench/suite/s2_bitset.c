/* s2_bitset — A BITSET: test, set and clear by index.
 * WHY: s1 packs fields in one word; this addresses a BIT in an array, so every
 * access is a shift to find the word, a shift to build the mask, and a
 * read-modify-write. It is how every allocator's free map and every graph's
 * visited set is written. */
#include <stdio.h>
#define NB (1<<20)
static unsigned long bs[NB/64];
int main(void){
    unsigned long i, s = 0, seed = 99u;
    for(i=0;i<2500000u;i++){
        unsigned long k, w, m;
        seed = seed*6364136223846793005UL + 1442695040888963407UL;
        k = (seed>>33) & (NB-1);
        w = k >> 6; m = 1UL << (k & 63);
        if(bs[w] & m){ bs[w] &= ~m; s += 1; }
        else { bs[w] |= m; s += 2; }
        s += (bs[w] >> (k & 31)) & 7UL;
    }
    printf("%lu\n", s);
    return 0;
}
