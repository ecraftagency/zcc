/* EXT(gcc): họ __sync_* (M12) — kiểm ngữ nghĩa giá trị trả về, đơn luồng.
   fetch_and_* trả giá trị CŨ, *_and_fetch trả MỚI; bool_cas trả 1/0;
   val_cas + lock_test_and_set trả cũ; lock_release ghi 0. */
#include <stdio.h>
int main(void)
{
    long l = 10;
    int i = 5;
    unsigned u = 4000000000u;
    long arr[2];
    long *p = 0;

    printf("%ld %ld\n", __sync_fetch_and_add(&l, 3), l);            /* 10 13 */
    printf("%ld %ld\n", __sync_add_and_fetch(&l, 3), l);            /* 16 16 */
    printf("%d %d\n", __sync_fetch_and_sub(&i, 2), i);              /* 5 3 */
    printf("%d %d\n", __sync_sub_and_fetch(&i, 2), i);              /* 1 1 */
    printf("%u\n", __sync_add_and_fetch(&u, 1));                    /* wrap không xảy ra: 4000000001 */
    printf("%d %ld\n", __sync_bool_compare_and_swap(&l, 16, 99), l); /* 1 99 */
    printf("%d %ld\n", __sync_bool_compare_and_swap(&l, 16, 7), l);  /* 0 99 */
    printf("%ld %ld\n", __sync_val_compare_and_swap(&l, 99, 1), l);  /* 99 1 */
    printf("%ld %ld\n", __sync_val_compare_and_swap(&l, 99, 2), l);  /* 1 1 */
    printf("%ld %ld\n", __sync_lock_test_and_set(&l, 55), l);        /* 1 55 */
    __sync_lock_release(&l);
    printf("%ld\n", l);                                              /* 0 */
    __sync_synchronize();

    /* con trỏ cũng atomic được (nginx ngx_atomic trên field con trỏ) */
    __sync_bool_compare_and_swap(&p, (long *)0, arr);
    printf("%d\n", p == arr);                                        /* 1 */

    /* int âm: kiểm canonical sign-extend của kết quả 32-bit */
    i = -5;
    printf("%d\n", __sync_fetch_and_add(&i, -3));                    /* -5 */
    printf("%d %d\n", i, i < 0);                                     /* -8 1 */
    return 0;
}
