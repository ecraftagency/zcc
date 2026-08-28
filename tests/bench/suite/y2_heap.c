/* y2_heap — A BINARY HEAP: sift-down, the array-index tree walk.
 * WHY: the child index is `2i+1`, so the loop's induction variable MULTIPLIES
 * rather than adds and the address cannot be strength-reduced to a stride —
 * the opposite of every array loop already in the suite. */
#include <stdio.h>
#define N 200000
static int h[N];
static void sift(int *v, long n, long i){
    for(;;){
        long c = 2*i + 1;
        if(c >= n) break;
        if(c+1 < n && v[c+1] > v[c]) c++;
        if(v[i] >= v[c]) break;
        { int t = v[i]; v[i] = v[c]; v[c] = t; }
        i = c;
    }
}
int main(void){
    long i, r; unsigned long s = 0, seed = 11u;
    for(r=0;r<14;r++){
        for(i=0;i<N;i++){ seed = seed*6364136223846793005UL + 1442695040888963407UL; h[i] = (int)(seed>>35); }
        for(i=N/2-1;i>=0;i--) sift(h, N, i);
        for(i=N-1;i>0;i--){ int t = h[0]; h[0] = h[i]; h[i] = t; sift(h, i, 0); }
        s += (unsigned long)h[0] + (unsigned long)h[N-1];
    }
    printf("%lu\n", s);
    return 0;
}
