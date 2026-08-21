#include <stdio.h>
struct P { int x, y; };
struct P mk(int a){ struct P p; p.x=a; p.y=a*2; return p; }
int main(void){
  int m = mk(5).x;                 /* Member(SRet) → lower_addr(SRet) */
  int n = mk(7).y;
  struct P a, b;
  b.x=1; b.y=2;
  int c = (a = b).x;               /* Member(Assign-struct) */
  int cond=1;
  struct P s={10,20}, t={30,40};
  int d = (cond ? s : t).y;        /* Member(Cond aggregate) */
  int e = ({ struct P q={99,88}; q; }).x;   /* Member(Block stmt-expr) */
  printf("%d %d %d %d %d\n", m, n, c, d, e);
  return 0;
}
