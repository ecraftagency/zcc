int printf(char *fmt, ...);
typedef int myint;
typedef char *string;
typedef myint pair[2];
int main() {
    myint x;
    string s;
    pair p;
    x = 7;
    s = "typedef";
    p[0] = 1;
    p[1] = 2;
    printf("%d %s %d\n", x, s, p[0] + p[1]);
    return sizeof x + sizeof s + sizeof p;
}
