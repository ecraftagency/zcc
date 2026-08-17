#!/bin/sh
# M11 — nuốt header THẬT của SDK: pthread, socket, kqueue, signal từ
# $SDK/usr/include (không stub). Kiểm chứng cả layout (sizeof sockaddr_in,
# kevent) lẫn hành vi runtime (thread chạy, socket mở, kqueue mở, sigaction).
# So stdout với trọng tài cc; chạy 3 lần (bẫy ASLR).
set -e
cd "$(dirname "$0")/.."
cargo build
ZCC="$PWD/target/debug/zcc"
WORK="${1:-$(mktemp -d)}"

cat > "$WORK/gate.c" <<'EOF'
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <pthread.h>
#include <signal.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <sys/event.h>

static void *worker(void *arg) {
    int *n = arg;
    *n += 41;
    return arg;
}

int main(void) {
    pthread_t th;
    int v = 1;
    pthread_create(&th, 0, worker, &v);
    pthread_join(th, 0);
    printf("thread=%d\n", v);

    {
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        struct sockaddr_in a;
        memset(&a, 0, sizeof a);
        a.sin_family = AF_INET;
        a.sin_port = htons(8080);
        printf("socket=%d size=%d port=%d\n", fd >= 0, (int)sizeof a, ntohs(a.sin_port));
        close(fd);
    }
    {
        int kq = kqueue();
        printf("kqueue=%d evsize=%d\n", kq >= 0, (int)sizeof(struct kevent));
        close(kq);
    }
    {
        struct sigaction sa;
        memset(&sa, 0, sizeof sa);
        sa.sa_handler = SIG_IGN;
        sigaction(SIGPIPE, &sa, 0);
        printf("signal=ok\n");
    }
    return 0;
}
EOF

cc -w -O0 "$WORK/gate.c" -o "$WORK/gate.ref"
"$WORK/gate.ref" > "$WORK/ref.txt"
"$ZCC" "$WORK/gate.c" -o "$WORK/gate.zcc"
for i in 1 2 3; do
    "$WORK/gate.zcc" > "$WORK/out.txt"
    cmp "$WORK/ref.txt" "$WORK/out.txt"
done
echo "M11 PASS: header SDK thật (pthread/socket/kqueue/signal) — stdout khớp cc 3/3"
