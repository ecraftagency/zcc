#include <stdio.h>
static int m[200][200];
long work(int n){ long s=0; int i,j; for(i=0;i<n;i++) for(j=0;j<n;j++) s += m[i][j]*(i-j); return s; }
int main(void){ int i,j; for(i=0;i<200;i++) for(j=0;j<200;j++) m[i][j]=(i*7+j*3)&255; long s=0,k; for(k=0;k<80;k++) s+=work(200); printf("%ld\n",s); return 0; }
