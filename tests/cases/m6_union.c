int printf(char *fmt, ...);
union u { int i; char c[4]; };
int main() {
    union u v;
    v.i = 1094861636; /* 0x41424344, little endian: c[0] = 'D' */
    printf("%c%c%c%c sz=%d\n", v.c[0], v.c[1], v.c[2], v.c[3], sizeof v);
    return 0;
}
