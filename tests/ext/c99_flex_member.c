/* EXT(c99): flexible array member — sizeof struct không tính mảng đuôi */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
struct msg { int len; char data[]; };
int main(void) {
    struct msg *m = malloc(sizeof(struct msg) + 16);
    m->len = 3;
    strcpy(m->data, "abc");
    printf("%d %s %d\n", m->len, m->data, (int)sizeof(struct msg));
    return 0;
}
