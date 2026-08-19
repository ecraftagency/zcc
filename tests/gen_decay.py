#!/usr/bin/env python3
# Gate decay: định lý lvalue conversion C99 6.3.2.1p3 — mọi biểu thức type
# T[N] phải convert thành T* trỏ phần tử đầu ở MỌI ngữ cảnh trừ {sizeof,
# unary &, string literal khởi tạo mảng}; hệ quả 6.5.15 (ternary 2 vế
# array → composite pointer) + 6.5.2.2p6 (default arg promotion decay
# trước khi thành arg). Án B 18/8 (git merge segv theo layout ASLR) sống
# đúng ô ternary-string-literal × vararg-stack: type giữ char[2] → store
# strh 2 byte → glibc vsnprintf đọc con trỏ rác.
# Vét cấu trúc: SOURCES (cách sinh expr type array) × CONTEXTS (chỗ tiêu
# thụ) × 2 nhánh ternary. Oracle differential cc trên observable DẪN XUẤT
# (nội dung chuỗi, ký tự, sizeof, so sánh) — không bao giờ in địa chỉ thô.
#
# ── PROOF (miền hợp lệ của oracle differential) ───────────────────────────
# decay(E)=&A[0], A = object mà E chỉ định. Tách 2 thành phần: (a) object A —
# do chuẩn quy định; (b) GIÁ TRỊ SỐ của &A[0] — implementation đặt tự do, KHÔNG
# spec-determined. Oracle sound trên observable o ⟺ o spec-determined (mọi impl
# tuân chuẩn cho cùng o). ⟹ o là hàm của nội dung/định danh A thì hợp lệ; hàm
# của (b) thì vô hiệu.
#   Bổ đề định danh: context `eq` = (p1==p2). Cả hai là &A_i[0] (không null/one-
#   past) ⟹ theo 6.5.9p6, (p1==p2) ⟺ (A1≡A2). Vậy eq spec-determined ⟺ "A1≡A2"
#   spec-determined:
#     • CASE 1 A=named object: 6.2.1 bind về đúng 1 object/declaration ⟹ A1≡A2
#       spec-det = TRUE ⟹ eq=1 duy nhất → oracle HỢP LỆ.
#     • CASE 2 A=mảng string literal: 6.4.5p6 "unspecified whether these arrays
#       are distinct" ⟹ eq ∈ {1,2} đều tuân chuẩn → KHÔNG spec-det → oracle VÔ
#       HIỆU (zcc .rodata trơn=distinct=2; gcc .rodata.str mergeable=1 — cả hai
#       hợp lệ). KHÔNG phải bug decay.
#   Vét cạn: trục định danh phân hoạch TOÀN PHẦN {named, literal-array} (ngoài
#   6.4.5p6 không construct C99 nào cho 2 lần eval cùng lvalue ra 2 object khác
#   cùng nội dung); c∈{0,1} quét cả 2 nhánh ternary. Cell vô hiệu = {(S,eq):
#   S∈CASE2}. id_stable=False đánh dấu ĐÚNG các S đó (soundness: mỗi cái có 1
#   eval rơi CASE 2; completeness: mọi S chỉ-named đều id_stable=True). Chỉ cắt
#   context eq — GIỮ va_stk/idx/arith/szof của chính literal (hàm của nội dung
#   6.4.5p5, spec-det) ⟹ decay của literal VẪN được chứng; chỉ bỏ ĐỊNH DANH,
#   thứ không thuộc định lý decay. Gate sau cắt = decision procedure sound+
#   complete trên lattice observable spec-determined.
# ──────────────────────────────────────────────────────────────────────────
import sys

# (tên, biểu thức, lvalue?, id_stable?)
#   lvalue?    — rvalue như ternary/comma cấm unary & (loại CTX_ADDR)
#   id_stable? — TRUE ⟺ mọi eval của E chỉ định NAMED object (CASE 1 ở PROOF)
#     ⟹ eq spec-determined. FALSE ⟺ có eval chỉ định string literal (CASE 2,
#     6.4.5p6 unspecified) ⟹ eq bị loại. Xem bổ đề định danh trên header.
SOURCES = [
    ("lit",     '"LIT"',                        True,  False),
    ("loc",     "la",                           True,  True),
    ("glo",     "ga",                           True,  True),
    ("mem",     "s.arr",                        True,  True),
    ("pmem",    "ps->arr",                      True,  True),
    ("row",     "m[1]",                         True,  True),
    ("paren",   "(la)",                         True,  True),
    ("ter_ll",  'c ? "T" : ""',                 False, False),  # ổ án B (2 vế literal)
    ("ter_la",  'c ? la : "xy"',                False, False),  # nhánh c==0 = literal
    ("ter_row", "c ? m[0] : m[2]",              False, True),
    ("ter_nst", 'c ? (c ? "a" : "bb") : "ccc"', False, False),  # toàn literal lồng
    ("comma",   "(0, la)",                      False, True),
]

# template dùng {E}; mọi lần thế đều bọc ({E})
CONTEXTS = [
    # variadic arg còn thanh ghi
    ("va_reg",  'printf("%s\\n", ({E}));'),
    # variadic arg TRÀN STACK (5 int ăn x3..x7 sau buf/size/fmt) — ổ án B
    ("va_stk",  'snprintf(buf, sizeof buf, "%d%d%d%d%d[%s]", 1, 2, 3, 4, 5, ({E})); puts(buf);'),
    # hai decay kề nhau trên stack
    ("va_stk2", 'snprintf(buf, sizeof buf, "%d%d%d%d%d[%s|%s]", 1, 2, 3, 4, 5, ({E}), ({E})); puts(buf);'),
    # named arg thanh ghi / tràn stack (6.5.2.2p7 qua prototype)
    ("nm_reg",  "puts(pick1(({E})));"),
    ("nm_stk",  "puts(pick9(1, 2, 3, 4, 5, 6, 7, 8, ({E})));"),
    # simple assignment 6.5.16.1 (phủ luôn ngữ nghĩa return)
    ("assign",  "{ char *q = ({E}); puts(q); }"),
    ("idx",     'printf("%d\\n", (int)({E})[0]);'),
    ("arith",   'printf("%d\\n", (int)*(({E}) + 0));'),
    # hai decay cùng mảng → con trỏ bằng nhau
    ("eq",      'printf("%d\\n", ({E}) == ({E}) ? 1 : 2);'),
    # NGOẠI LỆ định lý: sizeof KHÔNG decay (lvalue array giữ N; ternary đã
    # decay từ operand nên = sizeof(char*))
    ("szof",    'printf("%d\\n", (int)sizeof({E}));'),
]
# NGOẠI LỆ định lý: unary & không decay — &E type T(*)[N], chỉ lvalue
CTX_ADDR = ("addr", 'printf("%d\\n", (int)sizeof(*&({E})));')

def main(outdir):
    fns = []
    for i, (sn, e, lval, id_stable) in enumerate(SOURCES):
        body = []
        for cn, tpl in CONTEXTS + ([CTX_ADDR] if lval else []):
            if cn == "eq" and not id_stable:
                continue  # 6.4.5p6: pointer identity của literal là unspecified
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
