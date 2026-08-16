/* K&R 1.2/ex1-3,1-4: bang doi Fahrenheit-Celsius va nguoc lai, co header */
#include <stdio.h>
#define LOWER 0
#define UPPER 300
#define STEP 20
int main(void) {
    int fahr;
    printf("%4s %7s\n", "F", "C");
    for (fahr = LOWER; fahr <= UPPER; fahr = fahr + STEP)
        printf("%4d %7.1f\n", fahr, (5.0 / 9.0) * (fahr - 32));
    printf("%4s %7s\n", "C", "F");
    for (fahr = -20; fahr <= 100; fahr = fahr + 20)
        printf("%4d %7.1f\n", fahr, fahr * 9.0 / 5.0 + 32.0);
    return 0;
}
