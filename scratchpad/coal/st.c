struct P { int x, y, z, w; };
struct Q { struct P a; struct P b; long tag; };

int sum(void) {
    struct Q q;
    q.a.x = 1; q.a.y = 2; q.a.z = 3; q.a.w = 4;
    q.b.x = 5; q.b.y = 6; q.b.z = 7; q.b.w = 8;
    q.tag = 99;
    int s = 0;
    for (int i = 0; i < 10; i++) {
        s += q.a.x + q.a.y + q.b.z + q.b.w;
        q.a.x += q.tag;
    }
    return s + q.a.x + q.b.y;
}
