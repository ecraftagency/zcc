#include <stdio.h>
static int a[200000];
long redux(int *p,int n){ long sum=0; int mx=p[0]; int i; for(i=0;i<n;i++){ sum+=p[i]; if(p[i]>mx) mx=p[i]; } return sum+mx; }
int main(void){ int i; for(i=0;i<200000;i++) a[i]=(i*7919)&65535; long s=0,k; for(k=0;k<800;k++) s+=redux(a,200000); printf("%ld\n",s); return 0; }
