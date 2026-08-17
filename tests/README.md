# Sổ tài sản test của zcc

Nguyên tắc: **mọi tài sản test hoặc là script chạy được trong repo, hoặc được
ghi ở đây kèm cách dựng lại**. Không có tài sản mồ côi.

## Bản đồ 6 lớp proof (mỗi lớp một nền tảng toán, một cách discharge)

| # | Lớp | Nền tảng | Không gian | Gate |
|---|-----|----------|-----------|------|
| 1 | Lexer | ngôn ngữ chính quy, maximal munch, bảng phân loại literal 3.1.3.2 | bảng hữu hạn | `shape.sh lex` |
| 2 | Preprocessor | hệ viết lại hạng (terminating nhờ sơn xanh 3.8.3.4) | luật hữu hạn, hạng vô hạn | `cpp.sh` |
| 3 | Parser → AST | văn phạm phi ngữ cảnh + typedef context; cây declarator sâu ≤k hữu hạn | declarator vét được, còn lại vô hạn | `shape.sh decl` + differential (`run.sh`, suite ngoài) |
| 4 | Kiểu + UAC + const-eval | semilattice hữu hạn (3.2.1.5) | kiểu/op hữu hạn | `alg.sh` |
| 5 | Codegen -O0 | simulation per-node (mỗi node → mẫu lệnh cố định); layout = hàm đệ quy hữu hạn | layout vét được, codegen compositional | `shape.sh layout` + torture + đối chiếu `clang -S -O0` |
| 6 | ABI + system | automaton hữu hạn + hợp đồng toolchain | trạng thái hữu hạn | `abi.sh` + m9/m11/m13/m14 |

## Giới hạn trung thực (đọc trước khi tự hào)

"Vét cạn" ở đây LUÔN nghĩa là: vét cạn **không gian cấu trúc** (kiểu × op ×
trạng thái ABI × hình dạng declarator/struct), còn **không gian giá trị**
(2^64 mỗi toán hạng) chỉ lấy MẪU BIÊN (0, ±1, min, max, pattern giữa). Đó là
proof-by-enumeration cho phần cấu trúc + boundary testing cho phần giá trị —
KHÔNG phải chứng minh toàn phần. Nói "proof" trước công chúng thì phải kèm
câu này.

Mọi phép RÚT GỌN coverage phải kèm lý do soundness đọc được từ code:
- `abi.sh` chỉ quét đủ 0..8 cho counter LIÊN QUAN của kiểu (GPR cho int-kind,
  FPR cho float-kind), counter kia lấy {0, 8} — hợp lệ vì nhánh code
  placement của int-kind không đọc counter FPR và ngược lại (xem `call()`:
  hai nhánh if tách bạch theo `is_float`/`hfa`). Nếu mai này code trộn hai
  counter, PHẢI nâng lên tích đầy đủ 9×9.
- `alg.sh` bỏ `long long`: LP64 nó cùng biểu diễn/cùng đường codegen với
  `long` (TyTab map cùng size/align) — không thêm điểm mới vào không gian.
- `cpp.sh` họ mech không sinh tổ hợp tự động mà liệt kê TAY theo các điểm
  quyết định của thuật toán expansion — đủ dùng nhưng là chỗ yếu nhất của
  bộ gate; muốn nâng: sinh tổ hợp (định nghĩa × ngữ cảnh dùng) tự động.
- Trọng tài `cc` cũng có thể sai. Xác suất hai compiler độc lập sai GIỐNG
  NHAU tại cùng một điểm là nhỏ nhưng khác 0 — mọi diff bất thường phải
  đối chiếu spec trước khi kết luận zcc sai (đã có tiền lệ: generator sai
  chứ không phải compiler nào sai).

## Gate trong repo (tất cả là script, chạy từ repo root)

