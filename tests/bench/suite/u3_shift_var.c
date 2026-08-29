/* u3_shift_var — SHIFTS BY A RUNTIME AMOUNT, in both directions and both signs.
 * WHY: a shift by a constant folds into the operand of the next instruction on
 * A64; a shift by a register does not, so it is a real instruction and a real
 * dependence. Every codec, every bignum and every serializer does this. */
#include <stdio.h>
int main(void){
    unsigned long i, s = 0; unsigned u = 0xdeadbeefu; int v = -12345678;
    for(i=0;i<7000000UL;i++){
        unsigned k = (unsigned)(i & 31), j = (unsigned)((i>>5) & 31);
        u = (u << k) | (u >> ((32u - k) & 31u));
        v = (v >> (int)(j & 15)) - (int)(u & 255u);
        s += (unsigned long)(u ^ (unsigned)v) + (unsigned long)(k * j);
    }
    printf("%lu %u %d\n", s, u, v);
    return 0;
}
