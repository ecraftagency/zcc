#include <stdio.h>
int popc(unsigned x){ int c=0; while(x){ c += x&1; x>>=1; } return c; }
int main(void){ long s=0; unsigned k; for(k=0;k<8000000u;k++) s += popc(k*2654435761u); printf("%ld\n", s); return 0; }
