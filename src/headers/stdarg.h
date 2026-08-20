#ifndef _STDARG_H
#define _STDARG_H
/* AAPCS chuan (Linux arm64): va_list = struct 32 byte, arg vo danh di reg nhu
   named — prologue variadic do x0-x7/q0-q7 xuong save area, va_arg chon vung
   theo dau cua gr_offs/vr_offs. Hai builtin la node that trong parser/codegen. */
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
/* glibc protocol: header (sys/syslog.h, stdio.h) #define __need___va_list roi
   #include <stdarg.h>, doi COMPILER cung cap __gnuc_va_list — thieu la
   "__gnuc_va_list __ap" thanh param khong kieu. */
typedef va_list __gnuc_va_list;
#define va_end(ap) ((void)(ap)) /* arg phai duoc eval — side effect (va-arg-21) */
#define va_copy(d, s) ((d) = (s))
#endif
