/* maxrss — run a command and report its wall time and PEAK resident set.
 *
 * REARCH.md §19 needs both axes of both sides: how long the compiler takes and
 * how much memory it holds, then the same for the program it produced. The box
 * has no `time(1)` and no busybox, and polling /proc/<pid>/status races the
 * child's own peak, so the measurement is taken the only exact way there is:
 * wait4() fills in a struct rusage whose ru_maxrss IS the high-water mark the
 * kernel recorded for that child (getrusage(2), RUSAGE_CHILDREN semantics).
 *
 * The child's stdout and stderr are untouched — they are the differential the
 * clean-input law compares — so the measurement goes to fd 3 when it is open
 * and to stderr otherwise, one line: `<wall_ms> <peak_kb> <exit_code>`.
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

static long long now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (long long)t.tv_sec * 1000 + t.tv_nsec / 1000000;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: maxrss <cmd> [args...]\n");
        return 2;
    }
    long long t0 = now_ms();
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
    long long ms = now_ms() - t0;
    int code = WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
    /* ru_maxrss is in kilobytes on Linux (getrusage(2)). */
    FILE *out = fdopen(3, "w");
    if (!out) out = stderr;
    fprintf(out, "%lld %ld %d\n", ms, (long)ru.ru_maxrss, code);
    fflush(out);
    return code;
}
