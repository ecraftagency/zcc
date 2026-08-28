/* o4_fp_fft — a radix-2 BUTTERFLY, the FP kernel with a permuted address.
 * WHY: o1 is one stream, o2 is five at fixed offsets; a butterfly's two operands
 * are a variable distance apart and the distance changes per pass, so the
 * address arithmetic cannot be strength-reduced to a constant stride. */
#include <stdio.h>
#define N 4096
static double re[N], im[N];
int main(void){
    long i, s, j, r;
    for(i=0;i<N;i++){ re[i] = (double)((i*37)%101) - 50.0; im[i] = (double)((i*17)%53) - 26.0; }
    for(r=0;r<600;r++)
        for(s=1;s<N;s<<=1){
            double wr = 1.0 - (double)s/(double)N, wi = (double)s/(double)(2*N);
            for(i=0;i<N;i+=(s<<1))
                for(j=i;j<i+s;j++){
                    double ar = re[j], ai = im[j], br = re[j+s], bi = im[j+s];
                    double tr = br*wr - bi*wi, ti = br*wi + bi*wr;
                    re[j] = ar + tr; im[j] = ai + ti;
                    re[j+s] = ar - tr; im[j+s] = ai - ti;
                }
        }
    { double t=0.0; for(i=0;i<N;i+=131) t += re[i] + im[i]; printf("%.4f\n", t); }
    return 0;
}
