/* C99 6.5.15p6 — the conditional operator's result type when the operands are
   not both arithmetic. The two rows the parser used to get wrong:
     - one operand a pointer, the other a NULL POINTER CONSTANT: the result has
       the pointer type, and the constant is converted to it. Written the other
       way round (`0` first) the old code took the arithmetic path and narrowed
       the pointer to `int`; written this way it kept the pointer type but left
       the `0` arm an `int`. musl's `return l < 0 ? 0 : password;` is the first.
     - one operand `void *`, the other a pointer to an object type: `void *`. */
#include <stdio.h>
static char buf[128];
static char *pick(long l) { return l < 0 ? 0 : buf; }
static char *pick2(long l) { return l < 0 ? buf : 0; }
static void *vp(int c, void *v, char *p) { return c ? v : p; }
int main(void)
{
    buf[0] = 'x';
    printf("%d %d\n", pick(-1) == 0, pick(1) == buf);
    printf("%d %d\n", pick2(-1) == buf, pick2(1) == 0);
    printf("%d %d\n", vp(1, buf, buf) == buf, vp(0, buf, buf) == buf);
    printf("%zu %zu\n", sizeof(0 ? (char *)0 : buf), sizeof(1 ? buf : (char *)0));
    return 0;
}
