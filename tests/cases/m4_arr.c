int main() {
    int a[5];
    int i;
    for (i = 0; i < 5; i = i + 1) a[i] = i * i;
    return a[0] + a[1] + a[2] + a[3] + a[4];
}
