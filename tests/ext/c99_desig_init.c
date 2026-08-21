/* EXT(c99): designated initializer for struct and array */
#include <stdio.h>
struct pt { int x, y, z; };
struct pt p = { .z = 9, .x = 3 };
int a[8] = { [2] = 5, [5] = 7 };
int main(void) {
    printf("%d %d %d %d %d\n", p.x, p.y, p.z, a[2], a[5]);
    return 0;
}
