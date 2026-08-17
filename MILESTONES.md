# zcc — thang milestone, thành tích, sổ nợ

(Hiến chương/luật ở `CLAUDE.md`; sổ tài sản test ở `tests/README.md`. File này
là SỬ KÝ: đi tuần tự, không nhảy cóc; mỗi milestone chốt bằng gate lặp lại được.)

## Giai đoạn 1 — C89 thuần (M0–M8, ĐẠT trọn 2026-08-17)

- **M0**: `int main() { return N; }` → .s → `cc` link → exit code đúng. Chứng minh toàn pipeline.
- **M1**: biểu thức `+ - * / %`, ngoặc, unary, so sánh — vẫn chỉ trong return.
- **M2**: biến local (stack slot), `=`, `if/else`, `while`, `for`, block.
- **M3**: định nghĩa + gọi hàm nhiều tham số, đệ quy (fib chạy được).
- **M4**: con trỏ, `&` `*`, mảng, pointer arithmetic, `int`/`char`/`long` + sizeof.
- **M5**: string literal, `char *`, gọi `printf` (nhớ luật varargs-lên-stack). Từ đây test diff được stdout.
- **M6**: struct/union, typedef, enum, global variables, initializer.
- **M7**: preprocessor C89 đầy đủ (`#include #define #if...` — macro expansion/rescan là boss thật của cả dự án).
- **M8** (cúp): compile được `chibicc` hoặc `tcc`, binary sinh ra compile được hello world (kiểm chứng bắc cầu — thay cho self-hosting vì Rust không self-host được). ĐẠT 2026-08-17, lặp lại bằng `tests/m8.sh`.

## Giai đoạn 2 — C89+ (đích: nginx + redis trên M1 Mach-O, ĐẠT trọn 2026-08-17)

"C89+" = giữ khung C89, cherry-pick đúng phần C99/C11/GCC-extension mà nginx/redis đạp phải. KHÔNG claim C99. Mỗi milestone: suite cũ (run.sh, m8.sh) phải giữ xanh.