### Suite nền
- `run.sh` — mỗi case `cases/*.c` compile bằng cả `cc -std=c89 -w -O0`
  (trọng tài) lẫn zcc, chạy 2 binary, diff exit code + stdout.
  `run.sh ext` — như trên cho `ext/*.c` (trọng tài `cc` KHÔNG kèm `-std=c89`,
  vì đây là extension; luật decouple: xem CLAUDE.md).

### Gate khoa học (vét cạn không gian hữu hạn — chạy khi đụng vùng tương ứng)
- `abi.sh` + `gen_abi.py` — **lớp 6**. ABI mô hình hoá automaton: trạng thái
  (GPR 0..8, FPR 0..8, offset stack); 292 case = 14 kiểu × quét trạng thái +
  sentinel bắt lệch counter + cặp stack kề + variadic + quét RETURN (x0,
  cặp x0/x1, HFA v0..v3, sret x8). Mỗi call kiểm CẢ trực tiếp (bl) lẫn qua
  function pointer (blr). Link **CHÉO cc↔zcc cả hai chiều** + 2 control:
  lỗi ABI cùng-compiler hai đầu tự triệt tiêu, chỉ link chéo mới phơi.
  Thành tích: bắt 2 bug tàng hình suốt M8→M14 (packed-stack-args,
  HFA-tràn-khóa-nhầm-C.11) trong 1 lần chạy đầu.
- `shape.sh` (+`gen_lex.py`, `gen_decl.py`, `gen_layout.py`) — **lớp 1, 3, 5**:
  bảng phân loại integer literal (base × suffix × giá trị biên) + escape +
  maximal munch; đại số declarator sâu ≤3 (ptr/mảng/fn-ptr, có GỌI thật qua
  fn-ptr); layout struct/union/bitfield vét tổ hợp member ≤3 + offset từng
  member. Thành tích lần chạy đầu: bắt zcc từ chối multi-char constant 'ab'
  (3.1.3.4) + bitfield xen member thường layout lệch clang (12 vs 4 byte —
  đã viết lại theo ABI Itanium).
- `alg.sh` + `gen_alg.py` — **lớp 4**. Vét cạn (op × kiểu × kiểu × corner²):
  ~31k điểm runtime + 21k điểm fold. UB lọc TẠI GENERATOR (signed overflow,
  chia 0, INT_MIN/-1, shift tràn, float→int ngoài miền) vì tại điểm UB hai
  compiler đều "đúng" — diff vô nghĩa. 4 phép so: run zcc↔cc, fold zcc↔cc,
  và biểu đồ giao hoán fold↔runtime NỘI BỘ zcc (const-eval và codegen là hai
  đường từ cùng AST — phải gặp nhau).
- `cpp.sh` + `gen_cpp.py` — **lớp 2**. Preprocessor như hệ viết lại hạng:
  ma trận tương tác cơ chế expansion (prescan/paste/stringize/sơn-xanh/rescan,
  reify dạng chuẩn qua `#` thành string runtime — KHÔNG diff `-E` vì format
  đó không được spec đặc tả) + vét cạn số học `#if` (long/ulong, 3.8.1,
  oracle kép zcc==cc==python). Lý thuyết + danh sách LOẠI TRỪ
  (undefined/unspecified của C89): đọc đầu `gen_cpp.py`. Thành tích: bắt bug
  evaluator #if bỏ dấu unsigned (`-1L < 0xFF..UL` ra sai) ngay lần chạy đầu.

### Cross-target proof (M15 — arm64 ELF Linux, 2026-08-17)

Ba gate khoa học + hai suite nền chạy NGUYÊN XI trong box Debian 13 (gcc 14.2 /
binutils / glibc 2.41 làm trọng tài thay cc), zcc là binary Linux static
(cross-compile `aarch64-unknown-linux-musl`) đóng vai native compiler:

