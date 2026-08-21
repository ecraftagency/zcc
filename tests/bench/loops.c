/* Tight scalar arithmetic: many simultaneously-live temporaries in the inner loop.
   This is exactly the surface the -O0 naive all-spill model penalises and the
   Stage-5b register allocator rewards (values kept in registers, not memory). */
#include <stdio.h>

int main(void) {
    unsigned long a = 1, b = 2, c = 3, d = 4, e = 5, f = 6, sum = 0;
    unsigned long i, j;
    for (i = 0; i < 4000; i++) {
        for (j = 0; j < 100000; j++) {
            a = a * 1103515245UL + 12345UL;
            b = b ^ (a >> 7);
            c = c + a * b;
            d = d - (b ^ c);
            e = e * 3 + d;
            f = (f << 1) ^ (e + a);
            sum += a ^ b ^ c ^ d ^ e ^ f;
        }
    }
    printf("%lu\n", sum);
    return 0;
}
