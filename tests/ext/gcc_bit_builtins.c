#include <stdio.h>
int main(void) {
    unsigned long long v = 0x1122334455667788ULL;
    printf("%llx %x %d %d %d %d %d %d\n",
        __builtin_bswap64(v), __builtin_bswap32(0xdeadbeefu),
        __builtin_clz(0x00f00000u), __builtin_clzll(1ULL),
        __builtin_ctzll(0x1000ULL), __builtin_popcount(0xffabu),
        __builtin_ctz(64u), (int)__builtin_popcountll(0xffffffffffULL));
    return 0;
}
