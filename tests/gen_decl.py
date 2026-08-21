#!/usr/bin/env python3
# =============================================================================
# LAYER-3 HARNESS — DECLARATOR ALGEBRA: the finite part of the parser grammar
# =============================================================================
# The C grammar is infinite in general — not exhaustible. But the DECLARATOR TREE
# up to depth k is finite: a declarator = a tree over 4 constructors
#   Ptr(t)  = pointer to t           Arr2(t)/Arr3(t) = array [2]/[3] of t
#   PFn(t)  = pointer to a FUNCTION(int) returning t
# with the C89 constraint (3.5.4): a function returns neither an array nor a
# function — the generator filters out invalid trees. This is the hardest corner
# of C syntax (the "spiral rule") — where a parser most easily attaches * [] ()
# wrongly, and exactly the concern that a nonconforming function pointer segfaults
# real software.
#
# For each valid tree (depth ≤ 3, 4 bases × ~156 trees/base):
#   - render the declaration with the standard inside-out algorithm (grammar-
#     correct),
#   - observable: sizeof(x), and sizeof through ONE deconstruction step
#     (x[0] if the outermost is an array, *x if a pointer) — forcing both parsers
#     to agree on both the structure and the element type.
#   - a PFn is additionally ASSIGNED a real function and CALLED through the pointer
#     (the blr ABI path) when the return type is int — runtime behavior, not just
#     sizeof.
# Oracle: differential cc -std=c89. There is no UB in this space.
# =============================================================================
import sys
from itertools import product

BASES = ["char", "int", "double", "char *"]
OPS = ["P", "A2", "A3", "F"]

def valid(chain):
    # chain applies from OUTSIDE INWARD: chain[0] is the outermost layer of the type.
    # Constraint: F (pointer-to-function) may not return an array/function directly →
    # the element AFTER F may not be A*/F? No — a PFn returns its element type; the
    # element type is the rest of the chain. A function returns neither array nor
    # function:
    for i, op in enumerate(chain):
        if op == "F" and i + 1 < len(chain) and chain[i + 1] in ("A2", "A3", "F"):
            return False
    return True

def render2(chain, base, name):
    """chain[0] = the OUTERMOST constructor ("x is a ___") → binds MOST tightly to
    the name → applied FIRST. [] () bind tighter than *, so applying [n] to a chain
    beginning with '*' requires parentheses (this is where the spiral rule arises)."""
    d = name
    for op in chain:
        if op == "P":
            d = "*" + d
        elif op == "F":
            d = "(*" + d + ")(int)"   # ptr-to-function: '*' must sit inside the parens with the name
        else:
            if d.startswith("*"):
                d = "(" + d + ")"
            d += "[2]" if op == "A2" else "[3]"
    return base + " " + d

def main(outdir):
    lines, k = [], 0
    fnimpls = ["static int fn_int_helper(int q) { return q + 7; }"]
    for depth in (1, 2, 3):
        for chain in product(OPS, repeat=depth):
            if not valid(list(chain)):
                continue
            for base in BASES:
                nm = "d%d" % k
                decl = render2(list(chain), base, nm)
                obs = ['(int)sizeof(%s)' % nm]
                if chain[0] in ("A2", "A3"):
                    obs.append('(int)sizeof(%s[0])' % nm)
                elif chain[0] == "P":
                    obs.append('(int)sizeof(*%s)' % nm)
                fmt = " ".join(["%d"] * len(obs))
                lines.append('    { static %s; printf("d%d %s\\n", %s); }'
                             % (decl, k, fmt, ", ".join(obs)))
                # outermost PFn returning int directly: assign a real function + CALL via blr
                if chain == ("F",) and base == "int":
                    lines.append('    { int (*fp)(int) = fn_int_helper;'
                                 ' printf("call%d %%d\\n", fp(35)); }' % k)
                k += 1
    # the hardest classic combinations, hand-written and hard-locked (spiral rule showcase)
    hard = [
        ("int (*ha[2])(int)", "ha", "array of 2 function pointers"),
        ("int (*(*hb)(int))(int)", "hb", "ptr to function returning a function ptr"),
        ("char *(*(*hc[3])(int))[2]", "hc", "array of ptr-to-function returning ptr-to-array-of-2-char*"),
        ("int (*(*hd)[3])(int)", "hd", "ptr to array of 3 function pointers"),
    ]
    for decl, nm, _ in hard:
        lines.append('    { static %s; printf("h_%s %%d\\n", (int)sizeof(%s)); }'
                     % (decl, nm, nm))
    with open(outdir + "/decl_cases.c", "w") as fp:
        fp.write("#include <stdio.h>\n%s\nint main(void) {\n%s\n    return 0;\n}\n"
                 % ("\n".join(fnimpls), "\n".join(lines)))
    print("decl=%d trees" % k)

if __name__ == "__main__":
    main(sys.argv[1])
