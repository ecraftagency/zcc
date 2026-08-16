int printf(char *fmt, ...);
int x = 100;
int main() {
    int i; int j; int x;
    for (i = 0; i < 10; i++)
        for (j = 0; j < 10; j++)
            if (i * j == 42) goto out;
out:
    printf("%d %d\n", i, j);
    x = 1;
    { int x; x = 2; { x = 3; } printf("%d\n", x); }
    printf("%d\n", x);
    { int y; y = x + 10; printf("%d\n", y); }
    ;
    goto skip;
    printf("bad\n");
skip:
    return 0;
}
