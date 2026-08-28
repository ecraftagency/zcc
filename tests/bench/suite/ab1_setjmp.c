/* ab1_setjmp — NON-LOCAL EXIT, C's other error mechanism.
 *
 * WHY IT IS HERE.  `x1_goto_cleanup` covers the intra-function ladder;
 * `setjmp`/`longjmp` is what real C uses when the error must cross frames —
 * every interpreter, every parser with a bail-out, every library with an
 * allocation failure path.  It constrains the compiler in a way nothing else
 * here does: C99 7.13.2.1 says an object with automatic storage that is not
 * `volatile` has an INDETERMINATE value after a `longjmp`, which means every
 * value the compiler kept in a register across the `setjmp` is one it must be
 * able to account for, and the call itself is a point no value may be assumed
 * to survive.  A compiler that treats `setjmp` as an ordinary call is wrong,
 * and one that treats it as a full barrier gives up more than it must.
 */
#include <stdio.h>
#include <setjmp.h>
static jmp_buf esc;
static unsigned long deep(unsigned long v, int d){
    volatile unsigned long acc = v;
    if(d == 0){
        if((v & 63u) == 7u) longjmp(esc, (int)((v & 31u) + 1u));
        return acc * 3u + 1u;
    }
    acc = deep(v + (unsigned long)d, d - 1) ^ (unsigned long)d;
    return acc;
}
int main(void){
    unsigned long i, sum = 0; volatile unsigned long caught = 0;
    for(i=0;i<900000;i++){
        volatile unsigned long seed = i * 2654435761u;
        int rc = setjmp(esc);
        if(rc == 0){
            sum += deep(seed >> 8, 6);
        } else {
            caught += (unsigned long)rc;
        }
    }
    printf("%lu %lu\n", sum, caught);
    return 0;
}
