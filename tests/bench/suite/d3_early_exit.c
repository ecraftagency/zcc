#include <stdio.h>
static int a[100000];
int find(int *p,int n,int key){ int i; for(i=0;i<n;i++) if(p[i]==key) return i; return -1; }
int main(void){ int i; for(i=0;i<100000;i++) a[i]=(i*2654435761u)&65535; long s=0,k; for(k=0;k<3000;k++) s += find(a,100000,(int)(k&65535)); printf("%ld\n",s); return 0; }
