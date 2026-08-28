/* ac1_huffman — BITSTREAM encode and decode, sub-byte I/O.
 *
 * WHY IT IS HERE.  `z2_rle` is byte-granular; every real codec, container and
 * wire format is not.  A bitstream keeps a shift accumulator and a bit count in
 * registers across a loop whose branch depends on data, and decoding walks a
 * table one bit at a time — so the loop carries TWO recurrences (the
 * accumulator and the count) where the rest of the suite carries one, and the
 * shifts are by a VARIABLE the compiler cannot fold.
 */
#include <stdio.h>
#define NSYM 32
#define NIN  (1<<15)
static unsigned char sym[NIN];
static unsigned char clen[NSYM];
static unsigned      code[NSYM];
static unsigned char bits[NIN*2];
int main(void){
    unsigned long r, total = 0, digest = 0;
    unsigned i;
    /* a fixed, valid prefix code: lengths 2..7, canonical */
    { unsigned c = 0, l, k = 0;
      for(l=2;l<=7 && k<NSYM;l++){
        unsigned cnt = (l < 7u) ? (1u << (l - 2u)) : (NSYM - k);
        for(i=0;i<cnt && k<NSYM;i++){ clen[k] = (unsigned char)l; code[k] = c; c++; k++; }
        c <<= 1;
      }
    }
    for(i=0;i<NIN;i++){ unsigned h = i*2654435761u >> 21; sym[i] = (unsigned char)((h % 100u < 70u) ? (h & 7u) : (h & 31u)); }
    for(r=0;r<70;r++){
        unsigned acc = 0, nbits = 0, o = 0;
        for(i=0;i<NIN;i++){
            unsigned s = sym[i], l = clen[s];
            acc = (acc << l) | code[s];
            nbits += l;
            while(nbits >= 8u){ nbits -= 8u; bits[o++] = (unsigned char)(acc >> nbits); }
        }
        if(nbits > 0u) bits[o++] = (unsigned char)(acc << (8u - nbits));
        total += o;
        { /* decode by walking the canonical code one bit at a time */
          unsigned p = 0, held = 0, nh = 0, cur = 0, cl = 0, n = 0;
          while(n < NIN && p <= o){
            if(nh == 0u){ if(p >= o) break; held = bits[p++]; nh = 8u; }
            cur = (cur << 1) | ((held >> (nh - 1u)) & 1u);
            nh--; cl++;
            if(cl >= 2u && cl <= 7u){
              unsigned t;
              for(t=0;t<NSYM;t++) if(clen[t] == (unsigned char)cl && code[t] == cur){ digest = digest*31u + t; n++; cur = 0; cl = 0; break; }
            }
            if(cl > 7u){ cur = 0; cl = 0; }
          }
        }
    }
    printf("%lu %lu\n", total, digest);
    return 0;
}
