/* u2_div_var — DIVISION AND MODULO BY A RUNTIME VALUE.
 * WHY: a2/a3 divide by constants, which the compiler rewrites. Dividing by a
 * value it cannot see leaves the real `udiv`/`sdiv` — 7 cycles (MEASURED M10) —
 * on the critical path, and what is measured is everything arranged around it. */
#include <stdio.h>
int main(void){
    unsigned long i, s = 0, d = 3;
    for(i=1;i<=4000000UL;i++){
        unsigned long q = i / d, m = i % d;
        long si = (long)i - 2000000L, sd = (long)d - 1L;
        s += q + m * 3UL;
        if(sd != 0) s += (unsigned long)((si / sd) & 1023L) + (unsigned long)((si % sd) & 15L);
        d = 2UL + ((d * 7UL + i) & 63UL);
    }
    printf("%lu\n", s);
    return 0;
}
