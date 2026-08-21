/* Array indexing + inner-product accumulation: the inner k-loop keeps an index,
   two base addresses and an accumulator live at once. */
#include <stdio.h>

#define N 240
static long A[N][N], B[N][N], C[N][N];

int main(void) {
    int i, j, k;
    for (i = 0; i < N; i++)
        for (j = 0; j < N; j++) { A[i][j] = (i * 7 + j) % 13; B[i][j] = (i + j * 5) % 11; }
    int r;
    for (r = 0; r < 12; r++)
        for (i = 0; i < N; i++)
            for (j = 0; j < N; j++) {
                long s = 0;
                for (k = 0; k < N; k++) s += A[i][k] * B[k][j];
                C[i][j] = s;
            }
    long sum = 0;
    for (i = 0; i < N; i++) for (j = 0; j < N; j++) sum += C[i][j];
    printf("%ld\n", sum);
    return 0;
}
