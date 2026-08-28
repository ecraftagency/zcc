/* w1_punning — UNION TYPE PUNNING between double and its bits.
 * WHY: the union idiom crosses the GPR/FPR register files through MEMORY when
 * a compiler is naive and through `fmov` when it is not. Nothing else in the
 * suite forces that decision, and every fast-math and serialization library
 * does it. */
#include <stdio.h>
union U { double d; unsigned long u; };
int main(void){
    long i; unsigned long s = 0; double acc = 0.0;
    for(i=0;i<4000000;i++){
        union U a, b;
        a.d = (double)(i & 65535) * 0.125 + 1.0;
        a.u ^= 0x0008000000000000UL;
        b.u = (a.u >> 3) | 0x3ff0000000000000UL;
        acc += b.d;
        s += (a.u >> 52) + (b.u & 4095UL);
    }
    printf("%lu %.6f\n", s, acc);
    return 0;
}
