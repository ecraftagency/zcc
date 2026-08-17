#!/usr/bin/env python3
# =============================================================================
# HARNESS LỚP 2 — PREPROCESSOR NHƯ HỆ VIẾT LẠI HẠNG (term rewriting system)
# =============================================================================
#
# ## Nền tảng lý thuyết
#
# Macro expansion C89 (3.8.3) là một hệ viết lại hạng trên chuỗi token:
#   - Mỗi #define là một luật viết lại (rewrite rule).
#   - Expansion + rescan là quá trình tìm dạng chuẩn (normal form).
#   - Điều kiện DỪNG không phải may mắn mà là định lý: "blue paint" — tên macro
#     đang được expand bị SƠN trong kết quả của chính nó (3.8.3.4), nên mỗi
#     bước rescan chỉ giảm tập luật áp dụng được → hệ terminating.
#
# KHÔNG GIAN HẠNG (mọi chương trình có thể viết) là vô hạn — không vét được.
# Nhưng KHÔNG GIAN CƠ CHẾ hữu hạn: thuật toán expansion chỉ có chừng này điểm
# quyết định, mỗi điểm 2-3 nhánh:
#   (1) object-like vs function-like vs tên fn-like không có "(" theo sau
#   (2) prescan argument: arg được expand đầy đủ TRƯỚC khi thế — TRỪ KHI là
#       toán hạng của # hoặc ## (3.8.3.1) → 3 số phận của một arg
#   (3) # stringize: spelling THÔ (chưa expand), whitespace chuẩn hoá về 1
#       space, escape \ và " bên trong string literal (3.8.3.2)
#   (4) ## paste: spelling thô hai bên, token kết quả THAM GIA rescan
#       (3.8.3.3) — tức có thể tạo ra tên macro mới
#   (5) rescan: kết quả expansion + phần text theo sau hợp lại thành miền
#       rescan (3.8.3.4) — expansion có thể "với tay" lấy (args) phía sau
#   (6) sơn xanh: tự đệ quy trực tiếp/gián tiếp/qua arg đều phải dừng
# Bug preprocessor sống ở TƯƠNG TÁC giữa các điểm này (paste-rồi-rescan,
# stringize-của-arg-chứa-macro, sơn-xuyên-arg...) — harness này liệt kê có hệ
# thống ma trận tương tác đó, không liệt kê chương trình.
#
# ## Quan sát dạng chuẩn (observable)
#
# Không diff `-E` output: C89 KHÔNG đặc tả format text sau preprocess
# (whitespace/newline tuỳ compiler) — diff -E là oracle KHÔNG hợp lệ.
# Thay vào đó REIFY dạng chuẩn thành giá trị runtime:
#   #define STR_(x) #x            → spelling THÔ của arg
#   #define XSTR(x) STR_(x)       → spelling SAU KHI expand đầy đủ
# Spelling do # sinh ra là thứ DUY NHẤT spec đặc tả chính xác từng byte
# (3.8.3.2), nên printf các string này rồi diff stdout zcc↔cc là oracle chuẩn.
# (# cũng thuộc hệ đang test — chấp nhận được vì luật spelling của nó là phần
# đặc tả chặt nhất, và mọi lệch # sẽ tự lộ trong chính họ test C.)
#
# ## Các vùng LOẠI TRỪ khỏi oracle (undefined/unspecified trong C89 — diff ở
# đó vô nghĩa, hai compiler khác nhau đều "đúng"):
#   - arg RỖNG cho macro fn-like (3.8.3: undefined; C99 mới hợp pháp)
#   - token `defined` SINH RA từ expansion trong #if (3.8.1: undefined —
#     zcc CÓ hỗ trợ vì pthread.h cần, nhưng đó là hành vi EXT khoá theo
#     clang, test ở tests/ext/, không phải chuẩn C89)
#   - ## tạo ra token KHÔNG hợp lệ (3.8.3.3: undefined)
#   - THỨ TỰ evaluate # và ## trong cùng replacement list (3.8.3.2-3:
#     unspecified) — chỉ test case mà mọi thứ tự cho cùng kết quả
#   - invocation "vắt" qua biên expansion kiểu `#define LP (` rồi `F LP x)`
#     (vùng xám nổi tiếng của spec; ví dụ `h 5)` trong EXAMPLE 3.8.3.5 bị
#     cắt khỏi bản trích ở đây vì lý do này)
#   - __DATE__/__TIME__ (không tất định), số học UB trong #if (lọc như alg)
#
# ## Họ test
#   A prescan-matrix   B paste-matrix      C stringize-spelling
#   D blue-paint       E rescan-với-text-sau   F #if arithmetic (vét cạn,
#     tái dùng ieval của gen_alg — #if tính trong long/unsigned long, 3.8.1)
#   G cấu trúc điều kiện lồng   H undef/redefine   I include guard + #line
# =============================================================================
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gen_alg import IINFO, icorners, uac, wrap, fits, ieval, ARITH, BITS, RELS

