#!/usr/bin/env python3
# =============================================================================
# LAYER-1 HARNESS — LEXER: regular languages + a finite classification table
# =============================================================================
# The three finite spaces of the lexer, all exhaustible:
#
# 1. INTEGER-LITERAL CLASSIFICATION (C89 3.1.3.2): a literal's type is a function
#    of (base × suffix × value) — a suffixless dec climbs int→long→ulong, oct/hex
#    climb int→uint→long→ulong (different!), U forces unsigned, L forces ≥long.
#    Space = 3 bases × 4 suffixes × the boundary values (INT_MAX±1, UINT_MAX±1,
#    LONG_MAX, ULONG_MAX) — finite, fully enumerated.
#    Observable: (bit value, sizeof, signedness) — 3 probes suffice to separate
#    every type:
#      %lu of (unsigned long)LIT    → value
#      sizeof(LIT)                  → 4 vs 8 (int-family vs long-family, LP64)
#      (LIT) - (LIT) - 1 < 0        → 1 if the type is signed (result -1), 0 if
#                                     unsigned (wraps to max) — a UB-free probe
#                                     (subtract then subtract 1 does not overflow)
#
# 2. ESCAPE SEQUENCE (3.1.3.4): a finite escape table (simple + 1-3-digit octal +
#    hex) × 2 contexts (char-const, string). Observable: the %d value (char signed
#    on Darwin) and the string sizeof.
#
# 3. MAXIMAL MUNCH (3.1): the finite ambiguity points of the C89 token table —
#    `a+++b` = (a++)+b, `a---b` = (a--)-b, `x+++++y` is an ERROR (excluded), `..`
#    vs `...`, `>>=` vs `> >=`... testing the valid tokenizations.
#
# UB/exclusions: a literal beyond ULONG_MAX (3.1.3.2: error), '\x' beyond the char
# range (impl-def — KEPT because it is platform-locked), multi-char 'ab' (impl-def
# — kept, locked).
# =============================================================================
import sys

L = []
def em(fmt, *a):
    L.append(fmt % a if a else fmt)

def int_literals():
    IMAX = 2**31 - 1; UIMAX = 2**32 - 1; LMAX = 2**63 - 1; ULMAX = 2**64 - 1
    vals = [0, 1, IMAX, IMAX + 1, UIMAX, UIMAX + 1, LMAX, ULMAX]
    for v in vals:
        for base, spell in (("dec", str(v)), ("oct", "0%o" % v), ("hex", "0x%X" % v)):
            for sfx in ("", "U", "L", "UL"):
                lit = spell + sfx
                em('    printf("L %s %%lu %%d %%d\\n", (unsigned long)(%s), '
                   '(int)sizeof(%s), ((%s) - (%s) - 1 < 0) ? 1 : 0);',
                   lit, lit, lit, lit, lit)

def escapes():
    simple = ["\\n", "\\t", "\\v", "\\b", "\\r", "\\f", "\\a", "\\\\", "\\'", "\\\"", "\\?"]
    octs = ["\\0", "\\7", "\\17", "\\177", "\\377"]
    hexs = ["\\x0", "\\x7f", "\\xff", "\\x41"]
    for e in simple + octs + hexs:
        label = e.replace("\\", "\\\\").replace('"', '\\"')
        em("    printf(\"E [%s] %%d\\n\", '%s');", label, e)
    # escape in a string: sizeof counts the correct byte count after escape translation
    em('    printf("ES %%d %%d\\n", (int)sizeof("\\x41\\102z"), (int)sizeof("a\\0b"));')
    # multi-char const: impl-defined, locked to the same-platform referee
    em("    printf(\"MC %%d\\n\", ('ab' == 'ab') ? 1 : 0);")

def munch():
    em('    { int a = 1, b = 2; printf("M1 %%d %%d\\n", a+++b, a); }')      # (a++)+b
    em('    { int a = 5, b = 2; printf("M2 %%d %%d\\n", a---b, a); }')      # (a--)-b
    em('    { int a = 4; int b = a >> 1 >= 2 ? 1 : 0; printf("M3 %%d\\n", b); }')
    em('    { int x = 1; x <<= 2; x >>= 1; printf("M4 %%d\\n", x); }')
    em('    { double d = 1.5; printf("M5 %%a\\n", d); }')
    em('    { double e = 1e3, f = .5, g = 5., h = 1E-3; printf("M6 %%a %%a %%a %%a\\n", e, f, g, h); }')
    em('    { float fl = 0.5f; printf("M7 %%a\\n", (double)fl); }')

def main(outdir):
    int_literals(); escapes(); munch()
    with open(outdir + "/lex_cases.c", "w") as fp:
        fp.write("#include <stdio.h>\nint main(void) {\n%s\n    return 0;\n}\n" % "\n".join(L))
    print("lex=%d lines" % len(L))

if __name__ == "__main__":
    main(sys.argv[1])
