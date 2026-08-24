#include <stdio.h>
long acker_lite(int a,int b){ if(a==0) return b+1; if(b==0) return acker_lite(a-1,1); return acker_lite(a-1, (int)acker_lite(a,b-1)); }
int main(void){ long s=0,k; for(k=0;k<200;k++) s += acker_lite(2, 500); printf("%ld\n", s); return 0; }
