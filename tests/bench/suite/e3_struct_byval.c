#include <stdio.h>
struct V{ int a,b,c,d; };
long sum(struct V v){ return (long)v.a + v.b - v.c + v.d; }
int main(void){ long s=0; int k; for(k=0;k<4000000;k++){ struct V v; v.a=k; v.b=k&7; v.c=k%5; v.d=k&255; s += sum(v); } printf("%ld\n", s); return 0; }
