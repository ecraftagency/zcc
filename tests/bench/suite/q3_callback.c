/* q3_callback — A CALLBACK PASSED DOWN THREE LEVELS, the qsort/bsearch shape.
 * WHY: q1 dispatches at one site; here the function pointer is an ARGUMENT
 * carried through frames that also hold live values, so it competes for the
 * callee-saved half at every level. */
#include <stdio.h>
typedef long (*Cmp)(long, long);
static long c_lt(long a, long b){ return a < b ? -1 : (a > b ? 1 : 0); }
static long c_bit(long a, long b){ return ((a ^ b) & 7) - 3; }
static long inner(long *v, long n, Cmp f, long k){
    long i, s = 0;
    for(i=0;i<n;i++) s += f(v[i], k) * (i & 3);
    return s;
}
static long mid(long *v, long n, Cmp f, long k){ return inner(v, n, f, k) + inner(v, n/2, f, k+1); }
static long outer(long *v, long n, Cmp f){ long i, s = 0; for(i=0;i<8;i++) s += mid(v, n, f, i*11); return s; }
static long buf[512];
int main(void){
    long i, s = 0;
    for(i=0;i<512;i++) buf[i] = (i*2654435761u) & 1023;
    for(i=0;i<1200;i++) s += outer(buf, 512, (i & 1) ? c_lt : c_bit);
    printf("%ld\n", s);
    return 0;
}
