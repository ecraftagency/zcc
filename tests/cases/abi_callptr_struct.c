#include <stdio.h>
struct S { int a, b, c; };            /* 12B */
struct S mk(int x){ struct S s; s.a=x; s.b=x+1; s.c=x+2; return s; }
int use(struct S s){ return s.a+s.b+s.c; }
int main(void){
  struct S (*pmk)(int) = mk;
  int (*puse)(struct S) = use;
  struct S s = pmk(10);               /* CallPtr returns a struct */
  int r = puse(s);                    /* CallPtr accepts a struct */
  printf("%d %d %d %d\n", s.a, s.b, s.c, r);
  return 0;
}
