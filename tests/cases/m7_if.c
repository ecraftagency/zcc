#define A 2
#if A == 1
#error nhanh sai khong duoc vao day
int x = khong parse duoc dau
#elif A == 2
#define R 10
#else
int y = cung khong parse duoc
#endif
#ifdef A
#define R2 5
#ifndef B
#define R3 1
#endif
#endif
#undef A
#ifdef A
int z = da undef roi ma
#endif
#if defined(R) && !defined Q
#define R4 26
#endif
int main() { return R + R2 + R3 + R4; }
