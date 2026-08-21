#!/usr/bin/env python3
# =============================================================================
# LAYER-2 HARNESS — THE PREPROCESSOR AS A TERM-REWRITING SYSTEM
# =============================================================================
#
# ## Theoretical foundation
#
# C89 macro expansion (3.8.3) is a term-rewriting system over token sequences:
#   - Each #define is a rewrite rule.
#   - Expansion + rescan is the process of finding the normal form.
#   - The TERMINATION condition is not luck but a theorem: "blue paint" — the name
#     of the macro being expanded is PAINTED in its own result (3.8.3.4), so each
#     rescan step only shrinks the applicable rule set → the system is terminating.
#
# The TERM SPACE (every writable program) is infinite — not exhaustible. But the
# MECHANISM SPACE is finite: the expansion algorithm has only this many decision
# points, each with 2-3 branches:
#   (1) object-like vs function-like vs an fn-like name not followed by "("
#   (2) argument prescan: the arg is fully expanded BEFORE substitution — EXCEPT
#       when it is the operand of # or ## (3.8.3.1) → 3 fates of an arg
#   (3) # stringize: RAW spelling (unexpanded), whitespace normalized to 1 space,
#       \ and " escaped inside the string literal (3.8.3.2)
#   (4) ## paste: raw spelling on both sides, the resulting token PARTICIPATES in
#       rescan (3.8.3.3) — i.e. it can create a new macro name
#   (5) rescan: the expansion result + the following text combine into the rescan
#       region (3.8.3.4) — an expansion can "reach out" for the (args) that follow
#   (6) blue paint: direct/indirect/through-arg self-recursion must all terminate
# Preprocessor bugs live in the INTERACTION between these points (paste-then-rescan,
# stringize-of-arg-containing-macro, paint-through-arg...) — this harness
# systematically enumerates that interaction matrix, not the programs.
#
# ## Observing the normal form (observable)
#
# Do not diff `-E` output: C89 does NOT specify the text format after preprocessing
# (whitespace/newline are compiler-dependent) — diff -E is an INVALID oracle.
# Instead REIFY the normal form into a runtime value:
#   #define STR_(x) #x            → the RAW spelling of the arg
#   #define XSTR(x) STR_(x)       → the spelling AFTER full expansion
# The spelling produced by # is the ONLY thing the spec specifies byte-for-byte
# (3.8.3.2), so printing these strings then diffing stdout zcc↔cc is a valid oracle.
# (# is itself part of the system under test — acceptable because its spelling rule
# is the most tightly specified part, and any # divergence self-exposes within the
# C test family itself.)
#
# ## Regions EXCLUDED from the oracle (undefined/unspecified in C89 — a diff there
# is meaningless; two different compilers are both "correct"):
#   - an EMPTY arg for an fn-like macro (3.8.3: undefined; only legal in C99)
#   - a `defined` token PRODUCED by expansion inside #if (3.8.1: undefined —
#     zcc DOES support it because pthread.h needs it, but that is EXT behavior
#     locked to clang, tested in tests/ext/, not standard C89)
#   - ## producing an INVALID token (3.8.3.3: undefined)
#   - the evaluation ORDER of # and ## within the same replacement list (3.8.3.2-3:
#     unspecified) — only test cases where every order yields the same result
#   - an invocation "straddling" an expansion boundary such as `#define LP (` then
#     `F LP x)` (a well-known gray area of the spec; e.g. `h 5)` in EXAMPLE 3.8.3.5
#     is trimmed from the excerpt here for this reason)
#   - __DATE__/__TIME__ (nondeterministic), UB arithmetic in #if (filtered as in alg)
#
# ## Test families
#   A prescan-matrix   B paste-matrix      C stringize-spelling
#   D blue-paint       E rescan-with-following-text   F #if arithmetic (exhaustive,
#     reusing gen_alg's ieval — #if computes in long/unsigned long, 3.8.1)
#   G nested conditional structure   H undef/redefine   I include guard + #line
# =============================================================================
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_alg import IINFO, icorners, uac, wrap, fits, ieval, ARITH, BITS, RELS

