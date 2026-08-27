#include <stdio.h>
static int a[100000];
long work(int *p,int n,int stride){ long s=0; int i; for(i=0;i<n;i+=stride) s += p[i]; return s; }
int main(void){ int i; for(i=0;i<100000;i++) a[i]=i&1023; long s=0,k; for(k=0;k<500;k++) s+=work(a,100000-(int)(s&1),7); printf("%ld\n",s); return 0; }
