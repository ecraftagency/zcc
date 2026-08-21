/* EXT(gcc): real __thread TLS — each thread has its own copy (Mach-O @TLVP).
   4 threads concurrently hash the init + zero + static TLS variables; if TLS
   were a no-op (shared) the result would be wrong/racy; when correct, each
   thread sees its own local copy. The worker does NOT printf (thread ordering
   is unspecified) — it returns the result via pthread_join and main prints
   sequentially. */
#include <pthread.h>
#include <stdio.h>

__thread int tl_init = 1000; /* __thread_data */
__thread long tl_zero;       /* .tbss */
static __thread int tl_stat = 7;

extern __thread int tl_init; /* re-declaring extern does not break the definition */

static void *worker(void *arg) {
    long id = (long)arg;
    int i;
    for (i = 0; i < 100000; i++) {
        tl_init += (int)id; /* each thread adds its own id to its OWN copy */
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
        /* expected: 1000+100000*id + 100000 + 7+100000*id */
        printf("j%ld %ld\n", i, (long)r);
    }
    /* main's own copy is untouched by the 4 workers */
    printf("main %d %ld %d\n", tl_init, tl_zero, tl_stat);
    return 0;
}
