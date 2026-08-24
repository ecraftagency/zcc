#include <stdio.h>
static long ps[100000]; static int a[100000];
void prefix(int *p,long *out,int n){ long acc=0; int i; for(i=0;i<n;i++){ acc+=p[i]; out[i]=acc; } }
int main(void){ int i; for(i=0;i<100000;i++) a[i]=(i&63)-32; long s=0,k; for(k=0;k<1500;k++){ prefix(a,ps,100000); s += ps[99999]; } printf("%ld\n",s); return 0; }