```
docker run --rm -v "$PWD":/repo:ro \
  -v "$PWD/target/aarch64-unknown-linux-musl/release/zcc":/usr/local/bin/zcc:ro \
  zcc-box bash -c 'cd /repo && ZCC=/usr/local/bin/zcc sh tests/abi.sh'   # (alg.sh, cpp.sh tương tự)
```

- abi.sh: **292 case × 4 hướng link chéo zcc↔gcc — 0 fail** (proof AAPCS chuẩn,
  gồm variadic-đi-reg + va_list struct 32B).
- alg.sh: **43036 điểm runtime + 21552 fold — PASS** (4 phép so, trọng tài gcc).
- cpp.sh: **mech 37 dòng + 1425 điểm #if — PASS**.
- cases 64/65 (fail duy nhất `float_h`: lệch chuẩn CÓ CHỦ ĐÍCH long double =
  double, gcc in LDBL quad); ext 14/14 (kể cả TLS 4-thread `tpidr_el0` +
  `__sync_*` LL/SC với pthread glibc thật).

### Gate milestone (integration — mỗi cái tự clone sạch, tự kiểm chứng)
- `m8.sh` — zcc compile tcc (amalgam) → tcc compile tcc → compile hello
  (kiểm chứng bắc cầu, thay self-hosting).
- `m9.sh` — build tcc bằng Makefile GỐC (nhiều .c → .o → link) qua `CC=zcc`.
- `m11.sh` — nuốt header THẬT của SDK (pthread, socket, kqueue, signal),
  chạy socket + kqueue thật.
- `m12.sh` — atomics: 4 thread × 100k fetch_add + spinlock CAS + macro
  `atomicvar.h` trích nguyên văn redis.
- `m13.sh` — nginx: clone sạch → configure + make CC=zcc → serve → curl
  khớp từng byte (kể cả file 200KB).
- `m14.sh` — redis: clone sạch → make CC=zcc MALLOC=libc (deps vendored đi
  qua zcc hết) → PING/SET/GET/INCR đúng. **Cúp giai đoạn 2.**

## Suite công nghiệp ngoài — `tests/suites/*.sh` (trụ 2)

Harness trong repo, source suite cache ở `~/.cache/zcc-suites`
(`ZCC_SUITE_CACHE` đổi được; mất cache thì clone lại theo bảng). Khuôn chung:
referee-filter (case `cc -std=c89` không nhận = ngoài scope → skip),
differential exit+stdout, **gate = tập FAIL ⊆ baseline `*.known-fail`** đã
triage từng dòng. CHẠY TUẦN TỰ từng suite — mỗi cái đã ăn đủ 8 core, chạy đè
nhau là nghẽn (đã trả học phí).

| Suite | Nguồn (clone --depth 1) | Điểm 2026-08-17 | Ghi chú triage baseline |
|---|---|---|---|
| `torture.sh` | gcc-mirror/gcc (sparse `gcc/testsuite/gcc.c-torture/execute`) | 1389 pass, 213 skip, 92 fail baselined / 1694 | fail toàn ext ngoài scope: 28 vector_size, 21 _Complex, 25 `__builtin_*` (abort/…_overflow/llabs-as-builtin), 4 VLA, 4 attribute corner, 3 predef macro gcc (`__FLT_MIN__`…), 2 hex-float C99, case range, `{}` rỗng, wchar, 1 source Latin-1 (zcc đòi UTF-8). Lần chạy đầu bắt 2 bug THẬT đã fix: pr60017 (global init bitfield phát trọn container → đè member chen byte trống — Itanium), pr33631 (designator không bị tiêu thụ khi elision descend → parser lặp VÔ HẠN; kéo theo fix `[1] = 5` mảng lồng apply desig 2 lần) |
| `cts.sh` | c-testsuite/c-testsuite | 218/220 | 00162 = C99 array-qualifier `[const 5]`; 00219 = C11 `_Generic` |
| `nora.sh` | nlsandler/writing-a-c-compiler-tests | 555 pass, 137 skip, 0 fail | skip = referee c89 từ chối (chương ngoài C89) |
| `chibicc.sh` | rui314/chibicc | 14 pass, 24 skip, 3 fail | string = C11 `u""`; typeof trần = TỪ CHỐI CÓ CHỦ ĐÍCH (va tên biến C89 — xem parser.rs); vla ngoài subset; skip gồm cả test referee tự fail trên arm64 (oracle vô hiệu) |
| `kr.sh` | caisah/K-and-R-exercises-and-examples | 87 pass, 30 skip, 11 fail | fail = UB của CHÍNH bài tập (scanf fail → đọc biến chưa init, itoa off-by-one, fsize đọc directory) — rác stack đổi theo run nên baseline so THEO TÊN, hội tụ sau vài lần chạy; mọi entrant mới phải triage tay trước khi thêm |
| `tcc.sh` | TinyCC/tinycc (tests2 chạy trên tcc DO ZCC BUILD — bắc cầu mức suite) | 108 pass, 31 skip, 0 fail | trọng tài kép: tcc(cc) không tái tạo được .expect (case cần args/stdin của Makefile tcc, hoặc tự segfault trên arm64) → skip; baseline rỗng ngay lần chạy đầu |

