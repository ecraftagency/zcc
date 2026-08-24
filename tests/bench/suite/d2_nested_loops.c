#include <stdio.h>
long work(int n){ long s=0; int i,j,k; for(i=0;i<n;i++) for(j=0;j<n;j++) for(k=0;k<n;k++) s += (i*j+k)&31; return s; }
int main(void){ printf("%ld\n", work(300)); return 0; }
