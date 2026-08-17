/* computed include (C89 3.8.2): #include MACRO — không khớp hai dạng chuẩn
 * thì expand macro rồi dispatch lại (rax.c của redis dùng) */
#define STDIO_ALIAS <stdio.h>
#include STDIO_ALIAS

/* dạng string qua macro, và string init được bọc ngoặc (C89 3.5.7) */
static char msg[] = { "computed include ok" };

int main() {
    printf("%s %d\n", msg, (int)sizeof(msg));
    return 0;
}
