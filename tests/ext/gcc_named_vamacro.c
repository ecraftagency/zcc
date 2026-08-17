/* EXT(gcc): named variadic macro "#define F(args...)" */
#include <stdio.h>
#define LOG(fmt, args...) printf(fmt, args)
int main(void) {
    LOG("%d %d\n", 4, 5);
    return 0;
}