Nguyên tắc baseline: mỗi dòng known-fail phải có lời giải thích trong bảng
trên (hoặc commit message thêm nó). Baseline KHÔNG phải thùng rác giấu bug —
fail mới không giải thích được = bug zcc cho tới khi chứng minh ngược lại.

## Suite chính chủ nginx/redis (trụ 3 — phần mềm thật tự kiểm chứng chính nó)

- **nginx-tests** (github.com/nginx/nginx-tests, 2026-08-17, zcc hiện hành,
  log full lưu `~/.cache/zcc-suites/nginx-tests-prove-*.log` — nợ rerun ĐÃ TRẢ):
  - Build minimal (M13: without rewrite/gzip): **493 file / 1136 test, All
    tests successful, 73s** (`prove -j4`); 397 file skip vì thiếu module.
  - Build FULL (`--with-http_ssl_module --with-http_v2_module --with-stream
    --with-stream_ssl_module --with-mail --with-mail_ssl_module
    --with-http_realip_module --with-http_sub_module`, link pcre2 + openssl@3
    homebrew — zcc nuốt trọn header openssl/pcre2, cấm khẩu không cần vá gì):
    **493 file / 5346 test**, fail chỉ gom trong nhóm SSL; triage bằng trọng
    tài nginx-cc CÙNG config chạy CÙNG danh sách file: **tập fail-chỉ-zcc =
    RỖNG** — 19 file fail giống hệt cả hai binary (openssl 3.6.1 lệch kỳ vọng
    suite: session reuse, chacha prio, stapling…), phần còn lại là flake tạo
    cert của harness khi không có tty (`openssl req` "Inappropriate ioctl",
    đổi nạn nhân ngẫu nhiên theo run) + 2 file `*_ssl_conf_command.t` treo
    với CẢ cc. Bài học: `prove -j4` làm flake cert nặng thêm — triage SSL
    phải chạy tuần tự.
