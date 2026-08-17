#ifndef _UNISTD_H
#define _UNISTD_H
typedef long ssize_t;
typedef long off_t;
#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif
#define STDIN_FILENO 0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2
#define _SC_PAGESIZE 29 /* Darwin */
ssize_t read(int, void *, unsigned long);
ssize_t write(int, const void *, unsigned long);
int close(int);
off_t lseek(int, off_t, int);
int unlink(const char *);
int ftruncate(int, off_t);
char *getcwd(char *, unsigned long);
int chdir(const char *);
int access(const char *, int);
int dup(int);
int dup2(int, int);
int pipe(int *);
int isatty(int);
long sysconf(int);
int getpagesize(void);
int execvp(const char *, char *const *);
int fork(void);
unsigned int sleep(unsigned int);
int usleep(unsigned int);
int getpid(void);
int mkstemp(char *);
#endif
