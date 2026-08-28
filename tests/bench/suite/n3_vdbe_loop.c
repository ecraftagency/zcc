/* n3_vdbe_loop — a REGISTER BYTECODE INTERPRETER, the sqlite `VdbeExec` shape.
 *
 * WHY IT IS HERE.  The per-function localizer says `sqlite3VdbeExec` alone
 * carries 42.6% of zcc's gap against gcc -O1 on sqlite, and nothing in this
 * suite has its shape: one enormous switch inside one loop, a register file
 * addressed by operand indices, a cursor whose state stays live across every
 * arm, and a working set far larger than the register file — so the allocator's
 * residency decisions, not the arithmetic, decide the time.  `k1_dispatch` is a
 * toy of the dispatch alone; this adds the live set and the memory traffic.
 *
 * The program interpreted below is the compiled form of, roughly,
 *
 *     SELECT k, sum(v) FROM t WHERE v > 0 AND (k & 7) != 3 GROUP BY k & 15;
 *
 * with the grouping done in registers, which is what a small aggregate looks
 * like once sqlite has planned it.
 */
#include <stdio.h>

#define NROW  400000
#define NREG  24
#define NGRP  16

enum {
    OP_Init, OP_Rewind, OP_Column, OP_Integer, OP_Copy, OP_Add, OP_Subtract,
    OP_Multiply, OP_Remainder, OP_BitAnd, OP_ShiftRight, OP_Le, OP_Lt, OP_Ge,
    OP_Ne, OP_Eq, OP_Goto, OP_IfPos, OP_IfNot, OP_AggStep, OP_AggFinal,
    OP_ResultRow, OP_Next, OP_Halt
};

/* One register, deliberately wider than a machine word so the interpreter has
 * to keep a struct alive rather than a scalar (sqlite's `Mem` is 40+ bytes). */
struct Mem {
    long i;
    double r;
    long n;
    unsigned flags;
};

struct Op {
    unsigned char op;
    short p1, p2, p3;
    long p4;
};

static long tab[NROW][3];
static struct Mem agg[NGRP];

static void fill(void) {
    unsigned long s = 0x2545f4914f6cdd1dUL;
    long i;
    for (i = 0; i < NROW; i++) {
        s = s * 6364136223846793005UL + 1442695040888963407UL;
        tab[i][0] = (long)((s >> 33) & 0xffff);      /* k  */
        tab[i][1] = (long)((s >> 17) & 0x3ff) - 300; /* v  */
        tab[i][2] = (long)((s >> 3) & 0xff);         /* w  */
    }
}

