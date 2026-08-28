/* y1_qsort — QUICKSORT with an insertion-sort tail, written out.
 * WHY: j5 is one insertion sort; a real sort is recursion plus a partition loop
 * plus a small-case switch, and the partition is a two-pointer scan with a
 * swap — the shape of every sort in every library. */
#include <stdio.h>
#define N 60000
static int a[N], w[N];
static void isort(int *v, long n){ long i, j; for(i=1;i<n;i++){ int t = v[i]; for(j=i-1;j>=0 && v[j]>t;j--) v[j+1]=v[j]; v[j+1]=t; } }
static void qs(int *v, long n){
    while(n > 12){
        int p = v[n>>1]; long i = 0, j = n-1;
        while(i <= j){ while(v[i] < p) i++; while(v[j] > p) j--; if(i <= j){ int t=v[i]; v[i]=v[j]; v[j]=t; i++; j--; } }
        if(j+1 < n-i){ qs(v, j+1); v += i; n -= i; } else { qs(v+i, n-i); n = j+1; }
    }
    isort(v, n);
}
int main(void){
    long i, r; unsigned long s = 0, seed = 3u;
    for(i=0;i<N;i++){ seed = seed*1103515245u + 12345u; w[i] = (int)(seed>>13); }
    for(r=0;r<28;r++){ for(i=0;i<N;i++) a[i] = w[i] ^ (int)r; qs(a, N); s += (unsigned long)a[0] + (unsigned long)a[N-1] + (unsigned long)a[N/2]; }
    printf("%lu\n", s);
    return 0;
}
