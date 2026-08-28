/* o2_fp_stencil — a FIVE-POINT STENCIL, floating point with a real address
 * pattern under it.
 *
 * WHY IT IS HERE.  o1 is one stream and one recurrence; this is the other half
 * of numeric code — several streams at different offsets, an address per
 * stream, and no loop-carried FP dependence at all.  What it measures is
 * therefore the ADDRESSING around the arithmetic: five loads whose addresses
 * differ by a constant, which is exactly where a compiler either shares one
 * induction variable or rebuilds five.
 */
#include <stdio.h>
#define W 258
static double u[W*W], v[W*W];
int main(void){
    long i, j, r;
    for(i=0;i<W*W;i++) u[i] = (double)((i*7)%23) * 0.125;
    for(r=0;r<220;r++){
        for(i=1;i<W-1;i++)
            for(j=1;j<W-1;j++)
                v[i*W+j] = 0.2*(u[i*W+j] + u[(i-1)*W+j] + u[(i+1)*W+j] + u[i*W+j-1] + u[i*W+j+1]);
        for(i=0;i<W*W;i++) u[i] = v[i];
    }
    { double s=0.0; for(i=0;i<W*W;i+=97) s += u[i]; printf("%.6f\n", s); }
    return 0;
}
