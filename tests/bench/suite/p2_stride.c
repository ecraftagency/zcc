/* p2_stride — A STRIDE THAT DEFEATS THE PREFETCHER, with real work per step.
 *
 * WHY IT IS HERE.  p1 is pure latency; this is the case between p1 and the L1
 * kernels — a 64 MB array walked at a stride of one cache line, so every access
 * misses but the addresses are PREDICTABLE and several are in flight at once.
 * Memory-level parallelism is what the loop lives on, and it is exactly what an
 * extra instruction in front of an address destroys (`MEASURED M9`).
 */
#include <stdio.h>
#define N (8*1024*1024)
static int big[N];
int main(void){
    long i, r; unsigned long s = 0;
    for(i=0;i<N;i++) big[i] = (int)(i*2654435761u >> 12);
    for(r=0;r<6;r++)
        for(i=0;i<N;i+=16)
            s += (unsigned long)(big[i] ^ (int)r) + (unsigned long)big[(i+1024)&(N-1)];
    printf("%lu\n", s);
    return 0;
}
