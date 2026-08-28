/* z2_rle — RUN-LENGTH ENCODE and DECODE, a two-pointer streaming codec.
 * WHY: the inner loop counts a run, so its trip count is data and its exit is a
 * compare against the NEXT byte — an unpredictable branch guarding a variable
 * stride, with a second pointer advancing at a different rate. */
#include <stdio.h>
#define N (1<<18)
static unsigned char in[N], enc[N*2], dec[N];
int main(void){
    long i, r; unsigned long s = 0;
    for(i=0;i<N;i++){ unsigned k = (unsigned)((i*2654435761u)>>25); in[i] = (unsigned char)((k % 11u < 7u) ? (k & 7u) : (k & 255u)); }
    for(r=0;r<90;r++){
        long o = 0, d = 0;
        for(i=0;i<N;){
            long j = i + 1; while(j < N && in[j] == in[i] && j - i < 255) j++;
            enc[o++] = (unsigned char)(j - i); enc[o++] = in[i]; i = j;
        }
        for(i=0;i<o;i+=2){ long k, n = enc[i]; for(k=0;k<n && d<N;k++) dec[d++] = enc[i+1]; }
        s += (unsigned long)o + (unsigned long)dec[N-1] + (unsigned long)d;
    }
    printf("%lu\n", s);
    return 0;
}
