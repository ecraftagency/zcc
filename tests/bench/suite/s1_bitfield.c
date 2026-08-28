/* s1_bitfield — PACKED BITFIELDS read and written in a loop.
 *
 * WHY IT IS HERE.  Law 3c names bitfields as unsampled.  c2_bitfield exists but
 * is not timed against a gcc that keeps up; this one is a header parser's shape
 * — several fields in one word, read, tested, and written back — which is a
 * mask/shift/insert sequence per access and exactly where `bfi` either fires or
 * does not.
 */
#include <stdio.h>
struct Hdr {
    unsigned kind : 4;
    unsigned flags : 6;
    unsigned len : 12;
    unsigned prio : 3;
    unsigned live : 1;
};
#define N 4096
static struct Hdr h[N];
int main(void){
    long i, r; unsigned long s = 0;
    for(i=0;i<N;i++){ h[i].kind = (unsigned)(i & 15); h[i].flags = (unsigned)((i>>2) & 63);
                      h[i].len = (unsigned)((i*7) & 4095); h[i].prio = (unsigned)(i & 7); h[i].live = 1u; }
    for(r=0;r<2500;r++)
        for(i=0;i<N;i++){
            if(h[i].kind > 8u){ h[i].flags = (h[i].flags + 1u) & 63u; h[i].live = 0u; }
            else { h[i].len = (h[i].len + h[i].prio) & 4095u; h[i].live = 1u; }
            s += (unsigned long)h[i].len + h[i].flags + h[i].live;
        }
    printf("%lu\n", s);
    return 0;
}
