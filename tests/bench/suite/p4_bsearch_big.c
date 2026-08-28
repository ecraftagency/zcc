/* p4_bsearch_big — BINARY SEARCH over an array far larger than the cache.
 * WHY: j4_binsearch fits in L1, so it measures the compare chain. Here every
 * one of the first ~18 probes misses, and the loop is a dependent load whose
 * address comes from a shift and an add — latency plus a short chain. */
#include <stdio.h>
#define N (4*1024*1024)
static int a[N];
int main(void){
    long i; unsigned long s = 0, seed = 1u;
    for(i=0;i<N;i++) a[i] = (int)(i*3);
    for(i=0;i<250000;i++){
        long lo = 0, hi = N-1; int key;
        seed = seed*6364136223846793005UL + 1442695040888963407UL;
        key = (int)((seed>>33) % (unsigned long)(N*3));
        while(lo <= hi){ long m = (lo+hi) >> 1; if(a[m] == key){ lo = m; break; } if(a[m] < key) lo = m+1; else hi = m-1; }
        s += (unsigned long)lo;
    }
    printf("%lu\n", s);
    return 0;
}
