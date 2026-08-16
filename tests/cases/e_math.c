#include <stdio.h>
#include <math.h>
int main(void) {
    printf("%.4f %.4f %.4f\n", sqrt(2.0), pow(2.0, 10.0), fabs(-3.5));
    printf("%.1f %.1f %.4f\n", floor(3.7), ceil(3.2), fmod(7.5, 2.0));
    printf("%.4f %.4f\n", sin(0.0), exp(1.0));
    return 0;
}
