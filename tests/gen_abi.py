#!/usr/bin/env python3
# Sinh harness ABI theo mô hình automaton: placement của một arg chỉ phụ thuộc
# (kiểu T, số GPR đã dùng g, số FPR đã dùng f, offset stack) — phủ hết transition
# = phủ hết ABI. Mỗi case: prefix g long + f double đẩy máy tới trạng thái đích,
# payload T, rồi 2 sentinel (long, double) bắt lỗi lệch counter phía sau.
# Callee trả tổng có trọng số 2^i (mọi phép toán exact trong double, giá trị bé)
# — caller so sánh == với hằng tính sẵn ở đây. Test CHÉO cc↔zcc hai chiều:
# cùng-compiler hai đầu thì lỗi ABI tự triệt tiêu, không bao giờ lộ.
import sys

STRUCTS = {
    "SI": [("int", "a"), ("int", "b")],          # 8B, 2 int
    "SL": [("long", "a"), ("long", "b")],        # 16B, 2 GPR
    "SB": [("char", "a"), ("long", "b")],        # 16B, padding giữa
    "S3": [("char", "a"), ("char", "b"), ("char", "c")],  # 3B — stack tròn 8
    "HF2": [("float", "a"), ("float", "b")],     # HFA 2 float
    "HD2": [("double", "a"), ("double", "b")],   # HFA 2 double
    "HF4": [("float", "a"), ("float", "b"), ("float", "c"), ("float", "d")],
    "SBIG": [("long", "a"), ("long", "b"), ("long", "c")],  # >16B → con trỏ
    "SA16": [("long", "a"), ("long", "b")],      # 16B aligned(16) — AAPCS C.9
}
# thuộc tính đính kèm định nghĩa struct (SA16: gcc round NGRN chẵn — pr92904;
# clang Darwin KHÔNG round — differential per-platform nên hai bên đều tự khớp)
ATTRS = {"SA16": " __attribute__((aligned(16)))"}
SCALARS = ["char", "short", "int", "long", "float", "double"]
FLOATKIND = {"float", "double", "HF2", "HD2", "HF4"}
ALL = SCALARS + list(STRUCTS)

def ctype(t):
    return ("struct %s" % t) if t in STRUCTS else t

class Case:
    def __init__(self, name):
        self.name = name
        self.params = []   # (ctype string, tên, [giá trị lá])
        self.leaf = 0
        self.expected = 0.0
    def add(self, t, values=None):
        pn = "p%d" % len(self.params)
        leaves = STRUCTS[t] if t in STRUCTS else [(t, None)]
        vals = []
        for _ in leaves:
            v = (self.leaf * 7) % 120 + 1
            self.expected += float(v) * float(1 << self.leaf)
            vals.append(v)
            self.leaf += 1
        self.params.append((t, pn, vals))
        return pn

def emit_case(c, callee, caller):
    sig = ", ".join("%s %s" % (ctype(t), n) for t, n, _ in c.params)
    terms, k = [], 0
    for t, n, vals in c.params:
        leaves = STRUCTS[t] if t in STRUCTS else [(t, None)]
        for (lt, m), _ in zip(leaves, vals):
            acc = "%s.%s" % (n, m) if m else n
            terms.append("(double)%s * %s.0" % (acc, 1 << k))
            k += 1
    callee.append("double %s(%s) {\n    return %s;\n}" %
                  (c.name, sig, " + ".join(terms)))
    setup, argv = [], []
    for t, n, vals in c.params:
        if t in STRUCTS:
            setup.append("    struct %s %s_v = {%s};" %
                         (t, n, ", ".join(str(v) for v in vals)))
            argv.append(n + "_v")
        else:
            argv.append("(%s)%d" % (t, vals[0]))
    # gọi CẢ trực tiếp (bl) lẫn qua function pointer (blr) — ABI phải đồng
    # nhất hai đường; fn-ptr chéo compiler là đúng chỗ nginx hay segfault
    sigt = ", ".join(ctype(t) for t, _, _ in c.params)
    args = ", ".join(argv)
    caller.append(
        "    {\n%s\n        double (*fp)(%s) = %s;\n"
        "        if (%s(%s) != %s) { printf(\"FAIL %s\\n\"); bad++; }\n"
        "        if (fp(%s) != %s) { printf(\"FAIL fp:%s\\n\"); bad++; }\n    }"
        % ("\n".join(setup) if setup else "", sigt, c.name,
           c.name, args, repr(c.expected), c.name,
           args, repr(c.expected), c.name))

