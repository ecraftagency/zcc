#include <stdio.h>
long work(int n){
  long s=0; int i;
  for(i=1;i<=n;i++){ s += (i/7) - (i%11); }
  return s;
}
int main(void){ printf("%ld\n", work(3000000)); return 0; }
