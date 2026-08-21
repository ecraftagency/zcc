#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>
#include <string.h>
long double g = 1.25L;
long double id(long double x) { return x; }
long double many(long double a, long double b, long double c, long double d,
                 long double e, long double f, long double g2, long double h,
                 long double i, long double j) {
    return a + b + c + d + e + f + g2 + h + i + j; /* 8 q-reg + 2 stack quad */
}
double sumva(int n, ...) {
    va_list ap; double s = 0; int i;
    va_start(ap, n);
    for (i = 0; i < n; i++) s += (double)va_arg(ap, long double);
    va_end(ap);
    return s;
}
/* the exact ld2string pattern from redis (util.c) */
int ld2string(char *buf, size_t len, long double value) {
    size_t l = snprintf(buf, len, "%.17Lf", value);
    if (l == 0 || l >= len) return 0;
    while (l > 0 && buf[l - 1] == '0') l--;
    if (l > 0 && buf[l - 1] == '.') l--;
    buf[l] = '\0';
    return (int)l;
}
int main(void) {
    char buf[128];
    long double v = strtold("3.0e3", 0);
    v += 200;                      /* INCRBYFLOAT 3000 + 200 */
    ld2string(buf, sizeof buf, v);
    printf("A %s\n", buf);
    ld2string(buf, sizeof buf, strtold("0.00000000000000001", 0)); /* original failing case */
    printf("B %s\n", buf);
    printf("C %.6Lf %.6Lg\n", g, id(3.5L));
    printf("D %.3Lf\n", many(1.0L, 2, 3, 4, 5, 6, 7, 8, 9, 10));
    printf("E %.2f\n", sumva(3, 1.5L, 2.5L, 3.5L));
    printf("F %d %d %d\n", (int)sizeof(long double), (int)v, (int)(long double)7.9L);
    { long double a = 10; int k = 3; printf("G %.10Lg\n", a / k); }
    { struct s { long double x; char c; }; printf("H %d\n", (int)sizeof(struct s)); }
    { long double m = -0.5L; printf("I %.2Lf %d %d\n", -m, m < 0.0L, m == m); }
    ld2string(buf, sizeof buf, 5.0e3L);
    printf("J %s\n", buf);         /* an L literal fed directly into arithmetic/formatting */
    return 0;
}
