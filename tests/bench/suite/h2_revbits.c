#include <stdio.h>
unsigned revbits(unsigned x){ unsigned r=0; int i; for(i=0;i<32;i++){ r=(r<<1)|(x&1); x>>=1; } return r; }
int main(void){ unsigned long s=0; unsigned k; for(k=0;k<3000000u;k++) s += revbits(k); printf("%lu\n", s); return 0; }
