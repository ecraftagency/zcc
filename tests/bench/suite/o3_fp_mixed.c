/* o3_fp_mixed — INT AND FLOAT IN ONE LOOP, with conversions on the hot path.
 *
 * WHY IT IS HERE.  Real numeric code is not pure float: it indexes with
 * integers, tests with integers, and converts between the two.  A64 moves
 * between the GPR and FPR files with `scvtf`/`fcvtzs`, and each move is a
 * register-file crossing the scheduler must cover.  No program in the suite has
 * both files busy at once, so the crossing has never been measured.
 */
#include <stdio.h>
#define N 2048
static int xi[N];
static float xf[N];
int main(void){
    long i, r; double acc = 0.0; long hits = 0;
    for(i=0;i<N;i++){ xi[i] = (int)((i*2654435761u)>>20) % 1000 - 500; xf[i] = (float)xi[i] * 0.03125f; }
    for(r=0;r<9000;r++){
        float t = 0.0f; int c = 0;
        for(i=0;i<N;i++){
            float d = xf[i] * (float)(xi[i] & 15);
            if(d > 1.0f){ t += d; c++; }
            else t -= (float)(xi[i] >> 3) * 0.5f;
        }
        acc += (double)t; hits += c;
    }
    printf("%.4f %ld\n", acc, hits);
    return 0;
}