M = []   # dòng nội dung cpp_mech.c (sau phần #define mở đầu)
def case(directives, printfs):
    M.append(directives.rstrip() + "\n" + printfs.rstrip())

def mech():
    # ---- A: ma trận prescan — 3 số phận của một argument ----------------
    # arg ∈ {object macro, fn-like call, fn-like name KHÔNG có "(", ident
    # thường, self-painted} × cách dùng ∈ {RAW (#x: thô), XSTR (expand)}
    case("""#define A_OBJ obj_val
#define A_FN(a) fn(a)
#define A_SELF A_SELF""",
"""    printf("A1 [%s][%s]\\n", STR_(A_OBJ), XSTR(A_OBJ));       /* tho vs expand */
    printf("A2 [%s][%s]\\n", STR_(A_FN(1)), XSTR(A_FN(1)));
    printf("A3 [%s][%s]\\n", STR_(A_FN), XSTR(A_FN));           /* fn-like khong "(" -> giu nguyen */
    printf("A4 [%s][%s]\\n", STR_(plain_id), XSTR(plain_id));
    printf("A5 [%s][%s]\\n", STR_(A_SELF), XSTR(A_SELF));       /* son xanh qua prescan */""")

    # ---- B: ma trận paste — spelling thô + kết quả tham gia rescan ------
    case("""#define CAT(a,b) a##b
#define XCAT(a,b) CAT(a,b)
#define PA one
#define PB two
#define AB 7
#define SCAT(a,b) STR_(a##b)""",
"""    { int onetwo = 12, PAPB = 99;
      printf("B1 %d %d\\n", CAT(PA,PB), XCAT(PA,PB));   /* tho->PAPB=99, expand-truoc->onetwo=12 */
      printf("B2 %d\\n", CAT(A,B));                     /* paste tao TEN MACRO -> rescan -> 7 */
      printf("B3 %d\\n", CAT(12,34));                   /* pp-number paste */
      printf("B4 %d\\n", CAT(0x1,F));                   /* 0x1F = 31 */
      printf("B5 %d\\n", (int)(CAT(199506,L) / 100000L)); /* suffix paste — bug M13 cdefs khoa o day */
      printf("B6 [%s]\\n", SCAT(PA,PB));                /* stringize cua paste: "PAPB" */
      printf("B7 %d\\n", XCAT(CAT(o,ne),CAT(t,wo)) == 12 ? 1 : 0); }""")

    # ---- C: spelling stringize (3.8.3.2 — đặc tả từng byte) -------------
    case("",
"""    printf("C1 [%s]\\n", STR_( spaced   +   out ));    /* whitespace -> 1 space, dau/cuoi cat */
    printf("C2 [%s]\\n", STR_("q\\n"));                 /* escape " va \\ trong string arg */
    printf("C3 [%s]\\n", STR_(0x1F));                   /* giu spelling goc, khong ra 31 */
    printf("C4 [%s]\\n", STR_(a/*cmt*/b));              /* comment = 1 whitespace (phase 3) */
    printf("C5 [%s]\\n", STR_(a \\
b));                                                    /* splice \\-newline trong arg */
    printf("C6 [%s]\\n", "con" "cat");                  /* phase 6: adjacent string literal */""")

    # ---- D: sơn xanh — mọi hình thái đệ quy phải DỪNG -------------------
    # kèm bản trích EXAMPLE chuẩn 3.8.3.5 (phần specified; `h 5)` và các
    # invocation vắt biên bị cắt — xem exclusion list đầu file)
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
"""    printf("D1 [%s]\\n", XSTR(REC));                  /* tu-de-quy object */
    printf("D2 [%s]\\n", XSTR(FREC(1)));               /* tu-de-quy fn-like */
    printf("D3 [%s]\\n", XSTR(MU1));                   /* de quy gian tiep: MU1->MU2->MU1(son) */
    printf("D4 [%s]\\n", XSTR(G_ID(REC)));             /* son xuyen arg-substitution */
    printf("D5 [%s]\\n", XSTR(f(y+1)));                /* 3.8.3.5: "f(2 * (y+1))" */
    printf("D6 [%s]\\n", XSTR(f(f(z))));               /* "f(2 * (f(2 * (z[0]))))" */
    printf("D7 [%s]\\n", XSTR(t(t(g)(0) + t)(1)));     /* "f(2 * (0)) + t(1)" */""")

    # ---- E: rescan hợp nhất với text theo sau ---------------------------
    case("""#define ID(q) q
#define FN2(y) ((y)+1)
#define OBJ2 FN2""",
"""    printf("E1 %d\\n", ID(FN2)(7));                   /* "(" tu SOURCE sau khi ID tra ve FN2 */
    printf("E2 %d\\n", OBJ2(3));                       /* object macro tra ve ten fn-like */""")

    # ---- G: cấu trúc điều kiện lồng nhau (đủ 8 tổ hợp defined 2 cờ) -----
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
    printf("H2 %d %d\\n", HX, HSAME);                  /* redefine sau undef + benign redef */""")

    # ---- I: include guard, directive lệch chuẩn hình thức, #line --------
    case("""#include "cpp_aux.h"
