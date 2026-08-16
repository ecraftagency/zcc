int printf(char *fmt, ...);
#define N 6
#define M (N + 4)
#define GREET "hi mac"
#define BIG 30 + \
            2
int main() {
    printf("%d %d %s %d\n", N, M, GREET, BIG);
    return M + BIG;
}
