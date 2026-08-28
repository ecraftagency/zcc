/* t2_tokenize — SPLIT A BUFFER ON DELIMITERS, the strtok shape.
 * WHY: m1/m2 are state machines over bytes; this is the simpler and far more
 * common one — a scan loop with two nested conditions and a pointer written
 * back. Its inner loop is four instructions, so any excess is a large fraction. */
#include <stdio.h>
#define BUF (1<<18)
static char text[BUF];
int main(void){
    long i, r, toks = 0, chars = 0;
    for(i=0;i<BUF-1;i++){ unsigned k = (unsigned)((i*2654435761u)>>24); text[i] = (k % 7u == 0u) ? ' ' : (char)('a' + (k % 26u)); }
    text[BUF-1] = 0;
    for(r=0;r<40;r++){
        const char *p = text;
        while(*p){
            while(*p == ' ') p++;
            if(!*p) break;
            { const char *q = p; while(*p && *p != ' ') p++; chars += p - q; toks++; }
        }
    }
    printf("%ld %ld\n", toks, chars);
    return 0;
}
