#include <stdio.h>
#define R 2000
#define Sz 2000
static int A[R], B[Sz];
/* 24 values that come from memory (unfoldable), are updated in the OUTER loop,
   and are all read AFTER both loops — so they are live ACROSS the inner loop
   and must compete with it for registers. */
long joinit(int *pa, int *pb, int n, int m){
  long c0=pa[0],c1=pa[1],c2=pa[2],c3=pa[3],c4=pa[4],c5=pa[5];
  long c6=pa[6],c7=pa[7],c8=pa[8],c9=pa[9],c10=pa[10],c11=pa[11];
  long c12=pb[0],c13=pb[1],c14=pb[2],c15=pb[3],c16=pb[4],c17=pb[5];
  long c18=pb[6],c19=pb[7],c20=pb[8],c21=pb[9],c22=pb[10],c23=pb[11];
  long hits=0; int i,j;
  for(i=0;i<n;i++){
    int key = pa[i];
    for(j=0;j<m;j++){            /* hot: n*m iterations */
      if(pb[j]==key) hits++;
    }
    c0+=i; c1^=c2; c3+=c4; c5^=c6; c7+=c8; c9^=c10; c11+=c12;
    c13^=c14; c15+=c16; c17^=c18; c19+=c20; c21^=c22; c23+=i;
  }
  return hits+c0+c1+c2+c3+c4+c5+c6+c7+c8+c9+c10+c11
             +c12+c13+c14+c15+c16+c17+c18+c19+c20+c21+c22+c23;
}
int main(void){
  int k; for(k=0;k<R;k++) A[k]=k%97;
  for(k=0;k<Sz;k++) B[k]=k%97;
  printf("%ld\n", joinit(A,B,R,Sz));
  return 0;
}
