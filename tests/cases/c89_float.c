int printf(char *fmt, ...);
double fabs(double x);
double sqrt(double x);
int main() {
    double d; float f; int i;
    d = 1.5;
    f = 0.25f;
    printf("%f %f\n", d, f);
    printf("%f\n", d + f);
    printf("%f %f %f\n", d * 3.0, d / 2.0, d - 0.75);
    i = 7;
    d = i;
    printf("%f\n", d / 2);
    d = 9.9;
    i = d;
    printf("%d\n", i);
    printf("%d\n", (int)-3.7);
    f = 1.0e10f;
    printf("%f\n", f);
    printf("%f\n", sqrt(2.0));
    printf("%f\n", fabs(-2.5));
    d = 0.1;
    if (d > 0.05 && d < 0.2) printf("cmp ok\n");
    if (d == 0.1) printf("eq ok\n");
    while (d < 0.5) d += 0.1;
    printf("%f\n", d);
    printf("%d\n", 1.5 > 1.0);
    printf("%f\n", -d);
    /* K&R 1.2: bảng Fahrenheit-Celsius */
    for (i = 0; i <= 300; i += 100)
        printf("%3d %6.1f\n", i, (5.0 / 9.0) * (i - 32));
    return 0;
}
