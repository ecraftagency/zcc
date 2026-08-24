#include <stdio.h>
static char buf[100000];
long work(char *p,int n){ char *q=p; char *e=p+n; long s=0; while(q<e){ if(*q=='x'){ s += (q-p); } q++; } return s; }
int main(void){ int i; for(i=0;i<100000;i++) buf[i]=(i%17==0)?'x':'.'; long s=0,k; for(k=0;k<400;k++) s+=work(buf,100000); printf("%ld\n",s); return 0; }