#include "cpp_aux.h"
#
   #   define IND 4""",
"""    printf("I1 %d %d\\n", AUX_VAL, IND);              /* guard chan lan 2; "#" tran + indent */
#line 7000
    printf("I2 %d\\n", __LINE__);
    printf("I3 %d\\n", __STDC__);
    printf("I4 %d\\n", 'A');                           /* char-const (ASCII khoa platform) */""")

def ppif():
    """Họ F: vét cạn số học #if — C89 3.8.1: mọi số học trong #if tính bằng
    long/unsigned long (LP64: 64-bit). Tái dùng ieval + luật UAC của gen_alg,
    thu hẹp về 2 kiểu. Oracle KÉP: (zcc == cc) VÀ (cc == python) — vế sau
    tự kiểm định generator: python sai luật thì cc in FAIL ngay."""
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
                            continue  # UB (overflow, chia 0, MIN/-1) — loại
                        expect = str(r) if op in RELS else lit(rt, r)
                        out.append(
                            "#if (%s %s %s) == %s\n    ok++;\n#else\n"
                            "    printf(\"FAIL f%d\\n\"); bad++;\n#endif"
                            % (lit(t1, v1), op, lit(t2, v2), expect, k))
                        k += 1
    for t in TS:            # shift: lhs signed âm bỏ << (UB-ish), >> giữ (khoá platform)
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
    for t in TS:            # unary trong #if
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
    # &&/|| ngắn mạch trong #if: vế phải chia 0 KHÔNG được tính
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
