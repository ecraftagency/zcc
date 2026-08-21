#ifndef _STDARG_H
#define _STDARG_H
/* Standard AAPCS (Linux arm64): va_list = a 32-byte struct, anonymous args go in
   registers like named ones — the variadic prologue spills x0-x7/q0-q7 to the
   save area, and va_arg selects the area by the sign of gr_offs/vr_offs. The two
   builtins are real nodes in the parser/codegen. */
struct __zcc_va_list {
    void *__stack;
    void *__gr_top;
    void *__vr_top;
    int __gr_offs;
    int __vr_offs;
};
typedef struct __zcc_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, t) __builtin_va_arg(ap, t)
/* glibc protocol: a header (sys/syslog.h, stdio.h) #defines __need___va_list then
   #includes <stdarg.h>, expecting the COMPILER to supply __gnuc_va_list — without
   it, "__gnuc_va_list __ap" becomes an untyped parameter. */
typedef va_list __gnuc_va_list;
#define va_end(ap) ((void)(ap)) /* the arg must be evaluated — side effect (va-arg-21) */
#define va_copy(d, s) ((d) = (s))
#endif
