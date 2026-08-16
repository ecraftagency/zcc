int printf(char *fmt, ...);
int main() {
    int i; int n; int a[5]; int *p;
    i = 5;
    n = i++; printf("%d %d\n", n, i);
    n = ++i; printf("%d %d\n", n, i);
    n = i--; printf("%d %d\n", n, i);
    n = --i; printf("%d %d\n", n, i);
    for (i = 0; i < 5; i++) a[i] = i * 10;
    p = a;
    p++;
    printf("%d\n", *p++);
    printf("%d\n", *p);
    printf("%d\n", *--p);
    i = 3; i += 4; printf("%d\n", i);
    i -= 2; i *= 6; i /= 3; i %= 7;
    printf("%d\n", i);
    i = 1; i <<= 4; i |= 5; i &= 12; i ^= 3; i >>= 1;
    printf("%d\n", i);
    return 0;
}
