/* K&R 6.5: dem tan suat tu bang cay nhi phan — malloc, struct tu tro, de quy */
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <ctype.h>
#define MAXWORD 100
struct tnode {
    char *word;
    int count;
    struct tnode *left;
    struct tnode *right;
};
char *my_strdup(char *s) {
    char *p = malloc(strlen(s) + 1);
    if (p != NULL) strcpy(p, s);
    return p;
}
struct tnode *talloc(void) { return malloc(sizeof(struct tnode)); }
struct tnode *addtree(struct tnode *p, char *w) {
    int cond;
    if (p == NULL) {
        p = talloc();
        p->word = my_strdup(w);
        p->count = 1;
        p->left = p->right = NULL;
    } else if ((cond = strcmp(w, p->word)) == 0)
        p->count++;
    else if (cond < 0)
        p->left = addtree(p->left, w);
    else
        p->right = addtree(p->right, w);
    return p;
}
void treeprint(struct tnode *p) {
    if (p != NULL) {
        treeprint(p->left);
        printf("%4d %s\n", p->count, p->word);
        treeprint(p->right);
    }
}
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
int main(void) {
    struct tnode *root;
    char word[MAXWORD];
    root = NULL;
    while (getword(word, MAXWORD) != EOF)
        if (isalpha(word[0])) root = addtree(root, word);
    treeprint(root);
    return 0;
}
