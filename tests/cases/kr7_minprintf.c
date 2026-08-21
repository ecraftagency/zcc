/* K&R 7.3: minprintf — variadic + va_arg per format */
#include <stdio.h>
#include <stdarg.h>
void minprintf(char *fmt, ...) {
    va_list ap;
    char *p, *sval;
    int ival;
    double dval;
    va_start(ap, fmt);
    for (p = fmt; *p; p++) {
        if (*p != '%') {
            putchar(*p);
            continue;
        }
        switch (*++p) {
        case 'd':
            ival = va_arg(ap, int);
            printf("%d", ival);
            break;
        case 'f':
            dval = va_arg(ap, double);
            printf("%f", dval);
            break;
        case 's':
            for (sval = va_arg(ap, char *); *sval; sval++) putchar(*sval);
            break;
        default:
            putchar(*p);
            break;
        }
    }
    va_end(ap);
}
int main(void) {
    minprintf("hello %s, %d years old, %f m tall\n", "Ada", 30, 1.75);
    minprintf("100%% chac chan: %d %s %d\n", 1, "ne", -2);
    return 0;
}