- **redis tests** (`./runtest` chính chủ, lịch 156 unit, 2026-08-17):
  - Run 1 (--clients 4, build `__thread` no-op): chạy tới 147+/156 thì crash
    tại unit io-threads ≥ 2 — đúng nợ TLS; trả nợ TLS thật (M14) mở khóa.
  - Run 2 FULL (--clients 8, build TLS thật): **156/156 unit, 6959 ok / 6 err**
    — 6 err ("Timeout waiting for blocked clients", 5 unit/type/list +
    1 cluster/slot-stats) tưởng flake tải nhưng TÁI HIỆN SOLO; trọng tài
    redis-cc cùng source chạy solo PASS SẠCH → luật "fail mới = bug zcc" nghiệm
    đúng lần 2. Thủ phạm: **`ceill` không prototype trong math.h nhúng** →
    implicit-int đọc x0 thay d0 → `BLPOP key <giây>` lưu timeout=0 = block
    vĩnh viễn (mọi lệnh block theo GIÂY đi qua long double + `ceill` của
    `getTimeoutFromObjectOrReply`; đường milliseconds đi integer nên thoát).
    Fix: map họ `*l` về bản double trong math.h nhúng (đóng lỗ soundness của
    lệch chuẩn long double=double; trên ELF còn né ABI fp128 glibc) + ext test
    `c99_ldbl_mathl.c`. Rerun sau fix: unit/type/list **295 ok/0 err**,
    slot-stats **73 ok/0 err**.
  - Run 3 ĐÓNG SỔ (--clients 8, sau fix ceill): **156/156 unit, 7274 ok /
    0 err / 0 exception — "All tests passed without errors!"** Suite chính
    chủ redis PASS TUYỆT ĐỐI, không cần baseline.
  - Suite này tổng cộng bắt 2 bug thật: float.h thiếu `DBL_MANT_DIG` (Lua
    double2ll trả 0, fix 3f67a0e) + ceill trên. Bài học kit: socket path phải
    ngắn (`sun_path` 104 trên macOS — chạy từ `/tmp/zr.*`), test modules phải
    build `CC=zcc LD=zcc LIBS=` TRƯỚC `make -C src all` (Makefile modules gọi
    `ld` trần với SDK path chết — fail cả với cc stock), cần `make all`
    (redis-check-aof/rdb là copy của redis-server). Triage nhanh: `./runtest
    --single <unit> --only "<tên test>"` — KHÔNG chạy full suite khi mổ.
    CHÚ Ý khi đọc log: `integration/logging.tcl` CỐ Ý crash server để test
    crash report — block `REDIS BUG REPORT` từ unit đó là hành vi đúng; grep
    đếm err phải bóc mã màu ANSI trước (`sed 's/\x1b\[[0-9;]*m//g'`).

## Rổ M17 — probe theo coverage-per-LOC (2026-08-17, sau khi redis đóng sổ)

- **sqlite 3.50.4 amalgamation** (`sqlite-amalgamation-3500400`, cache
  `~/.cache/zcc-suites/sqlite`): `zcc -c sqlite3.c` (262 899 dòng) compile
  SẠCH ngay nhát đầu, 4.3s, **zero error, zero LOC phát sinh** — dự báo
  zero-cost nghiệm đúng. CLI `sqlite3` (sqlite3.c + shell.c) build + chạy:
  CREATE/INSERT/INDEX/aggregate/LIKE/PRAGMA integrity_check = ok.
  **Differential vs cc-built cùng source**: workload CTE đệ quy 5000 dòng +
  JOIN + window function + JSON + view + trigger + UPDATE/DELETE + VACUUM +
  integrity_check → **31 dòng output khớp từng byte**. Suite chính chủ TCL
  (make test/testfixture): CHƯA chạy được trên mac — không có tcl dev
  (tclConfig.sh); đường trả nợ: box Linux + apt tcl + zcc-ELF (ghi sổ nợ).

