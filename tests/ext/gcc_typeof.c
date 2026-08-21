/* EXT(gcc): __typeof__(expr | typename) as a type-specifier */
#include <stdio.h>
int g = 7;
__typeof__(g) h = 9;
#define SWAP(a, b) do { __typeof__(a) t_ = (a); (a) = (b); (b) = t_; } while (0)
int main(void) {
    __typeof__(int) d = 8;
    __typeof__(&g) p = &g;
    int x = 1, y = 2;
    SWAP(x, y);
    printf("%d %d %d %d %d %d\n", h, d, *p, x, y, (int)sizeof(__typeof__(g)));
    return 0;
}
