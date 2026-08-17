#ifndef _STDDEF_H
#define _STDDEF_H
#ifndef _ZCC_SIZE_T
#define _ZCC_SIZE_T
typedef unsigned long size_t;
#endif
typedef long ptrdiff_t;
typedef int wchar_t;
#define NULL ((void *)0)
#define offsetof(t, m) ((size_t)&((t *)0)->m)
#endif
