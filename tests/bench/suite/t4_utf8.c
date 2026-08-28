/* t4_utf8 — UTF-8 DECODE, a length-driven VARIABLE-STRIDE loop.
 * WHY: the stride is decided by the byte just read, so the next address depends
 * on a load and a compare chain — an induction variable no strength reduction
 * can touch, and the exact shape a text codec is. */
#include <stdio.h>
#define N (1<<18)
static unsigned char b[N];
int main(void){
    long i, r; unsigned long cps = 0, sum = 0;
    /* `b[i++] = f(i)` would read and modify `i` with no sequence point between
     * them — undefined by C99 6.5p2, and a differential taken at an undefined
     * point says nothing about either compiler (Article E). Every step below
     * writes through an index it does not also advance in the same expression. */
    i = 0;
    while(i < N-4){
        unsigned k = (unsigned)((i*2654435761u)>>26) % 4u;
        if(k == 1){ b[i] = 0xC2; b[i+1] = 0xA9; i += 2; }
        else if(k == 2){ b[i] = 0xE2; b[i+1] = 0x82; b[i+2] = 0xAC; i += 3; }
        else { b[i] = (unsigned char)(i & 127); i += 1; }
    }
    while(i < N){ b[i] = 0x41; i += 1; }
    for(r=0;r<60;r++){
        i = 0;
        while(i < N){
            unsigned c = b[i];
            if(c < 0x80){ sum += c; i += 1; }
            else if((c & 0xE0) == 0xC0 && i+1 < N){ sum += ((c & 31u)<<6) | (b[i+1] & 63u); i += 2; }
            else if((c & 0xF0) == 0xE0 && i+2 < N){ sum += ((c & 15u)<<12) | ((b[i+1] & 63u)<<6) | (b[i+2] & 63u); i += 3; }
            else { sum += 0xFFFD; i += 1; }
            cps++;
        }
    }
    printf("%lu %lu\n", cps, sum);
    return 0;
}
