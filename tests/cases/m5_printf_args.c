int printf(char *fmt, ...);
int main() {
    int x;
    long l;
    char *s;
    x = 42;
    l = 60000;
    s = "abc";
    printf("x=%d s=%s c=%c\n", x, s, s[1]);
    printf("neg=%d sum=%d long=%ld\n", -x, 1 + 2 + 3, l + l);
    printf("tab\there \"quoted\" backslash\\\n");
    return 0;
}
