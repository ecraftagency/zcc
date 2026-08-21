/* GNU stmt-expr lồng, value = struct trả về (SRet) — regress seal-Block. */
struct A { unsigned int a, b, c; };
extern void abort(void); extern void exit(int);
struct A bar(void){ return (struct A){176,52,31}; }
int main(void){
  struct A d;
  d = ({ ({ bar(); }); });
  if (d.a!=176 || d.b!=52 || d.c!=31) abort();
  int x = ({ int t=5; t*2; });
  if (x != 10) abort();
  exit(0);
}
