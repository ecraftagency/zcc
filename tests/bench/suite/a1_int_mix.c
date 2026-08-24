#include <stdio.h>
long work(int n){
  long s=0; int i;
  for(i=1;i<=n;i++){ short a=(short)(i*3); char b=(char)(i&7); s += (long)a - b + (i%5); }
  return s;
}
int main(void){ printf("%ld\n", work(2000000)); return 0; }
