/* C99 6.5.15 + 6.5.2.2p6: operand mảng của ternary và arg variadic phải decay
   về con trỏ. Bug 18/8: type ternary giữ char[2] → arg thứ 9+ tràn stack bị
   store_narrow cắt 2 byte → glibc đọc con trỏ rác (git diff.c `? " " : ""`,
   segv theo layout ASLR). Case ép arg decay RƠI XUỐNG STACK (>8 GP). */
#include <stdio.h>

static char b[128];

int main(void) {
    unsigned long v = 3;
    int n;
    /* named 3 (buf,size,fmt) + 5 vararg đầu ăn hết x3-x7, v và ternary đi stack */
    n = snprintf(b, sizeof b, " %s%s%*s | %*lu%s", "P", "name", 5, "",
                 4, v, v ? "T" : "");
    printf("%d[%s]\n", n, b);
    /* ternary cả hai vế mảng, chọn vế phải + sizeof kiểm type decay = con trỏ */
    printf("%s %d\n", 0 ? "left" : "right", (int)sizeof(1 ? "ab" : "cdef"));
    /* mảng thật (không phải literal) làm vararg stack */
    {
        char arr[6] = "array";
        printf("%d %d %d %d %d %d %d %d %s\n", 1, 2, 3, 4, 5, 6, 7, 8, arr);
    }
    return 0;
}
