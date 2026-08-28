/* aa2_wide_live — ONE STRAIGHT-LINE REGION with far more live values than the
 * machine has registers, the other half of the pressure question `aa1` asks
 * with a loop.
 *
 * WHY IT IS HERE.  A dispatch loop spills because values live ACROSS a back
 * edge; a wide expression spills because too many are live AT ONE POINT.  The
 * allocator answers those with different mechanisms — loop-carried next-use
 * against local eviction and reload placement — and the ninety-program suite
 * sampled neither, having no function large enough to run out of registers.
 * Forty words, all read and all rewritten four times before any of them dies:
 * on a machine with thirty-one general registers the allocator has no choice
 * but to spill, and WHICH ones it picks is the measurement.
 *
 * This is the shape of every block cipher round, every hash core and every
 * fixed-size state update in real software.
 */
#include <stdio.h>
#define N 40
int main(void){
    unsigned long r, out = 0;
    unsigned s[N];
    unsigned i;
    for(i=0;i<N;i++) s[i] = 0x243f6a88u + i*0x9e3779b9u;
    for(r=0;r<420000;r++){
    unsigned v0=s[0], v1=s[1], v2=s[2], v3=s[3], v4=s[4], v5=s[5], v6=s[6], v7=s[7], v8=s[8], v9=s[9], v10=s[10], v11=s[11], v12=s[12], v13=s[13], v14=s[14], v15=s[15], v16=s[16], v17=s[17], v18=s[18], v19=s[19], v20=s[20], v21=s[21], v22=s[22], v23=s[23], v24=s[24], v25=s[25], v26=s[26], v27=s[27], v28=s[28], v29=s[29], v30=s[30], v31=s[31], v32=s[32], v33=s[33], v34=s[34], v35=s[35], v36=s[36], v37=s[37], v38=s[38], v39=s[39];
        v0 += (unsigned)r;
        v0 += v7 ^ (v1 * 3u); v1 ^= v10 ^ (v12 * 5u); v2 += v13 ^ (v23 * 7u); v3 ^= v16 ^ (v34 * 9u); v4 += v19 ^ (v5 * 11u); v5 ^= v22 ^ (v16 * 13u); v6 += v25 ^ (v27 * 15u); v7 ^= v28 ^ (v38 * 17u); v8 += v31 ^ (v9 * 19u); v9 ^= v34 ^ (v20 * 3u);
        v10 += v37 ^ (v31 * 5u); v11 ^= v0 ^ (v2 * 7u); v12 += v3 ^ (v13 * 9u); v13 ^= v6 ^ (v24 * 11u); v14 += v9 ^ (v35 * 13u); v15 ^= v12 ^ (v6 * 15u); v16 += v15 ^ (v17 * 17u); v17 ^= v18 ^ (v28 * 19u); v18 += v21 ^ (v39 * 3u); v19 ^= v24 ^ (v10 * 5u);
        v20 += v27 ^ (v21 * 7u); v21 ^= v30 ^ (v32 * 9u); v22 += v33 ^ (v3 * 11u); v23 ^= v36 ^ (v14 * 13u); v24 += v39 ^ (v25 * 15u); v25 ^= v2 ^ (v36 * 17u); v26 += v5 ^ (v7 * 19u); v27 ^= v8 ^ (v18 * 3u); v28 += v11 ^ (v29 * 5u); v29 ^= v14 ^ (v0 * 7u);
        v30 += v17 ^ (v11 * 9u); v31 ^= v20 ^ (v22 * 11u); v32 += v23 ^ (v33 * 13u); v33 ^= v26 ^ (v4 * 15u); v34 += v29 ^ (v15 * 17u); v35 ^= v32 ^ (v26 * 19u); v36 += v35 ^ (v37 * 3u); v37 ^= v38 ^ (v8 * 5u); v38 += v1 ^ (v19 * 7u); v39 ^= v4 ^ (v30 * 9u);
        v0 ^= v8 ^ (v6 * 3u); v1 += v11 ^ (v17 * 5u); v2 ^= v14 ^ (v28 * 7u); v3 += v17 ^ (v39 * 9u); v4 ^= v20 ^ (v10 * 11u); v5 += v23 ^ (v21 * 13u); v6 ^= v26 ^ (v32 * 15u); v7 += v29 ^ (v3 * 17u); v8 ^= v32 ^ (v14 * 19u); v9 += v35 ^ (v25 * 3u);
        v10 ^= v38 ^ (v36 * 5u); v11 += v1 ^ (v7 * 7u); v12 ^= v4 ^ (v18 * 9u); v13 += v7 ^ (v29 * 11u); v14 ^= v10 ^ (v0 * 13u); v15 += v13 ^ (v11 * 15u); v16 ^= v16 ^ (v22 * 17u); v17 += v19 ^ (v33 * 19u); v18 ^= v22 ^ (v4 * 3u); v19 += v25 ^ (v15 * 5u);
        v20 ^= v28 ^ (v26 * 7u); v21 += v31 ^ (v37 * 9u); v22 ^= v34 ^ (v8 * 11u); v23 += v37 ^ (v19 * 13u); v24 ^= v0 ^ (v30 * 15u); v25 += v3 ^ (v1 * 17u); v26 ^= v6 ^ (v12 * 19u); v27 += v9 ^ (v23 * 3u); v28 ^= v12 ^ (v34 * 5u); v29 += v15 ^ (v5 * 7u);
        v30 ^= v18 ^ (v16 * 9u); v31 += v21 ^ (v27 * 11u); v32 ^= v24 ^ (v38 * 13u); v33 += v27 ^ (v9 * 15u); v34 ^= v30 ^ (v20 * 17u); v35 += v33 ^ (v31 * 19u); v36 ^= v36 ^ (v2 * 3u); v37 += v39 ^ (v13 * 5u); v38 ^= v2 ^ (v24 * 7u); v39 += v5 ^ (v35 * 9u);
        v0 += v9 ^ (v11 * 3u); v1 ^= v12 ^ (v22 * 5u); v2 += v15 ^ (v33 * 7u); v3 ^= v18 ^ (v4 * 9u); v4 += v21 ^ (v15 * 11u); v5 ^= v24 ^ (v26 * 13u); v6 += v27 ^ (v37 * 15u); v7 ^= v30 ^ (v8 * 17u); v8 += v33 ^ (v19 * 19u); v9 ^= v36 ^ (v30 * 3u);
        v10 += v39 ^ (v1 * 5u); v11 ^= v2 ^ (v12 * 7u); v12 += v5 ^ (v23 * 9u); v13 ^= v8 ^ (v34 * 11u); v14 += v11 ^ (v5 * 13u); v15 ^= v14 ^ (v16 * 15u); v16 += v17 ^ (v27 * 17u); v17 ^= v20 ^ (v38 * 19u); v18 += v23 ^ (v9 * 3u); v19 ^= v26 ^ (v20 * 5u);
        v20 += v29 ^ (v31 * 7u); v21 ^= v32 ^ (v2 * 9u); v22 += v35 ^ (v13 * 11u); v23 ^= v38 ^ (v24 * 13u); v24 += v1 ^ (v35 * 15u); v25 ^= v4 ^ (v6 * 17u); v26 += v7 ^ (v17 * 19u); v27 ^= v10 ^ (v28 * 3u); v28 += v13 ^ (v39 * 5u); v29 ^= v16 ^ (v10 * 7u);
        v30 += v19 ^ (v21 * 9u); v31 ^= v22 ^ (v32 * 11u); v32 += v25 ^ (v3 * 13u); v33 ^= v28 ^ (v14 * 15u); v34 += v31 ^ (v25 * 17u); v35 ^= v34 ^ (v36 * 19u); v36 += v37 ^ (v7 * 3u); v37 ^= v0 ^ (v18 * 5u); v38 += v3 ^ (v29 * 7u); v39 ^= v6 ^ (v0 * 9u);
        v0 ^= v10 ^ (v16 * 3u); v1 += v13 ^ (v27 * 5u); v2 ^= v16 ^ (v38 * 7u); v3 += v19 ^ (v9 * 9u); v4 ^= v22 ^ (v20 * 11u); v5 += v25 ^ (v31 * 13u); v6 ^= v28 ^ (v2 * 15u); v7 += v31 ^ (v13 * 17u); v8 ^= v34 ^ (v24 * 19u); v9 += v37 ^ (v35 * 3u);
        v10 ^= v0 ^ (v6 * 5u); v11 += v3 ^ (v17 * 7u); v12 ^= v6 ^ (v28 * 9u); v13 += v9 ^ (v39 * 11u); v14 ^= v12 ^ (v10 * 13u); v15 += v15 ^ (v21 * 15u); v16 ^= v18 ^ (v32 * 17u); v17 += v21 ^ (v3 * 19u); v18 ^= v24 ^ (v14 * 3u); v19 += v27 ^ (v25 * 5u);
        v20 ^= v30 ^ (v36 * 7u); v21 += v33 ^ (v7 * 9u); v22 ^= v36 ^ (v18 * 11u); v23 += v39 ^ (v29 * 13u); v24 ^= v2 ^ (v0 * 15u); v25 += v5 ^ (v11 * 17u); v26 ^= v8 ^ (v22 * 19u); v27 += v11 ^ (v33 * 3u); v28 ^= v14 ^ (v4 * 5u); v29 += v17 ^ (v15 * 7u);
        v30 ^= v20 ^ (v26 * 9u); v31 += v23 ^ (v37 * 11u); v32 ^= v26 ^ (v8 * 13u); v33 += v29 ^ (v19 * 15u); v34 ^= v32 ^ (v30 * 17u); v35 += v35 ^ (v1 * 19u); v36 ^= v38 ^ (v12 * 3u); v37 += v1 ^ (v23 * 5u); v38 ^= v4 ^ (v34 * 7u); v39 += v7 ^ (v5 * 9u);
        s[0]=v0; s[1]=v1; s[2]=v2; s[3]=v3; s[4]=v4; s[5]=v5; s[6]=v6; s[7]=v7; s[8]=v8; s[9]=v9; s[10]=v10; s[11]=v11; s[12]=v12; s[13]=v13; s[14]=v14; s[15]=v15; s[16]=v16; s[17]=v17; s[18]=v18; s[19]=v19; s[20]=v20; s[21]=v21; s[22]=v22; s[23]=v23; s[24]=v24; s[25]=v25; s[26]=v26; s[27]=v27; s[28]=v28; s[29]=v29; s[30]=v30; s[31]=v31; s[32]=v32; s[33]=v33; s[34]=v34; s[35]=v35; s[36]=v36; s[37]=v37; s[38]=v38; s[39]=v39;
        out += (unsigned long)(v0 ^ v19) + (unsigned long)(v20 ^ v39);
    }
    for(i=0;i<N;i++) out += s[i];
    printf("%lu\n", out);
    return 0;
}
