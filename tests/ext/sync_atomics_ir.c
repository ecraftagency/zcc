#include <stdio.h>
int main(void){
  int i=10; unsigned u=100; long l=1000;
  int fa = __sync_fetch_and_add(&i, 5);     /* old 10 → i=15 */
  int af = __sync_add_and_fetch(&i, 5);     /* i=20, ret 20 */
  int fs = __sync_fetch_and_sub(&i, 3);     /* old 20 → i=17 */
  unsigned fo = __sync_fetch_and_or(&u, 0x0F);
  unsigned fx = __sync_fetch_and_xor(&u, 0xFF);
  unsigned fn = __sync_fetch_and_and(&u, 0x3C);
  int vc = __sync_val_compare_and_swap(&i, 17, 99);   /* old 17 → i=99 */
  int bc = __sync_bool_compare_and_swap(&i, 99, 7);   /* true → i=7 */
  int bc2 = __sync_bool_compare_and_swap(&i, 99, 8);  /* false, i stays 7 */
  long ts = __sync_lock_test_and_set(&l, 555);        /* old 1000 → l=555 */
  __sync_lock_release(&l);                            /* l=0 */
  __sync_synchronize();
  printf("%d %d %d %u %u %u %d %d %d %ld %ld\n", fa,af,fs,fo,fx,fn,vc,bc,bc2,ts,l);
  return 0;
}
