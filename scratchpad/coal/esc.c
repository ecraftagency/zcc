#include <stdio.h>
struct P { int x, y; };
void bump(struct P *p){ p->x += 100; }
int main(void){
    struct P q; q.x = 1; q.y = 2;
    bump(&q);              /* mutates q.x through a pointer */
    printf("%d\n", q.x);   /* must be 101 */
    return 0;
}
