int printf(char *fmt, ...);
int g;
int counter = 5;
int arr[4];
char *msg = "global string";
int bump() { counter = counter + 1; return counter; }
int main() {
    int i;
    g = 10;
    for (i = 0; i < 4; i = i + 1) arr[i] = i * g;
    bump();
    bump();
    printf("%d %d %d %s\n", g, counter, arr[3], msg);
    return 0;
}
