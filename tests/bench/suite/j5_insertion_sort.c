#include <stdio.h>
void isort(int *p,int n){ int i,j; for(i=1;i<n;i++){ int key=p[i]; j=i-1; while(j>=0 && p[j]>key){ p[j+1]=p[j]; j--; } p[j+1]=key; } }
static int a[2000];
int main(void){ long s=0,k; for(k=0;k<3000;k++){ int i; for(i=0;i<2000;i++) a[i]=(int)(((k+i)*7919)&8191); isort(a,2000); s += a[0]+a[1999]; } printf("%ld\n",s); return 0; }