M = []   # content lines of cpp_mech.c (after the opening #define block)
def case(directives, printfs):
    M.append(directives.rstrip() + "\n" + printfs.rstrip())

def mech():
    # ---- A: prescan matrix — the 3 fates of an argument -----------------
    # arg ∈ {object macro, fn-like call, fn-like name WITHOUT "(", plain ident,
    # self-painted} × usage ∈ {RAW (#x: raw), XSTR (expand)}
    case("""#define A_OBJ obj_val
#define A_FN(a) fn(a)
#define A_SELF A_SELF""",
"""    printf("A1 [%s][%s]\\n", STR_(A_OBJ), XSTR(A_OBJ));       /* raw vs expand */
    printf("A2 [%s][%s]\\n", STR_(A_FN(1)), XSTR(A_FN(1)));
    printf("A3 [%s][%s]\\n", STR_(A_FN), XSTR(A_FN));           /* fn-like without "(" -> kept as-is */
    printf("A4 [%s][%s]\\n", STR_(plain_id), XSTR(plain_id));
    printf("A5 [%s][%s]\\n", STR_(A_SELF), XSTR(A_SELF));       /* blue paint through prescan */""")

    # ---- B: paste matrix — raw spelling + result participates in rescan -
    case("""#define CAT(a,b) a##b
#define XCAT(a,b) CAT(a,b)
#define PA one
#define PB two
#define AB 7
#define SCAT(a,b) STR_(a##b)""",
"""    { int onetwo = 12, PAPB = 99;
      printf("B1 %d %d\\n", CAT(PA,PB), XCAT(PA,PB));   /* raw->PAPB=99, expand-first->onetwo=12 */
      printf("B2 %d\\n", CAT(A,B));                     /* paste creates a MACRO NAME -> rescan -> 7 */
      printf("B3 %d\\n", CAT(12,34));                   /* pp-number paste */
      printf("B4 %d\\n", CAT(0x1,F));                   /* 0x1F = 31 */
      printf("B5 %d\\n", (int)(CAT(199506,L) / 100000L)); /* suffix paste — a cdefs bug was locked here */
      printf("B6 [%s]\\n", SCAT(PA,PB));                /* stringize of a paste: "PAPB" */
      printf("B7 %d\\n", XCAT(CAT(o,ne),CAT(t,wo)) == 12 ? 1 : 0); }""")

    # ---- C: stringize spelling (3.8.3.2 — specified byte-for-byte) ------
    case("",
"""    printf("C1 [%s]\\n", STR_( spaced   +   out ));    /* whitespace -> 1 space, leading/trailing trimmed */
    printf("C2 [%s]\\n", STR_("q\\n"));                 /* escape " and \\ inside a string arg */
    printf("C3 [%s]\\n", STR_(0x1F));                   /* keep original spelling, not 31 */
    printf("C4 [%s]\\n", STR_(a/*cmt*/b));              /* comment = 1 whitespace (phase 3) */
    printf("C5 [%s]\\n", STR_(a \\
b));                                                    /* splice \\-newline inside the arg */
    printf("C6 [%s]\\n", "con" "cat");                  /* phase 6: adjacent string literal */""")

    # ---- D: blue paint — every recursion shape must TERMINATE -----------
    # includes the standard EXAMPLE excerpt 3.8.3.5 (the specified part; `h 5)` and
    # the boundary-straddling invocations are trimmed — see the exclusion list at
    # the top of the file)
    case("""#define REC REC
#define FREC(x) FREC(x)
#define MU1 MU2
#define MU2 MU1
#define G_ID(x) x
#define x 3
#define f(a) f(x * (a))
#undef x
#define x 2
#define g f
#define z z[0]
#define t(a) a""",
"""    printf("D1 [%s]\\n", XSTR(REC));                  /* self-recursive object */
    printf("D2 [%s]\\n", XSTR(FREC(1)));               /* self-recursive fn-like */
    printf("D3 [%s]\\n", XSTR(MU1));                   /* indirect recursion: MU1->MU2->MU1(painted) */
    printf("D4 [%s]\\n", XSTR(G_ID(REC)));             /* paint through arg-substitution */
    printf("D5 [%s]\\n", XSTR(f(y+1)));                /* 3.8.3.5: "f(2 * (y+1))" */
    printf("D6 [%s]\\n", XSTR(f(f(z))));               /* "f(2 * (f(2 * (z[0]))))" */
    printf("D7 [%s]\\n", XSTR(t(t(g)(0) + t)(1)));     /* "f(2 * (0)) + t(1)" */""")

    # ---- E: rescan unified with following text --------------------------
    case("""#define ID(q) q
#define FN2(y) ((y)+1)
#define OBJ2 FN2""",
"""    printf("E1 %d\\n", ID(FN2)(7));                   /* "(" from SOURCE after ID returns FN2 */
    printf("E2 %d\\n", OBJ2(3));                       /* object macro returns an fn-like name */""")

    # ---- G: nested conditional structure (all 8 combos of 2 defined flags) -
    for i, (fa, fb) in enumerate([(0, 0), (0, 1), (1, 0), (1, 1)]):
        d = ""
        if fa: d += "#define GF%dA 1\n" % i
        if fb: d += "#define GF%dB 1\n" % i
        d += ("#if defined(GF%dA)\n#  ifdef GF%dB\n#    define GV%d 3\n#  else\n"
              "#    define GV%d 2\n#  endif\n#elif !defined(GF%dB)\n#  define GV%d 0\n"
              "#else\n#  define GV%d 1\n#endif" % (i, i, i, i, i, i, i))
        case(d, '    printf("G%d %%d\\n", GV%d);' % (i, i))

    # ---- H: undef / redefine --------------------------------------------
    case("""#define HX 1
#define HSAME 5
#define HSAME 5""",
"""    printf("H1 %d\\n", HX);
#undef HX
#define HX 2
    printf("H2 %d %d\\n", HX, HSAME);                  /* redefine after undef + benign redef */""")

    # ---- I: include guard, formally off-form directive, #line -----------
    case("""#include "cpp_aux.h"
#include "cpp_aux.h"
#
   #   define IND 4""",
"""    printf("I1 %d %d\\n", AUX_VAL, IND);              /* guard blocks 2nd time; bare "#" + indent */
#line 7000
    printf("I2 %d\\n", __LINE__);
    printf("I3 %d\\n", __STDC__);
    printf("I4 %d\\n", 'A');                           /* char-const (ASCII platform-locked) */""")

