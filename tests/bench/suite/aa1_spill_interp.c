/* aa1_spill_interp — A FUNCTION LARGE ENOUGH TO SPILL, which no other program
 * in this suite is.
 *
 * WHY IT IS HERE.  Measured 2026-08-29 across the ninety-program suite: THREE
 * functions exceed 200 instructions and none but a `main` exceeds 400, while
 * sqlite has 154 functions above 200 and 18 above 1000 — and 21,331 of its
 * instructions touch `[sp` against the whole suite's 710.  So the register
 * allocator, which is over half of zcc's measured size gap, was never sampled
 * by a timed program: in a small function thirty registers are always enough
 * and the allocator never has to decide anything.  That is the whole of why the
 * suite reads 1.079 on the instruction axis and sqlite reads 1.108.
 *
 * THE SHAPE.  A register machine with sixteen registers held as sixteen LOCALS
 * — not an array, which would live in memory by construction and measure
 * nothing — four accumulators, and a forty-eight-arm dispatch around them.
 * Every arm reads three of the sixteen and writes two, so all twenty are live
 * across the entire loop and the allocator must choose what to keep.
 *
 * SIZE IS THE POINT, and this is the one program in the suite that cannot be
 * diffed by hand — `k1_dispatch` exists to be readable and deliberately stays
 * at forty arms of two instructions.  This one exists to make the allocator
 * spill, which requires more live values than the machine has registers, and no
 * amount of readability substitutes for that.  Read `k1` to understand
 * dispatch; measure this one to find out what the allocator does under load.
 */
