int main() {
    int i;
    int s;
    s = 0;
    for (i = 1; i <= 10; i = i + 1) s = s + i;
    for (;;) {
        s = s + 1;
        if (s > 60) return s;
    }
    return 0;
}
