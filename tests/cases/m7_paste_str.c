int printf(char *fmt, ...);
#define STR(x) #x
#define XSTR(x) STR(x)
#define CAT(a, b) a##b
#define N 12
int main() {
    int CAT(va, l);
    val = CAT(4, 2);
    printf("%s %s\n", STR(N), XSTR(N));
    printf("%s\n", STR(hello   world));
    printf("%s\n", STR("quoted \n"));
    printf("%s %d\n", __FILE__, __LINE__);
    return val;
}
