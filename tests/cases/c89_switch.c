int printf(char *fmt, ...);
int classify(int c) {
    switch (c) {
    case '0': case '1': case '2': case '3': case '4':
    case '5': case '6': case '7': case '8': case '9':
        return 1;
    case ' ':
    case '\n':
    case '\t':
        return 2;
    default:
        return 3;
    }
}
int main() {
    char *s; int i; int cnt[4]; int fell;
    s = "gio 14 con  7\tvit\n";
    for (i = 0; i < 4; i++) cnt[i] = 0;
    for (i = 0; s[i] != '\0'; i++)
        cnt[classify(s[i])]++;
    printf("%d %d %d\n", cnt[1], cnt[2], cnt[3]);
    switch (2) {
    case 1: fell = 1; break;
    case 2: fell = 2;
    case 3: fell = fell + 10; break;
    case 4: fell = 99; break;
    }
    printf("%d\n", fell);
    switch (7) {
    case 1: fell = 0; break;
    default: fell = 50;
    }
    printf("%d\n", fell);
    switch (8) { case 1: fell = 9; }
    printf("%d\n", fell);
    return 0;
}