def main(outdir):
    callee, caller, decls = [], [], []
    cases = []
    # phủ trạng thái: counter LIÊN QUAN của T quét đủ 0..8 (biên 7: struct 2-reg
    # tràn kích hoạt khóa C.11; 5..8: HFA 4 member tràn), counter kia lấy 2 cực trị
    for t in ALL:
        rel_f = t in FLOATKIND
        for rel in range(9):
            for other in (0, 8):
                g, f = (other, rel) if rel_f else (rel, other)
                c = Case("f_%s_g%d_f%d" % (t, g, f))
                for _ in range(g):
                    c.add("long")
                for _ in range(f):
                    c.add("double")
                c.add(t)
                c.add("long")    # sentinel bắt lệch counter GPR/stack
                c.add("double")  # sentinel bắt lệch counter FPR/stack
                cases.append(c)
    # cặp T,T khi cạn sạch thanh ghi: kiểm tra packing hai arg stack KỀ NHAU
    for t in ALL:
        c = Case("f_pair_%s" % t)
        for _ in range(8):
            c.add("long")
        for _ in range(8):
            c.add("double")
        c.add(t)
        c.add(t)
        c.add("char")  # sentinel packed ngay sau
        cases.append(c)
    for c in cases:
        emit_case(c, callee, caller)
        sig = ", ".join(ctype(t) for t, _, _ in c.params)
        decls.append("double %s(%s);" % (c.name, sig))

    # quét RETURN: mỗi kiểu một lần — phủ trả x0, cặp x0/x1, HFA v0..v3,
    # sret qua x8 (>16B) — và trả CẢ qua function pointer (blr)
    nret = 0
    for t in ALL:
        name = "r_%s" % t
        leaves = STRUCTS[t] if t in STRUCTS else [(t, None)]
        vals = [5 * i + 3 for i in range(len(leaves))]
        exp = sum(float(v) * float(1 << i) for i, v in enumerate(vals))
        if t in STRUCTS:
            body = "    struct %s x;\n" % t + "".join(
                "    x.%s = %d;\n" % (m, v) for (_, m), v in zip(leaves, vals))
            callee.append("struct %s %s(void) {\n%s    return x;\n}" % (t, name, body))
            probe = " + ".join("(double)r.%s * %d.0" % (m, 1 << i)
                               for i, (_, m) in enumerate(leaves))
            caller.append(
                "    {\n        struct %s (*rp)(void) = %s;\n"
                "        struct %s r = %s(); struct %s r2 = rp();\n"
                "        if (%s != %s) { printf(\"FAIL %s\\n\"); bad++; }\n"
                "        r = r2;\n"
                "        if (%s != %s) { printf(\"FAIL rp:%s\\n\"); bad++; }\n    }"
                % (t, name, t, name, t, probe, repr(exp), name, probe, repr(exp), name))
            decls.append("struct %s %s(void);" % (t, name))
        else:
            callee.append("%s %s(void) {\n    return (%s)%d;\n}" % (t, name, t, vals[0]))
            caller.append(
                "    {\n        %s (*rp)(void) = %s;\n"
                "        if ((double)%s() != %d.0 || (double)rp() != %d.0)"
                " { printf(\"FAIL %s\\n\"); bad++; }\n    }"
                % (t, name, name, vals[0], vals[0], name))
            decls.append("%s %s(void);" % (t, name))
        nret += 1

    # variadic: named prefix g long + int n, rồi 3 arg vô danh kiểu T (mỗi arg
    # một slot 8 — va_arg bước 8); T char/short/float đến dạng promoted
    va_callee, va_caller = [], []
    for t in SCALARS:
        pt = {"char": "int", "short": "int", "float": "double"}.get(t, t)
        for g in (0, 8):
            name = "v_%s_g%d" % (t, g)
            pre = ", ".join("long q%d" % i for i in range(g))
            sig = (pre + ", " if pre else "") + "int n, ..."
            exp = sum(float(10 + 3 * i) * float(1 << i) for i in range(3))
            va_callee.append(
                "double %s(%s) {\n    va_list ap; double s = 0; int i;\n"
                "    va_start(ap, n);\n"
                "    for (i = 0; i < n; i++) s += (double)va_arg(ap, %s) * (double)(1L << i);\n"
                "    va_end(ap); return s;\n}" % (name, sig, pt))
            argv = ", ".join("0L" for _ in range(g))
            vals = ", ".join("(%s)%d" % (t, 10 + 3 * i) for i in range(3))
            va_caller.append(
                "    if (%s(%s3, %s) != %s) { printf(\"FAIL %s\\n\"); bad++; }"
                % (name, argv + ", " if argv else "", vals, repr(exp), name))
            decls.append("double %s(%s);" % (name, sig))

    hdr = "\n".join("struct %s { %s }%s;" %
                    (t, " ".join("%s %s;" % (lt, m) for lt, m in ms), ATTRS.get(t, ""))
                    for t, ms in STRUCTS.items())
    with open(outdir + "/abi_defs.h", "w") as fp:
        fp.write(hdr + "\n" + "\n".join(decls) + "\n")
    with open(outdir + "/abi_callee.c", "w") as fp:
        fp.write('#include <stdarg.h>\n#include "abi_defs.h"\n\n'
                 + "\n\n".join(callee + va_callee) + "\n")
    with open(outdir + "/abi_caller.c", "w") as fp:
        fp.write('#include <stdio.h>\n#include "abi_defs.h"\n\n'
                 "int main(void) {\n    int bad = 0;\n"
                 + "\n".join(caller + va_caller)
                 + "\n    printf(\"%d case, %d fail\\n\", "
                 + str(len(cases) + len(va_caller) + nret)
                 + ", bad);\n    return bad != 0;\n}\n")

main(sys.argv[1])