def ppif():
    """Family F: exhaustive #if arithmetic — C89 3.8.1: all arithmetic in #if is
    computed in long/unsigned long (LP64: 64-bit). Reuses gen_alg's ieval + UAC
    rules, narrowed to 2 types. DUAL oracle: (zcc == cc) AND (cc == python) — the
    latter self-validates the generator: if python has the rule wrong, cc prints
    FAIL immediately."""
    def lit(t, v):
        if v == -(1 << 63):
            return "(-9223372036854775807L - 1)"
        return "(%d%s)" % (v, "UL" if t == "unsigned long" else "L")
    out, k = [], 0
    TS = ["long", "unsigned long"]
    for t1 in TS:
        for t2 in TS:
            rt = uac(t1, t2)
            for op in ARITH + BITS + RELS:
                for v1 in icorners(t1):
                    for v2 in icorners(t2):
                        a, b = wrap(rt, v1), wrap(rt, v2)
                        r = ieval(op, rt, a, b)
                        if r is None:
                            continue  # UB (overflow, div by 0, MIN/-1) — excluded
                        expect = str(r) if op in RELS else lit(rt, r)
                        out.append(
                            "#if (%s %s %s) == %s\n    ok++;\n#else\n"
                            "    printf(\"FAIL f%d\\n\"); bad++;\n#endif"
                            % (lit(t1, v1), op, lit(t2, v2), expect, k))
                        k += 1
    for t in TS:            # shift: a negative signed lhs drops << (UB-ish), keeps >> (platform-locked)
        s = IINFO[t][1]
        for v in icorners(t):
            for sh in (0, 1, 63):
                if not (s and v < 0 and sh > 0) and not (s and not fits(t, v << sh)):
                    out.append("#if (%s << %d) == %s\n    ok++;\n#else\n"
                               "    printf(\"FAIL s%d\\n\"); bad++;\n#endif"
                               % (lit(t, v), sh, lit(t, wrap(t, v << sh)), k)); k += 1
                rs = v >> sh if s else (v & ((1 << 64) - 1)) >> sh
                out.append("#if (%s >> %d) == %s\n    ok++;\n#else\n"
                           "    printf(\"FAIL t%d\\n\"); bad++;\n#endif"
                           % (lit(t, v), sh, lit(t, rs), k)); k += 1
    for t in TS:            # unary in #if
        s = IINFO[t][1]
        for v in icorners(t):
            if not (s and v == -(1 << 63)):
                out.append("#if (-(%s)) == %s\n    ok++;\n#else\n"
                           "    printf(\"FAIL n%d\\n\"); bad++;\n#endif"
                           % (lit(t, v), lit(t, wrap(t, -v)), k)); k += 1
            out.append("#if (~(%s)) == %s\n    ok++;\n#else\n"
                       "    printf(\"FAIL c%d\\n\"); bad++;\n#endif"
                       % (lit(t, v), lit(t, wrap(t, ~v)), k)); k += 1
            out.append("#if (!(%s)) == %d\n    ok++;\n#else\n"
                       "    printf(\"FAIL b%d\\n\"); bad++;\n#endif"
                       % (lit(t, v), int(v == 0), k)); k += 1
    # &&/|| short-circuit in #if: a div-by-0 on the right side must NOT be evaluated
    out.append("#if 0 && (1/0)\n    bad++;\n#else\n    ok++;\n#endif")
    out.append("#if 1 || (1/0)\n    ok++;\n#else\n    bad++;\n#endif")
    out.append("#if (1 ? 5 : (1/0)) == 5\n    ok++;\n#else\n"
               "    printf(\"FAIL q0\\n\"); bad++;\n#endif")
    return out

