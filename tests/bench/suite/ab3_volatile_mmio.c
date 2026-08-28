/* ab3_volatile_mmio — VOLATILE, the driver register-poking shape.
 *
 * WHY IT IS HERE.  C99 6.7.3p6: an object with volatile-qualified type may be
 * modified in ways unknown to the implementation, so every access is a side
 * effect and NONE of them may be added, removed, reordered against another, or
 * cached in a register.  That is a hard fence across every optimization in the
 * pipeline, and no timed program in this suite has one — so nothing measures
 * what the compiler does with the code AROUND a fence it may not cross.  The
 * question the program asks is narrow and real: does the non-volatile work
 * between two volatile accesses still get optimized, or does the fence
 * pessimize its neighbourhood?  A device driver is nothing but this shape.
 */
#include <stdio.h>
#define NREG 64
static volatile unsigned mmio[NREG];
static unsigned shadow[NREG];
int main(void){
    unsigned long r, acc = 0;
    unsigned i;
    for(i=0;i<NREG;i++){ mmio[i] = i * 2654435761u; shadow[i] = 0; }
    for(r=0;r<420000;r++){
        unsigned status = mmio[0];              /* poll */
        unsigned mask = (unsigned)r * 65599u;
        for(i=1;i<NREG;i+=2){
            unsigned v = mmio[i];               /* read — may not be hoisted */
            unsigned t = (v ^ mask) + (v >> 3); /* pure work between accesses */
            t = t * 31u + (t >> 11);
            shadow[i] = t;
            mmio[i] = t ^ status;               /* write — may not be sunk */
            acc += (unsigned long)(t & 255u);
        }
        if((status & 1u) != 0u) mmio[NREG-1] = status + 1u;
        acc += (unsigned long)mmio[NREG-1];
    }
    for(i=0;i<NREG;i++) acc += shadow[i];
    printf("%lu\n", acc);
    return 0;
}
