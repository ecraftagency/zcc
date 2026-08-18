/* EXT(c11): _Static_assert ở file-scope + block-scope (postgres18
   StaticAssertStmt/StaticAssertDecl). Pass = không sinh code; fail phải là
   LỖI BIÊN DỊCH (nhánh fail kiểm tay, không nằm trong corpus differential). */
#include <stdio.h>

_Static_assert(sizeof(long) == 8, "LP64");
_Static_assert(sizeof(int) == 4, "ILP" "32 nối chuỗi");

struct S { char a; long b; };

int main(void)
{
    _Static_assert(sizeof(struct S) == 16, "padding LP64");
    _Static_assert(1, "C23: msg vẫn kèm");
    printf("ok %d\n", (int)sizeof(struct S));
    return 0;
}
