int printf(char *fmt, ...);
int calls;
int t(int r) { calls = calls + 1; return r; }
int main() {
    calls = 0;
    if (t(0) && t(1)) printf("bad\n");
    printf("%d\n", calls);
    calls = 0;
    if (t(1) || t(1)) printf("ok %d\n", calls);
    while (t(0) && t(1)) printf("bad\n");
    printf("%d\n", calls);
    return 0;
}
