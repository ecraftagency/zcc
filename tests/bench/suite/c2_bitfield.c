#include <stdio.h>
struct F{ unsigned a:5, b:7, c:12, d:8; };
static struct F t[10000];
long work(struct F *p,int n){ long s=0; int i; for(i=0;i<n;i++) s += p[i].a + p[i].b + p[i].c + p[i].d; return s; }
int main(void){ int i; for(i=0;i<10000;i++){ t[i].a=i&31; t[i].b=i&127; t[i].c=i&4095; t[i].d=i&255; } long s=0,k; for(k=0;k<4000;k++) s+=work(t,10000); printf("%ld\n",s); return 0; }
