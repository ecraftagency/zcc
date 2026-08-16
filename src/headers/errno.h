#ifndef _ERRNO_H
#define _ERRNO_H
int *__error(void);
#define errno (*__error())
#define EDOM 33
#define ERANGE 34
#endif
