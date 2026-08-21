/* Memory traversal + branchy inner loop: sieve of Eratosthenes, repeated. */
#include <stdio.h>

#define LIM 3000000
static char is[LIM + 1];

int main(void) {
    long total = 0;
    int rep, i, j;
    for (rep = 0; rep < 8; rep++) {
        for (i = 0; i <= LIM; i++) is[i] = 1;
        is[0] = is[1] = 0;
        for (i = 2; (long)i * i <= LIM; i++)
            if (is[i])
                for (j = i * i; j <= LIM; j += i) is[j] = 0;
        for (i = 2; i <= LIM; i++) total += is[i];
    }
    printf("%ld\n", total);
    return 0;
}
