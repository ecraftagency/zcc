/* limits.h — zcc đóng vai "limits.h của compiler" (như clang/gcc tự ship):
   định nghĩa ISO C rồi #include_next xuống limits.h của libc (POSIX, IOV_MAX…).
   Linux/glibc: glibc chỉ tự định nghĩa ISO khi !__GNUC__; với __GNUC__ nó chờ
   compiler đưa qua _GCC_LIMITS_H_ — zcc định nghĩa hộ rồi mới nhường glibc. */
#ifndef _ZCC_LIMITS_H
#define _ZCC_LIMITS_H
#define _GCC_LIMITS_H_ 1
#define CHAR_BIT 8
#define SCHAR_MIN (-128)
#define SCHAR_MAX 127
#define UCHAR_MAX 255
#ifdef __CHAR_UNSIGNED__
#define CHAR_MIN 0
#define CHAR_MAX UCHAR_MAX
#else
#define CHAR_MIN SCHAR_MIN
#define CHAR_MAX SCHAR_MAX
#endif
#define SHRT_MIN (-32768)
#define SHRT_MAX 32767
#define USHRT_MAX 65535
#define INT_MAX 2147483647
#define INT_MIN (-INT_MAX - 1)
#define UINT_MAX 4294967295U
#define LONG_MAX 9223372036854775807L
#define LONG_MIN (-LONG_MAX - 1L)
#define ULONG_MAX 18446744073709551615UL
#define LLONG_MAX 9223372036854775807LL
#define LLONG_MIN (-LLONG_MAX - 1LL)
#define ULLONG_MAX 18446744073709551615ULL
#endif
#include_next <limits.h>
