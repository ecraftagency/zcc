int sum(int *v, int n) {
    int s;
    int i;
    s = 0;
    for (i = 0; i < n; i = i + 1) s = s + v[i];
    return s;
}
int main() {
    int a[10];
    char *p;
    int i;
    for (i = 0; i < 10; i = i + 1) a[i] = i;
    return sum(a, 10) + sizeof(a) + sizeof(a[0]) + sizeof(p) + sizeof *p;
}
