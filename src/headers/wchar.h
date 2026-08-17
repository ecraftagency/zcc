#ifndef _WCHAR_H
#define _WCHAR_H
typedef int wchar_t; /* trùng stddef.h — typedef overwrite, cùng kiểu */
typedef int wint_t;
#define WEOF (-1)
unsigned long wcslen(const wchar_t *);
#endif
