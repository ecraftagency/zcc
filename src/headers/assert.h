#ifndef _ZCC_ASSERT_DECLS
#define _ZCC_ASSERT_DECLS
int printf(const char *, ...);
void abort(void);
#endif
/* assert.h co the include lai voi NDEBUG khac — dinh nghia lai moi lan */
#undef assert
#ifdef NDEBUG
#define assert(e) ((void)0)
#else
#define assert(e) ((e) ? 0 : (printf("Assertion failed: %s\n", #e), abort(), 0))
#endif
