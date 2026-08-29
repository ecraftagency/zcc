/* k2_live_pressure — MANY VALUES LIVE ACROSS A DISPATCH, in 627 instructions.
 *
 * WHY IT IS HERE.  In sqlite3VdbeExec zcc uses 199 distinct frame slots where
 * gcc uses 43 — a 4.6x spilling gap in the hottest function in the program.
 * The cause is not the function's size, it is how many values are live ACROSS
 * the switch while each arm wants registers of its own.  That property does not
 * need 8,000 lines to express: this holds 40 VM-state values live across every
 * arm of an 8-arm loop and reproduces the shape in 228 lines and 627
 * instructions, with 19 zcc frame slots against gcc's 6.
 *
 * Small on purpose (see k1_dispatch): the whole function fits in a screen of
 * assembly, so a defect found here can be hand-edited in the .s and timed
 * before any compiler change is written.  Long enough to trust because the loop
 * runs 8,000,000 times, not because the code is big.
 */
typedef struct { unsigned char op; int p1, p2, p3; unsigned char p5; } Op;
#define NMEM 256
static unsigned long aMem[NMEM];
static Op prog[8000002];

unsigned long helper1(unsigned long, unsigned long, unsigned long);
unsigned long helper2(unsigned long, unsigned long, unsigned long, unsigned long, unsigned long);

