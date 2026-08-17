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

Ngân sách LOC suy từ hiện trạng 5281 (2026-08-17): M9 ~150, M10 ~350, M11 ~250, M12 ~150, M13+M14 vá không đoán trước — dự kiến ~6.6k. **Chốt sổ giai đoạn 2: 6107 LOC + 333 stub; sau TLS thật: 6301 + 328. Sau M15 (arm64 ELF): 7762 + 404 (arm64_elf.rs là bản sao song song ~1.26k — giá của "diff là tài liệu"; buffer còn ~2.2k dưới trần).**

## Giai đoạn 3 — đa target ELF (khởi công 2026-08-17)

Quyết định đã chốt: mở **arm64 ELF Linux trước** (tái dùng instruction selection sẵn có; chạy native trong Docker/OrbStack trên M1). **Windows/PE: BỎ** — LLP64 phá tiền đề LP64 lock + chiến dịch header MinGW thứ hai + thủng trần 10k; dev Windows ăn qua WSL2 với binary ELF.

- **M15 — arm64 ELF Linux (ĐẠT 2026-08-17)**: file `codegen/arm64_elf.rs` (bản sao có chủ đích của arm64_darwin.rs — `diff` hai file LÀ tài liệu "Mach-O vs ELF khác gì") + enum `Target` xuyên boundary ast.rs + flag `--target` (mặc định theo host OS lúc build zcc: binary cross-compile `aarch64-unknown-linux-musl` static tự thành native compiler trong box, drop-in `CC=zcc` không cần flag). Phần rẻ đúng dự đoán: section ELF, bỏ prefix `_`, `:lo12:`/`:got:` thay `@PAGE`/`@GOTPAGE`, `.weak`, GNU `.comm` align BYTE (Darwin = log2), TLS local-exec `tpidr_el0 + :tprel_*` thay descriptor @TLVPPAGE. Phần thịt như hoạch định: (1) **va_list = struct 32 byte AAPCS** — register-save area 192B (q0–q7 + x0–x7) ngay dưới frame trong prologue variadic; `__builtin_va_start/va_arg` thành **builtin thật** `Node::VaStart/VaArg` (Darwin vẫn đi đường macro char* — node không bao giờ xuất hiện); va_arg chọn vùng GP/VR/stack theo DẤU của `gr_offs/vr_offs`; (2) variadic vô danh đi reg như named; (3) stack args slot 8 tròn; (4) `char` mặc định UNSIGNED (tham số hóa qua `P.tgt`, `__CHAR_UNSIGNED__` predefine); (5) `long double` GIỮ = double, lệch chuẩn CÓ CHỦ ĐÍCH (case `float_h` diff LDBL_* với gcc — fail khách quan duy nhất trong box). Đường link ELF: crt1/crti/crtn + `-dynamic-linker /lib/ld-linux-aarch64.so.1` + `-lc -lm` (glibc tách libm, Darwin gộp libSystem). Món phát sinh: `#include_next` (subset: như `#include <>` nhưng bỏ bảng nhúng) + embedded `limits.h` đóng vai "limits.h của compiler" như clang/gcc rồi chuyển tiếp xuống glibc (`_GCC_LIMITS_H_`). **Gate ĐẠT: abi.sh (292 case × 4 hướng link chéo zcc↔gcc, 0 fail) + alg.sh (43036 điểm runtime + 21552 fold, 4 phép so) + cpp.sh (mech 37 dòng + 1425 điểm #if) chạy NGUYÊN XI trong box Debian/gcc 14; tests/cases 64/65 (1 fail = float_h chủ đích); tests/ext 14/14 kể cả TLS 4-thread + atomics.** Nợ M15 để lại: `-shared` ELF all-globals-qua-GOT chưa prove bằng .so thật; va_arg composite ≤16 chưa xử C.11 vắt ngang reg/stack (gate không đòi).
- **M16 — x86_64 ELF (HOÃN — quyết định Vu 2026-08-17)**: nhường LOC cho M17 trước; instruction selection mới + SysV classification khi nào mở lại tính sau.
- **M17 (ĐANG CHẠY) — chiến dịch độ phủ**: dùng LOC còn lại (~2.2k dưới trần sau khi hoãn M16) mua coverage dialect gcc/clang + cherry-pick C99/C11 — GIỮ LUẬT: không implement theo checklist, mỗi feature phải có phần mềm kinh điển thật đòi. Thứ tự Vu chốt: sqlite → git → musl libc, rồi rổ còn lại theo coverage-per-LOC: zlib → lua standalone → jemalloc (hẹn từ M14) → curl → sbase → coreutils (gnulib, để cuối). Khi ext.rs vượt ~300 LOC: tách `ext/gcc.rs, ext/clang.rs…` theo phương ngữ GỐC. Rổ đòi quá ngân sách → quyết định lại trần bằng số liệu probe, không nhồi.
  - **sqlite ĐẠT (2026-08-17)**: amalgamation 262 899 dòng compile sạch nhát đầu, zero LOC; CLI chạy, differential vs cc 31 dòng khớp byte. Nợ: suite TCL chính chủ cần box Linux + tcl.
  - **git ĐẠT (2026-08-17)**: binary 2.55.GIT link + smoke đủ (init/commit/log/diff/fsck); t/ suite: t0000 92/92, t0001 103/103, t3600 81/82 (1 fail = known breakage của git). Giá +62 LOC: `__STDC_VERSION__ 199901L`, `[restrict]` array param, PRI/SCN MAX, `__builtin_types_compatible_p`, `__extension__` expr/stmt, và 3 bug C89 thuần bị phơi (scope tên global trước init; ident sau specifier là tên dù trùng typedef; locals thừa rò vào ginit). Chi tiết tests/README.md. LOC sau git: 7824 Rust + 447 header.
  - **musl libc 1.2.5 ĐẠT (2026-08-17)**: full build trong box —
    **1350 object zero error, libc.a 4.3MB + crt**, install thành sysroot;
    static hello + smoke rộng (qsort/printf/math/file IO/strtod) **khớp
    từng byte vs gcc+glibc**. Suite chính chủ libc-test: zcc fail 77 err /
    referee musl-gcc fail 46 — differential chốt: phần lớn fail = baseline
    upstream; zcc-only còn ~10 nghi phạm thật (mbc/wide cluster, setjmp,
    vfork, tls_local_exec, ilogb/isless fp-exception) + nợ -shared + LDBL64
    hệ quả — chi tiết tests/README.md. Quyết định hệ trọng: `long double`
    giữ = double bằng cách port `bits/float.h` LDBL64 (nhánh arm32 upstream
    — 1 file, thế giới tự nhất quán); driver mọc `ZCC_SYSROOT`; phát hiện
    **GNU ld drop addend trên GOT local symbol** → ELF FunAddr static đi
    adrp/add trực tiếp (khác Darwin). Giá: `_Complex` desugar struct,
    weak/alias/top-level asm, `.s/.S` passthrough, builtin inf/nan, seed
    `__zcc_va_list` ELF. LOC sau musl: **8894 Rust + 424 header** (buffer
    ~1.1k dưới trần). Vu chốt sau musl: libc build được ≈ 90% userland mở
    khóa — M17 tạm dừng nhường **chiến dịch correctness (M18 kéo lên
    trước)**: math/sci gate + thêm suite, đưa correctness → 100%.

- **M18 (dự kiến) — chiến dịch correctness 100%** (Vu chốt 4 khía 2026-08-17, xếp sau/xen kẽ M17):
  1. **Csmith generative fuzzing** — sinh hàng loạt chương trình C ngẫu nhiên CAM KẾT không-UB, differential zcc↔gcc/cc (checksum output + crash-khi-compile = bug). Cùng huyết thống alg.sh (trọng tài + generator lọc UB) nhưng generative cả chương trình: vét được tổ hợp struct lồng/pointer sâu mà corpus tĩnh (torture/nginx/redis) không bao giờ chạm. Mục tiêu: harness `tests/csmith.sh` chạy nightly, reduce case bằng creduce khi bắt được.
  2. **Valgrind memcheck** — bọc runtest/binary thật để phơi stack lệch/ghi lấn thầm lặng chưa tới mức segfault. LƯU Ý: Valgrind KHÔNG hỗ trợ macOS arm64 → chạy trong box Linux với binary zcc-ELF (M15 mở khóa đúng cửa này); món rẻ hơn trên Darwin: `-fsanitize=address` không có (zcc -O0 không sanitizer) → dùng guard page/malloc debug của libc (`MallocScribble`).
  3. **Floating-point rigor** — Paranoia test suite (kinh điển IEEE-754: rounding/overflow/underflow/precision) + UCB ieeecc754 nếu tha được; hiện FP mới qua alg.sh mẫu biên + torture, chưa qua bài khắc nghiệt chuyên FP.
  4. **Linkage & symbol multi-file** — gate `link.sh` mới: tentative definitions rải nhiều TU (C89 3.7.2 — linker gộp), weak symbol, shadow global/local, gọi hàm KHÔNG prototype với default argument promotion (char→int, float→double) link chéo cc↔zcc (mở rộng tự nhiên của abi.sh sang không gian linkage).

### Sổ nợ (trả dần trong M15+, harness sẵn)

- ~~nginx-tests rerun~~ TRẢ XONG 2026-08-17: minimal 493 file/1136 test All PASS; build FULL (ssl+http2+stream+mail, pcre2+openssl@3) 5346 test, fail-chỉ-zcc = RỖNG (trọng tài nginx-cc cùng config fail giống hệt — tests/README.md).
- ~~tcc.sh smoke~~ TRẢ XONG 2026-08-17: 108 pass, 31 skip, 0 fail — baseline rỗng ngay lần đầu.
- ~~TLS thật cho `__thread` Mach-O~~ TRẢ XONG 2026-08-17 (xem M14); phần ELF TLS model chuyển vào M15.
- `tests/suites/musl.sh` (suite 7, VIẾT RỒI, HOÃN): compile-only trên Darwin; box Linux đã có (M15) — thăng cấp thành build thật + libc-test khi mở lại.
- Nợ M15: `-shared` ELF (.so + GOT toàn cục) chưa prove; va_arg composite vắt ngang reg/stack (AAPCS C.11) chưa xử.
- Driver harness riêng (flag matrix: -nostdinc/-bundle/-undefined/-MMD/-shared… hiện chỉ test gián tiếp qua m9/m13/m14).
- Probe sqlite/zlib/sbase/jemalloc… để đo bề mặt fail (nạp cho M17).

## Tầm nhìn xa

Dùng zcc phối hợp qemu viết một hệ điều hành đơn giản → cần mode freestanding (không libc, assembler directive bare-metal). KHÔNG thiết kế trước — chỉ giữ codegen tách file để thay được (đã là luật kiến trúc).
