#!/bin/sh
# M13 — nginx: clone source thật, configure + make bằng CC=zcc
# (--without-http_rewrite_module --without-http_gzip_module để né pcre/zlib),
# rồi chạy thật: master + worker, serve file tĩnh, curl so nội dung.
# File 200KB để đi qua đường ghi nhiều lần (writev), bắt lỗi truncation.
# Port 8971: dải registered ít đụng; nếu bận thì script fail sớm ở curl.
set -e
cd "$(dirname "$0")/.."
cargo build
ZCC="$PWD/target/debug/zcc"
WORK="${1:-$(mktemp -d)}"

if [ ! -d "$WORK/nginx" ]; then
    git clone --depth 1 https://github.com/nginx/nginx.git "$WORK/nginx"
fi
cd "$WORK/nginx"
CC="$ZCC" ./auto/configure --without-http_rewrite_module \
    --without-http_gzip_module > /dev/null
make -j8 > /dev/null

# prefix chạy thử độc lập, không đụng /usr/local
P="$WORK/prefix"
rm -rf "$P" && mkdir -p "$P/conf" "$P/logs" "$P/html"
echo "zcc serves nginx" > "$P/html/index.html"
head -c 200000 /dev/urandom > "$P/html/big.bin"
cat > "$P/conf/nginx.conf" <<'EOF'
worker_processes 1;
events { worker_connections 64; }
http {
  server {
    listen 127.0.0.1:8971;
    root html;
  }
}
EOF

./objs/nginx -p "$P" -c conf/nginx.conf
trap './objs/nginx -p "$P" -c conf/nginx.conf -s quit 2>/dev/null || true' EXIT
sleep 1

for i in 1 2 3; do
    [ "$(curl -s http://127.0.0.1:8971/)" = "zcc serves nginx" ]
done
curl -s http://127.0.0.1:8971/big.bin > "$P/big.out"
cmp "$P/html/big.bin" "$P/big.out"
# master + worker đều phải sống
[ "$(ps ax -o command | grep -c '^nginx:')" -ge 2 ]

echo "M13 PASS: nginx build bằng zcc — master+worker chạy, curl khớp (index 3/3 + 200KB)"
