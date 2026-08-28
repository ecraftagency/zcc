/* o5_fp_cmp — FLOAT COMPARES driving control flow.
 * WHY: every other FP program here is arithmetic; this one BRANCHES on `fcmp`,
 * which sets NZCV from the FP unit and costs a register-file crossing the
 * integer compares do not. A min/max/clamp reduction is the honest shape. */
#include <stdio.h>
#define N 8192
static double v[N];
int main(void){
    long i, r; double lo = 1e30, hi = -1e30, s = 0.0; long n = 0;
    for(i=0;i<N;i++) v[i] = (double)((i*2654435761u)>>18) * 0.001 - 4000.0;
    for(r=0;r<3000;r++)
        for(i=0;i<N;i++){
            double x = v[i] + (double)r * 0.001;
            if(x < lo) lo = x;
            if(x > hi) hi = x;
            if(x > 0.0 && x < 100.0){ s += x; n++; }
            else if(x <= 0.0) s -= x * 0.5;
        }
    printf("%.3f %.3f %.3f %ld\n", lo, hi, s, n);
    return 0;
}
