#include <stdio.h>
unsigned long work(unsigned n){
  unsigned long s=0; unsigned i;
  for(i=0;i<n;i++){ unsigned x=i*2654435761u; s += (x>>13) ^ (x<<3) ^ (x&0xFF00u); }
  return s;
}
int main(void){ printf("%lu\n", work(4000000u)); return 0; }
