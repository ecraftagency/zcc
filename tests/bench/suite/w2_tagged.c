/* w2_tagged — A TAGGED UNION evaluated in a loop, the AST-node shape.
 * WHY: a discriminated union is a load of the tag, a branch on it, and a read of
 * a differently-typed member at the same offset — the single most common data
 * structure in an interpreter or a compiler, and absent from the suite. */
#include <stdio.h>
enum { T_I, T_D, T_P, T_B };
struct V { int tag; union { long i; double d; struct V *p; int b; } u; };
#define N 4096
static struct V v[N];
int main(void){
    long i, r; double fs = 0.0; long is = 0;
    for(i=0;i<N;i++){ v[i].tag = (int)(i & 3);
        switch(v[i].tag){ case T_I: v[i].u.i = i; break; case T_D: v[i].u.d = (double)i * 0.5; break;
                          case T_P: v[i].u.p = &v[(i*7) & (N-1)]; break; default: v[i].u.b = (int)(i & 1); } }
    for(r=0;r<2500;r++)
        for(i=0;i<N;i++){
            switch(v[i].tag){
                case T_I: is += v[i].u.i + r; break;
                case T_D: fs += v[i].u.d; break;
                case T_P: is += v[i].u.p->tag * 3; break;
                default:  is += v[i].u.b ? 7 : -3; break;
            }
        }
    printf("%ld %.4f\n", is, fs);
    return 0;
}
