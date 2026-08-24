#include <stdio.h>
int next(void){ static unsigned st=12345; st = st*1103515245u + 12345u; return (int)((st>>16)&32767); }
int main(void){ long s=0; int k; for(k=0;k<8000000;k++) s += next(); printf("%ld\n", s); return 0; }
