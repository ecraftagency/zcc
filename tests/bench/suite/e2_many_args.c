#include <stdio.h>
long mix(int a,int b,int c,int d,int e,int f,int g,int h,int i,int j){ return (long)a*j + b*i - c*h + d*g - e*f; }
int main(void){ long s=0; int k; for(k=0;k<4000000;k++) s += mix(k,k+1,k+2,k+3,k+4,k+5,k+6,k+7,k+8,k+9); printf("%ld\n", s); return 0; }
