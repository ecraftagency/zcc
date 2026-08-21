/* K&R 4.3: reverse Polish calculator — getop/getch/ungetch/push/pop + atof */
#include <stdio.h>
#include <stdlib.h>
#include <ctype.h>
#define MAXOP 100
#define NUMBER '0'
#define MAXVAL 100
#define BUFSIZE 100
int sp = 0;
double val[MAXVAL];
char buf[BUFSIZE];
int bufp = 0;
int getch(void) { return (bufp > 0) ? buf[--bufp] : getchar(); }
void ungetch(int c) {
    if (bufp >= BUFSIZE)
        printf("ungetch: too many characters\n");
    else
        buf[bufp++] = c;
}
void push(double f) {
    if (sp < MAXVAL)
        val[sp++] = f;
    else
        printf("loi: stack day\n");
}
double pop(void) {
    if (sp > 0) return val[--sp];
    printf("loi: stack rong\n");
    return 0.0;
}
int getop(char s[]) {
    int i, c;
    while ((s[0] = c = getch()) == ' ' || c == '\t')
        ;
    s[1] = '\0';
    if (!isdigit(c) && c != '.' && c != '-') return c;
    i = 0;
    if (c == '-') {
        c = getch();
        if (!isdigit(c) && c != '.') {
            ungetch(c);
            return '-';
        }
        s[++i] = c;
    }
    if (isdigit(c))
        while (isdigit(s[++i] = c = getch()))
            ;
    if (c == '.')
        while (isdigit(s[++i] = c = getch()))
            ;
    s[i] = '\0';
    if (c != EOF) ungetch(c);
    return NUMBER;
}
int main(void) {
    int type;
    double op2;
    char s[MAXOP];
    while ((type = getop(s)) != EOF) {
        switch (type) {
        case NUMBER:
            push(atof(s));
            break;
        case '+':
            push(pop() + pop());
            break;
        case '*':
            push(pop() * pop());
            break;
        case '-':
            op2 = pop();
            push(pop() - op2);
            break;
        case '/':
            op2 = pop();
            if (op2 != 0.0)
                push(pop() / op2);
            else
                printf("error: divide by 0\n");
            break;
        case '%':
            op2 = pop();
            if (op2 != 0.0)
                push((double)((long)pop() % (long)op2));
            else
                printf("error: divide by 0\n");
            break;
        case '\n':
            printf("\t%.8g\n", pop());
            break;
        default:
            printf("loi: lenh la %s\n", s);
            break;
        }
    }
    return 0;
}
