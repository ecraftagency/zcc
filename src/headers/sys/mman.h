#ifndef _SYS_MMAN_H
#define _SYS_MMAN_H
/* gia tri Darwin <sys/mman.h> */
#define PROT_NONE 0x0
#define PROT_READ 0x1
#define PROT_WRITE 0x2
#define PROT_EXEC 0x4
#define MAP_SHARED 0x0001
#define MAP_PRIVATE 0x0002
#define MAP_FIXED 0x0010
#define MAP_ANON 0x1000
#define MAP_ANONYMOUS MAP_ANON
#define MAP_JIT 0x0800
#define MAP_FAILED ((void *)-1)
void *mmap(void *, unsigned long, int, int, int, long);
int munmap(void *, unsigned long);
int mprotect(void *, unsigned long, int);
#endif
