int printf(char *fmt, ...);
/* K&R 3.5-3.6: itoa + reverse implemented with do-while */
int strlen_(char *s) {
    int n;
    for (n = 0; s[n] != '\0'; n++)
        ;
    return n;
}
int reverse_(char *s) {
    int c; int i; int j;
    for (i = 0, j = strlen_(s) - 1; i < j; i++, j--) {
        c = s[i]; s[i] = s[j]; s[j] = c;
    }
    return 0;
}
int itoa_(int n, char *s) {
    int i; int sign;
    if ((sign = n) < 0) n = -n;
    i = 0;
    do {
        s[i++] = n % 10 + '0';
    } while ((n /= 10) > 0);
    if (sign < 0) s[i++] = '-';
    s[i] = '\0';
    reverse_(s);
    return i;
}
int main() {
    char buf[32]; int i; int sum;
    itoa_(-1234, buf);
    printf("%s\n", buf);
    itoa_(0, buf);
    printf("%s\n", buf);
    sum = 0;
    for (i = 0; i < 20; i++) {
        if (i % 3 == 0) continue;
        if (i > 15) break;
        sum += i;
    }
    printf("%d\n", sum);
    i = 10;
    do sum++; while (--i > 0);
    printf("%d %d\n", sum, i);
    return 0;
}
