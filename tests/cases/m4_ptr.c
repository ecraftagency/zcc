int main() {
    int x;
    int *p;
    x = 3;
    p = &x;
    *p = *p + 4;
    return *p + x;
}
