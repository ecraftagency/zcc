#include <stdio.h>
unsigned long work(unsigned n){
  unsigned long s=0; unsigned i;
  for(i=1;i<=n;i++){ s += (i/7u) + (i%13u); }
  return s;
}
int main(void){ printf("%lu\n", work(3000000u)); return 0; }
