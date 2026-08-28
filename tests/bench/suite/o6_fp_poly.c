/* o6_fp_poly — HORNER, a pure FP dependence chain with no memory at all.
 * WHY: it isolates fmadd latency from everything else. If zcc and gcc emit the
 * same chain the ratio is 1.000 by construction, so any deviation is a real
 * scheduling or materialization difference and nothing else. */
#include <stdio.h>
int main(void){
    long i; double s = 0.0;
    static const double c[9] = {1.5,-2.25,0.75,3.125,-0.5,1.0625,-4.0,2.5,0.125};
    for(i=0;i<4000000;i++){
        double x = (double)(i & 1023) * 0.0009765625, p = c[0];
        int k;
        for(k=1;k<9;k++) p = p*x + c[k];
        s += p;
    }
    printf("%.6f\n", s);
    return 0;
}
