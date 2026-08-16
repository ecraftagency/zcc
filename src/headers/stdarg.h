#ifndef _STDARG_H
#define _STDARG_H
/* Apple arm64: arg vo danh variadic nam tren stack caller, bat dau ngay sau
   frame record — __va_area__ la builtin cua zcc = x29 + 16 + 8*named_stack. */
typedef char *va_list;
#define va_start(ap, last) ((ap) = (va_list)__va_area__)
/* slot = sizeof lam tron 8; composite >16 byte truyen GIAN TIEP (con tro trong slot) */
#define va_arg(ap, t) (*(t *)(sizeof(t) > 16 \
    ? *(char **)(((ap) += 8) - 8) \
    : (((ap) += ((sizeof(t) + 7) & ~7UL)) - ((sizeof(t) + 7) & ~7UL))))
#define va_end(ap) ((void)0)
#define va_copy(d, s) ((d) = (s))
#endif
