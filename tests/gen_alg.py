#!/usr/bin/env python3
# Exhaustion of C89 expression algebra over a finite domain: (op × type × type ×
# corner × corner). The usual arithmetic conversions (C89 3.2.1.5) form a finite
# semilattice over the 10 arithmetic types — covering every type pair + boundary
# value covers the whole conversion space. Oracle = differential against cc, so UB
# must be FILTERED here (signed overflow, div by 0, INT_MIN/-1, shift overflow,
# float→int out of range): at a UB point both compilers are "correct" — a diff
# there is meaningless. Impl-defined cases (narrowing, negative %, negative >>) are
# KEPT because the referee is on the same platform (LP64 Darwin locked).
#
# Generates 3 program families (with an IDENTICAL enumeration order for line-wise
# diffing):
#   run_*.c  — operands pass through a VARIABLE (blocks const-fold), prints the
#              full-width result
#   fold_*.c — the same expression but as a literal in enum { E = (int)(...) }
#              (forced onto the compiler's const-eval path)
#   fri_*.c  — a mirror of fold via variables (the runtime codegen path)
# diff(run zcc, run cc) + diff(fold zcc, fold cc) = correct against the referee;
# diff(fold zcc, fri zcc) = the commuting diagram fold∘parse = runtime∘parse INTERNAL.
import sys

ITYPES = [("char", 8, True), ("unsigned char", 8, False),
          ("short", 16, True), ("unsigned short", 16, False),
          ("int", 32, True), ("unsigned int", 32, False),
          ("long", 64, True), ("unsigned long", 64, False)]
# long long ≡ long on LP64 (same representation) — adds no new point to the space
IINFO = {n: (w, s) for n, w, s in ITYPES}
RANK = {"char": 1, "unsigned char": 1, "short": 2, "unsigned short": 2,
        "int": 3, "unsigned int": 3, "long": 4, "unsigned long": 4}
FTYPES = ["float", "double"]
ARITH = ["+", "-", "*", "/", "%"]
BITS = ["&", "|", "^"]
RELS = ["<", "<=", ">", ">=", "==", "!="]

