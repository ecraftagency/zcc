int main() {
    int a[4];
    int *p;
    p = a;
    *p = 10;
    *(p + 1) = 20;
    *(p + 2) = 30;
    p = p + 3;
    *p = 40;
    return a[0] + a[1] + a[2] + a[3] + (p - a);
}
