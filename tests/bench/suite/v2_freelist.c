/* v2_freelist — ALLOCATE AND FREE through a singly-linked free list.
 * WHY: v1 never frees; a real allocator does, and the free list makes the next
 * allocation depend on a LOAD from the block just returned — the dependent-load
 * shape p1 measures, but interleaved with real work. */
#include <stdio.h>
#define NB 65536
struct Blk { struct Blk *next; long pay[7]; };
static struct Blk pool[NB];
static struct Blk *head;
int main(void){
    long i; unsigned long s = 0, seed = 31u;
    struct Blk *live[256];
    int nlive = 0;
    for(i=0;i<NB;i++){ pool[i].next = (i+1 < NB) ? &pool[i+1] : 0; }
    head = &pool[0];
    for(i=0;i<2000000;i++){
        seed = seed*6364136223846793005UL + 1442695040888963407UL;
        if(nlive < 256 && head && ((seed>>33) & 1UL)){
            struct Blk *b = head; head = b->next; b->pay[0] = i; b->pay[3] = (long)seed;
            live[nlive++] = b; s += (unsigned long)b->pay[0];
        } else if(nlive > 0){
            int k = (int)((seed>>34) % (unsigned long)nlive);
            struct Blk *b = live[k]; live[k] = live[--nlive];
            s += (unsigned long)(b->pay[0] ^ b->pay[3]);
            b->next = head; head = b;
        }
    }
    printf("%lu\n", s);
    return 0;
}
