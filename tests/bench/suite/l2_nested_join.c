/* l2_nested_join — REGISTER PRESSURE ACROSS A HOT LOOP, the repro that found
 * the spiller's back-edge defect.
 *
 * WHY IT IS HERE.  This program lived in tests/bench/ and was never in the
 * suite, which is why the suite could not see an 8x defect: 24 values live
 * ACROSS an inner loop that runs n*m times, against three the loop reads every
 * iteration.  The spiller ranked the three as dead — a back edge runs backwards
 * in reverse postorder, so `next_use` from the latch found nothing and answered
 * "never used again" — and spilled the loop index, the loop pointer and the
 * accumulator while the twenty-four cold values kept their registers.  Six of
 * the inner loop's eleven instructions were frame traffic; gcc -O1 ran the same
 * program eight times faster.  SPILL.md S1.
 *
 * It stays in the suite as the regression test for that: the shape is the whole
 * point, and 24-live-across-a-loop is not something any other member expresses.
 */
#include <stdio.h>
#define R 3000
#define Sz 3000
static int A[R], B[Sz];
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
