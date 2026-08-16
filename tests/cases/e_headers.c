#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <ctype.h>
#include <limits.h>
#include <assert.h>
int cmp(const void *a, const void *b) {
    return *(const int *)a - *(const int *)b;
}
int main(void) {
    char buf[64];
    int *v;
    int arr[6];
    int i;
    strcpy(buf, "xin ");
    strcat(buf, "chao");
    printf("%s %lu %d\n", buf, strlen(buf), strcmp(buf, "xin chao"));
    sprintf(buf, "%d-%d", 40, 2);
    printf("%s | %d %ld\n", buf, atoi("123"), atol("-456"));
    printf("%d %d %d %c\n", isdigit('7'), isalpha('7'), isspace(' '), toupper('q'));
    printf("%d %u %d\n", INT_MAX, UINT_MAX, CHAR_MAX);
    v = (int *)malloc(3 * sizeof(int));
    v[0] = 7; v[1] = 8; v[2] = 9;
    printf("%d\n", v[0] + v[1] + v[2]);
    free(v);
    arr[0] = 5; arr[1] = 3; arr[2] = 9; arr[3] = 1; arr[4] = 7; arr[5] = 2;
    qsort(arr, 6, sizeof(int), cmp);
    for (i = 0; i < 6; i++) printf("%d", arr[i]);
    printf("\n");
    assert(arr[0] == 1);
    assert(strchr("abcdef", 'd') != NULL);
    memset(buf, 'z', 4);
    buf[4] = 0;
    printf("%s %d\n", buf, memcmp("abc", "abd", 3) < 0);
    return 0;
}
