/* Heavy register pressure: ~64 simultaneously-live locals forced past a
   barrier so the allocator must spill. Exercises regalloc::spill on the
   host corpus so the compile-speed byte-identical gate covers the spiller. */
long z_spill(long *p, long n) {
  long a0=p[0],a1=p[1],a2=p[2],a3=p[3],a4=p[4],a5=p[5],a6=p[6],a7=p[7];
  long b0=p[8],b1=p[9],b2=p[10],b3=p[11],b4=p[12],b5=p[13],b6=p[14],b7=p[15];
  long c0=p[16],c1=p[17],c2=p[18],c3=p[19],c4=p[20],c5=p[21],c6=p[22],c7=p[23];
  long d0=p[24],d1=p[25],d2=p[26],d3=p[27],d4=p[28],d5=p[29],d6=p[30],d7=p[31];
  long e0=p[32],e1=p[33],e2=p[34],e3=p[35],e4=p[36],e5=p[37],e6=p[38],e7=p[39];
  long f0=p[40],f1=p[41],f2=p[42],f3=p[43],f4=p[44],f5=p[45],f6=p[46],f7=p[47];
  long g0=p[48],g1=p[49],g2=p[50],g3=p[51],g4=p[52],g5=p[53],g6=p[54],g7=p[55];
  long h0=p[56],h1=p[57],h2=p[58],h3=p[59],h4=p[60],h5=p[61],h6=p[62],h7=p[63];
  long acc = 0;
  for (long i = 0; i < n; i++) {
    acc += a0+a1+a2+a3+a4+a5+a6+a7 + b0+b1+b2+b3+b4+b5+b6+b7
         + c0+c1+c2+c3+c4+c5+c6+c7 + d0+d1+d2+d3+d4+d5+d6+d7
         + e0+e1+e2+e3+e4+e5+e6+e7 + f0+f1+f2+f3+f4+f5+f6+f7
         + g0+g1+g2+g3+g4+g5+g6+g7 + h0+h1+h2+h3+h4+h5+h6+h7 + i;
  }
  return acc;
}
