/* C99 6.3.1.2: conversion to _Bool = (value != 0) ? 1 : 0 — NOT a modular
   narrowing. Regression: the const-fold cast to _Bool once behaved as `v as u8`
   (size 1 + unsigned), so (_Bool)0x100 yielded 0 (low byte 0). yarpgen seeds
   59/62/96/145/178/187/229/262/275. */
int printf(char *fmt, ...);

int id(int x) { return x; } /* block folding: force the runtime path */

int main() {
    /* constants: exercise the parser const-fold path */
    printf("%d %d %d %d\n", (_Bool)256, (_Bool)0x666B00, (_Bool)0x10000, (_Bool)0);
    /* inside an if condition (the exact yarpgen case) */
    printf("%d %d\n", ((_Bool)6712832) ? 1 : 0, ((_Bool)0) ? 1 : 0);
    /* runtime: exercise the codegen ext(BOOL) path */
    printf("%d %d %d\n", (_Bool)id(256), (_Bool)id(0x10000), (_Bool)id(0));
    /* long with low 32 bits = 0 */
    printf("%d %d\n", (_Bool)0x100000000L, (_Bool)id(0));
    return 0;
}
