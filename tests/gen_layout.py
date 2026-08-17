#!/usr/bin/env python3
# =============================================================================
# HARNESS LỚP 5 (phần dữ liệu) — LAYOUT STRUCT/UNION/BITFIELD
# =============================================================================
# Layout là một HÀM ĐỆ QUY hữu hạn: (dãy kiểu member) → (offset từng member,
# size, align) — xác định bởi luật align LP64 Darwin (khoá trong TyTab).
# Sai layout khi interop struct với libc/SDK = hindenbug âm thầm (ghi đè
# field hàng xóm, hỏng dữ liệu không báo lỗi). Không gian: dãy member độ dài
# ≤3 trên 9 kiểu cơ sở (có nested struct + mảng — hai nguồn padding khó) =
# 9+81+729 = 819 struct + 90 union + họ bitfield.
#
# Observable: sizeof(S) + offset TỪNG member qua (char*)&s.m - (char*)&s
# (offsetof C89-kosher, không macro để zcc/cc cùng đường parse).
# Bitfield: C89 để impl-defined (thứ tự bit, straddle) — KHOÁ theo trọng tài
# cùng platform (như mọi impl-defined khác của dự án); quan sát thêm hành vi
# ghi/đọc từng field (pattern 0x5A5A...) chứ không chỉ hình dạng.
# Oracle: differential cc -std=c89. Không có UB trong không gian này.
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
    # union: cặp kiểu (size = max, align = max) — 81 cặp đủ phủ luật max
    for i, j in product(range(len(BASIS)), repeat=2):
        mems = mdecl(BASIS[i], "a") + " " + mdecl(BASIS[j], "b")
        lines.append('    { static union { %s } u; printf("u%d %%d\\n", (int)sizeof(u)); }'
                     % (mems, k))
        k += 1
    # bitfield: dãy width trên int (impl-defined nhưng khoá platform) + xen kẽ
    # member thường + unnamed + width 0 (ép mở đơn vị mới, 3.5.2.1)
    bf = [
        ("int a:1;",                          [("a", 0)]),
        ("int a:31;",                         [("a", 12345)]),
        ("int a:1; int b:1;",                 [("a", 0), ("b", -1)]),
        ("int a:5; int b:27;",                [("a", 9), ("b", 1000)]),
        ("int a:5; int b:28;",                [("a", 9), ("b", 1000)]),   # straddle đơn vị
        ("int a:3; char c; int b:3;",         [("a", 3), ("c", 7), ("b", 2)]),  # xen member thường
        ("int a:3; int :0; int b:3;",         [("a", 3), ("b", 2)]),      # :0 ép đơn vị mới
        ("int a:3; int :5; int b:3;",         [("a", 3), ("b", 2)]),      # unnamed đệm
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
    print("layout=%d hình" % k)

if __name__ == "__main__":
    main(sys.argv[1])
