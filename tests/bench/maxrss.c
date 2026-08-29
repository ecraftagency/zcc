/* maxrss — run a command and report its wall time and PEAK resident set.
 *
 * The measured surface needs both axes of both sides: how long the compiler takes and
 * how much memory it holds, then the same for the program it produced. The box
 * has no `time(1)` and no busybox, and polling /proc/<pid>/status races the
 * child's own peak, so the measurement is taken the only exact way there is:
 * wait4() fills in a struct rusage whose ru_maxrss IS the high-water mark the
 * kernel recorded for that child (getrusage(2), RUSAGE_CHILDREN semantics).
 *
 * The child's stdout and stderr are untouched — they are the differential the
 * clean-input law compares — so the measurement goes to fd 3 when it is open
 * and to stderr otherwise, one line: `<wall_us> <peak_kb> <exit_code>`.
 *
 * MICROSECONDS, since 2026-08-27, and the reason is worth recording because
 * this was the THIRD instrument in the tree with the same defect. It read
 * `clock_gettime(CLOCK_MONOTONIC)` — nanoseconds — and then wrote
 * `tv_nsec / 1000000`, throwing the resolution away before anyone could use
 * it. `exectime.sh` did the same thing and skipped 15 of 35 programs on the
 * strength of it; here it made every sqlite phase under ~30 ms unreadable, so
 * `p02_second` reported a ratio of 2.000 for a 2 ms run against a 1 ms one and
 * that number went into a geomean. The counter behind CLOCK_MONOTONIC runs at
 * 24 MHz on this target (41.7 ns/tick); nothing about the machine required the
 * truncation.
 *
 * Built by the referee (gcc), not by zcc: an instrument that the compiler under
 * test also compiles cannot report on that compiler independently.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/resource.h>
#include <sys/wait.h>

static long long now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (long long)t.tv_sec * 1000000 + t.tv_nsec / 1000;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: maxrss <cmd> [args...]\n");
        return 2;
    }
    long long t0 = now_us();
    pid_t pid = fork();
    if (pid < 0) {
        perror("fork");
        return 2;
    }
    if (pid == 0) {
        execvp(argv[1], argv + 1);
        perror(argv[1]);
        _exit(127);
    }
    int status = 0;
    struct rusage ru;
    memset(&ru, 0, sizeof ru);
    if (wait4(pid, &status, 0, &ru) < 0) {
        perror("wait4");
        return 2;
    }
    long long us = now_us() - t0;
    int code = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    /* ru_maxrss is in kilobytes on Linux (getrusage(2)). */
    FILE *out = fdopen(3, "w");
    if (!out) out = stderr;
    fprintf(out, "%lld %ld %d\n", us, (long)ru.ru_maxrss, code);
    fflush(out);
    return code;
}
