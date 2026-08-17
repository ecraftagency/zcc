/* EXT(c99): mixed declarations + declaration trong for-init */
#include <stdio.h>
int main(void) {
    printf("start\n");
    int x = 5; /* decl sau statement */
    for (int i = 0; i < 3; i++) printf("i=%d x=%d\n", i, x + i);
    int y = x * 2;
    printf("y=%d\n", y);
    return 0;
}