static long run(const struct Op *prog) {
    struct Mem reg[NREG];
    long pc = 0, row = -1, out = 0;
    long i;

    for (i = 0; i < NREG; i++) {
        reg[i].i = 0;
        reg[i].r = 0.0;
        reg[i].n = 0;
        reg[i].flags = 0;
    }
    for (i = 0; i < NGRP; i++) {
        agg[i].i = 0;
        agg[i].r = 0.0;
        agg[i].n = 0;
        agg[i].flags = 0;
    }

    for (;;) {
        const struct Op *o = &prog[pc];
        pc++;
        switch (o->op) {
        case OP_Init:
            row = -1;
            break;
        case OP_Rewind:
            row = 0;
            if (row >= NROW) pc = o->p2;
            break;
        case OP_Column:
            reg[o->p3].i = tab[row][o->p2];
            reg[o->p3].flags = 4;
            break;
        case OP_Integer:
            reg[o->p2].i = o->p4;
            reg[o->p2].flags = 4;
            break;
        case OP_Copy:
            reg[o->p2] = reg[o->p1];
            break;
        case OP_Add:
            reg[o->p3].i = reg[o->p1].i + reg[o->p2].i;
            reg[o->p3].flags = 4;
            break;
        case OP_Subtract:
            reg[o->p3].i = reg[o->p2].i - reg[o->p1].i;
            reg[o->p3].flags = 4;
            break;
        case OP_Multiply:
            reg[o->p3].i = reg[o->p1].i * reg[o->p2].i;
            reg[o->p3].flags = 4;
            break;
        case OP_Remainder:
            reg[o->p3].i = reg[o->p1].i == 0 ? 0 : reg[o->p2].i % reg[o->p1].i;
            reg[o->p3].flags = 4;
            break;
        case OP_BitAnd:
            reg[o->p3].i = reg[o->p1].i & reg[o->p2].i;
            reg[o->p3].flags = 4;
            break;
        case OP_ShiftRight:
            reg[o->p3].i = reg[o->p2].i >> (reg[o->p1].i & 63);
            reg[o->p3].flags = 4;
            break;
        case OP_Le:
            if (reg[o->p1].i <= reg[o->p3].i) pc = o->p2;
            break;
        case OP_Lt:
            if (reg[o->p1].i < reg[o->p3].i) pc = o->p2;
            break;
        case OP_Ge:
            if (reg[o->p1].i >= reg[o->p3].i) pc = o->p2;
            break;
        case OP_Ne:
            if (reg[o->p1].i != reg[o->p3].i) pc = o->p2;
            break;
        case OP_Eq:
            if (reg[o->p1].i == reg[o->p3].i) pc = o->p2;
            break;
        case OP_Goto:
            pc = o->p2;
            break;
        case OP_IfPos:
            if (reg[o->p1].i > 0) {
                reg[o->p1].i -= o->p4;
                pc = o->p2;
            }
            break;
        case OP_IfNot:
            if (reg[o->p1].i == 0) pc = o->p2;
            break;
        case OP_AggStep: {
            long g = reg[o->p1].i & (NGRP - 1);
            agg[g].i += reg[o->p2].i;
            agg[g].n++;
            agg[g].flags |= 4;
            break;
        }
        case OP_AggFinal: {
            long g;
            for (g = 0; g < NGRP; g++) {
                reg[o->p2].i = agg[g].n ? agg[g].i / agg[g].n : 0;
                out += reg[o->p2].i * (g + 1);
            }
            break;
        }
        case OP_ResultRow:
            out += reg[o->p1].i ^ (reg[o->p1].n << 1);
            break;
        case OP_Next:
            row++;
            if (row < NROW) pc = o->p2;
            break;
        case OP_Halt:
            return out;
        default:
            return -1;
        }
    }
}

int main(void) {
    /* the compiled query — indices are register numbers, p2 of a jump is a pc */
    static const struct Op prog[] = {
        /*  0 */ { OP_Init,       0, 0, 0, 0 },
        /*  1 */ { OP_Integer,    0, 1, 0, 0 },      /* r1 = 0 (zero)       */
        /*  2 */ { OP_Integer,    0, 2, 0, 3 },      /* r2 = 3              */
        /*  3 */ { OP_Integer,    0, 3, 0, 7 },      /* r3 = 7 (mask)       */
        /*  4 */ { OP_Rewind,     0, 20, 0, 0 },
        /*  5 */ { OP_Column,     0, 0, 4, 0 },      /* r4 = k              */
        /*  6 */ { OP_Column,     0, 1, 5, 0 },      /* r5 = v              */
        /*  7 */ { OP_Column,     0, 2, 6, 0 },      /* r6 = w              */
        /*  8 */ { OP_Le,         5, 18, 1, 0 },     /* if v <= 0 -> next   */
        /*  9 */ { OP_BitAnd,     3, 4, 7, 0 },      /* r7 = k & 7          */
        /* 10 */ { OP_Eq,         7, 18, 2, 0 },     /* if r7 == 3 -> next  */
        /* 11 */ { OP_Multiply,   5, 6, 8, 0 },      /* r8 = v * w          */
        /* 12 */ { OP_Integer,    0, 9, 0, 101 },
        /* 13 */ { OP_Remainder,  9, 8, 10, 0 },     /* r10 = r8 % 101      */
        /* 14 */ { OP_Add,        10, 5, 11, 0 },    /* r11 = r10 + v       */
        /* 15 */ { OP_ShiftRight, 2, 11, 12, 0 },    /* r12 = r11 >> 3      */
        /* 16 */ { OP_AggStep,    4, 12, 0, 0 },     /* agg[k & 15] += r12  */
        /* 17 */ { OP_ResultRow,  12, 0, 0, 0 },
        /* 18 */ { OP_Next,       0, 5, 0, 0 },
        /* 19 */ { OP_AggFinal,   0, 13, 0, 0 },
        /* 20 */ { OP_Halt,       0, 0, 0, 0 }
    };
    long v;
    fill();
    v = run(prog);
    printf("%ld\n", v);
    return 0;
}