def icorners(t):
    w, s = IINFO[t]
    if s:
        return [0, 1, -1, -(1 << (w - 1)), (1 << (w - 1)) - 1]
    m = (1 << w) - 1
    return [0, 1, m, 1 << (w - 1), m // 3]  # m//3 = 0x5555...

FCORNERS = {"float": [0.0, 1.0, -2.5, 0.5, 325000000.0],   # all exact in float
            "double": [0.0, 1.0, -2.5, 0.001, 325000000.0]}

def promote(t):
    return "int" if RANK[t] < 3 else t  # every char/short value fits int (LP64)

def uac(t1, t2):
    a, b = promote(t1), promote(t2)
    if RANK[b] > RANK[a] or (RANK[b] == RANK[a] and not IINFO[b][1]):
        a, b = b, a
    # a = the higher rank (unsigned wins at equal rank)
    if IINFO[a][1] == IINFO[b][1] or IINFO[a][1] is False:
        return a
    # a signed higher rank, b unsigned lower rank: LP64 long contains all unsigned int
    return a if IINFO[a][0] > IINFO[b][0] else "unsigned " + a

def wrap(t, v):
    w, s = IINFO[t]
    v &= (1 << w) - 1
    return v - (1 << w) if s and v >= (1 << (w - 1)) else v

def fits(t, v):
    w, s = IINFO[t]
    return -(1 << (w - 1)) <= v < (1 << (w - 1)) if s else 0 <= v < (1 << w)

def ieval(op, t, a, b):
    """The result of op in type t; None = UB (filtered out of the oracle)."""
    w, s = IINFO[t]
    if op in ("+", "-", "*"):
        r = a + b if op == "+" else a - b if op == "-" else a * b
        if s:
            return r if fits(t, r) else None  # signed overflow = UB
        return wrap(t, r)
    if op in ("/", "%"):
        if b == 0 or (s and a == -(1 << (w - 1)) and b == -1):
            return None
        q = abs(a) // abs(b)
        if (a < 0) != (b < 0):
            q = -q  # C: division truncates toward 0; % takes the sign of the dividend (platform-locked)
        return q if op == "/" else a - q * b
    if op in BITS:
        m = (1 << w) - 1
        ua, ub = a & m, b & m
        r = ua & ub if op == "&" else ua | ub if op == "|" else ua ^ ub
        return wrap(t, r)
    return int(eval("a %s b" % op))  # relational

def clit(t, v):
    if t in FTYPES:
        sfx = "f" if t == "float" else ""
        return "(%s)(%r%s)" % (t, v, sfx)
    if v == -(1 << 63):
        return "(%s)(-9223372036854775807L - 1)" % t
    return "(%s)(%d%s)" % (t, v, "UL" if v >= (1 << 63) else "L")

def prfmt(t):  # (format, cast) printing the full-width normalized result
    if t in FTYPES:
        return ("%a", "(double)")
    return ("%lu", "(unsigned long)") if not IINFO[t][1] else ("%ld", "(long)")

def enum_cases():
    """Generates valid (t1,v1,t2,v2,op,rt) — fixed order, shared by all program families."""
    out = []
    # int × int: all 14 ops
    for t1, _, _ in ITYPES:
        for t2, _, _ in ITYPES:
            rt = uac(t1, t2)
            for op in ARITH + BITS + RELS:
                for v1 in icorners(t1):
                    for v2 in icorners(t2):
                        a, b = wrap(rt, v1), wrap(rt, v2)  # convert up to rt (value preserved/wrapped)
                        r = ieval(op, rt, a, b)
                        if r is None:
                            continue
                        out.append((t1, v1, t2, v2, op, "int" if op in RELS else rt))
    return out

def float_cases():
    out = []
    types = [n for n, _, _ in ITYPES] + FTYPES
    for t1 in types:
        for t2 in types:
            if t1 not in FTYPES and t2 not in FTYPES:
                continue
            rt = "double" if "double" in (t1, t2) else "float"
            c1 = FCORNERS[t1] if t1 in FTYPES else icorners(t1)
            c2 = FCORNERS[t2] if t2 in FTYPES else icorners(t2)
            for op in ["+", "-", "*", "/"] + RELS:
                for v1 in c1:
                    for v2 in c2:
                        if op == "/" and float(v2) == 0.0:
                            continue  # float div by 0: UB per C89 (does not mandate IEEE)
                        out.append((t1, v1, t2, v2, op, "int" if op in RELS else rt))
    return out

def shift_cases():
    out = []
    for t1, _, _ in ITYPES:
        p = promote(t1)
        w, s = IINFO[p]
        for v in icorners(t1):
            pv = wrap(p, v)
            for sh in (0, 1, w - 1):
                if s and pv < 0 and sh > 0:
                    continue  # << of a negative / >> kept (impl-def arithmetic, platform-locked)
                if not (s and pv < 0):
                    if s and not fits(p, pv << sh):
                        pass  # << signed overflow = UB
                    else:
                        out.append((t1, v, "int", sh, "<<", p))
                out.append((t1, v, "int", sh, ">>", p))
    return out

def cast_cases():
    out = []
    types = [n for n, _, _ in ITYPES] + FTYPES
    for src in types:
        for dst in types:
            cs = FCORNERS[src] if src in FTYPES else icorners(src)
            for v in cs:
                if src in FTYPES and dst not in FTYPES:
                    tv = int(v)  # truncate toward 0
                    if not fits(dst, tv):
                        continue  # float→int out of range = UB
                out.append((src, v, dst))
    return out

def unary_cases():
    """(t, v, op, rt) — unary on int: the result in type promote(t)."""
    out = []
    for t, _, _ in ITYPES:
        p = promote(t)
        for v in icorners(t):
            pv = wrap(p, v)
            for op in ("-", "~", "!"):
                if op == "-" and IINFO[p][1] and pv == -(1 << (IINFO[p][0] - 1)):
                    continue  # -MIN = UB
                out.append((t, v, op, "int" if op == "!" else p))
    return out

def compound_cases():
    """a op= b ≡ a = (T1)((UAC) a op (UAC) b) — a DISTINCT parse/codegen path vs the
    plain binary one (read-modify-write + convert back to T1, narrowing locked LP64)."""
    out = []
    for t1, _, _ in ITYPES:
        for t2, _, _ in ITYPES:
            rt = uac(t1, t2)
            for op in ARITH + BITS:
                for v1 in icorners(t1):
                    for v2 in icorners(t2):
                        if ieval(op, rt, wrap(rt, v1), wrap(rt, v2)) is None:
                            continue
                        out.append((t1, v1, t2, v2, op))
    return out

def incdec_cases():
    """++/-- prefix/postfix: the new value + the expression value must both be correct."""
    out = []
    for t, w, s in ITYPES:
        for v in icorners(t):
            if not (s and v == (1 << (w - 1)) - 1):
                out.append((t, v, "++"))  # avoid signed overflow = UB
            if not (s and v == -(1 << (w - 1))):
                out.append((t, v, "--"))
    return out

def complex_cases():
    """ℂ = a field over ℝ² (C99 6.2.5). Exhausts field ops +,−,× over _Complex
    float/double × a corner grid (values exact in float ⇒ × is exact too, matching cc
    bit-for-bit — EXCLUDES / because cc's Smith __divdc3 deviates by a ULP from the
    straight formula, a deviation documented in cplx_bin). Also covers the conjugate
    ~ and the __real__/__imag__ projections (π₁,π₂). The imaginary constant `Nif`
    matches the lexer's INum."""
    out = []
    corners = [0.0, 1.0, -2.0, 3.0]
    for t in ("float", "double"):
        for are in corners:
            for aim in corners:
                for bre in corners:
                    for bim in corners:
                        for op in ("+", "-", "*"):
                            out.append((t, are, aim, bre, bim, op))
    return out

def main(outdir):
    bins = enum_cases()
    flts = float_cases()
    shs = shift_cases()
    csts = cast_cases()
    uns = unary_cases()
    cps = compound_cases()
    ids = incdec_cases()

    # ---- family run_*.c: every case via variables ----
    blocks = []
    for t1, v1, t2, v2, op, rt in bins + flts + shs:
        fmt, cast = prfmt(rt)
        blocks.append(
            "{ %s a = %s; %s b = %s; %s r = a %s b; printf(\"%s\\n\", %sr); }"
            % (t1, clit(t1, v1), t2, clit(t2, v2), rt, op, fmt, cast))
    for src, v, dst in csts:
        fmt, cast = prfmt(dst)
        blocks.append("{ %s a = %s; %s r = (%s)a; printf(\"%s\\n\", %sr); }"
                      % (src, clit(src, v), dst, dst, fmt, cast))
    for t, v, op, rt in uns:
        fmt, cast = prfmt(rt)
        blocks.append("{ %s a = %s; %s r = %s a; printf(\"%s\\n\", %sr); }"
                      % (t, clit(t, v), rt, op, fmt, cast))
    for t in FTYPES:  # unary float: - and ! (not ~)
        for v in FCORNERS[t]:
            blocks.append("{ %s a = %s; %s r = -a; printf(\"%%a\\n\", (double)r); }"
                          % (t, clit(t, v), t))
            blocks.append("{ %s a = %s; int r = !a; printf(\"%%ld\\n\", (long)r); }"
                          % (t, clit(t, v)))
    for t1, v1, t2, v2, op in cps:
        fmt, cast = prfmt(t1)
        blocks.append("{ %s a = %s; %s b = %s; a %s= b; printf(\"%s\\n\", %sa); }"
                      % (t1, clit(t1, v1), t2, clit(t2, v2), op, fmt, cast))
    for t, v, op in ids:
        fmt, cast = prfmt(t)
        blocks.append(
            "{ %s a = %s; %s p = a%s; printf(\"%s %s\\n\", %sa, %sp); }"
            % (t, clit(t, v), t, op, fmt, fmt, cast, cast))
        blocks.append(
            "{ %s a = %s; %s p = %sa; printf(\"%s %s\\n\", %sa, %sp); }"
            % (t, clit(t, v), t, op, fmt, fmt, cast, cast))
    # ---- complex: field ops + conjugate + projection (runtime, cc referee) ----
    for t, are, aim, bre, bim, op in complex_cases():
        sfx = "if" if t == "float" else "i"  # imaginary constant matches the lexer's INum
        ce = lambda v: "%r%s" % (v, "f" if t == "float" else "")
        blocks.append(
            "{ %s _Complex a = %s + (%s)*1.0%s; %s _Complex b = %s + (%s)*1.0%s;"
            " %s _Complex r = a %s b;"
            " printf(\"%%a %%a\\n\", (double)__real__ r, (double)__imag__ r); }"
            % (t, ce(are), ce(aim), sfx, t, ce(bre), ce(bim), sfx, t, op))
        if op == "+":  # conjugate ~z = (re,−im), once per pair (a)
            blocks.append(
                "{ %s _Complex a = %s + (%s)*1.0%s; %s _Complex r = ~a;"
                " printf(\"%%a %%a\\n\", (double)__real__ r, (double)__imag__ r); }"
                % (t, ce(are), ce(aim), sfx, t))
    write_prog(outdir, "run", blocks)

    # ---- families fold_*.c + fri_*.c: int only (the C89 const-expr domain), binary + unary ----
    folds, fris = [], []
    for t1, v1, t2, v2, op, rt in bins:
        k = len(folds)
        folds.append(("enum { E%d = (int)(%s %s %s) };" % (k, clit(t1, v1), op, clit(t2, v2)),
                      "printf(\"%%d\\n\", E%d);" % k))
        fris.append("{ %s a = %s; %s b = %s; int r = (int)(a %s b); printf(\"%%d\\n\", r); }"
                    % (t1, clit(t1, v1), t2, clit(t2, v2), op))
    for t, v, op, rt in uns:
        k = len(folds)
        folds.append(("enum { E%d = (int)(%s (%s)) };" % (k, op, clit(t, v)),
                      "printf(\"%%d\\n\", E%d);" % k))
        fris.append("{ %s a = %s; int r = (int)(%s a); printf(\"%%d\\n\", r); }"
                    % (t, clit(t, v), op))
    write_fold(outdir, folds)
    write_prog(outdir, "fri", fris)
    print("run=%d fold=%d" % (len(blocks), len(folds)))

CHUNK, PERFN = 3000, 500

def write_prog(outdir, stem, blocks):
    for ci in range(0, len(blocks), CHUNK):
        ch = blocks[ci:ci + CHUNK]
        fns, calls = [], []
        for fi in range(0, len(ch), PERFN):
            name = "part%d" % (fi // PERFN)
            fns.append("static void %s(void) {\n%s\n}" % (name, "\n".join(ch[fi:fi + PERFN])))
            calls.append("    %s();" % name)
        with open("%s/alg_%s_%d.c" % (outdir, stem, ci // CHUNK), "w") as fp:
            fp.write("#include <stdio.h>\n%s\nint main(void) {\n%s\n    return 0;\n}\n"
                     % ("\n".join(fns), "\n".join(calls)))

def write_fold(outdir, folds):
    for ci in range(0, len(folds), CHUNK):
        ch = folds[ci:ci + CHUNK]
        with open("%s/alg_fold_%d.c" % (outdir, ci // CHUNK), "w") as fp:
            fp.write("#include <stdio.h>\n%s\nint main(void) {\n%s\n    return 0;\n}\n"
                     % ("\n".join(e for e, _ in ch),
                        "\n".join("    " + p for _, p in ch)))

if __name__ == "__main__":
    main(sys.argv[1])
