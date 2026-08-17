#ifndef _STDLIB_H
#define _STDLIB_H
#ifndef _ZCC_SIZE_T
#define _ZCC_SIZE_T
typedef unsigned long size_t;
#endif
#define NULL ((void *)0)
#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
#define RAND_MAX 2147483647
typedef struct { int quot; int rem; } div_t;
typedef struct { long quot; long rem; } ldiv_t;
void *malloc(size_t);
void *calloc(size_t, size_t);
void *realloc(void *, size_t);
void free(void *);
void exit(int);
void abort(void);
int atexit(void (*)(void));
int atoi(const char *);
long atol(const char *);
double atof(const char *);
double strtod(const char *, char **);
long strtol(const char *, char **, int);
unsigned long strtoul(const char *, char **, int);
long long strtoll(const char *, char **, int);
unsigned long long strtoull(const char *, char **, int);
float strtof(const char *, char **);
long double strtold(const char *, char **);
char *realpath(const char *, char *);
int rand(void);
void srand(unsigned int);
int abs(int);
long labs(long);
div_t div(int, int);
ldiv_t ldiv(long, long);
int system(const char *);
char *getenv(const char *);
void qsort(void *, size_t, size_t, int (*)(const void *, const void *));
void *bsearch(const void *, const void *, size_t, size_t,
              int (*)(const void *, const void *));
#endif
/* Darwin <sys/wait.h> — glibc kéo qua stdlib.h nên code hay dùng không include */
#define WIFEXITED(x) (((x) & 0177) == 0)
#define WEXITSTATUS(x) (((x) >> 8) & 0x000000ff)
