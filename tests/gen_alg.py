#!/usr/bin/env python3
# Vét cạn đại số biểu thức C89 trên miền hữu hạn: (op × kiểu × kiểu × corner ×
# corner). Usual arithmetic conversions (C89 3.2.1.5) là semilattice hữu hạn
# trên 10 kiểu số học — phủ hết mọi cặp kiểu + giá trị biên là phủ hết không
# gian chuyển kiểu. Oracle = differential với cc, nên phải LỌC UB tại đây
# (signed overflow, chia 0, INT_MIN/-1, shift tràn, float→int ngoài miền):
# tại điểm UB cả hai compiler đều "đúng" — diff ở đó vô nghĩa. Impl-defined
# (narrowing, % âm, >> âm) GIỮ vì trọng tài cùng platform (khóa LP64 Darwin).
#
# Sinh 3 họ chương trình (thứ tự enumerate GIỐNG HỆT nhau để diff theo dòng):
#   run_*.c  — toán hạng đi qua BIẾN (chặn const-fold), in kết quả full width
#   fold_*.c — cùng biểu thức nhưng literal trong enum { E = (int)(...) }
#              (bắt buộc đi đường const-eval của compiler)
#   fri_*.c  — mirror của fold qua biến (đường runtime codegen)
# diff(run zcc, run cc) + diff(fold zcc, fold cc) = đúng so trọng tài;
# diff(fold zcc, fri zcc) = biểu đồ giao hoán fold∘parse = runtime∘parse NỘI BỘ.
import sys

ITYPES = [("char", 8, True), ("unsigned char", 8, False),
          ("short", 16, True), ("unsigned short", 16, False),
          ("int", 32, True), ("unsigned int", 32, False),
          ("long", 64, True), ("unsigned long", 64, False)]
# long long ≡ long trên LP64 (cùng biểu diễn) — không thêm điểm mới vào không gian
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

FCORNERS = {"float": [0.0, 1.0, -2.5, 0.5, 325000000.0],   # đều exact trong float
            "double": [0.0, 1.0, -2.5, 0.001, 325000000.0]}

def promote(t):
    return "int" if RANK[t] < 3 else t  # mọi giá trị char/short lọt int (LP64)

def uac(t1, t2):
    a, b = promote(t1), promote(t2)
    if RANK[b] > RANK[a] or (RANK[b] == RANK[a] and not IINFO[b][1]):
        a, b = b, a
    # a = rank cao hơn (unsigned thắng khi đồng rank)
    if IINFO[a][1] == IINFO[b][1] or IINFO[a][1] is False:
        return a
    # a signed rank cao, b unsigned rank thấp: LP64 long chứa hết unsigned int
    return a if IINFO[a][0] > IINFO[b][0] else "unsigned " + a

def wrap(t, v):
    w, s = IINFO[t]
    v &= (1 << w) - 1
    return v - (1 << w) if s and v >= (1 << (w - 1)) else v

def fits(t, v):
    w, s = IINFO[t]
    return -(1 << (w - 1)) <= v < (1 << (w - 1)) if s else 0 <= v < (1 << w)

def ieval(op, t, a, b):
    """Kết quả op trong kiểu t, None = UB (lọc khỏi oracle)."""
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
            q = -q  # C: chia cắt về 0; % cùng dấu số bị chia (khóa platform)
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

def prfmt(t):  # (format, cast) in kết quả chuẩn hoá full width
    if t in FTYPES:
        return ("%a", "(double)")
    return ("%lu", "(unsigned long)") if not IINFO[t][1] else ("%ld", "(long)")

def enum_cases():
    """Sinh (t1,v1,t2,v2,op,rt) hợp lệ — thứ tự cố định, mọi họ chương trình dùng chung."""
    out = []
    # int × int: đủ 14 op
    for t1, _, _ in ITYPES:
        for t2, _, _ in ITYPES:
            rt = uac(t1, t2)
            for op in ARITH + BITS + RELS:
                for v1 in icorners(t1):
                    for v2 in icorners(t2):
                        a, b = wrap(rt, v1), wrap(rt, v2)  # convert lên rt (giá trị bảo toàn/wrap)
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
                            continue  # chia 0 float: UB theo C89 (không ép IEEE)
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
                    continue  # << số âm / >> giữ (impl-def arithmetic, khóa platform)
                if not (s and pv < 0):
                    if s and not fits(p, pv << sh):
                        pass  # << tràn signed = UB
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
                    tv = int(v)  # truncate về 0
                    if not fits(dst, tv):
                        continue  # float→int ngoài miền = UB
                out.append((src, v, dst))
    return out

def main(outdir):
    bins = enum_cases()
    flts = float_cases()
    shs = shift_cases()
    csts = cast_cases()

    # ---- họ run_*.c: mọi case qua biến ----
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
    write_prog(outdir, "run", blocks)

    # ---- họ fold_*.c + fri_*.c: chỉ int×int (miền const-expr C89) ----
    folds, fris = [], []
    for t1, v1, t2, v2, op, rt in bins:
        k = len(folds)
        folds.append(("enum { E%d = (int)(%s %s %s) };" % (k, clit(t1, v1), op, clit(t2, v2)),
                      "printf(\"%%d\\n\", E%d);" % k))
        fris.append("{ %s a = %s; %s b = %s; int r = (int)(a %s b); printf(\"%%d\\n\", r); }"
                    % (t1, clit(t1, v1), t2, clit(t2, v2), op))
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
