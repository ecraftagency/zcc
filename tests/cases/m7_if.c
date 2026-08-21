#define A 2
#if A == 1
#error wrong branch must not be taken here
int x = unparseable if reached
#elif A == 2
#define R 10
#else
int y = also unparseable
#endif
#ifdef A
#define R2 5
#ifndef B
#define R3 1
#endif
#endif
#undef A
#ifdef A
int z = A is undefined now
#endif
#if defined(R) && !defined Q
#define R4 26
#endif
int main() { return R + R2 + R3 + R4; }
