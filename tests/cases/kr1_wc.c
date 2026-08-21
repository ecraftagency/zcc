/* K&R 1.5.4 + ex1-13: count lines/words/characters + word-length histogram */
#include <stdio.h>
#define IN 1
#define OUT 0
int main(void) {
    int c, nl, nw, nc, state, len;
    int hist[16];
    int i, j;
    state = OUT;
    nl = nw = nc = len = 0;
    for (i = 0; i < 16; i++) hist[i] = 0;
    while ((c = getchar()) != EOF) {
        ++nc;
        if (c == '\n') ++nl;
        if (c == ' ' || c == '\n' || c == '\t') {
            if (state == IN && len < 16) ++hist[len];
            state = OUT;
            len = 0;
        } else if (state == OUT) {
            state = IN;
            len = 1;
        } else
            ++len;
    }
    if (state == IN && len < 16) ++hist[len];
    printf("%d %d %d\n", nl, nw = 0, nc);
    for (i = 1; i < 10; i++) {
        printf("%2d: ", i);
        for (j = 0; j < hist[i]; j++) putchar('*');
        putchar('\n');
    }
    return 0;
}
