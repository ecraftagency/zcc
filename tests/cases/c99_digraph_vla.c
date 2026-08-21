/* Final C99-completeness batch: digraphs + runtime sizeof(VLA) — differential vs cc */
#include <stdio.h>

%: define SIX 6          /* %: = #, digraph in directive position */
%:define M(a) %:a        /* %: stringize via digraph */

int f(int n) <%          /* <% = { */
    int a<:n:>;           /* VLA + digraph [ ] */
    int m = n + 1;
    char c[n][3];         /* VLA whose element is a constant array */
    return (int)(sizeof a + sizeof(c) + sizeof(int<:m:>) + sizeof(long)*0);
%>

int g(void) {
    int n = 3;
    int (*p)[n];          /* pointer to a VLA */
    p = 0;
    return (int)sizeof(*p);
}

int main(void) <%
    printf("%d %d %d %s %d\n", f(SIX), f(11), g(), M(ok), (int)sizeof(int[SIX]));
    return 0;
%>
