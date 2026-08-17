/* EXT(c99): long long 64-bit + suffix LL/ULL */
#include <stdio.h>
long long big = 1234567890123LL;
unsigned long long ubig = 18446744073709551615ULL;
int main(void) {
    long long v = big * 2;
    printf("%lld %llu %lld\n", big, ubig, v);
    printf("%d\n", (int)sizeof(long long));
    return 0;
}
