/* ab2_format — INTEGER FORMATTING, the `snprintf` inner loop written out.
 *
 * WHY IT IS HERE.  Every program that prints does this, and it is a shape the
 * suite has nowhere: a division by a CONSTANT ten in a loop whose trip count is
 * the number of digits, a reversal, width and sign handling, and a bounded
 * write into a caller's buffer.  `MEASURED M25` recorded that zcc's
 * division-by-constant rewrite lost to gcc's ENCODING of the same theorem — this
 * is the program where that encoding is on the hot path rather than in a
 * synthetic loop, and it is also where the digit table versus repeated division
 * question shows up.
 */
#include <stdio.h>
static int fmt_i64(char *out, int cap, long v, int width, int zero){
    char tmp[24]; int n = 0, neg = 0, i, w;
    unsigned long u;
    if(v < 0){ neg = 1; u = (unsigned long)(-(v + 1)) + 1ul; } else u = (unsigned long)v;
    do { tmp[n++] = (char)('0' + (int)(u % 10ul)); u /= 10ul; } while(u != 0ul);
    w = n + neg;
    i = 0;
    if(!zero && w < width) for(; w < width; w++) if(i < cap) out[i++] = ' ';
    if(neg && i < cap) out[i++] = '-';
    if(zero && w < width) for(; w < width; w++) if(i < cap) out[i++] = '0';
    while(n > 0 && i < cap) out[i++] = tmp[--n];
    return i;
}
static int fmt_hex(char *out, int cap, unsigned long v, int width){
    char tmp[20]; int n = 0, i = 0, w;
    do { unsigned d = (unsigned)(v & 15ul); tmp[n++] = (char)(d < 10u ? '0' + (int)d : 'a' + (int)d - 10); v >>= 4; } while(v != 0ul);
    for(w = n; w < width; w++) if(i < cap) out[i++] = '0';
    while(n > 0 && i < cap) out[i++] = tmp[--n];
    return i;
}
int main(void){
    char buf[64];
    unsigned long r, total = 0, csum = 0;
    for(r=0;r<400000;r++){
        long v = (long)(r * 2654435761u >> 7) - 8000000L;
        int n = fmt_i64(buf, (int)sizeof buf, v, 12, (int)(r & 1u));
        n += fmt_hex(buf + n, (int)sizeof buf - n, (unsigned long)(v ^ (long)r), 8);
        total += (unsigned long)n;
        { int k; for(k=0;k<n;k++) csum = csum * 31u + (unsigned char)buf[k]; }
    }
    printf("%lu %lu\n", total, csum);
    return 0;
}
