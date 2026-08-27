/* l1_arg_marshal — CALL-ARGUMENT MARSHALLING, because that is where the
 * measured gap actually is.
 *
 * WHY IT IS HERE.  `excess.sh` on sqlite: of 18,333 excess instructions,
 * register-register `mov` is +10,464 — 57% of the whole gap — and 22,829 of
 * zcc's 31,352 movs write x0-x7.  Per call that is 1.78 against gcc's 1.23.
 * The cause is not spilling and not a missing peephole: it is that a value
 * computed for an argument does not end up IN the argument register, so a copy
 * is emitted at the call.  Nothing in the taxonomy suite was call-argument
 * dense, so the largest single class in the real gap had no program.
 *
 * THE SHAPE: many calls, each with enough arguments to reach and pass the
 * AAPCS64 register file — 8 GPR arguments (x0-x7) and beyond into the stack,
 * mixed integer and floating-point (the two files are marshalled separately,
 * §6.4), a small composite by value (two registers), and arguments that are
 * COMPUTED rather than already sitting somewhere, which is what makes the
 * register targeting decidable in the first place.
 *
 * The callees are deliberately opaque to inlining (separate, non-static, and
 * each used more than once) so that the call and its marshalling survive to
 * the backend: a suite program that gets inlined away measures nothing, which
 * is the trap the invariant repeat loops in this suite fell into.
 */
#include <stdio.h>

struct Pair { long a, b; };

long eight(long a, long b, long c, long d, long e, long f, long g, long h) {
    return a + (b << 1) - c + (d ^ e) + f - (g & h);
}

long eleven(long a, long b, long c, long d, long e, long f, long g, long h,
            long i, long j, long k) {
    return a - b + c - d + e - f + g - h + i - j + k;
}

double mixed(long a, double x, long b, double y, long c, double z, long d) {
    return (double)(a + b + c + d) + x * 2.0 - y + z * 0.5;
}

long composite(struct Pair p, long n, struct Pair q) {
    return p.a * 3 + p.b - q.a + q.b * 2 + n;
}

long chain(long v, int depth) {
    /* each level marshals a fresh set of arguments computed from the last */
    if (depth == 0) return v;
    return chain(eight(v, v + 1, v + 2, v + 3, v + 4, v + 5, v + 6, v + 7) & 0xffff,
                 depth - 1);
}

int main(void) {
    long s = 0;
    double d = 0;
    long k;
    for (k = 0; k < 300000; k++) {
        long v = k ^ (s & 63);
        s += eight(v, v + 1, v + 2, v + 3, v + 4, v + 5, v + 6, v + 7);
        s += eleven(v, v + 1, v + 2, v + 3, v + 4, v + 5, v + 6, v + 7, v + 8,
                    v + 9, v + 10);
        d += mixed(v, (double)(v & 7), v + 1, (double)(v & 15), v + 2,
                   (double)(v & 31), v + 3);
        {
            struct Pair p, q;
            p.a = v;
            p.b = v ^ 5;
            q.a = v + 1;
            q.b = v & 3;
            s += composite(p, v, q);
        }
        s += chain(v, 4);
    }
    printf("%ld %.0f\n", s, d);
    return 0;
}