def main(outdir):
    mech()
    with open(outdir + "/cpp_aux.h", "w") as fp:
        fp.write("#ifndef AUX_H\n#define AUX_H\n#define AUX_VAL 42\n#endif\n")
    with open(outdir + "/cpp_mech.c", "w") as fp:
        fp.write("#include <stdio.h>\n#define STR_(x) #x\n#define XSTR(x) STR_(x)\n"
                 "int main(void) {\n%s\n    return 0;\n}\n" % "\n".join(M))
    blocks = ppif()
    fns, calls = [], []
    for fi in range(0, len(blocks), 500):
        nm = "p%d" % (fi // 500)
        fns.append("static void %s(int *po, int *pb) {\n    int ok = 0, bad = 0;\n%s\n"
                   "    *po += ok; *pb += bad;\n}" % (nm, "\n".join(blocks[fi:fi + 500])))
        calls.append("    %s(&ok, &bad);" % nm)
    with open(outdir + "/cpp_if.c", "w") as fp:
        fp.write("#include <stdio.h>\n%s\nint main(void) {\n    int ok = 0, bad = 0;\n%s\n"
                 "    printf(\"%%d ok %%d bad\\n\", ok, bad);\n    return bad != 0;\n}\n"
                 % ("\n".join(fns), "\n".join(calls)))
    print("mech=%d-family if=%d" % (len(M), len(blocks)))

if __name__ == "__main__":
    main(sys.argv[1])
