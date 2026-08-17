/* EXT(gcc): ", ## __VA_ARGS__" — xóa phẩy khi arg rỗng */
#include <stdio.h>
#define LOG(fmt, ...) printf(fmt, ##__VA_ARGS__)
int main(void) {
    LOG("plain\n");
    LOG("%d\n", 3);
    LOG("%d %d\n", 4, 5);
    return 0;
}
