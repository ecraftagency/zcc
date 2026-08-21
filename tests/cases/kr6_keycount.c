/* K&R 6.3: count keywords — array of struct, binary search over struct, getword */
#include <stdio.h>
#include <string.h>
#include <ctype.h>
#define MAXWORD 100
struct key {
    char *word;
    int count;
} keytab[] = {
    {"auto", 0}, {"break", 0}, {"case", 0}, {"char", 0}, {"const", 0}, {"continue", 0},
    {"default", 0}, {"do", 0}, {"double", 0}, {"else", 0}, {"for", 0}, {"if", 0},
    {"int", 0}, {"return", 0}, {"void", 0}, {"while", 0}
};
#define NKEYS (sizeof keytab / sizeof keytab[0])
int getword(char *word, int lim) {
    int c;
    char *w = word;
    while (isspace(c = getchar()))
        ;
    if (c != EOF) *w++ = c;
    if (!isalpha(c)) {
        *w = '\0';
        return c;
    }
    for (; --lim > 0; w++)
        if (!isalnum(*w = getchar())) {
            *w = '\0';
            break;
        }
    *w = '\0';
    return word[0];
}
struct key *binsearch(char *word, struct key *tab, int n) {
    int cond;
    struct key *low = &tab[0];
    struct key *high = &tab[n];
    struct key *mid;
    while (low < high) {
        mid = low + (high - low) / 2;
        if ((cond = strcmp(word, mid->word)) < 0)
            high = mid;
        else if (cond > 0)
            low = mid + 1;
        else
            return mid;
    }
    return NULL;
}
int main(void) {
    char word[MAXWORD];
    struct key *p;
    unsigned i;
    while (getword(word, MAXWORD) != EOF)
        if (isalpha(word[0]) && (p = binsearch(word, keytab, NKEYS)) != NULL) p->count++;
    for (i = 0; i < NKEYS; i++)
        if (keytab[i].count > 0) printf("%4d %s\n", keytab[i].count, keytab[i].word);
    return 0;
}
