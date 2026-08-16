int printf(char *fmt, ...);
int main() {
    int a; int b;
    a = 0x1F; b = 010;
    printf("%d %d %d\n", a & b, a | b, a ^ b);
    printf("%d %d\n", a << 2, a >> 3);
    printf("%d %d %d\n", 1 && 2, 0 || 0, !5);
    printf("%d\n", a > b ? a - b : b - a);
    printf("%c%c\n", 'A', 'A' + 1);
    b = (a = 3, a + 4);
    printf("%d %d\n", a, b);
    printf("%d %d\n", ~0 == -1, '\n' + '\t' + '\\' + '\'' + '\x41' + '\101');
    printf("%d %d\n", -6 >> 1, -6 / 2);
    return 0;
}
