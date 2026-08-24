#include <stdio.h>
static int sq(int x){ return x*x; }
static int cube(int x){ return x*x*x; }
long work(int n){ long s=0; int i; for(i=0;i<n;i++) s += sq(i&255) - cube(i&63); return s; }
int main(void){ printf("%ld\n", work(6000000)); return 0; }
