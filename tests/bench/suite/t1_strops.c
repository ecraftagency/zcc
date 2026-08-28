/* t1_strops — strlen / strcmp / strchr written out, over SHORT strings.
 * WHY: g2_strlen is one loop over one long string. Real string code runs short
 * strings many times, so loop setup and the early exit dominate rather than the
 * steady state — a profile the suite never sampled. */
#include <stdio.h>
static long slen(const char *s){ const char *p = s; while(*p) p++; return p - s; }
static int scmp(const char *a, const char *b){ while(*a && *a == *b){ a++; b++; } return (int)((unsigned char)*a) - (int)((unsigned char)*b); }
static const char *schr(const char *s, int c){ for(; *s; s++) if(*s == (char)c) return s; return 0; }
static char buf[64][24];
int main(void){
    long i, r, s = 0;
    for(i=0;i<64;i++){ int j, n = 3 + (int)(i % 18); for(j=0;j<n;j++) buf[i][j] = (char)('a' + ((i*7+j*3) % 26)); buf[i][n] = 0; }
    for(r=0;r<400000;r++){
        long a = r & 63, b = (r>>3) & 63;
        s += slen(buf[a]);
        s += scmp(buf[a], buf[b]) & 15;
        { const char *p = schr(buf[a], 'e' + (int)(r & 3)); s += p ? (p - buf[a]) : 1; }
    }
    printf("%ld\n", s);
    return 0;
}
