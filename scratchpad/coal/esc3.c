struct P { int x, y, z; };
void bump(struct P *p){ p->y += 100; p->z += 20; }
int main(void){
    struct P q; q.x = 1; q.y = 2; q.z = 3;
    bump(&q);
    return q.x + q.y + q.z;   /* must be 1 + 102 + 23 = 126 */
}
