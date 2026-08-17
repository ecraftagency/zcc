#!/usr/bin/env python3
# =============================================================================
# HARNESS LỚP 3 — ĐẠI SỐ DECLARATOR: phần hữu hạn của văn phạm parser
# =============================================================================
# Văn phạm C nói chung vô hạn — không vét được. Nhưng CÂY DECLARATOR đến độ
# sâu k là hữu hạn: declarator = cây trên 4 constructor
#   Ptr(t)  = con trỏ tới t          Arr2(t)/Arr3(t) = mảng [2]/[3] phần tử t
#   PFn(t)  = con trỏ tới HÀM(int) trả t
# với ràng buộc C89 (3.5.4): hàm không trả mảng, không trả hàm — generator
# lọc cây không hợp lệ. Đây chính là góc khó nhất của cú pháp C ("spiral
# rule") — chỗ parser dễ gắn sai * [] () nhất, và cũng là đúng nỗi sợ
# "function pointer làm không chuẩn thì nginx segfault".
#
# Với mỗi cây hợp lệ (độ sâu ≤ 3, 4 base × ~156 cây/base):
#   - render declaration bằng thuật toán inside-out chuẩn (đúng văn phạm),
#   - observable: sizeof(x), và sizeof qua MỘT bước giải cấu trúc
#     (x[0] nếu ngoài cùng là mảng, *x nếu là con trỏ) — ép parser hai bên
#     đồng ý cả cấu trúc lẫn kiểu con.
#   - PFn còn được GÁN hàm thật và GỌI qua con trỏ (ABI đường blr) khi kiểu
#     trả về là int — hành vi runtime, không chỉ sizeof.
# Oracle: differential cc -std=c89. Không có UB trong không gian này.
# =============================================================================
import sys
from itertools import product

BASES = ["char", "int", "double", "char *"]
OPS = ["P", "A2", "A3", "F"]

def valid(chain):
    # chain áp từ NGOÀI vào TRONG: chain[0] là tầng ngoài cùng của kiểu.
    # Ràng buộc: F (pointer-to-function) không được trả mảng/hàm trực tiếp →
    # phần tử SAU F không được là A*/F? Không — PFn trả kiểu con; kiểu con là
    # phần còn lại của chain. Hàm không trả mảng và không trả hàm:
    for i, op in enumerate(chain):
        if op == "F" and i + 1 < len(chain) and chain[i + 1] in ("A2", "A3", "F"):
            return False
    return True

def render2(chain, base, name):
    """chain[0] = constructor NGOÀI cùng ("x là một ___") → bind CHẶT nhất vào
    tên → áp TRƯỚC. [] () bind chặt hơn *, nên khi áp [n] lên chuỗi đang bắt
    đầu bằng '*' phải bọc ngoặc (đây chính là chỗ đẻ ra spiral rule)."""
    d = name
    for op in chain:
        if op == "P":
            d = "*" + d
        elif op == "F":
            d = "(*" + d + ")(int)"   # ptr-tới-hàm: '*' phải nằm trong ngoặc với tên
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
                # PFn ngoài cùng, trả int trực tiếp: gán hàm thật + GỌI qua blr
                if chain == ("F",) and base == "int":
                    lines.append('    { int (*fp)(int) = fn_int_helper;'
                                 ' printf("call%d %%d\\n", fp(35)); }' % k)
                k += 1
    # tổ hợp kinh điển khó nhất, viết tay khoá cứng (spiral rule showcase)
    hard = [
        ("int (*ha[2])(int)", "ha", "mảng 2 con trỏ hàm"),
        ("int (*(*hb)(int))(int)", "hb", "ptr hàm trả ptr hàm"),
        ("char *(*(*hc[3])(int))[2]", "hc", "mảng ptr-hàm trả ptr-mảng-2-char*"),
        ("int (*(*hd)[3])(int)", "hd", "ptr tới mảng 3 con trỏ hàm"),
    ]
    for decl, nm, _ in hard:
        lines.append('    { static %s; printf("h_%s %%d\\n", (int)sizeof(%s)); }'
                     % (decl, nm, nm))
    with open(outdir + "/decl_cases.c", "w") as fp:
        fp.write("#include <stdio.h>\n%s\nint main(void) {\n%s\n    return 0;\n}\n"
                 % ("\n".join(fnimpls), "\n".join(lines)))
    print("decl=%d cây" % k)

if __name__ == "__main__":
    main(sys.argv[1])
