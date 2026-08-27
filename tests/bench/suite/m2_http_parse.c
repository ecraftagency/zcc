/* m2_http_parse — an NGINX-style HTTP request parser.
 *
 * WHY IT IS HERE.  nginx's `ngx_http_parse_request_line` and its header parser
 * are hand-written state machines of exactly this shape, and they are where a
 * web server's user-space CPU goes.  The shape is distinct from every other
 * member of this suite: a large switch over a state, byte loads, character
 * classification, case folding, and a per-header hash — branch-dense code with
 * no arithmetic to speak of, which is the opposite of the loops the suite is
 * otherwise made of.
 *
 * As with m1, the socket is not here: a syscall benchmark measures the kernel.
 */
#include <stdio.h>
#define BUF (1 << 20)
static unsigned char buf[BUF];

enum { S_METHOD, S_URI, S_VER, S_EOL, S_HNAME, S_HCOLON, S_HVALUE, S_HEOL, S_DONE };

static long parse(const unsigned char *p, long n) {
    long reqs = 0, hdrs = 0, uribytes = 0;
    unsigned long h = 5381;
    int st = S_METHOD;
    long i;
    for (i = 0; i < n; i++) {
        unsigned char c = p[i];
        switch (st) {
        case S_METHOD:
            if (c == ' ') st = S_URI;
            else h = h * 33 + c;
            break;
        case S_URI:
            if (c == ' ') st = S_VER;
            else { uribytes += (c == '%') ? 2 : 1; h ^= c; }
            break;
        case S_VER:
            if (c == '\r') st = S_EOL;
            break;
        case S_EOL:
            st = (c == '\n') ? S_HNAME : S_VER;
            break;
        case S_HNAME:
            if (c == ':') st = S_HCOLON;
            else if (c == '\r') st = S_DONE;
            else {
                unsigned char lc = (c >= 'A' && c <= 'Z') ? (unsigned char)(c | 0x20) : c;
                h = h * 31 + lc;
            }
            break;
        case S_HCOLON:
            if (c != ' ') { st = S_HVALUE; h ^= c; }
            break;
        case S_HVALUE:
            if (c == '\r') { hdrs++; st = S_HEOL; }
            else h += c;
            break;
        case S_HEOL:
            st = (c == '\n') ? S_HNAME : S_HVALUE;
            break;
        case S_DONE:
            if (c == '\n') { reqs++; st = S_METHOD; }
            break;
        }
    }
    return reqs * 1000003 + hdrs * 131 + uribytes + (long)(h & 0xffffff);
}

int main(void) {
    static const char *hdr[] = {
        "Host: example.com", "User-Agent: bench/1.0", "Accept: */*",
        "Accept-Encoding: gzip, deflate", "Connection: keep-alive",
        "Cache-Control: no-cache", "X-Forwarded-For: 10.0.0.1"
    };
    long n = 0;
    int k;
    for (k = 0; n < BUF - 512; k++) {
        n += (long)sprintf((char *)buf + n,
                           "GET /path/%d/resource?q=%d&lang=en HTTP/1.1\r\n", k % 997, k % 31);
        int j, nh = 3 + (k % 5);
        for (j = 0; j < nh; j++)
            n += (long)sprintf((char *)buf + n, "%s\r\n", hdr[(k + j) % 7]);
        buf[n++] = '\r'; buf[n++] = '\n';
    }
    long s = 0, r;
    for (r = 0; r < 40; r++) s += parse(buf, n - (s & 1));
    printf("%ld\n", s);
    return 0;
}
