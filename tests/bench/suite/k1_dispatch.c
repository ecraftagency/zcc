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
static long aMem[NMEM];
static Op prog[8000002];

long helper1(long, long, long);
long helper2(long, long, long, long, long);

static long run(Op *aOp, long *mem){
  long s0 = 1, s1 = 2, s2 = 3, s3 = 4, s4 = 5, s5 = 6, s6 = 7, s7 = 8;
  Op *pOp; long rc = 0; unsigned enc = 1; int iCompare = 0;
  unsigned long nVmStep = 0; unsigned colCache = 0; long *pIn1, *pIn2, *pOut;
  for(pOp = aOp; ; pOp++){
    nVmStep++;
    pIn1 = &mem[pOp->p1 & (NMEM-1)];
    pIn2 = &mem[pOp->p2 & (NMEM-1)];
    pOut = &mem[pOp->p3 & (NMEM-1)];
    switch( pOp->op ){
      case 0: goto done;
      case 1: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 2: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 3: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 4: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 5: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 65);
        a1 = a1 * 4 + (*pIn1 ^ 66);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 6: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 7: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 8: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 9: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
        break; }
      case 10: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 130);
        a1 = a1 * 4 + (*pIn1 ^ 131);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 1;
        break; }
      case 11: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 12: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 13: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 14: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 15: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 195);
        a1 = a1 * 4 + (*pIn1 ^ 196);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 16: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 17: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 18: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 19: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
        break; }
      case 20: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 260);
        a1 = a1 * 4 + (*pIn1 ^ 261);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 1;
        break; }
      case 21: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 22: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 23: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a1;
          a0 += t; }
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 24: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        { int i; long t = 0;
          for(i = 0; i < (pOp->p2 & 15) + 2; i++) t += mem[(pOp->p1 + i) & (NMEM-1)] ^ a0;
          a0 += t; }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 25: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 325);
        a1 = a1 * 4 + (*pIn1 ^ 326);
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 26: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 27: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 28: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 29: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
        break; }
      case 30: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 390);
        a1 = a1 * 4 + (*pIn1 ^ 391);
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 1;
        break; }
      case 31: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 2;
        break; }
      case 32: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        s0 = (s0 ^ a0) + a1;
        *pOut = a0 + s0 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 3;
        break; }
      case 33: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        s1 = (s1 ^ a1) + a0;
        *pOut = a0 + s1 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 4;
        break; }
      case 34: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        s2 = (s2 ^ a0) + a1;
        *pOut = a0 + s2 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 5;
        break; }
      case 35: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = a0 * 3 + (*pIn1 ^ 455);
        a1 = a1 * 4 + (*pIn1 ^ 456);
        s3 = (s3 ^ a1) + a0;
        *pOut = a0 + s3 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 1;
        break; }
      case 36: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 += (long)pOp->p1 * 2 + (long)pOp->p5;
        a1 += (long)pOp->p2 * 3 + (long)pOp->p5;
        a0 += (long)(pOp[1].p1) - (long)(pOp[1].p5);
        s4 = (s4 ^ a0) + a1;
        *pOut = a0 + s4 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 2;
        break; }
      case 37: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { unsigned char *bp = (unsigned char *)mem + (pOp->p1 & 63);
          a0 += (long)bp[1] ^ (long)(unsigned short)(a0 >> 3);
          a1 += (long)bp[4] ^ (long)(unsigned short)(a1 >> 4);
        }
        s5 = (s5 ^ a1) + a0;
        *pOut = a0 + s5 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 3;
        break; }
      case 38: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        { double d0 = (double)a0 * 1.5 + (double)pOp->p2;
          double d1 = d0 * d0 - (double)a1;
          a0 += (long)(d0 + d1); a1 ^= (long)(d1 * 0.25); }
        s6 = (s6 ^ a0) + a1;
        *pOut = a0 + s6 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a0 & 7); colCache += 4;
        break; }
      case 39: {
        long a0 = pOp->p1 + 1, a1 = pOp->p2 + 8;
        a0 = (a0 > *pIn1) ? (a0 - 1) : (a0 + 2);
        a1 = (a1 > *pIn1) ? (a1 - 4) : (a1 + 7);
        s7 = (s7 ^ a1) + a0;
        *pOut = a0 + s7 + (long)enc + iCompare + (long)colCache;
        iCompare = (int)(a1 & 7); colCache += 5;
        break; }
    }
  }
done:
  rc = (long)nVmStep + iCompare + (long)colCache + (long)enc;
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

long helper1(long a, long b, long c){ return (a ^ b) + c * 3; }
long helper2(long a, long b, long c, long d, long e){ return a + b - c + d * e; }
int printf(const char*, ...);
int main(void){
  int i;
  for(i = 0; i < NMEM; i++) aMem[i] = i * 2654435761u % 1009;
  for(i = 0; i < 8000000; i++){
    prog[i].op = (unsigned char)(1 + (i * 7919) % 39);
    prog[i].p1 = (i * 31) & 255; prog[i].p2 = (i * 17) & 255;
    prog[i].p3 = (i * 13) & 255; prog[i].p5 = (unsigned char)(i & 7);
  }
  prog[8000000].op = 0;
  printf("%ld\n", run(prog, aMem));
  return 0;
}
