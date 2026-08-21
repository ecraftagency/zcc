/* K&R 3.5-3.6 + ex3-4..3-6: atoi, itoa (handles INT_MIN), reverse */
#include <stdio.h>
#include <string.h>
#include <ctype.h>
#include <limits.h>
int my_atoi(char s[]) {
    int i, n, sign;
    for (i = 0; isspace(s[i]); i++)
        ;
    sign = (s[i] == '-') ? -1 : 1;
    if (s[i] == '+' || s[i] == '-') i++;
    for (n = 0; isdigit(s[i]); i++) n = 10 * n + (s[i] - '0');
    return sign * n;
}
void reverse(char s[]) {
    int c, i, j;
    for (i = 0, j = strlen(s) - 1; i < j; i++, j--) {
        c = s[i];
        s[i] = s[j];
        s[j] = c;
    }
}
void my_itoa(int n, char s[]) {
    int i, sign;
    unsigned un;
    sign = n;
    un = (n < 0) ? -(unsigned)n : (unsigned)n;
    i = 0;
    do {
        s[i++] = un % 10 + '0';
    } while ((un /= 10) > 0);
    if (sign < 0) s[i++] = '-';
    s[i] = '\0';
    reverse(s);
}
int main(void) {
    char buf[32];
    printf("%d %d %d\n", my_atoi("  -1234"), my_atoi("+99"), my_atoi("42abc"));
    my_itoa(-987654, buf);
    printf("%s\n", buf);
    my_itoa(INT_MIN, buf);
    printf("%s\n", buf);
    my_itoa(0, buf);
    printf("%s\n", buf);
    strcpy(buf, "a cat");
    reverse(buf);
    printf("%s\n", buf);
    return 0;
}
