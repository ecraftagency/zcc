/* C99-đủ đợt chót: digraphs + sizeof(VLA) runtime — differential vs cc */
#include <stdio.h>

%: define SIX 6          /* %: = #, digraph ở vị trí directive */
%:define M(a) %:a        /* %: stringize qua digraph */

int f(int n) <%          /* <% = { */
    int a<:n:>;           /* VLA + digraph [ ] */
    int m = n + 1;
    char c[n][3];         /* VLA elem là mảng hằng */
    return (int)(sizeof a + sizeof(c) + sizeof(int<:m:>) + sizeof(long)*0);
%>

int g(void) {
    int n = 3;
    int (*p)[n];          /* con trỏ tới VLA */
    p = 0;
    return (int)sizeof(*p);
}

int main(void) <%
    printf("%d %d %d %s %d\n", f(SIX), f(11), g(), M(ok), (int)sizeof(int[SIX]));
    return 0;
%>