static unsigned long run(Op *aOp, unsigned long *mem){
  unsigned long s0 = 1, s1 = 2, s2 = 3, s3 = 4, s4 = 5, s5 = 6, s6 = 7, s7 = 8, s8 = 9, s9 = 10, s10 = 11, s11 = 12, s12 = 13, s13 = 14, s14 = 15, s15 = 16, s16 = 17, s17 = 18, s18 = 19, s19 = 20, s20 = 21, s21 = 22, s22 = 23, s23 = 24, s24 = 25, s25 = 26, s26 = 27, s27 = 28, s28 = 29, s29 = 30, s30 = 31, s31 = 32, s32 = 33, s33 = 34, s34 = 35, s35 = 36, s36 = 37, s37 = 38, s38 = 39, s39 = 40;
  Op *pOp; unsigned long rc = 0; unsigned enc = 1; int iCompare = 0;
  unsigned long nVmStep = 0; unsigned colCache = 0; unsigned long *pIn1, *pIn2, *pOut;
  for(pOp = aOp; ; pOp++){
    nVmStep++;
    pIn1 = &mem[pOp->p1 & (NMEM-1)];
    pIn2 = &mem[pOp->p2 & (NMEM-1)];
    pOut = &mem[pOp->p3 & (NMEM-1)];
    switch( pOp->op ){
      case 0: goto done;
      case 1: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8, a2 = pOp->p3 + 15, a3 = pOp->p1 + 22, a4 = pOp->p2 + 29, a5 = pOp->p3 + 36, a6 = pOp->p1 + 43, a7 = pOp->p2 + 50, a8 = pOp->p3 + 57, a9 = pOp->p1 + 64, a10 = pOp->p2 + 71, a11 = pOp->p3 + 78;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a2 += (unsigned long)pOp->p3 * 4 + (unsigned long)pOp->p5;
        a3 += (unsigned long)pOp->p1 * 5 + (unsigned long)pOp->p5;
        a4 += (unsigned long)pOp->p2 * 6 + (unsigned long)pOp->p5;
        a5 += (unsigned long)pOp->p3 * 7 + (unsigned long)pOp->p5;
        a6 += (unsigned long)pOp->p1 * 8 + (unsigned long)pOp->p5;
        a7 += (unsigned long)pOp->p2 * 9 + (unsigned long)pOp->p5;
        a8 += (unsigned long)pOp->p3 * 10 + (unsigned long)pOp->p5;
        a9 += (unsigned long)pOp->p1 * 11 + (unsigned long)pOp->p5;
        a10 += (unsigned long)pOp->p2 * 12 + (unsigned long)pOp->p5;
        a11 += (unsigned long)pOp->p3 * 13 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s1 = (s1 ^ a1) + a2;
        *pOut = a0 + s1 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a3 & 7); colCache += 2;
        break; }
      case 2: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8, a2 = pOp->p3 + 15, a3 = pOp->p1 + 22, a4 = pOp->p2 + 29, a5 = pOp->p3 + 36, a6 = pOp->p1 + 43, a7 = pOp->p2 + 50, a8 = pOp->p3 + 57, a9 = pOp->p1 + 64, a10 = pOp->p2 + 71, a11 = pOp->p3 + 78;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
          a2 += (unsigned long)bp[7] ^ (unsigned long)(unsigned short)(a2 >> 5);
          a3 += (unsigned long)bp[10] ^ (unsigned long)(unsigned short)(a3 >> 6);
          a4 += (unsigned long)bp[13] ^ (unsigned long)(unsigned short)(a4 >> 7);
          a5 += (unsigned long)bp[16] ^ (unsigned long)(unsigned short)(a5 >> 8);
        }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a2;
          a0 += t; }
        s2 = (s2 ^ a2) + a3;
        *pOut = a0 + s2 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a4 & 7); colCache += 3;
        break; }
      case 3: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8, a2 = pOp->p3 + 15, a3 = pOp->p1 + 22, a4 = pOp->p2 + 29, a5 = pOp->p3 + 36, a6 = pOp->p1 + 43, a7 = pOp->p2 + 50, a8 = pOp->p3 + 57, a9 = pOp->p1 + 64, a10 = pOp->p2 + 71, a11 = pOp->p3 + 78;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        a2 = a2 + (*pIn2 >> 3);
        a3 = a3 + (*pIn2 >> 4);
        a4 = a4 + (*pIn2 >> 5);
        a5 = a5 + (*pIn2 >> 6);
        a6 = a6 + (*pIn2 >> 7);
        a7 = a7 + (*pIn2 >> 1);
        a8 = a8 + (*pIn2 >> 2);
        a9 = a9 + (*pIn2 >> 3);
        a10 = a10 + (*pIn2 >> 4);
        a11 = a11 + (*pIn2 >> 5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a3;
          a0 += t; }
        s3 = (s3 ^ a3) + a4;
        *pOut = a0 + s3 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a5 & 7); colCache += 4;
        break; }
      case 4: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8, a2 = pOp->p3 + 15, a3 = pOp->p1 + 22, a4 = pOp->p2 + 29, a5 = pOp->p3 + 36, a6 = pOp->p1 + 43, a7 = pOp->p2 + 50, a8 = pOp->p3 + 57, a9 = pOp->p1 + 64, a10 = pOp->p2 + 71, a11 = pOp->p3 + 78;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        a2 = (a2 > *pIn1) ? (a2 - 7) : (a2 + 12);
        a3 = (a3 > *pIn1) ? (a3 - 10) : (a3 + 17);
        a4 = (a4 > *pIn1) ? (a4 - 13) : (a4 + 22);
        a5 = (a5 > *pIn1) ? (a5 - 16) : (a5 + 27);
        a6 = (a6 > *pIn1) ? (a6 - 19) : (a6 + 32);
        a7 = (a7 > *pIn1) ? (a7 - 22) : (a7 + 37);
        a8 = (a8 > *pIn1) ? (a8 - 25) : (a8 + 42);
        a9 = (a9 > *pIn1) ? (a9 - 28) : (a9 + 47);
        a10 = (a10 > *pIn1) ? (a10 - 31) : (a10 + 52);
        a11 = (a11 > *pIn1) ? (a11 - 34) : (a11 + 57);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a4;
          a0 += t; }
        s4 = (s4 ^ a4) + a5;
        *pOut = a0 + s4 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a6 & 7); colCache += 5;
        break; }
      case 5: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8, a2 = pOp->p3 + 15, a3 = pOp->p1 + 22, a4 = pOp->p2 + 29, a5 = pOp->p3 + 36, a6 = pOp->p1 + 43, a7 = pOp->p2 + 50, a8 = pOp->p3 + 57, a9 = pOp->p1 + 64, a10 = pOp->p2 + 71, a11 = pOp->p3 + 78;
        a0 = a0 * 3 + (*pIn1 ^ 65);
        a1 = a1 * 4 + (*pIn1 ^ 66);
        a2 = a2 * 5 + (*pIn1 ^ 67);
        a3 = a3 * 6 + (*pIn1 ^ 68);
        a4 = a4 * 7 + (*pIn1 ^ 69);
        a5 = a5 * 3 + (*pIn1 ^ 70);
        a6 = a6 * 4 + (*pIn1 ^ 71);
        a7 = a7 * 5 + (*pIn1 ^ 72);
        a8 = a8 * 6 + (*pIn1 ^ 73);
        a9 = a9 * 7 + (*pIn1 ^ 74);
        a10 = a10 * 3 + (*pIn1 ^ 75);
        a11 = a11 * 4 + (*pIn1 ^ 76);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a5;
          a0 += t; }
        s5 = (s5 ^ a5) + a6;
        *pOut = a0 + s5 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a7 & 7); colCache += 1;
        break; }
      case 6: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8, a2 = pOp->p3 + 15, a3 = pOp->p1 + 22, a4 = pOp->p2 + 29, a5 = pOp->p3 + 36, a6 = pOp->p1 + 43, a7 = pOp->p2 + 50, a8 = pOp->p3 + 57, a9 = pOp->p1 + 64, a10 = pOp->p2 + 71, a11 = pOp->p3 + 78;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a2 += (unsigned long)pOp->p3 * 4 + (unsigned long)pOp->p5;
        a3 += (unsigned long)pOp->p1 * 5 + (unsigned long)pOp->p5;
        a4 += (unsigned long)pOp->p2 * 6 + (unsigned long)pOp->p5;
        a5 += (unsigned long)pOp->p3 * 7 + (unsigned long)pOp->p5;
        a6 += (unsigned long)pOp->p1 * 8 + (unsigned long)pOp->p5;
        a7 += (unsigned long)pOp->p2 * 9 + (unsigned long)pOp->p5;
        a8 += (unsigned long)pOp->p3 * 10 + (unsigned long)pOp->p5;
        a9 += (unsigned long)pOp->p1 * 11 + (unsigned long)pOp->p5;
        a10 += (unsigned long)pOp->p2 * 12 + (unsigned long)pOp->p5;
        a11 += (unsigned long)pOp->p3 * 13 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a6;
          a0 += t; }
        s6 = (s6 ^ a6) + a7;
        *pOut = a0 + s6 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a8 & 7); colCache += 2;
        break; }
      case 7: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8, a2 = pOp->p3 + 15, a3 = pOp->p1 + 22, a4 = pOp->p2 + 29, a5 = pOp->p3 + 36, a6 = pOp->p1 + 43, a7 = pOp->p2 + 50, a8 = pOp->p3 + 57, a9 = pOp->p1 + 64, a10 = pOp->p2 + 71, a11 = pOp->p3 + 78;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
          a2 += (unsigned long)bp[7] ^ (unsigned long)(unsigned short)(a2 >> 5);
          a3 += (unsigned long)bp[10] ^ (unsigned long)(unsigned short)(a3 >> 6);
          a4 += (unsigned long)bp[13] ^ (unsigned long)(unsigned short)(a4 >> 7);
          a5 += (unsigned long)bp[16] ^ (unsigned long)(unsigned short)(a5 >> 8);
        }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a7;
          a0 += t; }
        s7 = (s7 ^ a7) + a8;
        *pOut = a0 + s7 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a9 & 7); colCache += 3;
        break; }
    }
  }
