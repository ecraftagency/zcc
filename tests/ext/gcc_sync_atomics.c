/* EXT(gcc): the __sync_* family (M12) — checks return-value semantics,
   single-threaded. fetch_and_* returns the OLD value, *_and_fetch returns the
   NEW one; bool_cas returns 1/0; val_cas + lock_test_and_set return the old
   value; lock_release writes 0. */
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
    printf("%u\n", __sync_add_and_fetch(&u, 1));                    /* no wrap: 4000000001 */
    printf("%d %ld\n", __sync_bool_compare_and_swap(&l, 16, 99), l); /* 1 99 */
    printf("%d %ld\n", __sync_bool_compare_and_swap(&l, 16, 7), l);  /* 0 99 */
    printf("%ld %ld\n", __sync_val_compare_and_swap(&l, 99, 1), l);  /* 99 1 */
    printf("%ld %ld\n", __sync_val_compare_and_swap(&l, 99, 2), l);  /* 1 1 */
    printf("%ld %ld\n", __sync_lock_test_and_set(&l, 55), l);        /* 1 55 */
    __sync_lock_release(&l);
    printf("%ld\n", l);                                              /* 0 */
    __sync_synchronize();

    /* pointers can be atomic too (nginx ngx_atomic on a pointer field) */
    __sync_bool_compare_and_swap(&p, (long *)0, arr);
    printf("%d\n", p == arr);                                        /* 1 */

    /* negative int: checks canonical sign-extension of the 32-bit result */
    i = -5;
    printf("%d\n", __sync_fetch_and_add(&i, -3));                    /* -5 */
    printf("%d %d\n", i, i < 0);                                     /* -8 1 */

    /* bitwise fetch (postgres18 atomics/generic-gcc.h): returns OLD, writes op */
    u = 0xF0F0F0F0u;
    printf("%u %u\n", __sync_fetch_and_and(&u, 0xFF00FF00u), u);     /* old, F000F000 */
    printf("%u %u\n", __sync_fetch_and_or(&u, 0x0F0F0F0Fu), u);      /* F000F000, FF0FFF0F */
    printf("%u %u\n", __sync_fetch_and_xor(&u, 0xFFFFFFFFu), u);     /* FF0FFF0F, ~ */
    l = 0x7000000000000003L;
    printf("%ld %ld\n", __sync_fetch_and_and(&l, 0x7000000000000001L), l);
    printf("%ld %ld\n", __sync_fetch_and_or(&l, 0x0800000000000004L), l);
    printf("%ld %ld\n", __sync_fetch_and_xor(&l, 0x0000000000000005L), l);
    /* negative int: sign-extend the 32-bit bitwise result */
    i = -8;
    printf("%d %d\n", __sync_fetch_and_xor(&i, 7), i);               /* -8, -15 */
    return 0;
}
