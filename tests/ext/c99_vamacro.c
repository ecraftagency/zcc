/* EXT(c99): standard variadic macro __VA_ARGS__ */
#include <stdio.h>
#define LOG(fmt, ...) printf("[log] " fmt, __VA_ARGS__)
int main(void) {
    LOG("%d\n", 1);
    LOG("%d %s\n", 2, "two");
    return 0;
}
