#ifndef _SETJMP_H
#define _SETJMP_H
/* Darwin arm64: _JBLEN = 48 int (192 byte). Goi thang libc setjmp/longjmp. */
typedef int jmp_buf[48];
int setjmp(jmp_buf);
void longjmp(jmp_buf, int);
#endif
