/* The gcc/C99 surface that the git build requires — regression:
   __builtin_types_compatible_p constant-folded inside a git-style ARRAY_SIZE
   (BUILD_ASSERT_OR_ZERO = sizeof(char[1-2*!(cond)])), __extension__
   both before an expr and at the head of a statement (obstack.h),
   [restrict n] in an array parameter (SDK _regex.h when claiming C99),
   PRIuMAX/SCNuMAX (inttypes.h), __STDC_VERSION__ >= 199901L
   (the gate in git-compat-util.h). */
#include <stdio.h>
#include <inttypes.h>
#include <string.h>

#if __STDC_VERSION__ - 0 < 199901L
#error "must claim C99"
#endif

#define BUILD_ASSERT_OR_ZERO(cond) (sizeof(char [1 - 2*!(cond)]) - 1)
#define BARF_UNLESS_AN_ARRAY(arr) \
	BUILD_ASSERT_OR_ZERO(!__builtin_types_compatible_p(__typeof__(arr), \
							   __typeof__(&(arr)[0])))
#define ARRAY_SIZE(x) (sizeof(x) / sizeof((x)[0]) + BARF_UNLESS_AN_ARRAY(x))

static int tbl[7];

static int sum_first(int arr[restrict 3]) { return arr[0] + arr[1] + arr[2]; }

int main(void) {
	unsigned long i;
	uintmax_t big = 12345;
	for (i = 0; i < ARRAY_SIZE(tbl); i++)
		tbl[i] = (int) i;
	/* __extension__ before an expr */
	i = __extension__ ({ int t = tbl[6]; t + 1; });
	/* __extension__ at the head of a statement */
	__extension__ ({ tbl[0] = 9; (void) 0; });
	printf("n=%lu last+1=%lu first=%d\n", (unsigned long) ARRAY_SIZE(tbl),
	       i, tbl[0]);
	printf("sum=%d big=%" PRIuMAX "\n", sum_first(tbl), big);
	if (sscanf("777", "%" SCNuMAX, &big) == 1)
		printf("scn=%" PRIuMAX "\n", big);
	return 0;
}
