/* v3_struct_copy — STRUCT ASSIGNMENT of several sizes, by value.
 * WHY: e3_struct_byval passes one; this ASSIGNS them in a loop, which is the
 * memcpy-inline decision (MEASURED M14) on the hot path at four different
 * sizes at once. */
#include <stdio.h>
struct S8  { long a; };
struct S24 { long a, b, c; };
struct S64 { long a[8]; };
struct S96 { long a[12]; };
static struct S8 t8[64]; static struct S24 t24[64]; static struct S64 t64[64]; static struct S96 t96[64];
int main(void){
    long i, r; unsigned long s = 0;
    for(i=0;i<64;i++){ int k; t8[i].a = i; t24[i].a = i; t24[i].b = i*2; t24[i].c = i*3;
        for(k=0;k<8;k++) t64[i].a[k] = i+k; for(k=0;k<12;k++) t96[i].a[k] = i*k; }
    for(r=0;r<160000;r++){
        long a = r & 63, b = (r>>3) & 63;
        struct S8 x8 = t8[a]; struct S24 x24 = t24[a]; struct S64 x64 = t64[a]; struct S96 x96 = t96[a];
        x8.a += r; x24.b += r; x64.a[5] += r; x96.a[11] += r;
        t8[b] = x8; t24[b] = x24; t64[b] = x64; t96[b] = x96;
        s += (unsigned long)(x8.a + x24.c + x64.a[5] + x96.a[11]);
    }
    printf("%lu\n", s);
    return 0;
}
