/* m1_resp_parse — a REDIS protocol parser, byte-at-a-time state machine.
 *
 * WHY IT IS HERE.  The taxonomy suite is arithmetic and array loops; real
 * server software spends its CPU in protocol parsing, and that code has a shape
 * none of the other members has: a byte-at-a-time loop over a buffer, a switch
 * on the current state, narrow (8- and 16-bit) loads, unpredictable branches,
 * and integer accumulation from ASCII.  Redis's RESP is the smallest honest
 * example — `*3\r\n$3\r\nSET\r\n$5\r\nmykey\r\n$7\r\nmyvalue\r\n` — and its
 * inner loop is the same one `nginx`'s request parser runs.
 *
 * The IO itself is deliberately NOT here: a benchmark of read()/epoll measures
 * the kernel, not the compiler.  What is measured is the part that is actually
 * zcc's: the parse.
 */
#include <stdio.h>
#define BUF (1 << 20)
static unsigned char buf[BUF];

enum { S_TYPE, S_COUNT, S_BULKLEN, S_BULK, S_CR, S_LF };

static long parse(const unsigned char *p, long n) {
    long items = 0, bytes = 0, num = 0, want = 0;
    int st = S_TYPE, neg = 0;
    long i;
    for (i = 0; i < n; i++) {
        unsigned char c = p[i];
        switch (st) {
        case S_TYPE:
            if (c == '*' || c == '$') { st = (c == '*') ? S_COUNT : S_BULKLEN; num = 0; neg = 0; }
            else if (c == '+' || c == '-' || c == ':') { st = S_CR; }
            break;
        case S_COUNT:
        case S_BULKLEN:
            if (c >= '0' && c <= '9') { num = num * 10 + (c - '0'); }
            else if (c == '-') { neg = 1; }
            else if (c == '\r') {
                if (neg) num = -num;
                if (st == S_COUNT) { items += num; st = S_LF; }
                else { want = num; st = S_LF; }
            }
            break;
        case S_LF:
            st = (want > 0) ? S_BULK : S_TYPE;
            break;
        case S_BULK:
            bytes += c;
            if (--want == 0) st = S_CR;
            break;
        case S_CR:
            if (c == '\n') st = S_TYPE;
            break;
        }
    }
    return items * 131 + bytes;
}

int main(void) {
    /* a deterministic stream of SET commands with varying key/value lengths */
    long n = 0;
    int k;
    for (k = 0; n < BUF - 64; k++) {
        int kl = 3 + (k % 9), vl = 4 + (k % 27), j;
        n += (long)sprintf((char *)buf + n, "*3\r\n$3\r\nSET\r\n$%d\r\n", kl);
        for (j = 0; j < kl; j++) buf[n++] = (unsigned char)('a' + ((k + j) % 26));
        buf[n++] = '\r'; buf[n++] = '\n';
        n += (long)sprintf((char *)buf + n, "$%d\r\n", vl);
        for (j = 0; j < vl; j++) buf[n++] = (unsigned char)('A' + ((k * 3 + j) % 26));
        buf[n++] = '\r'; buf[n++] = '\n';
    }
    long s = 0, r;
    for (r = 0; r < 60; r++) s += parse(buf, n - (s & 1));
    printf("%ld\n", s);
    return 0;
}
