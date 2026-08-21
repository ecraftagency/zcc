/* EXT(gcc): ", ## __VA_ARGS__" — drop the comma when the arg is empty */
#include <stdio.h>
#define LOG(fmt, ...) printf(fmt, ##__VA_ARGS__)
int main(void) {
    LOG("plain\n");
    LOG("%d\n", 3);
    LOG("%d %d\n", 4, 5);
    return 0;
}
