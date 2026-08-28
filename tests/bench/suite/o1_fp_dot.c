/* o1_fp_dot — DENSE DOUBLE-PRECISION REDUCTION, the shape every numeric kernel
 * has and the suite did not.
 *
 * WHY IT IS HERE.  Law 3c names heavy floating point as a class the taxonomy
 * suite does not sample: f1..f3 touch scalar float arithmetic and nothing
 * touches an FP RECURRENCE at length.  A dot product is the smallest honest
 * one — the accumulator is loop-carried through an `fmadd` whose latency the
 * loop cannot hide, so the kernel is bound by FP dependence rather than by
 * issue width, and a compiler that does not split the accumulator pays it in
 * full.  gcc -O1 does not split it either, which is what makes this a fair
 * comparison of the code around it rather than of a vectorizer.
 */
#include <stdio.h>
#define N 4096
static double a[N], b[N];
int main(void){
    long i, r; double s = 0.0;
    for(i=0;i<N;i++){ a[i] = (double)(i%97) * 0.5 + 1.0; b[i] = (double)(i%89) * 0.25 - 3.0; }
    for(r=0;r<3000;r++){
        double t = 0.0;
        for(i=0;i<N;i++) t += a[i]*b[i];
        s += t / (double)(r+1);
    }
    printf("%.6f\n", s);
    return 0;
}