done:
  rc = (unsigned long)nVmStep + iCompare + (unsigned long)colCache + (unsigned long)enc;
  rc += s0;
  rc += s1;
  rc += s2;
  rc += s3;
  rc += s4;
  rc += s5;
  rc += s6;
  rc += s7;
  rc += s8;
  rc += s9;
  rc += s10;
  rc += s11;
  rc += s12;
  rc += s13;
  rc += s14;
  rc += s15;
  rc += s16;
  rc += s17;
  rc += s18;
  rc += s19;
  rc += s20;
  rc += s21;
  rc += s22;
  rc += s23;
  rc += s24;
  rc += s25;
  rc += s26;
  rc += s27;
  rc += s28;
  rc += s29;
  rc += s30;
  rc += s31;
  rc += s32;
  rc += s33;
  rc += s34;
  rc += s35;
  rc += s36;
  rc += s37;
  rc += s38;
  rc += s39;
  return rc;
}

unsigned long helper1(unsigned long a, unsigned long b, unsigned long c){ return (a ^ b) + c * 3; }
unsigned long helper2(unsigned long a, unsigned long b, unsigned long c, unsigned long d, unsigned long e){ return a + b - c + d * e; }
int printf(const char*, ...);
int main(void){
  int i;
  for(i = 0; i < NMEM; i++) aMem[i] = i * 2654435761u % 1009;
  for(i = 0; i < 8000000; i++){
    prog[i].op = (unsigned char)(1 + ((unsigned)i * 7919u) % 7u);
    prog[i].p1 = (i * 31) & 255; prog[i].p2 = (i * 17) & 255;
    prog[i].p3 = (i * 13) & 255; prog[i].p5 = (unsigned char)(i & 7);
  }
  prog[8000000].op = 0;
  printf("%lu\n", run(prog, aMem));
  return 0;
}
