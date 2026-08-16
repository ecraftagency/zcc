#include <stdarg.h>
#include <stdio.h>
int sum(int n, ...) {
    va_list ap;
    int s, i;
    va_start(ap, n);
    s = 0;
    for (i = 0; i < n; i++) s += va_arg(ap, int);
    va_end(ap);
    return s;
}
double favg(int n, ...) {
    va_list ap;
    double s;
    int i;
    va_start(ap, n);
    s = 0.0;
    for (i = 0; i < n; i++) s += va_arg(ap, double);
    va_end(ap);
    return s / n;
}
int vjoin(char *dst, int n, ...) {
    va_list ap;
    int i;
    va_start(ap, n);
    dst[0] = 0;
    for (i = 0; i < n; i++) sprintf(dst + i * 2, "%s", va_arg(ap, char *));
    va_end(ap);
    return 0;
}
int main(void) {
    char buf[16];
    printf("%d %d\n", sum(3, 1, 2, 3), sum(5, 10, 20, 30, 40, 50));
    printf("%.2f\n", favg(4, 1.5, 2.5, 3.0, 5.0));
    vjoin(buf, 3, "ab", "cd", "ef");
    printf("%s\n", buf);
    return 0;
}
