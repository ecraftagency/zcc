struct P { int x, y, z, w; };
int f(int n) {
    struct P p;
    p.x = n; p.y = n + 1; p.z = n + 2; p.w = n + 3;
    int s = 0;
    for (int i = 0; i < n; i++)
        s = s + p.x + p.y + p.z + p.w;
    return s;
}