- **M9 — driver đúng chuẩn cc** (điều kiện sống còn để tích hợp toolchain): nhiều input một lệnh (`zcc a.c b.o c.o -o app` — compile phần .c, forward tất cả .o cho ld), `-l`/`-L`, `-U`, `-MMD -MF` (make của redis cần), `-v`, exit code + diagnostic format `file:line:` chuẩn (configure grep stderr), flag lạ nuốt im lặng nhưng KHÔNG nuốt nhầm flag có tham số đi kèm (`-o` `-I`…). **Gate: build tcc bằng chính Makefile gốc của nó (không ONE_SOURCE) — nhiều .c → .o → link — rồi m8.sh vẫn pass.** ĐẠT 2026-08-17, lặp lại bằng `tests/m9.sh` (hack `-Dinline=__inline` hết cần).
- **M10 — ngôn ngữ C89+**: mixed declarations + decl trong `for(...)`, `long long` thật + `_Bool`, variadic macros `__VA_ARGS__` (+ named `args...`, `,##__VA_ARGS__`), `__typeof__`, flexible array member, `inline`/`__restrict`/`__extension__` no-op, `__attribute__` parse-skip + honor `aligned`/`packed`, `__builtin_expect/unreachable`, designated init C99, `#warning`. **Gate: mỗi feature một case trong tests/ext/ (`tests/run.sh ext`).** ĐẠT 2026-08-17 — đa số đã có sẵn từ thời torture, chỉ phải thêm `__typeof__`, named variadic, comma-deletion, `#warning`. Case range `1 ... 5`, `$` trong ident: CHƯA làm (chưa ai đòi).
- **M11 — nuốt header THẬT của SDK** (bỏ dần stub — trả nợ library): `_Nullable` family, `__attribute__((availability...))`, blocks `^` (parse-skip ở vị trí declarator), `__asm("_rename")` (đổi symbol khi emit), `__has_include`, `#pragma` skip. Driver mặc định `-I $SDK/usr/include` khi include không có trong embedded. **Gate `tests/m11.sh`: một file include `pthread.h`, `sys/socket.h`, `netinet/in.h`, `sys/event.h`, `signal.h` từ SDK thật — compile, tạo socket + kqueue, chạy đúng.** ĐẠT 2026-08-17. Quyết định hệ trọng: **zcc xưng `__GNUC__ 4.2.1`** (như clang) vì SDK chỉ viết nhánh arm64 dưới `#ifdef __GNUC__` — nhánh non-GNUC là x86-only và thiếu định nghĩa. KHÔNG xưng `__clang__`, KHÔNG define `__BLOCKS__` (block `^` tự biến mất sau #if guard — khỏi parse). `__has_feature/__has_builtin` KHÔNG implement — cdefs.h tự fallback về 0. `defined(...)` phải resolve cả SAU macro expansion (pthread.h expand macro ra defined). `__uint128_t` = storage 16-byte align-16 (mcontext), không arithmetic.
- **M12 — atomics + đa luồng (ĐẠT 2026-08-17)**: họ `__sync_*` (fetch_and_add/sub + chiều ngược, val/bool_compare_and_swap, lock_test_and_set, lock_release, synchronize) hạ xuống `Node::Sync` → vòng LL/SC ldaxr/stlxr (acquire+release = seq_cst), operand integer/pointer 4|8 byte; bảng tên ở `src/ext.rs` (file khai sinh từ đây theo luật decouple). **Gate `tests/m12.sh`: 4 thread × 100000 băm counter fetch_add + spinlock CAS + macro trích nguyên văn `atomicvar.h` redis, chính xác 3/3; ngữ nghĩa đơn luồng khóa ở `tests/ext/gcc_sync_atomics.c`.**
- **M13 — nginx (ĐẠT 2026-08-17)**: configure + make + chạy thật, lặp lại bằng `tests/m13.sh` (clone sạch → serve → curl khớp cả file 200KB). Vá để đạt: họ `__has_feature/extension/builtin/attribute` thành operator #if trả 0 (bảng ở ext.rs — arm/_types.h gọi trước khi cdefs.h kịp fallback), `__APPLE_CC__ 6000` (TargetConditionals chọn nhánh GNUC bằng nó), `##` paste dùng raw spelling (199506L từng rớt suffix → sai tên macro cdefs), **inline definition hạ về static** (gnu89 `extern __inline` của SDK hết phát duplicate symbol), stub teo 5 cái: sys/time.h, unistd.h, fcntl.h, time.h, errno.h → SDK thật, stdio.h stub thêm `sys_nerr`. Diff probe configure với cc chỉ còn 2 lệch vô hại (-Wl,-E nuốt-nên-found, __builtin_bswap64 thiếu-có-fallback).
- **M14 — redis (cúp giai đoạn 2, ĐẠT 2026-08-17)**: `make CC=zcc MALLOC=libc` — server + cli + benchmark link, vendored deps (lua, hiredis, hdr_histogram, fpconv, xxhash, tre, linenoise) đều compile bằng zcc. **Gate `tests/m14.sh`: clone sạch → build → `redis-server` chạy, PING→PONG + SET/GET/INCR đúng.** Quyết định hệ trọng:
  - **Đảo giáo lý "không extended asm"**: xxhash đòi thật → hỗ trợ SUBSET `Node::Asm` — constraint chỉ `=r`/`+r`/`r`, ≤7 operand gán cứng x9..x15, clobber parse-rồi-bỏ (an toàn ở -O0: mọi statement reload từ memory), template phát nguyên văn.
  - **`__atomic_*` = macro đổ về `__sync_*`** (bảng ATOMIC_MACROS ext.rs) — load = fetch_add(p,0), CAS qua statement-expr + `__typeof__`; memorder bỏ qua (luôn seq_cst). fetch_or/and/xor chỉ jemalloc đòi — chưa làm (MALLOC=libc là default macOS).
  - **gnu89 inline**: định nghĩa inline không có declaration trần → `.weak_definition` (non-static) + **DCE per-TU** (như clang; bắt buộc vì body inline trong server.h tham chiếu symbol mà redis-cli không link). Có declaration trần → external thật (C99 6.7.4p7, logreqres.c dựa vào). Kéo theo `.subsections_via_symbols`.
  - **VLA hạ xuống con trỏ + `Node::Alloca(n*sizeof(elem))`** (networking.c `iov[iovmax]`); sizeof(vla) trả size con trỏ — sai spec CÓ CHỦ ĐÍCH; VLA nhiều chiều không hằng / có initializer / ngoài local = error.
  - Builtin bit-manip (`__builtin_bswap*/clz*/ctz*/popcount*`) = macro statement-expr thuần trong ext.rs BIT_MACROS. Computed include `#include MACRO` (C89 3.8.2 chuẩn). String init bọc ngoặc `char x[]={ "s" }` (C89 3.5.7). Splice `\`+newline TRONG string literal. Driver `-shared`/`-dynamiclib` → `ld -dylib`. `typedef enum : type` (SDK malloc.h). Local shadow typedef (`quicklist *quicklist`). Predefine `__ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__ 110000` (thiếu nó redis config.h rơi vào nhánh `struct stat64` ẩn). Stub xóa: dlfcn.h, stdint.h, limits.h (SDK thật phục vụ). Stub thêm: INFINITY/NAN/M_PI/fpclassify (math.h), `_IO*`+setvbuf (stdio.h), `getprogname` (stdlib.h — thiếu là implicit-int CẮT POINTER, đúng bẫy ASLR/implicit-int từ M8).
  - `__thread` ban đầu = no-op (đủ cho io-threads=1); **2026-08-17 nâng thành TLS Mach-O THẬT** khi redis-tests unit io-threads≥2 đòi: data `__thread_data`/`.tbss`, descriptor 3 quad ở `__thread_vars`, access `@TLVPPAGE` + `blr` tlv_get_addr (bảo toàn mọi reg trừ x0/x16/x17). Gate `tests/ext/gcc_thread_tls.c`.

Ngân sách LOC suy từ hiện trạng 5281 (2026-08-17): M9 ~150, M10 ~350, M11 ~250, M12 ~150, M13+M14 vá không đoán trước — dự kiến ~6.6k. **Chốt sổ giai đoạn 2: 6107 LOC + 333 stub; sau TLS thật: 6301 + 328.**

## Giai đoạn 3 — đa target ELF (chốt hướng 2026-08-17, chưa khởi công)

Quyết định đã chốt: mở **arm64 ELF Linux trước, x86_64 ELF sau** (arm64 ELF tái dùng instruction selection sẵn có; chạy native trong Docker/OrbStack trên M1). **Windows/PE: BỎ** — LLP64 phá tiền đề LP64 lock + chiến dịch header MinGW thứ hai + thủng trần 10k; dev Windows ăn qua WSL2 với binary ELF. Ngân sách ước: M15 +~1.2k, M16 +~1.8k → ~9.3k.

- **M15 — arm64 ELF Linux**: file `codegen/arm64_elf.rs` + flag `--target`. Phần rẻ (thay chuỗi ~20 điểm): section ELF (`.text/.data/.rodata/.bss/.weak`), bỏ prefix `_`, reloc `:lo12:`/`:got:` thay `@PAGE`/`@GOTPAGE`, bỏ `.subsections_via_symbols`; bảng predefine theo target (`__linux__ __ELF__`, bỏ họ `__APPLE__`). Phần thịt: (1) **va_list = struct 32 byte AAPCS** (`__stack/__gr_top/__vr_top/__gr_offs/__vr_offs`) + register-save area trong prologue — món to nhất ~200 LOC; (2) variadic vô danh vào x0–x7/v0–v7 như named (bỏ đặc sản Apple stack-only); (3) stack args slot 8 tròn (bỏ packing natural-align — đơn giản hóa cả 3 nơi khớp-từng-byte); (4) **`char` mặc định unsigned trên Linux arm64** — thấm lexer/parser, tham số hóa xuyên boundary; (5) `long double`: GIỮ = double, lệch chuẩn CÓ CHỦ ĐÍCH (`%Lf` sẽ sai, ai đạp thì tính); (6) TLS ELF model cho `__thread` (Mach-O đã có @TLVP). Toolchain: cross-compile zcc thành binary Linux (`aarch64-unknown-linux-musl` static) chạy trong box, as/ld binutils native. .so ELF: khi `-shared` mọi truy cập global qua GOT. **Gate: abi.sh + alg.sh chạy NGUYÊN XI trong box với trọng tài gcc** — bộ gate khoa học thành cross-target proof.
- **M16 — x86_64 ELF**: instruction selection mới + SysV classification (boss ABI cuối cùng). TyTab tham số hóa size/align làm từ M15.
- **M17 (dự kiến) — chiến dịch độ phủ**: dùng LOC còn lại sau M16 (~700 dưới trần) mua coverage dialect gcc/clang + cherry-pick C99/C11 — GIỮ LUẬT: không implement theo checklist, mỗi feature phải có phần mềm kinh điển thật đòi. Rổ mục tiêu theo coverage-per-LOC: sqlite (amalgamation, khả năng gần zero-cost) → zlib → lua standalone (gate riêng, đã qua trong redis) → jemalloc (hẹn từ M14) → curl → git → sbase → coreutils (gnulib, để cuối). Khi ext.rs vượt ~300 LOC: tách `ext/gcc.rs, ext/clang.rs…` theo phương ngữ GỐC. Rổ đòi quá ngân sách → quyết định lại trần bằng số liệu probe, không nhồi.

### Sổ nợ (trả dần trong M15+, harness sẵn)

- ~~nginx-tests rerun~~ TRẢ XONG 2026-08-17: minimal 493 file/1136 test All PASS; build FULL (ssl+http2+stream+mail, pcre2+openssl@3) 5346 test, fail-chỉ-zcc = RỖNG (trọng tài nginx-cc cùng config fail giống hệt — tests/README.md).
- ~~tcc.sh smoke~~ TRẢ XONG 2026-08-17: 108 pass, 31 skip, 0 fail — baseline rỗng ngay lần đầu.
- ~~TLS thật cho `__thread` Mach-O~~ TRẢ XONG 2026-08-17 (xem M14); phần ELF TLS model chuyển vào M15.
- `tests/suites/musl.sh` (suite 7, VIẾT RỒI, HOÃN): compile-only trên Darwin; thăng cấp thành build thật + libc-test khi M15 có box Linux.
- Driver harness riêng (flag matrix: -nostdinc/-bundle/-undefined/-MMD/-shared… hiện chỉ test gián tiếp qua m9/m13/m14).
- Probe sqlite/zlib/sbase/jemalloc… để đo bề mặt fail (nạp cho M17).

## Tầm nhìn xa

Dùng zcc phối hợp qemu viết một hệ điều hành đơn giản → cần mode freestanding (không libc, assembler directive bare-metal). KHÔNG thiết kế trước — chỉ giữ codegen tách file để thay được (đã là luật kiến trúc).
