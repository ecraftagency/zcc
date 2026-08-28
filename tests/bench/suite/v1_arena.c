/* v1_arena — A BUMP ALLOCATOR with alignment, and the objects it hands out.
 * WHY: allocation is the one thing every C program does that the suite does
 * not. A bump allocator is a load, an align, a compare, a store-back and a
 * return — five instructions on the hot path, so the ratio is sensitive. */
#include <stdio.h>
#define CAP (1<<22)
static unsigned char arena[CAP];
static unsigned long off;
static void *bump(unsigned long n, unsigned long al){
    unsigned long p = (off + al - 1UL) & ~(al - 1UL);
    if(p + n > CAP){ off = 0; p = 0; }
    off = p + n;
    return &arena[p];
}
int main(void){
    unsigned long i, s = 0, seed = 5u;
    for(i=0;i<1500000UL;i++){
        unsigned long n, al;
        long *q;
        seed = seed*6364136223846793005UL + 1442695040888963407UL;
        n = 8UL + ((seed>>33) & 127UL);
        al = 1UL << ((seed>>20) & 3UL);
        q = (long *)bump(n, al < 8UL ? 8UL : al);
        *q = (long)i;
        s += (unsigned long)*q + (unsigned long)((unsigned char *)q - arena);
    }
    printf("%lu\n", s);
    return 0;
}
