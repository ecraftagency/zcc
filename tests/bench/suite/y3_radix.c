/* y3_radix — LSD RADIX SORT: a counting pass and a scatter pass.
 * WHY: the scatter writes to an address taken from a COUNTER that the same loop
 * increments — a store whose address depends on a load-modify-store of another
 * array. No suite program has that dependence, and it is what every histogram,
 * bucket and partition does. */
#include <stdio.h>
#define N 400000
static unsigned a[N], b[N];
static unsigned cnt[256];
int main(void){
    long i, r, p; unsigned long s = 0, seed = 17u;
    for(i=0;i<N;i++){ seed = seed*6364136223846793005UL + 1442695040888963407UL; a[i] = (unsigned)(seed>>33); }
    for(r=0;r<12;r++){
        for(p=0;p<32;p+=8){
            unsigned t = 0, k;
            for(i=0;i<256;i++) cnt[i] = 0;
            for(i=0;i<N;i++) cnt[(a[i]>>p) & 255u]++;
            for(i=0;i<256;i++){ k = cnt[i]; cnt[i] = t; t += k; }
            for(i=0;i<N;i++) b[cnt[(a[i]>>p) & 255u]++] = a[i];
            for(i=0;i<N;i++) a[i] = b[i];
        }
        s += a[0] + a[N-1] + a[N/2];
    }
    printf("%lu\n", s);
    return 0;
}
