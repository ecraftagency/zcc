#include <stdio.h>
struct bf {
    int a : 3;
    unsigned b : 5;
    int c : 10;
    int : 0;
    int d : 7;
};
struct big {
    long a, b, c, d;
};
struct big mk(long x) {
    struct big r;
    r.a = x; r.b = x + 1; r.c = x + 2; r.d = x + 3;
    return r;
}
long sum(struct big s) { return s.a + s.b + s.c + s.d; }
struct bf g = { -2, 19, -300, 40 };
int main(void) {
    struct bf v;
    struct big w;
    v.a = -2; v.b = 19; v.c = -300; v.d = 40;
    printf("%d %u %d %d\n", v.a, v.b, v.c, v.d);
    printf("%d %u %d %d\n", g.a, g.b, g.c, g.d);
    v.a = 3; v.a++; /* wrap 3 bit: 4 -> -4 */
    printf("%d\n", v.a);
    w = mk(10);
    printf("%ld %ld\n", sum(w), sum(mk(100)));
    printf("%d\n", ((struct bf){ 1, 2, 3, 4 }).c);
    printf("%lu\n", (unsigned long)sizeof(struct bf));
    return 0;
}
