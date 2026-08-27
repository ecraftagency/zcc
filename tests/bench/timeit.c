/* timeit — wall time of a child process, in MICROSECONDS, best of N.
 *
 * WHY THIS EXISTS. `exectime.sh` timed with `date +%s%N` and then wrote
 * `(t1-t0)/1000000`, truncating a nanosecond reading to whole milliseconds —
 * and, on the strength of that, declared every program under 5 ms
 * "unmeasurable" and skipped it. Fifteen of the taxonomy suite's 35 programs
 * never produced an exec number at all, and six more were reported as `fast`.
 * The resolution was not missing from the machine; it was discarded by the
 * arithmetic, and a shell `fork` for `date` sat between the two readings on top
 * of that.
 *
 * `clock_gettime(CLOCK_MONOTONIC)` is a vDSO read on this target — no syscall,
 * no fork — and the counter behind it runs at 24 MHz here (41.7 ns/tick,
 * measured 0.5% run-to-run over ten million iterations). So the floor is set by
 * how long `fork`+`execve` takes, not by the clock, and this prints that
 * baseline so a reader can see it rather than trust a constant:
 *
 *     timeit 20 /bin/true          -> the floor
 *     timeit 20 ./prog             -> the program, same instrument
 *
 * Output: one line, `min_us <n> med_us <n> runs <n>`. MIN, not mean: the
 * fastest run is the one least polluted by scheduling, and the suite compares
 * two compilers on the same machine in the same session.
 */
#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>

static long long now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (long long)t.tv_sec * 1000000 + t.tv_nsec / 1000;
}

static int cmp(const void *a, const void *b) {
    long long x = *(const long long *)a, y = *(const long long *)b;
    return x < y ? -1 : x > y;
}

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: timeit N prog [args...]\n");
        return 2;
    }
    int n = atoi(argv[1]);
    if (n < 1) n = 1;
    long long *v = malloc((size_t)n * sizeof *v);
    if (!v) return 2;
    for (int i = 0; i < n; i++) {
        long long t0 = now_us();
        pid_t p = fork();
        if (p == 0) {
            freopen("/dev/null", "w", stdout);
            execv(argv[2], &argv[2]);
            _exit(127);
        }
        int st = 0;
        waitpid(p, &st, 0);
        v[i] = now_us() - t0;
        if (WIFEXITED(st) && WEXITSTATUS(st) == 127) {
            fprintf(stderr, "timeit: cannot exec %s\n", argv[2]);
            return 2;
        }
    }
    qsort(v, (size_t)n, sizeof *v, cmp);
    printf("min_us %lld med_us %lld runs %d\n", v[0], v[n / 2], n);
    return 0;
}
