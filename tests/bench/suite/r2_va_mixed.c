/* r2_va_mixed — VARARGS MIXING INTEGER AND DOUBLE.
 * WHY: AAPCS64 gives a variadic call two save areas — the general one and the
 * FP one — and the callee walks them independently. r1 exercises only the
 * first. This is the printf("%d %f") shape every C program has. */
#include <stdio.h>
#include <stdarg.h>
static double mix(int n, ...){
    va_list ap; double s = 0.0; int i;
    va_start(ap, n);
    for(i=0;i<n;i++){
        if(i & 1) s += va_arg(ap, double);
        else s += (double)va_arg(ap, int) * 0.5;
    }
    va_end(ap);
    return s;
}
int main(void){
    long i; double s = 0.0;
    for(i=0;i<300000;i++)
        s += mix(6, (int)(i&255), (double)(i&63)*0.25, (int)(i&127), (double)(i&31)*0.5, (int)(i&15), (double)(i&7));
    printf("%.4f\n", s);
    return 0;
}
