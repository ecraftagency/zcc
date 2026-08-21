#include <stdio.h>
int main(void){
  int a=5, b=7, out;
  __asm__("add %w0, %w1, %w2" : "=r"(out) : "r"(a), "r"(b));   /* out=12 */
  int x=10;
  __asm__("lsl %w0, %w0, #1" : "+r"(x));                        /* rw: x=20 */
  long mem=42, got;
  __asm__("ldr %0, %1" : "=r"(got) : "m"(mem));                 /* mem operand */
  double d=3.0, dr;
  __asm__("fadd %d0, %d1, %d1" : "=w"(dr) : "w"(d));            /* fp: dr=6.0 */
  float f=2.5f, fr;
  __asm__("fadd %s0, %s1, %s1" : "=w"(fr) : "w"(f));            /* fp float: fr=5.0 */
  printf("%d %d %ld %.1f %.1f\n", out, x, got, dr, fr);
  return 0;
}
