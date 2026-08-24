#include <stdio.h>
double poly(double x){ return ((((2.0*x-3.0)*x+5.0)*x-7.0)*x+11.0); }
int main(void){ double s=0.0; int k; for(k=0;k<4000000;k++){ double x=(double)(k&1023)/1024.0; s += poly(x); } printf("%.0f\n", s); return 0; }
