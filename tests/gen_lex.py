#!/usr/bin/env python3
# =============================================================================
# HARNESS LỚP 1 — LEXER: ngôn ngữ chính quy + bảng phân loại hữu hạn
# =============================================================================
# Ba không gian hữu hạn của lexer, vét được:
#
# 1. PHÂN LOẠI INTEGER LITERAL (C89 3.1.3.2): kiểu của một literal là hàm của
#    (base × suffix × giá trị) — dec không suffix leo int→long→ulong, oct/hex
#    leo int→uint→long→ulong (khác nhau!), U ép unsigned, L ép ≥long.
#    Không gian = 3 base × 4 suffix × các giá trị biên (INT_MAX±1, UINT_MAX±1,
#    LONG_MAX, ULONG_MAX) — hữu hạn, liệt kê hết.
#    Observable: (giá trị bit, sizeof, signedness) — 3 probe đủ tách mọi kiểu:
#      %lu của (unsigned long)LIT   → giá trị
#      sizeof(LIT)                  → 4 vs 8 (int-họ vs long-họ, LP64)
#      (LIT) - (LIT) - 1 < 0        → 1 nếu kiểu signed (kết quả -1), 0 nếu
#                                     unsigned (wrap thành max) — probe không
#                                     UB (trừ rồi trừ 1 không tràn)
#
# 2. ESCAPE SEQUENCE (3.1.3.4): bảng escape hữu hạn (simple + octal 1-3 chữ số
#    + hex) × 2 ngữ cảnh (char-const, string). Observable: giá trị %d (char
#    signed Darwin) và sizeof string.
#
# 3. MAXIMAL MUNCH (3.1): điểm nhập nhằng hữu hạn của bảng token C89 —
#    `a+++b` = (a++)+b, `a---b` = (a--)-b, `x+++++y` là LỖI (loại), `..` vs
#    `...`, `>>=` vs `> >=`... test các cắt hợp lệ.
#
# UB/loại trừ: literal vượt ULONG_MAX (3.1.3.2: lỗi), '\x' quá miền char
# (impl-def — GIỮ vì khoá platform), multi-char 'ab' (impl-def — giữ, khoá).
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
    # escape trong string: sizeof đếm đúng số byte sau dịch escape
    em('    printf("ES %%d %%d\\n", (int)sizeof("\\x41\\102z"), (int)sizeof("a\\0b"));')
    # multi-char const: impl-defined, khoá theo trọng tài cùng platform
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
    print("lex=%d dòng" % len(L))

if __name__ == "__main__":
    main(sys.argv[1])
