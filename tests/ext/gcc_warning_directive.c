/* EXT(gcc): #warning — reports to stderr then continues compiling */
#include <stdio.h>
#warning "experimental warning"
int main(void) {
    printf("ok\n");
    return 0;
}
