#include <stdio.h>
static float a[10000];
double work(float *p,int n){ float mn=p[0],mx=p[0]; int i; for(i=1;i<n;i++){ if(p[i]<mn) mn=p[i]; if(p[i]>mx) mx=p[i]; } return (double)(mx-mn); }
int main(void){ int i; for(i=0;i<10000;i++) a[i]=(float)((i*7919)&8191); double s=0; long k; for(k=0;k<4000;k++) s+=work(a,10000); printf("%.0f\n",s); return 0; }
