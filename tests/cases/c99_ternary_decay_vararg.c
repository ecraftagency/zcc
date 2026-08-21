/* C99 6.5.15 + 6.5.2.2p6: an array operand of a ternary and a variadic
   argument must decay to a pointer. Bug: the ternary type retained char[2],
   so the 9th-and-later argument overflowing to the stack was truncated to 2
   bytes by store_narrow, making glibc read a garbage pointer (git diff.c
   `? " " : ""`, segfault depending on ASLR layout). This case forces a decayed
   argument to FALL ONTO THE STACK (>8 GP registers). */
#include <stdio.h>

static char b[128];

int main(void) {
    unsigned long v = 3;
    int n;
    /* 3 named (buf,size,fmt) + first 5 varargs consume x3-x7; v and the ternary go on the stack */
    n = snprintf(b, sizeof b, " %s%s%*s | %*lu%s", "P", "name", 5, "",
                 4, v, v ? "T" : "");
    printf("%d[%s]\n", n, b);
    /* ternary with both branches arrays, selecting the right branch; sizeof checks the decayed type = pointer */
    printf("%s %d\n", 0 ? "left" : "right", (int)sizeof(1 ? "ab" : "cdef"));
    /* a real array (not a literal) used as a stack vararg */
    {
        char arr[6] = "array";
        printf("%d %d %d %d %d %d %d %d %s\n", 1, 2, 3, 4, 5, 6, 7, 8, arr);
    }
    return 0;
}
