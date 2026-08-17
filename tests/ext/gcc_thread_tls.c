/* EXT(gcc): __thread TLS thật — mỗi thread một bản riêng (Mach-O @TLVP).
   4 thread cùng băm biến TLS init + zero + static; nếu TLS là no-op (share)
   thì kết quả sai/race; đúng thì mỗi thread thấy bản cục bộ của mình.
   Worker KHÔNG printf (thứ tự thread không định trước) — trả kết quả qua
   pthread_join rồi main in tuần tự. */
#include <pthread.h>
#include <stdio.h>

__thread int tl_init = 1000; /* __thread_data */
__thread long tl_zero;       /* .tbss */
static __thread int tl_stat = 7;

extern __thread int tl_init; /* re-decl extern không phá definition */

static void *worker(void *arg) {
    long id = (long)arg;
    int i;
    for (i = 0; i < 100000; i++) {
        tl_init += (int)id; /* mỗi thread cộng id riêng vào BẢN riêng */
        tl_zero += 1;
        tl_stat += (int)id;
    }
    return (void *)(tl_init + tl_zero + (long)tl_stat);
}

int main(void) {
    pthread_t th[4];
    long i;
    for (i = 1; i <= 4; i++)
        pthread_create(&th[i - 1], 0, worker, (void *)i);
    for (i = 1; i <= 4; i++) {
        void *r;
        pthread_join(th[i - 1], &r);
        /* kỳ vọng: 1000+100000*id + 100000 + 7+100000*id */
        printf("j%ld %ld\n", i, (long)r);
    }
    /* bản của main không bị 4 worker đụng */
    printf("main %d %ld %d\n", tl_init, tl_zero, tl_stat);
    return 0;
}
