/* VLA local (C99) — zcc lowers to pointer + alloca; sizeof(vla) is NOT tested
 * (zcc returns the pointer size — an intentional spec deviation, not exercised
 * by the corpus) */
#include <stdio.h>
#include <string.h>

int sum_squares(int n) {
    int a[n];
    char buf[n * 2 + 1];
    int i, s = 0;
    memset(buf, 'a', n * 2);
    buf[n * 2] = 0;
    for (i = 0; i < n; i++) a[i] = i * i;
    for (i = 0; i < n; i++) s += a[i];
    return s + (int)strlen(buf);
}

/* VLA element is a struct, size derived from a parameter via an expression */
struct pt { int x, y; };
int diag(int k) {
    struct pt m[k + 1];
    int i, s = 0;
    for (i = 0; i <= k; i++) { m[i].x = i; m[i].y = -i; }
    for (i = 0; i <= k; i++) s += m[i].x - m[i].y;
    return s;
}

int main(void) {
    printf("%d %d %d\n", sum_squares(5), sum_squares(1), diag(4));
    return 0;
}
