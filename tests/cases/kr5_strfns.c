/* K&R 5.5 + ex5-3..5-5: pointer versions of strcpy/strcat/strncpy/strend */
#include <stdio.h>
#include <string.h>
void my_strcpy(char *s, char *t) {
    while ((*s++ = *t++))
        ;
}
void my_strcat(char *s, char *t) {
    while (*s) s++;
    while ((*s++ = *t++))
        ;
}
char *my_strncpy(char *s, char *t, int n) {
    char *r = s;
    while (n-- > 0 && *t) *s++ = *t++;
    while (n-- > 0) *s++ = '\0';
    *s = '\0';
    return r;
}
int strend(char *s, char *t) {
    int ls = strlen(s), lt = strlen(t);
    if (lt > ls) return 0;
    return strcmp(s + ls - lt, t) == 0;
}
int main(void) {
    char a[64], b[32];
    my_strcpy(a, "hello ");
    my_strcat(a, "world");
    printf("%s\n", a);
    my_strncpy(b, "1234567890", 4);
    printf("%s\n", b);
    printf("%d %d %d\n", strend(a, "world"), strend(a, "hello"), strend(a, a));
    return 0;
}
