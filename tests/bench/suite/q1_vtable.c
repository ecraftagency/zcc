/* q1_vtable — DISPATCH THROUGH A TABLE OF FUNCTION POINTERS, three deep.
 *
 * WHY IT IS HERE.  Law 3c names deep call graphs and indirect dispatch as
 * unsampled.  n7 has ONE indirect call in its inner loop; this has a chain of
 * them — an operation dispatches to a handler which dispatches to a kernel —
 * which is how an interpreter, a driver layer and any C program with a "class"
 * is written.  Every level is a call boundary the allocator must respect, so
 * what it measures is argument marshalling and caller-saved traffic at depth,
 * not the call itself.
 */
#include <stdio.h>
typedef long (*Kern)(long, long);
static long k_add(long a, long b){ return a + b; }
static long k_xor(long a, long b){ return a ^ b; }
static long k_mul(long a, long b){ return a * 3 + b; }
static long k_sub(long a, long b){ return a - b; }
static Kern kerns[4] = { k_add, k_xor, k_mul, k_sub };
typedef long (*Hand)(long, long, int);
static long h_direct(long a, long b, int k){ return kerns[k & 3](a, b); }
static long h_twice(long a, long b, int k){ return kerns[k & 3](kerns[(k+1) & 3](a, b), b); }
static Hand hands[2] = { h_direct, h_twice };
int main(void){
    long i, s = 0; unsigned long seed = 0xfeedfaceUL;
    for(i=0;i<3000000;i++){
        int k, h;
        seed = seed*6364136223846793005UL + 1442695040888963407UL;
        k = (int)((seed>>33) & 3); h = (int)((seed>>35) & 1);
        s = hands[h](s, (long)(seed>>40), k) & 0xffffffffL;
    }
    printf("%ld\n", s);
    return 0;
}
