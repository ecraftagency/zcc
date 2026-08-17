/* EXT(c99): compound literal */
#include <stdio.h>
struct pt { int x, y; };
int sum(struct pt p) { return p.x + p.y; }
int main(void) {
    int *a = (int[]){ 10, 20, 30 };
    printf("%d %d\n", sum((struct pt){ 3, 4 }), a[2]);
    return 0;
}
