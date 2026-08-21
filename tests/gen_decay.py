#!/usr/bin/env python3
# decay gate: the lvalue-conversion theorem C99 6.3.2.1p3 — every expression of
# type T[N] must convert to a T* pointing at the first element in EVERY context
# except {sizeof, unary &, a string literal initializing an array}; corollaries
# 6.5.15 (a ternary with two array operands → a composite pointer) + 6.5.2.2p6
# (default argument promotion decays before becoming an arg). A defect (an
# ASLR-layout-dependent git-merge segv) lived exactly in the
# ternary-string-literal × vararg-stack cell: the type kept char[2] → a 2-byte strh
# store → glibc vsnprintf read a junk pointer.
# Structural exhaustion: SOURCES (ways to produce an array-typed expr) × CONTEXTS
# (consumption sites) × 2 ternary branches. Differential oracle cc over DERIVED
# observables (string content, character, sizeof, comparison) — never printing a
# raw address.
#
# ── PROOF (the valid domain of the differential oracle) ───────────────────
# decay(E)=&A[0], A = the object E designates. Separate 2 components: (a) the object
# A — fixed by the standard; (b) the NUMERIC VALUE of &A[0] — chosen freely by the
# implementation, NOT spec-determined. The oracle is sound over an observable o ⟺ o
# is spec-determined (every conforming impl yields the same o). ⟹ o that is a
# function of A's content/identity is valid; a function of (b) is invalid.
#   Identity lemma: context `eq` = (p1==p2). Both are &A_i[0] (not null/one-past) ⟹
#   by 6.5.9p6, (p1==p2) ⟺ (A1≡A2). So eq is spec-determined ⟺ "A1≡A2" is
#   spec-determined:
#     • CASE 1 A=named object: 6.2.1 binds to exactly 1 object/declaration ⟹ A1≡A2
#       spec-det = TRUE ⟹ eq=1 uniquely → the oracle is VALID.
#     • CASE 2 A=string-literal array: 6.4.5p6 "unspecified whether these arrays
#       are distinct" ⟹ eq ∈ {1,2} are both conforming → NOT spec-det → the oracle
#       is INVALID (zcc plain .rodata=distinct=2; gcc .rodata.str mergeable=1 — both
#       valid). NOT a decay bug.
#   Exhaustion: the identity axis TOTALLY partitions {named, literal-array} (outside
#   6.4.5p6 no C99 construct makes 2 evaluations of the same lvalue yield 2 distinct
#   objects with the same content); c∈{0,1} sweeps both ternary branches. The
#   invalid cell = {(S,eq): S∈CASE2}. id_stable=False marks EXACTLY those S
#   (soundness: each has 1 eval falling in CASE 2; completeness: every named-only S
#   has id_stable=True). Only the eq context is cut — the literal's own
#   va_stk/idx/arith/szof are KEPT (functions of content 6.4.5p5, spec-det) ⟹ the
#   literal's decay is STILL proven; only IDENTITY, which is not part of the decay
#   theorem, is dropped. The gate after the cut = a decision procedure sound +
#   complete over the spec-determined observable lattice.
# ──────────────────────────────────────────────────────────────────────────
import sys

# (name, expression, lvalue?, id_stable?)
#   lvalue?    — an rvalue such as ternary/comma forbids unary & (excludes CTX_ADDR)
#   id_stable? — TRUE ⟺ every eval of E designates a NAMED object (CASE 1 in the
#     PROOF) ⟹ eq spec-determined. FALSE ⟺ some eval designates a string literal
#     (CASE 2, 6.4.5p6 unspecified) ⟹ eq is excluded. See the identity lemma in the
#     header.
SOURCES = [
    ("lit",     '"LIT"',                        True,  False),
    ("loc",     "la",                           True,  True),
    ("glo",     "ga",                           True,  True),
    ("mem",     "s.arr",                        True,  True),
    ("pmem",    "ps->arr",                      True,  True),
    ("row",     "m[1]",                         True,  True),
    ("paren",   "(la)",                         True,  True),
    ("ter_ll",  'c ? "T" : ""',                 False, False),  # the defect cell (both literal operands)
    ("ter_la",  'c ? la : "xy"',                False, False),  # the c==0 branch = literal
    ("ter_row", "c ? m[0] : m[2]",              False, True),
    ("ter_nst", 'c ? (c ? "a" : "bb") : "ccc"', False, False),  # all-literal, nested
    ("comma",   "(0, la)",                      False, True),
]

