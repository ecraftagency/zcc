/* A folded arithmetic operation has the TYPE OF ITS OPERANDS, and an unsigned
 * one wraps modulo 2^N at that width (C99 6.2.5/9, 6.3.1.3). Folding in a wider
 * accumulator and stopping there keeps a value no C type can hold, and the next
 * unsigned comparison reads the sign bit that should not be there.
 *
 * Measured (yarpgen s02611, 2026-08-27): `232U - 3008373104U` is 1286594424,
 * but folded as a raw i64 it is -3008372872, so
 *     (unsigned)(signed char)-22 > 232U - 3008373104U
 * folded FALSE where it is true. A whole `if` body never ran, a global kept its
 * initial value, and the checksum was wrong AT -O0 — no optimizer involved.
 */
int printf(const char *fmt, ...);

/* the shape that was wrong, in the contexts that force a fold */
static int guard = ((unsigned int)((int)(signed char)-22))
                 > (((unsigned int)((int)(unsigned char)232)) - 3008373104U);

enum { WRAPPED = (int)(232U - 3008373104U) };

/* an array size is a constant expression: this is 8 only if the subtraction
 * wraps at 32 bits (1286594424 >> 28 == 4, + 4) */
static char sized[((232U - 3008373104U) >> 28) + 4];

int main(void) {
    printf("guard   %d\n", guard);
    printf("enumval %d\n", WRAPPED);
    printf("size    %d\n", (int)sizeof sized);

    /* the same comparison as a live branch condition */
    if (((unsigned int)((int)(signed char)-22))
            > (((unsigned int)((int)(unsigned char)232)) - 3008373104U))
        printf("branch taken\n");
    else
        printf("branch NOT taken\n");

    /* each width wraps at its own width, signed and unsigned */
    printf("uc  %d\n", (int)(unsigned char)(200U + 100U));
    printf("us  %d\n", (int)(unsigned short)(60000U + 10000U));
    printf("ui  %u\n", (unsigned int)(4000000000U + 400000000U));
    printf("mul %u\n", (unsigned int)(65536U * 65536U));
    printf("shl %u\n", (unsigned int)(1U << 31) * 2U);

    /* an unsigned comparison whose operands only differ once truncated */
    printf("cmp1 %d\n", (0U - 1U) > 1U);
    printf("cmp2 %d\n", (int)((0U - 1U) < 1U));
    printf("cmp3 %d\n", (0UL - 1UL) > 1UL);

    /* division and modulo take the same path */
    printf("div %u\n", (unsigned int)(0U - 2U) / 3U);
    printf("mod %u\n", (unsigned int)(0U - 2U) % 7U);

    /* _Bool is != 0, never modular: (_Bool)0x100 is 1, not 0 */
    printf("bool %d\n", (int)(_Bool)0x100);
    printf("boolw %d\n", (int)(_Bool)(256U * 256U));

    /* signed narrowing still wraps into range */
    printf("sc  %d\n", (int)(signed char)(120 + 120));
    printf("ss  %d\n", (int)(short)(32000 + 32000));
    return 0;
}
