/* computed include (C89 3.8.2): #include MACRO — when it matches neither of
 * the two standard forms, expand the macro and dispatch again (used by
 * redis rax.c) */
#define STDIO_ALIAS <stdio.h>
#include STDIO_ALIAS

/* string form via a macro, and a string initializer wrapped in braces (C89 3.5.7) */
static char msg[] = { "computed include ok" };

int main() {
    printf("%s %d\n", msg, (int)sizeof(msg));
    return 0;
}