# template uses {E}; every substitution is wrapped as ({E})
CONTEXTS = [
    # variadic arg still in registers
    ("va_reg",  'printf("%s\\n", ({E}));'),
    # variadic arg OVERFLOWING THE STACK (5 int take x3..x7 after buf/size/fmt) — the defect cell
    ("va_stk",  'snprintf(buf, sizeof buf, "%d%d%d%d%d[%s]", 1, 2, 3, 4, 5, ({E})); puts(buf);'),
    # two adjacent decays on the stack
    ("va_stk2", 'snprintf(buf, sizeof buf, "%d%d%d%d%d[%s|%s]", 1, 2, 3, 4, 5, ({E}), ({E})); puts(buf);'),
    # named arg in register / overflowing stack (6.5.2.2p7 via prototype)
    ("nm_reg",  "puts(pick1(({E})));"),
    ("nm_stk",  "puts(pick9(1, 2, 3, 4, 5, 6, 7, 8, ({E})));"),
    # simple assignment 6.5.16.1 (also covers return semantics)
    ("assign",  "{ char *q = ({E}); puts(q); }"),
    ("idx",     'printf("%d\\n", (int)({E})[0]);'),
    ("arith",   'printf("%d\\n", (int)*(({E}) + 0));'),
    # two decays of the same array → equal pointers
    ("eq",      'printf("%d\\n", ({E}) == ({E}) ? 1 : 2);'),
    # theorem EXCEPTION: sizeof does NOT decay (an lvalue array keeps N; a ternary
    # has already decayed from its operand so = sizeof(char*))
    ("szof",    'printf("%d\\n", (int)sizeof({E}));'),
]
# theorem EXCEPTION: unary & does not decay — &E has type T(*)[N], lvalue only
CTX_ADDR = ("addr", 'printf("%d\\n", (int)sizeof(*&({E})));')

def main(outdir):
    fns = []
    for i, (sn, e, lval, id_stable) in enumerate(SOURCES):
        body = []
        for cn, tpl in CONTEXTS + ([CTX_ADDR] if lval else []):
            if cn == "eq" and not id_stable:
                continue  # 6.4.5p6: the pointer identity of a literal is unspecified
            body.append('    printf("%s.%s ");' % (sn, cn))
            body.append("    " + tpl.replace("{E}", e))
        fns.append(
            "static void src_%02d(int c) {\n"
            '    char la[] = "LOCAL";\n'
            '    static char ga[] = "GLOB";\n'
            '    static char m[3][6] = {"row_0", "row_1", "row_2"};\n'
            '    struct S s = {1, "MEMB"};\n'
            "    struct S *ps = &s;\n"
            "    char buf[64];\n"
            "    (void)c; (void)ga; (void)m; (void)ps;\n"
            "%s\n}" % (i, "\n".join(body)))
    calls = "\n".join(
        '        printf("== %s c=%%d\\n", c);\n        src_%02d(c);' % (sn, i)
        for i, (sn, _, _, _) in enumerate(SOURCES))
    with open(outdir + "/decay_t.c", "w") as fp:
        fp.write(
            "#include <stdio.h>\n\n"
            "struct S { int pad; char arr[5]; };\n"
            "static char *pick1(char *p) { return p; }\n"
            "static char *pick9(long a, long b, long c, long d, long e,\n"
            "                   long f, long g, long h, char *p) {\n"
            "    return (a + b + c + d + e + f + g + h) ? p : p;\n"
            "}\n\n" + "\n\n".join(fns) + "\n\n"
            "int main(void) {\n    int c;\n"
            "    for (c = 0; c < 2; c++) {\n" + calls + "\n    }\n"
            "    return 0;\n}\n")

main(sys.argv[1])
