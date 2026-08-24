#include <stdio.h>
static int a[20000];
void rev(int *p,int n){ int i=0,j=n-1; while(i<j){ int t=p[i]; p[i]=p[j]; p[j]=t; i++; j--; } }
int main(void){ int i; for(i=0;i<20000;i++) a[i]=i; long s=0,k; for(k=0;k<8000;k++){ rev(a,20000); s += a[0]; } printf("%ld\n",s); return 0; }
