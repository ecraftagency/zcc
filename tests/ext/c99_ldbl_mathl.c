/* The math *l functions (C99) on zcc's "long double = double" model — mapped to
   the plain double versions in math.h. Regression for the redis BLPOP bug:
   ceill has no prototype -> implicit int -> reads garbage x0 instead of d0 ->
   timeout = 0 = blocks forever (deterministic fail, cc referee passes). */
#include <stdio.h>
#include <math.h>
#include <stdlib.h>

int main(void) {
    long double v;
    long long tval;
    /* reproduces verbatim the timeout path of redis timeout.c */
    v = strtold("1", 0);
    v *= 1000.0;
    tval = (long long) ceill(v);
    printf("tval=%lld\n", tval);
    printf("ceill=%.1f floorl=%.1f fabsl=%.1f\n",
           (double) ceill(2.3L), (double) floorl(2.7L), (double) fabsl(-5.5L));
    printf("sqrtl=%.1f powl=%.1f fmodl=%.1f\n",
           (double) sqrtl(81.0L), (double) powl(2.0L, 10.0L),
           (double) fmodl(7.5L, 2.0L));
    return 0;
}
