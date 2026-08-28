/* p5_list_walk — A LINKED LIST of real nodes, walked and mutated.
 * WHY: p1 chases indices in one array; this chases POINTERS through 48-byte
 * nodes, so each step is a load of a pointer plus loads of fields at offsets —
 * the shape of every allocator, cache and intrusive-list in real C. */
#include <stdio.h>
#define N 400000
struct Node { struct Node *next; long a, b, c, d, e; };
static struct Node pool[N];
int main(void){
    long i; unsigned long s = 0, seed = 7u; struct Node *p;
    for(i=0;i<N;i++){ pool[i].a = i; pool[i].b = i*3; pool[i].c = i^0x5a5a; pool[i].d = 0; pool[i].e = i&63; }
    for(i=0;i<N;i++){ seed = seed*1103515245u + 12345u; pool[i].next = &pool[(seed>>16) % N]; }
    p = &pool[0];
    for(i=0;i<3000000;i++){ p->d += p->a ^ p->c; s += (unsigned long)p->d + (unsigned long)p->e; p = p->next; }
    printf("%lu\n", s);
    return 0;
}
