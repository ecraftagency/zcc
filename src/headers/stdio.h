#ifndef _STDIO_H
#define _STDIO_H
#ifndef _ZCC_SIZE_T
#define _ZCC_SIZE_T
typedef unsigned long size_t;
#endif
typedef struct __sFILE FILE;
extern const int sys_nerr; /* nginx dò NGX_SYS_NERR qua biến này */
extern FILE *__stdinp;
extern FILE *__stdoutp;
extern FILE *__stderrp;
#define stdin __stdinp
#define stdout __stdoutp
#define stderr __stderrp
#define NULL ((void *)0)
#define EOF (-1)
#define BUFSIZ 1024
#define FILENAME_MAX 1024
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
int printf(const char *, ...);
int fprintf(FILE *, const char *, ...);
int sprintf(char *, const char *, ...);
int vprintf(const char *, char *);
int vfprintf(FILE *, const char *, char *);
int vsnprintf(char *, unsigned long, const char *, char *);
FILE *fdopen(int, const char *);
int vsprintf(char *, const char *, char *);
int scanf(const char *, ...);
int fscanf(FILE *, const char *, ...);
int sscanf(const char *, const char *, ...);
int getchar(void);
int getc(FILE *);
int fgetc(FILE *);
char *fgets(char *, int, FILE *);
char *gets(char *);
int putchar(int);
int putc(int, FILE *);
int fputc(int, FILE *);
int puts(const char *);
int fputs(const char *, FILE *);
int ungetc(int, FILE *);
FILE *fopen(const char *, const char *);
FILE *freopen(const char *, const char *, FILE *);
int fclose(FILE *);
int fflush(FILE *);
size_t fread(void *, size_t, size_t, FILE *);
size_t fwrite(const void *, size_t, size_t, FILE *);
int fseek(FILE *, long, int);
long ftell(FILE *);
void rewind(FILE *);
int feof(FILE *);
int ferror(FILE *);
void clearerr(FILE *);
int remove(const char *);
int rename(const char *, const char *);
FILE *tmpfile(void);
char *tmpnam(char *);
void perror(const char *);
void setbuf(FILE *, char *);
#endif
