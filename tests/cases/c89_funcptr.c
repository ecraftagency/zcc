int printf(char *fmt, ...);
int add(int a, int b) { return a + b; }
int sub(int a, int b) { return a - b; }
int apply(int (*op)(int, int), int x, int y) { return op(x, y); }
long lret(void) { return 123456789012L; }
char cret(void) { return 'z'; }
char *sret(void) { return "string"; }
void nothing(void) { return; }
static int hidden(int x) { return x * 2; }
int main(void) {
    int (*f)(int, int);
    int (*ops[2])(int, int);
    long l;
    f = add;
    printf("%d\n", f(3, 4));
    f = &sub;
    printf("%d\n", (*f)(10, 4));
    printf("%d\n", apply(add, 20, 22));
    ops[0] = add; ops[1] = sub;
    printf("%d %d\n", ops[0](1, 2), ops[1](5, 2));
    l = lret();
    printf("%ld %c %s\n", l, cret(), sret());
    nothing();
    printf("%d\n", hidden(21));
    return 0;
}
