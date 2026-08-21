#!/usr/bin/env python3
# =============================================================================
# LAYER-5 HARNESS (data part) — STRUCT/UNION/BITFIELD LAYOUT
# =============================================================================
# Layout is a finite RECURSIVE FUNCTION: (member type sequence) → (per-member
# offset, size, align) — determined by the Darwin LP64 alignment rules (locked in
# TyTab). A wrong layout when a struct interoperates with libc/SDK = a silent
# heisenbug (overwriting a neighboring field, corrupting data with no error). The
# space: member sequences of length ≤3 over 9 base types (including a nested struct
# + an array — the two hard padding sources) = 9+81+729 = 819 structs + 90 unions +
# the bitfield family.
#
# Observable: sizeof(S) + the offset of EACH member via (char*)&s.m - (char*)&s
# (C89-kosher offsetof, no macro so zcc/cc parse the same path).
# Bitfield: C89 leaves it impl-defined (bit order, straddle) — LOCKED to the
# same-platform referee (like every other impl-defined item in the project);
# additionally observe the per-field write/read behavior (pattern 0x5A5A...), not
# just the shape.
# Oracle: differential cc -std=c89. There is no UB in this space.
# =============================================================================
import sys
from itertools import product

BASIS = ["char", "short", "int", "long", "float", "double", "char *",
         "struct { char c; long l; }", "char [3]"]

def mdecl(t, nm):
    if t == "char [3]":
        return "char %s[3];" % nm
    return "%s %s;" % (t, nm)

def main(outdir):
    lines, k = [], 0
    for depth in (1, 2, 3):
        for seq in product(range(len(BASIS)), repeat=depth):
            mems = " ".join(mdecl(BASIS[i], "m%d" % j) for j, i in enumerate(seq))
            probes = ", ".join(
                "(int)((char *)&s.m%d - (char *)&s)" % j for j in range(depth))
            fmt = " ".join(["%d"] * (depth + 1))
            lines.append(
                '    { static struct { %s } s; printf("s%d %s\\n", (int)sizeof(s), %s); }'
                % (mems, k, fmt, probes))
            k += 1
    # union: type pairs (size = max, align = max) — 81 pairs suffice to cover the max rule
    for i, j in product(range(len(BASIS)), repeat=2):
        mems = mdecl(BASIS[i], "a") + " " + mdecl(BASIS[j], "b")
        lines.append('    { static union { %s } u; printf("u%d %%d\\n", (int)sizeof(u)); }'
                     % (mems, k))
        k += 1
    # bitfield: width sequences over int (impl-defined but platform-locked) +
    # interleaved plain members + unnamed + width 0 (forces a new unit, 3.5.2.1)
    bf = [
        ("int a:1;",                          [("a", 0)]),
        ("int a:31;",                         [("a", 12345)]),
        ("int a:1; int b:1;",                 [("a", 0), ("b", -1)]),
        ("int a:5; int b:27;",                [("a", 9), ("b", 1000)]),
        ("int a:5; int b:28;",                [("a", 9), ("b", 1000)]),   # straddles a unit
        ("int a:3; char c; int b:3;",         [("a", 3), ("c", 7), ("b", 2)]),  # interleaved plain member
        ("int a:3; int :0; int b:3;",         [("a", 3), ("b", 2)]),      # :0 forces a new unit
        ("int a:3; int :5; int b:3;",         [("a", 3), ("b", 2)]),      # unnamed padding
        ("unsigned int a:4; unsigned int b:4;", [("a", 15), ("b", 9)]),
        ("int a:16; short s2; int b:7;",      [("a", 900), ("s2", 55), ("b", 60)]),
    ]
    for spec, nv in bf:
        sets = " ".join("s.%s = %d;" % (n, v) for n, v in nv)
        gets = ", ".join("(int)s.%s" % n for n, _ in nv)
        fmt = " ".join(["%d"] * (len(nv) + 1))
        lines.append('    { static struct { %s } s; %s printf("b%d %s\\n", '
                     '(int)sizeof(s), %s); }' % (spec, sets, k, fmt, gets))
        k += 1
    with open(outdir + "/layout_cases.c", "w") as fp:
        fp.write("#include <stdio.h>\nint main(void) {\n%s\n    return 0;\n}\n"
                 % "\n".join(lines))
    print("layout=%d shapes" % k)

if __name__ == "__main__":
    main(sys.argv[1])
