/* C89 2.2.4.2: float.h must provide the full set of macros — if any is
   missing, a user #if silently falls into the wrong branch (redis util.c
   double2ll — a real bug caught by runtest). */
#include <float.h>
int printf(const char *, ...);
int main(void)
{
    printf("%d %d %d %d\n", FLT_RADIX, FLT_MANT_DIG, DBL_MANT_DIG, LDBL_MANT_DIG);
    printf("%d %d %d\n", FLT_DIG, DBL_DIG, LDBL_DIG);
    printf("%d %d %d %d\n", FLT_MIN_EXP, FLT_MAX_EXP, FLT_MIN_10_EXP, FLT_MAX_10_EXP);
    printf("%d %d %d %d\n", DBL_MIN_EXP, DBL_MAX_EXP, DBL_MIN_10_EXP, DBL_MAX_10_EXP);
    printf("%d %d %d %d\n", LDBL_MIN_EXP, LDBL_MAX_EXP, LDBL_MIN_10_EXP, LDBL_MAX_10_EXP);
    printf("%d\n", FLT_ROUNDS);
#if (DBL_MANT_DIG >= 52) && (DBL_MANT_DIG <= 63)
    printf("mant ok\n");
#endif
    printf("%d %d\n", DBL_EPSILON == 2.2204460492503131e-16,
           DBL_MAX == 1.7976931348623157e+308);
    printf("%d %d\n", FLT_EPSILON == 1.19209290e-07F, DBL_MIN == 2.2250738585072014e-308);
    return 0;
}
