#ifndef _DLFCN_H
#define _DLFCN_H
#define RTLD_LAZY 0x1
#define RTLD_NOW 0x2
#define RTLD_LOCAL 0x4
#define RTLD_GLOBAL 0x8
#define RTLD_DEFAULT ((void *)-2)
#define RTLD_NEXT ((void *)-1)
void *dlopen(const char *, int);
int dlclose(void *);
void *dlsym(void *, const char *);
char *dlerror(void);
#endif
