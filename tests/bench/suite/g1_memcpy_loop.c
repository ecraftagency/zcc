#include <stdio.h>
static char src[50000], dst[50000];
void mycopy(char *d,const char *s,int n){ int i; for(i=0;i<n;i++) d[i]=s[i]; }
int main(void){ int i; for(i=0;i<50000;i++) src[i]=(char)(i&255); long s=0,k; for(k=0;k<3000;k++){ mycopy(dst,src,50000); s += dst[k%50000]; } printf("%ld\n",s); return 0; }
