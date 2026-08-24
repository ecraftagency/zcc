struct S { unsigned a:3; unsigned b:5; int c:10; unsigned d:1; };
int rd(struct S* s){ return s->a + s->b + s->c + s->d; }
void wr(struct S* s, int v){ s->a=v; s->b=v>>1; s->c=v<<2; s->d=v&1; }
