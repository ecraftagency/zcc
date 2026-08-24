#include <stdio.h>
long work(int n){ long s=0; int i=0; loop: if(i>=n) goto done; s += (i&3)? i : -i; i++; goto loop; done: return s; }
int main(void){ printf("%ld\n", work(8000000)); return 0; }