#include <stdio.h>
#define NPROG 4096
static unsigned char prog[NPROG];
int main(void){
    unsigned long i, r, digest = 0;
    unsigned r0,r1,r2,r3,r4,r5,r6,r7,r8,r9,r10,r11,r12,r13,r14,r15;
    for(i=0;i<NPROG;i++) prog[i] = (unsigned char)((i*167u + (i>>3)*29u) & 255u);
    for(r=0;r<2200;r++){
        unsigned pc;
        unsigned acc0 = (unsigned)r, acc1 = 0x9e3779b9u, acc2 = 0x85ebca6bu, acc3 = 0xc2b2ae35u;
        r0=1;r1=2;r2=3;r3=5;r4=7;r5=11;r6=13;r7=17;
        r8=19;r9=23;r10=29;r11=31;r12=37;r13=41;r14=43;r15=47;
        for(pc=0;pc<NPROG;pc++){
            unsigned op = prog[pc];
            switch(op % 48u){
            case  0: r0 += r1 ^ (r3 * 3u); acc0 = acc0*3u + r2; break;
            case  1: r1 ^= r8 + (r8 * 5u); acc1 = acc1*5u + r13; break;
            case  2: r2 += r15 + (r13 * 7u); acc2 = acc2*7u + r8; break;
            case  3: r3 ^= r6 ^ (r2 * 9u); acc3 = acc3*9u + r3; break;
            case  4: r4 += r13 ^ (r7 * 11u); acc0 = acc0*11u + r14; break;
            case  5: r5 ^= r4 + (r12 * 13u); acc1 = acc1*13u + r9; break;
            case  6: r6 += r11 ^ (r1 * 3u); acc2 = acc2*15u + r4; break;
            case  7: r7 ^= r2 + (r6 * 5u); acc3 = acc3*3u + r15; break;
            case  8: r8 += r9 + (r11 * 7u); acc0 = acc0*5u + r10; break;
            case  9: r9 ^= r0 ^ (r0 * 9u); acc1 = acc1*7u + r5; break;
            case 10: r10 += r7 ^ (r5 * 11u); acc2 = acc2*9u + r0; break;
            case 11: r11 ^= r14 + (r10 * 13u); acc3 = acc3*11u + r11; break;
            case 12: r12 += r5 ^ (r15 * 3u); acc0 = acc0*13u + r6; break;
            case 13: r13 ^= r12 + (r4 * 5u); acc1 = acc1*15u + r1; break;
            case 14: r14 += r3 + (r9 * 7u); acc2 = acc2*3u + r12; break;
            case 15: r15 ^= r10 ^ (r14 * 9u); acc3 = acc3*5u + r7; break;
            case 16: r0 += r1 ^ (r3 * 11u); acc0 = acc0*7u + r2; break;
            case 17: r1 ^= r8 + (r8 * 13u); acc1 = acc1*9u + r13; break;
            case 18: r2 += r15 ^ (r13 * 3u); acc2 = acc2*11u + r8; break;
            case 19: r3 ^= r6 + (r2 * 5u); acc3 = acc3*13u + r3; break;
            case 20: r4 += r13 + (r7 * 7u); acc0 = acc0*15u + r14; break;
            case 21: r5 ^= r4 ^ (r12 * 9u); acc1 = acc1*3u + r9; break;
            case 22: r6 += r11 ^ (r1 * 11u); acc2 = acc2*5u + r4; break;
            case 23: r7 ^= r2 + (r6 * 13u); acc3 = acc3*7u + r15; break;
            case 24: r8 += r9 ^ (r11 * 3u); acc0 = acc0*9u + r10; break;
            case 25: r9 ^= r0 + (r0 * 5u); acc1 = acc1*11u + r5; break;
            case 26: r10 += r7 + (r5 * 7u); acc2 = acc2*13u + r0; break;
            case 27: r11 ^= r14 ^ (r10 * 9u); acc3 = acc3*15u + r11; break;
            case 28: r12 += r5 ^ (r15 * 11u); acc0 = acc0*3u + r6; break;
            case 29: r13 ^= r12 + (r4 * 13u); acc1 = acc1*5u + r1; break;
            case 30: r14 += r3 ^ (r9 * 3u); acc2 = acc2*7u + r12; break;
            case 31: r15 ^= r10 + (r14 * 5u); acc3 = acc3*9u + r7; break;
            case 32: r0 += r1 + (r3 * 7u); acc0 = acc0*11u + r2; break;
            case 33: r1 ^= r8 ^ (r8 * 9u); acc1 = acc1*13u + r13; break;
            case 34: r2 += r15 ^ (r13 * 11u); acc2 = acc2*15u + r8; break;
            case 35: r3 ^= r6 + (r2 * 13u); acc3 = acc3*3u + r3; break;
            case 36: r4 += r13 ^ (r7 * 3u); acc0 = acc0*5u + r14; break;
            case 37: r5 ^= r4 + (r12 * 5u); acc1 = acc1*7u + r9; break;
            case 38: r6 += r11 + (r1 * 7u); acc2 = acc2*9u + r4; break;
            case 39: r7 ^= r2 ^ (r6 * 9u); acc3 = acc3*11u + r15; break;
            case 40: r8 += r9 ^ (r11 * 11u); acc0 = acc0*13u + r10; break;
            case 41: r9 ^= r0 + (r0 * 13u); acc1 = acc1*15u + r5; break;
            case 42: r10 += r7 ^ (r5 * 3u); acc2 = acc2*3u + r0; break;
            case 43: r11 ^= r14 + (r10 * 5u); acc3 = acc3*5u + r11; break;
            case 44: r12 += r5 + (r15 * 7u); acc0 = acc0*7u + r6; break;
            case 45: r13 ^= r12 ^ (r4 * 9u); acc1 = acc1*9u + r1; break;
            case 46: r14 += r3 ^ (r9 * 11u); acc2 = acc2*11u + r12; break;
            case 47: r15 ^= r10 + (r14 * 13u); acc3 = acc3*13u + r7; break;
            default: r15 += r0 ^ r3; acc3 ^= r15; break;
            }
            if((op & 64u) != 0u) acc0 = acc0*3u + r8;
            if((op & 128u) != 0u) acc1 = acc1*5u + r9;
        }
        digest += (unsigned long)(acc0 ^ acc1) + (unsigned long)(acc2 ^ acc3)
                + (unsigned long)(r0 + r7 + r15);
    }
    printf("%lu\n", digest);
    return 0;
}
