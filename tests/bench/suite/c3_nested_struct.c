#include <stdio.h>
struct Inner{ int lo,hi; };
struct Outer{ struct Inner a,b; int tag; };
static struct Outer o[10000];
long work(struct Outer *p,int n){ long s=0; int i; for(i=0;i<n;i++) s += p[i].a.lo + p[i].b.hi - p[i].tag; return s; }
int main(void){ int i; for(i=0;i<10000;i++){ o[i].a.lo=i; o[i].a.hi=i*2; o[i].b.lo=i-1; o[i].b.hi=i+3; o[i].tag=i&15; } long s=0,k; for(k=0;k<4000;k++) s+=work(o,10000-(int)(s&1)); printf("%ld\n",s); return 0; }
