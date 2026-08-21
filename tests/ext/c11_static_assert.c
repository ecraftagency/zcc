/* EXT(c11): _Static_assert at file-scope + block-scope (postgres18
   StaticAssertStmt/StaticAssertDecl). Pass = no code emitted; fail must be a
   COMPILE ERROR (the fail branch is checked by hand, not part of the
   differential corpus). */
#include <stdio.h>

_Static_assert(sizeof(long) == 8, "LP64");
_Static_assert(sizeof(int) == 4, "ILP" "32 concatenated");

struct S { char a; long b; };

int main(void)
{
    _Static_assert(sizeof(struct S) == 16, "padding LP64");
    _Static_assert(1, "C23: message still attached");
    printf("ok %d\n", (int)sizeof(struct S));
    return 0;
}
