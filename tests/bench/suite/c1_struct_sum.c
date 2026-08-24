#include <stdio.h>
struct P{ int x,y,z; };
static struct P a[10000];
long work(struct P *p,int n){ long s=0; int i; for(i=0;i<n;i++) s += p[i].x + p[i].y*2 - p[i].z; return s; }
int main(void){ int i; for(i=0;i<10000;i++){ a[i].x=i; a[i].y=i&7; a[i].z=i%5; } long s=0,k; for(k=0;k<4000;k++) s+=work(a,10000); printf("%ld\n",s); return 0; }
