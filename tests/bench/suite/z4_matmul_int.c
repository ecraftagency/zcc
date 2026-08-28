/* z4_matmul_int — INTEGER MATRIX MULTIPLY, the row-strided address in its
 * pure form.
 * WHY: `tests/bench/matmul.c` is the float version and lives outside the suite.
 * The integer one is the case MEASURED M9 was taken on — a row walked with a
 * multiply in front of the load — and it belongs in the timed set rather than
 * in a side file. */
#include <stdio.h>
#define M 200
static int A[M][M], B[M][M], C[M][M];
int main(void){
    long i, j, k, r; unsigned long s = 0;
    for(i=0;i<M;i++) for(j=0;j<M;j++){ A[i][j] = (int)((i*3+j) & 63); B[i][j] = (int)((i+j*5) & 63); }
    for(r=0;r<6;r++){
        for(i=0;i<M;i++)
            for(j=0;j<M;j++){
                int t = 0;
                for(k=0;k<M;k++) t += A[i][k] * B[k][j];
                C[i][j] = t + (int)r;
            }
        s += (unsigned long)C[0][0] + (unsigned long)C[M-1][M-1] + (unsigned long)C[M/2][M/2];
    }
    printf("%lu\n", s);
    return 0;
}
