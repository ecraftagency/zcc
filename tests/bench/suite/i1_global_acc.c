#include <stdio.h>
long gsum; int gtab[256];
void accumulate(int n){ int i; for(i=0;i<n;i++) gsum += gtab[i&255]; }
int main(void){ int i; for(i=0;i<256;i++) gtab[i]=i*i-i; long k; for(k=0;k<40000;k++) accumulate(1000); printf("%ld\n", gsum); return 0; }