- **git 2.55.GIT** (clone depth-1, cache `~/.cache/zcc-suites/git`):
  `make CC=zcc NO_GETTEXT=1 NO_TCLTK=1 FSMONITOR_DAEMON_BACKEND=
  FSMONITOR_OS_SETTINGS=` (fsmonitor darwin cần Objective-C/CoreFoundation —
  ngoài phạm vi C compiler, tắt bằng knob chuẩn của Makefile git, không sửa
  build file) → **binary `git` 10.9MB link xong**. Smoke: version, init,
  2×commit, log, diff --stat, fsck sạch, status. Suite chính chủ `t/`:
  t0000-basic **92/92**, t0001-init **103/103**, t3600-rm **81/82** (fail
  duy nhất = "TODO known breakage" của chính git, expected fail, không phải
  bug zcc); batch 11 file subsystem pass sạch **1788 test**: t0002-gitfile
  14, t1300-config 518, t1400-update-ref 316, t2200-add-update 20,
  t3200-branch 171, t3903-stash 143, t4001-diff-rename 23, t4014-format-patch
  221, t5510-fetch 239, t6402-merge-rename 46, t7501-commit 77 (stash/
  format-patch "passed all remaining" = có known-breakage TODO của git, như
  t3600). Tổng t/ đã chạy: **2064 pass / 0 fail thật**. Regression trong
  repo: `cases/c89_scope_git.c` (3 bug C89) + `ext/gcc_git_compat.c` (bề mặt
  gcc git đòi). Gate đóng sổ sau các sửa parser: shape 910 + abi 292×4 +
  alg 43036/21552 + cpp 1425 + m12 + m14 — PASS hết, cases 66/66, ext 16/16.
  Giá phải trả (test-first, mỗi mục có file git thật đòi): xưng
  `__STDC_VERSION__ 199901L` (git-compat-util.h #error nếu thiếu — tiền lệ
  xưng __GNUC__ 4.2.1 cho SDK), `[restrict n]` trong array param (C99
  6.7.5.3p21, SDK _regex.h mở nhánh C99), họ PRI*/SCN* MAX vào inttypes.h,
  `__builtin_types_compatible_p` fold hằng (ARRAY_SIZE/BUILD_ASSERT_OR_ZERO),
  `__extension__` trước expr + đầu stmt (obstack.h), và **3 bug C89 thuần
  của zcc bị git phơi ra**: (1) tên global phải vào scope TRƯỚC initializer
  (LIST_HEAD: `static struct x = { &x, &x }`); (2) ident sau specifier trong
  declarator LUÔN là tên kể cả trùng typedef (param `report_fn` trùng
  typedef report_fn của usage.h); (3) locals thừa của hàm trước rò vào
  resolve tên ở ginit toàn cục (param `index_only` của check_local_mod đè
  global cùng tên → "cần biểu thức hằng" ma ở builtin/rm.c). Tổng +62 LOC.

## Bẫy đã trả học phí (đọc trước khi debug "ma")

- **Lỗi ABI cùng-compiler tự triệt tiêu** — nginx/redis chạy ngon suốt vẫn
  sai ABI; chỉ `abi.sh` (link chéo) phơi ra. Đừng lấy integration test làm
  bằng chứng đúng ABI.
- **Thuật toán offset arg sống ở 3 nơi phải khớp từng byte**: codegen
  `call()`, codegen spill prologue, parser `va_off`. Sửa 1 nơi = sửa 3 nơi,
  rồi chạy `abi.sh`.
- **Implicit-int cắt pointer/double**: libc thiếu prototype trong header
  nhúng → return int → hai biến thể: (1) pointer bị sxtw cắt 64-bit →
  segfault PHỤ THUỘC ASLR (getprogname, lldb tắt ASLR nên không repro —
  heisenbug đúng nghĩa đen); (2) return double nằm ở d0 nhưng caller đọc x0
  rác → giá trị sai LẶNG LẼ, không crash (ceill → redis BLPOP timeout=0 =
  block vĩnh viễn, fail deterministic mà nhìn như flake timing). Nghi bug
  kiểu này: `nm -u` binary, đối chiếu từng symbol với `src/headers/*.h`.
- **Stale .o thế hệ cũ** trong cây build ngoài → lỗi link "ma" (duplicate/
  undefined không giải thích được). Luôn `make distclean` + `rm src/*.o`
  trước khi debug lỗi link. (Mandelbug: phụ thuộc LỊCH SỬ build, không phụ
  thuộc source.)
- **Diff tại điểm UB là vô nghĩa** — mọi generator differential phải lọc UB
  trước, nếu không sẽ đuổi theo "bug" không tồn tại.
