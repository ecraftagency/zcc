/* n7_nested_subq — a correlated subquery: for every outer row, scan the inner
 * relation and evaluate a predicate reached through a FUNCTION POINTER.
 *
 * WHY IT IS HERE.  `l2_nested_join` samples the nested loop, but its predicate
 * is a comparison the compiler can see through.  A database cannot: the WHERE
 * clause arrives as a tree, and the innermost operation of the innermost loop
 * is an INDIRECT CALL to a small function through a table — sqlite's
 * comparison and collation callbacks, and every user-defined function.  That
 * shape defeats inlining by construction, so what is left to optimize is the
 * call sequence itself: the argument marshalling, the caller-saved traffic
 * around the call, and whether the loop's own values survive it in registers.
 * The suite has no other program where an indirect call sits in the hot loop.
 */
#include <stdio.h>

#define NOUT  2400
#define NIN   2400

struct Row {
    long k;
    long a;
    long b;
};

static struct Row outer[NOUT];
static struct Row inner[NIN];

static long p_eq(const struct Row *o, const struct Row *i) {
    return o->k == i->k;
}
static long p_range(const struct Row *o, const struct Row *i) {
    return i->a >= o->a && i->a <= o->a + 96;
}
static long p_parity(const struct Row *o, const struct Row *i) {
    return ((o->b ^ i->b) & 3) == 0;
}
static long p_sum(const struct Row *o, const struct Row *i) {
    return (o->a + i->b) % 17 == 0;
}

typedef long (*Pred)(const struct Row *, const struct Row *);

static Pred plan[4] = { p_range, p_parity, p_sum, p_eq };

static void fill(void) {
    unsigned long s = 0xda3e39cb94b95bdbUL;
    long i;
    for (i = 0; i < NOUT; i++) {
        s = s * 6364136223846793005UL + 1442695040888963407UL;
        outer[i].k = (long)((s >> 33) % 512);
        outer[i].a = (long)((s >> 19) & 0x3ff);
        outer[i].b = (long)((s >> 7) & 0xff);
    }
    for (i = 0; i < NIN; i++) {
        s = s * 6364136223846793005UL + 1442695040888963407UL;
        inner[i].k = (long)((s >> 33) % 512);
        inner[i].a = (long)((s >> 19) & 0x3ff);
        inner[i].b = (long)((s >> 7) & 0xff);
    }
}

int main(void) {
    long i, j, p, chk = 0, hits = 0;
    fill();
    for (i = 0; i < NOUT; i++) {
        const struct Row *o = &outer[i];
        long acc = 0;
        for (j = 0; j < NIN; j++) {
            const struct Row *n = &inner[j];
            long ok = 1;
            for (p = 0; p < 2; p++) {
                /* the correlated predicate, chosen per outer row the way a
                 * planner chooses an access path — an indirect call the
                 * compiler cannot devirtualize */
                if (!plan[(i + p) & 3](o, n)) {
                    ok = 0;
                    break;
                }
            }
            if (ok) {
                acc += n->a - o->b;
                hits++;
            }
        }
        chk += acc * (i % 13 + 1);
    }
    printf("%ld %ld\n", chk, hits);
    return 0;
}
