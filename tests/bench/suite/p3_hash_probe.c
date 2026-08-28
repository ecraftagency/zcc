/* p3_hash_probe — AN OPEN-ADDRESSED TABLE LARGER THAN THE CACHE.
 *
 * WHY IT IS HERE.  m3_dict_rehash sizes its table to fit; a real one does not.
 * Each probe is a dependent load whose address comes from a multiply and a mask,
 * so this is p1's latency with an ARITHMETIC chain in front of it — the shape
 * where the address computation and the miss overlap or do not.
 */
#include <stdio.h>
#define B 22
#define M ((1u<<B)-1u)
static unsigned key[1u<<B];
static int val[1u<<B];
int main(void){
    unsigned i; unsigned long s = 0, seed = 987654321u;
    for(i=0;i<(1u<<B);i++){ key[i] = 0u; val[i] = 0; }
    for(i=0;i<2000000u;i++){
        unsigned k, h, n = 0;
        seed = seed*6364136223846793005u + 1442695040888963407u;
        k = (unsigned)(seed>>33) | 1u;
        h = (k*2654435761u) & M;
        while(key[h] && key[h] != k){ h = (h+1u) & M; if(++n > 8u) break; }
        if(!key[h]) key[h] = k;
        val[h] += 1;
        s += (unsigned long)val[h] + h;
    }
    printf("%lu\n", s);
    return 0;
}
