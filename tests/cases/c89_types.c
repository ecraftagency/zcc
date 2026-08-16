int printf(char *fmt, ...);
int main() {
    unsigned int u; unsigned long ul; short s; unsigned short us;
    unsigned char uc; signed char sc; long l;
    u = -1;
    printf("%u %d\n", u, u > 100);
    ul = -1;
    printf("%d\n", ul > 100);
    s = -32769;
    printf("%d\n", s);
    us = 70000;
    printf("%d\n", us);
    uc = 300; sc = 300;
    printf("%d %d\n", uc, sc);
    l = 2147483647L + 1;
    printf("%ld\n", l);
    printf("%d\n", (int)(2147483647 + 1));
    printf("%u\n", 7u / 2);
    printf("%d\n", -7 / 2);
    printf("%d\n", (int)((unsigned)-8 >> 1 > 0));
    printf("%d\n", -8 >> 1);
    printf("%lu %lu %lu %lu\n", sizeof(char), sizeof(short), sizeof(unsigned), sizeof(long));
    printf("%lu %lu %lu\n", sizeof(int *), sizeof(float), sizeof(double));
    printf("%d\n", (char)1000);
    printf("%d\n", (unsigned char)-1);
    printf("%d\n", (short)0x12345678);
    return 0;
}
