/* Ho ham math *l (C99) tren model "long double = double" cua zcc — map ve ban
   double trong math.h nhung. Regression cho bug redis BLPOP: ceill khong co
   prototype -> implicit int -> doc x0 rac thay d0 -> timeout = 0 = block
   vinh vien (fail deterministic unit/type/list, cc referee pass). */
#include <stdio.h>
#include <math.h>
#include <stdlib.h>

int main(void) {
    long double v;
    long long tval;
    /* tai hien nguyen van duong timeout cua redis timeout.c */
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
