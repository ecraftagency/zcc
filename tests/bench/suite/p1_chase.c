/* p1_chase — A POINTER CHASE PAST THE LAST-LEVEL CACHE.
 *
 * WHY IT IS HERE.  Law 3c names working sets past cache as unsampled, and every
 * other program in this suite fits in L1 or L2 — which is why zcc's instruction
 * excess has cost so little time in it.  Here the loop is ONE dependent load per
 * iteration over 32 MB in a random permutation, so the machine is stalled on
 * memory and the only thing a compiler can change is how many instructions sit
 * on the critical path BETWEEN two loads.  A compiler that adds one is invisible
 * here; a compiler that adds one to the ADDRESS is not.
 */
#include <stdio.h>
#define N (4*1024*1024)
static unsigned next[N];
int main(void){
    unsigned i, p = 0; unsigned long s = 0, seed = 12345u;
    for(i=0;i<N;i++) next[i] = i;
    for(i=N-1;i>0;i--){ unsigned j; seed = seed*1103515245u + 12345u; j = (unsigned)((seed>>16) % (i+1u));
        { unsigned t = next[i]; next[i] = next[j]; next[j] = t; } }
    for(i=0;i<2u*1024u*1024u;i++){ p = next[p]; s += p; }
    printf("%lu %u\n", s, p);
    return 0;
}
