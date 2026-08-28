/* q2_deep_rec — MUTUAL RECURSION with a real frame at every level.
 *
 * WHY IT IS HERE.  e1_recursion is fib: one argument, one frame, tail-heavy.
 * A recursive-descent parser is not that — it carries several live values
 * across each nested call, so the frame is real and the callee-saved half is
 * where they must live.  That is the allocator's hardest case and no timed
 * program in the suite reaches it.
 */
#include <stdio.h>
static long ev(long n, long acc);
static long od(long n, long acc){
    long a, b;
    if(n <= 0) return acc;
    a = acc ^ (n * 3);
    b = a + (n >> 1);
    return ev(n - 1, a + b + ev(n / 3, b));
}
static long ev(long n, long acc){
    long a, b;
    if(n <= 0) return acc + 1;
    a = acc + (n * 5);
    b = a ^ (n << 2);
    return od(n - 1, a - b);
}
int main(void){
    long i, s = 0;
    for(i=0;i<9000;i++) s += ev(24 + (i & 7), i) & 0xffffL;
    printf("%ld\n", s);
    return 0;
}
