#include <stdio.h>
double work(int n){ double s=0.0; int i; for(i=1;i<=n;i++){ float x=(float)i; s += (double)(x*1.5f - x/3.0f); } return s; }
int main(void){ printf("%.0f\n", work(4000000)); return 0; }
