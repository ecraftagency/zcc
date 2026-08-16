/* K&R 2.9 + ex2-6..2-9: getbits, setbits, invert, rightrot, bitcount */
#include <stdio.h>
unsigned getbits(unsigned x, int p, int n) {
    return (x >> (p + 1 - n)) & ~(~0 << n);
}
unsigned setbits(unsigned x, int p, int n, unsigned y) {
    unsigned m = ~(~0 << n);
    return (x & ~(m << (p + 1 - n))) | ((y & m) << (p + 1 - n));
}
unsigned invert(unsigned x, int p, int n) {
    unsigned m = ~(~0 << n);
    return x ^ (m << (p + 1 - n));
}
unsigned rightrot(unsigned x, int n) {
    while (n-- > 0) {
        if (x & 1)
            x = (x >> 1) | (1u << 31);
        else
            x = x >> 1;
    }
    return x;
}
int bitcount(unsigned x) {
    int b;
    for (b = 0; x != 0; x &= x - 1) b++;
    return b;
}
int main(void) {
    printf("%u %u %u\n", getbits(0xF0, 7, 4), setbits(0xFF, 4, 3, 0), invert(0xFF, 4, 3));
    printf("%u %u\n", rightrot(0x80000001u, 1), rightrot(0xF, 4));
    printf("%d %d %d\n", bitcount(0xFF), bitcount(0x80000000u), bitcount(0));
    return 0;
}
