/* x2_nested_break — BREAK AND CONTINUE out of three nested loops.
 * WHY: d3_early_exit leaves one loop; this leaves three, so the CFG has edges
 * skipping several latches at once and the loop forest is not a chain. Block
 * placement and the induction variables both have to survive it. */
#include <stdio.h>
#define N 96
static int g[N][N];
int main(void){
    long i, j, k, r; long s = 0, found = 0;
    for(i=0;i<N;i++) for(j=0;j<N;j++) g[i][j] = (int)((i*31 + j*17) & 255);
    for(r=0;r<900;r++)
        for(i=0;i<N;i++){
            if((i & 7) == (r & 7)) continue;
            for(j=0;j<N;j++){
                if(g[i][j] > 250) break;
                for(k=j;k<N;k++){
                    if(g[i][k] == (int)(r & 255)){ found++; goto next_i; }
                    s += g[i][k] & 3;
                    if(k - j > 12) break;
                }
            }
next_i: ;
        }
    printf("%ld %ld\n", s, found);
    return 0;
}
