int printf(const char *fmt, ...);
int garr[5] = {10, 20, 30};
char gs[] = "global chuoi";
char gs2[20] = "ngan";
double gd[3] = {1.5, 2.5};
int gm[2][3] = {{1, 2, 3}, {4, 5, 6}};
struct pt { int x; int y; };
struct pt gp = {7, 8};
char *names[3] = {"mot", "hai", "ba"};
int main(void) {
    int a[5] = {1, 2, 3};
    int b[] = {9, 8, 7, 6};
    char s[] = "xin chao";
    char s2[12] = "hi";
    int m[2][3] = {{1, 2, 3}, {4, 5, 6}};
    struct pt p = {40, 2};
    double d[2] = {0.5, 1.5};
    int x = {42};
    int i, j;
    for (i = 0; i < 5; i++) printf("%d ", a[i]);
    printf("| %lu", sizeof b / sizeof b[0]);
    for (i = 0; i < 4; i++) printf(" %d", b[i]);
    printf("\n%s %lu | %s\n", s, sizeof s, s2);
    for (i = 0; i < 2; i++)
        for (j = 0; j < 3; j++) printf("%d", m[i][j]);
    printf(" %d %d %d\n", p.x + p.y, x, (int)(d[0] * 4.0 + d[1]));
    for (i = 0; i < 5; i++) printf("%d ", garr[i]);
    printf("| %s | %s | %lu %lu\n", gs, gs2, sizeof gs, sizeof gs2);
    printf("%f %f %d %d\n", gd[1], gd[2], gm[1][2], gp.x + gp.y);
    printf("%s %s %s\n", names[0], names[1], names[2]);
    return 0;
}
