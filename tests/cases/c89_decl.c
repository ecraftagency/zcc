int printf(const char *fmt, ...);
static int counter = 0;
extern int printf(const char *fmt, ...);
const int limit = 10;
int m[3][4];
int bump(void) {
    static int calls;
    calls++;
    counter += 2;
    return calls;
}
int main(void) {
    int a, b, *p, arr[3];
    unsigned long big;
    register int r;
    auto int au;
    const char *msg;
    int i, j;
    a = 1; b = 2; p = &a; arr[0] = 5;
    r = 3; au = 4;
    msg = "constant";
    big = 5;
    printf("%d %d %d %d %d %d %lu %s\n", a, b, *p, arr[0], r, au, big, msg);
    bump(); bump();
    printf("%d %d\n", bump(), counter);
    for (i = 0; i < 3; i++)
        for (j = 0; j < 4; j++)
            m[i][j] = i * 10 + j;
    printf("%d %d %lu %lu\n", m[2][3], m[1][2], sizeof m, sizeof m[0]);
    printf("%d\n", limit);
    return 0;
}
