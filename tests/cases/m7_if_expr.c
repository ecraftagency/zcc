#if (3 + 4 * 2) == 11 && 10 / 3 == 3 && (1 << 4) == 16 && (7 & 3) == 3
#define OK 40
#endif
#if (0 || 2) && !(5 % 3 == 1) && (6 | 1) == 7 && (6 ^ 3) == 5 && -1 < 0 && ~0 == -1
#define OK2 1
#endif
#if 5 > 3 ? 1 : 0
#define OK3 1
#else
#define OK3 100
#endif
#if 0
#define OK4 100
#elif 8 >= 8
#define OK4 0
#elif 1
#define OK4 100
#endif
int main() { return OK + OK2 + OK3 + OK4; }
