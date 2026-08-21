int printf(const char *fmt, ...);
/* K&R ch6: struct passed by value through functions */
struct point { int x; int y; };
struct rect { struct point p1; struct point p2; };
struct point makepoint(int x, int y) {
    struct point tmp;
    tmp.x = x;
    tmp.y = y;
    return tmp;
}
struct point addpoint(struct point p1, struct point p2) {
    p1.x += p2.x;
    p1.y += p2.y;
    return p1;
}
int ptinrect(struct point p, struct rect r) {
    return p.x >= r.p1.x && p.x < r.p2.x && p.y >= r.p1.y && p.y < r.p2.y;
}
int main(void) {
    struct point a, b, c;
    struct rect screen;
    a = makepoint(1, 2);
    b = makepoint(10, 20);
    c = addpoint(a, b);
    printf("%d %d\n", c.x, c.y);
    screen.p1 = makepoint(0, 0);
    screen.p2 = makepoint(100, 100);
    printf("%d %d\n", ptinrect(c, screen), ptinrect(b, screen));
    c = a;
    c.x = 99;
    printf("%d %d %d\n", a.x, c.x, c.y);
    return 0;
}
