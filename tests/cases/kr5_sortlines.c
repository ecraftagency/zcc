/* K&R 5.6: sap xep dong text — mang con tro char*, qsort tu viet, swap */
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#define MAXLINES 100
#define MAXLEN 100
char *lineptr[MAXLINES];
int my_getline(char s[], int lim) {
    int c, i;
    i = 0;
    while (--lim > 0 && (c = getchar()) != EOF && c != '\n') s[i++] = c;
    if (c == '\n') s[i++] = c;
    s[i] = '\0';
    return i;
}
int readlines(char *lineptr[], int maxlines) {
    int len, nlines;
    char *p, line[MAXLEN];
    nlines = 0;
    while ((len = my_getline(line, MAXLEN)) > 0) {
        if (nlines >= maxlines || (p = malloc(len)) == NULL)
            return -1;
        else {
            line[len - 1] = '\0';
            strcpy(p, line);
            lineptr[nlines++] = p;
        }
    }
    return nlines;
}
void swap(char *v[], int i, int j) {
    char *temp;
    temp = v[i];
    v[i] = v[j];
    v[j] = temp;
}
void my_qsort(char *v[], int left, int right) {
    int i, last;
    if (left >= right) return;
    swap(v, left, (left + right) / 2);
    last = left;
    for (i = left + 1; i <= right; i++)
        if (strcmp(v[i], v[left]) < 0) swap(v, ++last, i);
    swap(v, left, last);
    my_qsort(v, left, last - 1);
    my_qsort(v, last + 1, right);
}
int main(void) {
    int nlines, i;
    if ((nlines = readlines(lineptr, MAXLINES)) >= 0) {
        my_qsort(lineptr, 0, nlines - 1);
        for (i = 0; i < nlines; i++) printf("%s\n", lineptr[i]);
        return 0;
    }
    printf("loi: input qua lon\n");
    return 1;
}
