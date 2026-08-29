/* k1_dispatch — a JUMP-TABLE DISPATCH LOOP, small enough to read.
 *
 * WHY IT IS HERE.  sqlite3VdbeExec is one for(;;) over a bytecode array around
 * a switch with 196 arms, and it is the function every query runs; zcc emits
 * 10,791 instructions there to gcc's 6,041.  What this program keeps from that
 * is the DISPATCH — 40 arms, past the 24-case jump-table threshold
 * (MEASURED M4), tiny bodies, a bytecode array walked by a pointer — and
 * nothing else.
 *
 * SIZE IS A REQUIREMENT, NOT AN ACCIDENT.  The first version of this program
 * was generated with the real function's 196 arms and ran to 3,641 lines, which
 * no one can read instruction by instruction — and the method that produced
 * every parity win in this project is exactly that: hand-edit one instruction
 * in the .s, link it, time it, and only then build the mechanism.  A benchmark
 * you cannot diff by hand is a number without a lever.  So the run time comes
 * from ITERATION COUNT (8,000,000 dispatches) and the code stays at ~1,800
 * instructions.  Measured: the 196-arm version scored 1.561 on instructions and
 * this one scores 1.258, so almost none of that ratio was ever about arm count.
 *
 * What is deliberately NOT here is instruction-cache pressure — a 3,641-line
 * dispatch loop feels it and this does not.  That effect belongs to
 * realprog.sh, which exists to measure exactly what a kernel suite cannot.
 * Register pressure is k2_live_pressure's job, for the same reason: one program,
 * one property.
 */
typedef struct { unsigned char op; int p1, p2, p3; unsigned char p5; } Op;
#define NMEM 256
static unsigned long aMem[NMEM];
static Op prog[8000002];

unsigned long helper1(unsigned long, unsigned long, unsigned long);
unsigned long helper2(unsigned long, unsigned long, unsigned long, unsigned long, unsigned long);

static unsigned long run(Op *aOp, unsigned long *mem){
  unsigned long s0 = 1, s1 = 2, s2 = 3, s3 = 4, s4 = 5, s5 = 6, s6 = 7, s7 = 8;
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
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 2: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 3: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 4: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 5: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 65);
        a1 = a1 * 4 + (*pIn1 ^ 66);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 6: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 7: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 8: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 9: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
        break; }
      case 10: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 130);
        a1 = a1 * 4 + (*pIn1 ^ 131);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 1;
        break; }
      case 11: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 12: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 13: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 14: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 15: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 195);
        a1 = a1 * 4 + (*pIn1 ^ 196);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 16: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 17: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 18: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 19: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
        break; }
      case 20: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 260);
        a1 = a1 * 4 + (*pIn1 ^ 261);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 1;
        break; }
      case 21: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 22: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 23: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 24: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; unsigned long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 25: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 325);
        a1 = a1 * 4 + (*pIn1 ^ 326);
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 26: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 27: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 28: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 29: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
        break; }
      case 30: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 390);
        a1 = a1 * 4 + (*pIn1 ^ 391);
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 1;
        break; }
      case 31: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 32: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 33: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 34: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 35: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 455);
        a1 = a1 * 4 + (*pIn1 ^ 456);
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 36: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (unsigned long)pOp->p1 * 2 + (unsigned long)pOp->p5;
        a1 += (unsigned long)pOp->p2 * 3 + (unsigned long)pOp->p5;
        a0 += (unsigned long)(pOp[1].p1) - (unsigned long)(pOp[1].p5);
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 37: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (unsigned long)bp[1] ^ (unsigned long)(unsigned short)(a0 >> 3);
          a1 += (unsigned long)bp[4] ^ (unsigned long)(unsigned short)(a1 >> 4);
        }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 38: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (unsigned long)(d0 + d1); a1 ^= (unsigned long)(d1 * 0.25); }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 39: {
        unsigned long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (unsigned long)enc + iCompare + (unsigned long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
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
  return rc;
}

unsigned long helper1(unsigned long a, unsigned long b, unsigned long c){ return (a ^ b) + c * 3; }
unsigned long helper2(unsigned long a, unsigned long b, unsigned long c, unsigned long d, unsigned long e){ return a + b - c + d * e; }
int printf(const char*, ...);
int main(void){
  int i;
  for(i = 0; i < NMEM; i++) aMem[i] = i * 2654435761u % 1009;
  for(i = 0; i < 8000000; i++){
    prog[i].op = (unsigned char)(1 + ((unsigned)i * 7919u) % 39u);
    prog[i].p1 = (i * 31) & 255; prog[i].p2 = (i * 17) & 255;
    prog[i].p3 = (i * 13) & 255; prog[i].p5 = (unsigned char)(i & 7);
  }
  prog[8000000].op = 0;
  printf("%lu\n", run(prog, aMem));
  return 0;
}
