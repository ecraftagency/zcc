#include <stdio.h>
struct Small { int a, b; };           /* 8B → 1 GPR */
struct Big { long a, b, c; };         /* 24B → stack/x8 */
struct HFAf { float x, y, z; };        /* HFA float ×3 */
struct HFAd { double p, q; };          /* HFA double ×2 */
struct Mid { long a, b; };            /* 16B → 2 GPR */

int sums(struct Small s){ return s.a + s.b; }
long sumb(struct Big b){ return b.a + b.b + b.c; }
float sumhf(struct HFAf h){ return h.x + h.y + h.z; }
double sumhd(struct HFAd h){ return h.p + h.q; }
long summ(struct Mid m){ return m.a * m.b; }
struct Small mksmall(int a, int b){ struct Small s; s.a=a; s.b=b; return s; }
struct Big mkbig(long v){ struct Big b; b.a=v; b.b=v*2; b.c=v*3; return b; }
struct HFAd mkhd(double v){ struct HFAd h; h.p=v; h.q=v+1; return h; }
/* nhiều arg tràn reg + mix */
long many(int a,int b,int c,int d,int e,int f,int g,int h,int i,int j){
  return a+b+c+d+e+f+g+h+i+j;
}
double manyf(double a,double b,double c,double d,double e,double f,double g,double h,double i){
  return a+b+c+d+e+f+g+h+i;
}
long double ld_add(long double a, long double b){ return a+b; }

int main(void){
  struct Small s = mksmall(3,4);
  struct Big b = mkbig(5);
  struct HFAd hd = mkhd(2.5);
  struct HFAf hf = {1.0f,2.0f,3.0f};
  struct Mid m = {6,7};
  printf("%d %ld %.1f %.1f %ld\n", sums(s), sumb(b), sumhf(hf), sumhd(hd), summ(m));
  printf("%ld %.1f\n", many(1,2,3,4,5,6,7,8,9,10), manyf(1,2,3,4,5,6,7,8,9));
  long double r = ld_add(1.5L, 2.25L);
  printf("%.2Lf\n", r);
  return 0;
}
