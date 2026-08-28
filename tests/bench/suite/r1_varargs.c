/* r1_varargs — A PRINTF-SHAPED VARIADIC on the hot path.
 *
 * WHY IT IS HERE.  Law 3c names varargs as unsampled, and the ABI cost is real
 * and specific: AAPCS64 makes a variadic callee spill its argument registers to
 * a 192-byte save area before it can walk them, so a call that costs three
 * instructions at the caller costs a dozen at the callee.  Nothing in the suite
 * makes that trade, so `abi.rs`'s save-area path has never been on a clock.
 */
#include <stdio.h>
#include <stdarg.h>
static long acc(int n, ...){
    va_list ap; long s = 0; int i;
    va_start(ap, n);
    for(i=0;i<n;i++){
        int k = va_arg(ap, int);
        if(k & 1) s += (long)k * 3; else s -= (long)k;
    }
    va_end(ap);
    return s;
}
int main(void){
    long i, s = 0;
    for(i=0;i<400000;i++){
        int a = (int)(i & 255), b = (int)((i>>3) & 127), c = (int)((i>>7) & 63);
        s += acc(3, a, b, c);
        s += acc(6, a, b, c, a ^ b, b ^ c, c ^ a);
        s &= 0xffffffffL;
    }
    printf("%ld\n", s);
    return 0;
}
