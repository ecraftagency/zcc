#include <stdio.h>
static int a[1000];
long work(int *p,int n){ long s=0; int *e=p+n; while(p<e){ s += *p; p++; } return s; }
int main(void){ int i; for(i=0;i<1000;i++) a[i]=i*i-3*i+1; long s=0,k; for(k=0;k<5000;k++) s+=work(a,1000); printf("%ld\n",s); return 0; }
