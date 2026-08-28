/* x1_goto_cleanup — THE `goto fail` CLEANUP LADDER, C's only error mechanism.
 * WHY: every kernel, every library and every parser is written this way, and the
 * shape is distinctive — many early exits converging on one block, so the CFG is
 * a fan-in and the values live across all of it. No timed program has it. */
#include <stdio.h>
static int step(int v, int k, long *acc){
    int r = -1;
    int *a = 0, *b = 0, *c = 0;
    static int p1[8], p2[8], p3[8];
    if((v & 7) == 0) goto out;
    a = p1; a[0] = v;
    if((v & 15) == 3) goto free_a;
    b = p2; b[0] = v ^ k;
    if((v & 31) == 7) goto free_b;
    c = p3; c[0] = v + k;
    if((v & 63) == 15) goto free_c;
    *acc += a[0] + b[0] + c[0];
    r = 0;
free_c: if(c) *acc += 1;
free_b: if(b) *acc += 2;
free_a: if(a) *acc += 4;
out:    return r;
}
int main(void){
    long i, acc = 0; int ok = 0;
    for(i=0;i<3000000;i++) if(step((int)(i & 255), (int)((i>>4) & 63), &acc) == 0) ok++;
    printf("%ld %d\n", acc, ok);
    return 0;
}
