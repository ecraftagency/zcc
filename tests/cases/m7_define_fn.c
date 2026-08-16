int printf(char *fmt, ...);
#define ADD(a, b) ((a) + (b))
#define SQR(x) ((x) * (x))
#define TWICE(x) ADD(x, x)
#define EMPTYBODY(x)
int main() {
    int n;
    n = TWICE(SQR(3));
    EMPTYBODY(junk tokens here)
    printf("%d %d\n", n, SQR(1 + 2));
    return ADD(n, TWICE(1));
}
