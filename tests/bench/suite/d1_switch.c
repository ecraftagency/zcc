#include <stdio.h>
long work(int n){ long s=0; int i; for(i=0;i<n;i++){ switch(i&7){ case 0: s+=1; break; case 1: s+=i; break; case 2: s-=2; break; case 3: s+=i*2; break; case 4: s+=7; break; case 5: s-=i; break; case 6: s+=3; break; default: s+=i&3; } } return s; }
int main(void){ printf("%ld\n", work(8000000)); return 0; }
