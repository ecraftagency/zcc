/* Call-heavy: recursive fib stresses prologue/epilogue + call-crossing register
   homes (a value live across `bl` must land in a callee-saved reg, else spill). */
#include <stdio.h>

static long fib(int n) {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}

int main(void) {
    long acc = 0;
    int i;
    for (i = 0; i < 3; i++) acc += fib(35 + (i & 1));
    printf("%ld\n", acc);
    return 0;
}
