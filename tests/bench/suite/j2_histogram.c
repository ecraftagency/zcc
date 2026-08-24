#include <stdio.h>
static int data[200000]; static int hist[256];
void build(int *p,int n){ int i; for(i=0;i<256;i++) hist[i]=0; for(i=0;i<n;i++) hist[p[i]&255]++; }
int main(void){ int i; for(i=0;i<200000;i++) data[i]=(i*2654435761u)&255; long s=0,k; for(k=0;k<800;k++){ build(data,200000); s += hist[k&255]; } printf("%ld\n",s); return 0; }
