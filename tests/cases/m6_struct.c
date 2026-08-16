int printf(char *fmt, ...);
struct point { int x; int y; };
struct rect { struct point tl; struct point br; char tag; };
int area(struct rect *r) {
    return (r->br.x - r->tl.x) * (r->br.y - r->tl.y);
}
int main() {
    struct rect r;
    struct point *q;
    r.tl.x = 1;
    r.tl.y = 2;
    r.br.x = 5;
    r.br.y = 7;
    r.tag = 65;
    q = &r.br;
    q->x = q->x + 1;
    printf("area=%d tag=%c sz=%d\n", area(&r), r.tag, sizeof r);
    return 0;
}
