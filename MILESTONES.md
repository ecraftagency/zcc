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

- **M18 (dự kiến) — chiến dịch correctness 100%** (Vu chốt 4 khía 2026-08-17; bổ sung 2026-08-18: sau khi pass hết suite hiện hành, replenish theo 2 trục — (a) độ phủ định lý cao nhất có thể, (b) suite công nghiệp bổ sung).
  - **CẬP NHẬT 2026-08-21 — Csmith/yarpgen TÁI KÍCH HOẠT (trục correctness IR-era):** điều kiện nhập của "HOÃN-CÓ-ĐIỀU-KIỆN" (tool ở vai máy-sinh/trọng tài NGOÀI repo như gcc, cài trong box không vendor không vào build path) nay THỎA — csmith 2.3.0 + yarpgen 2.0 cài apt/build trong image `zcc-box:fuzz`, oracle gcc, differential 3-way (gcc vs zcc--ir-noopt vs zcc--ir-opt). Kết quả đợt đầu: **csmith 300 (seed1-300) → 300 PARITY / 0 DIVERGE** sau khi trị 1 lowering bug (orphan-temp dead-code, commit `5c6a65d`); yarpgen + csmith seed 301+ đang mở rộng. Đây là chứng nghiệm THỰC TIỄN đắt (tầng dưới sci-gate) đúng doctrine hai-tầng: case đơn-file deterministic, seek-được, thỏa luật tốc-độ.
  **LUẬT FREEZE (Vu chốt 2026-08-18): trong suốt M18, bề mặt feature ĐÓNG BĂNG — không mua feature/dialect mới (M17 coverage campaign tạm khóa, kể cả visibility attr của M20), để proof hội tụ trên mục tiêu đứng yên và mọi kết luận suite/gate được BẢO TOÀN. Chỉ được sửa: bug làm bề mặt HIỆN HÀNH sai ngữ nghĩa (fix ≠ feature), và chỉ phục vụ các compile goal hiện tại (musl, sqlite, git, redis, nginx + suite/gate của chúng) — không fix đón đầu cho phần mềm chưa vào rổ. Mở băng = sự kiện có sổ: từng feature mới phải khai dòng PROOF.md + gate trước khi merge.**
  **ĐIỀU CHỈNH "ĐIỂM DỪNG" (Vu 2026-08-18 khuya): suite surface ĐÓNG BĂNG — rổ suite chính thức = 4 món Vu điểm danh: nginx, redis, musl, git ("có suite là đủ rồi")**; sqlite ở lại vai corpus compile + byte-diff/bench (không chạy suite tcl nữa); gate nội bộ + corpus tĩnh đã có (torture, c-testsuite, chibicc, kr, nora) giữ nguyên. Khía 1 (Csmith/yarpgen/suite bổ sung) **HỦY — để làm HOMEWORK cho người fork** (Vu: "Csmith gen flow chaos rồi md5 với gcc — toàn Bohrbug tất định; ai muốn cứ fork repo mà chạy"): "thêm test suite chỉ thêm bug để đuổi, không phải nỗ lực xóa bug — bản thân C cũng broken; workspace phải có điểm dừng, đó cũng là một cách diệt bug; tcc chắc hẳn cũng có bug." Bằng chứng cùng đêm ủng hộ: differential git t/ 3× hai phía ra **nhiễu nền đối xứng** (zcc-only 86 dòng vs gcc-only 106 dòng, t7112#57 flaky xuyên-session cả hai compiler) — suite công nghiệp flaky không đáng làm quan tòa. Hệ quả: (i) correctness từ đây đi MỘT trục — theory/math (PROOF.md khía 0 + gate khoa học), khía 2–4 giữ vì là gate/định lý chứ không phải suite mới; (ii) chuẩn nghiệm thu kiểu tcc — baseline known-fail có sổ + differential, KHÔNG đòi 100% trên suite flaky; (iii) sau khi trục theory củng cố → **tiến thẳng assembly/ld (M22 kéo lên)**; crt0 + loader vay musl nguyên trạng (libc là firstborn — ld-musl = libc.so, crt1.o musl), không tự viết.
  **DOCTRINE HAI TẦNG (Vu chốt 18/8 khuya, hoàn thiện "điểm dừng")**: bề mặt proof của compiler tách làm hai tầng có vai trò và nhịp test khác nhau. **Tầng wiring** = compiler ↔ realworld: OS/arch/ABI/memory/relocation/libc/build-system — không gian mình không tự vét được, nhân chứng duy nhất là phần mềm thật → 4 suite nginx/redis/git/sqlite(+musl) chứng nghiệm MỘT LẦN cho tới khi chốt baseline; sau đó chỉ chạy lại khi CHÍNH WIRING đổi (target mới, zas/zld thay GNU, bump musl…), KHÔNG chạy lại khi semantics compiler đổi — việc đó của gate. (Toàn bộ bug thật 18/8 đều là bug tầng wiring: va_arg HFA, GOT-static, transparent_union, char signedness, stack size — đúng vai falsifier của nó.) **Tầng theory** = ngữ nghĩa ngôn ngữ thuần: không gian đóng, deterministic, đơn-file → gate vét cạn + generative differential. **Csmith/yarpgen thuộc tầng THEORY, không phải suite công nghiệp** (case = đơn-file deterministic checksum, thỏa cả 3 tiêu chí luật tốc độ dưới) → lệnh hủy 18/8 tinh chỉnh thành HOÃN-CÓ-ĐIỀU-KIỆN: khi tầng wiring chứng nghiệm xong (baseline chốt + gate ABI/reloc đứng), quay lại theory focus thì csmith/yarpgen ĐƯỢC tái xét — điều kiện nhập: tool ở vai máy-sinh/trọng tài NGOÀI repo như gcc (cài trong box, không vendor, không vào build path), và đi SAU gen_flow.py tự viết (probe rẻ đo độ dày của không gian đó trước khi trả giá import tool 80k).
  **LUẬT TỐC ĐỘ TEST (phát biểu lại "điểm dừng" thành tiêu chí đo được — Vu nêu lý do gốc 18/8 khuya: suite chậm + không seek được về unit fail)**: bộ test mới chỉ được nhận nếu (a) mỗi case = MỘT file .c tự chứa, deterministic (UB-free by construction, seed ghi trong tên file); (b) fail seek được về đúng case, re-test sau fix = mili-giây–giây; (c) toàn gate chạy đơn vị phút trên host, không docker, không harness state. Suite integration (git t/, nginx-tests…) vĩnh viễn ở vai nhân chứng release-only. Hai gate mới đạt chuẩn này, dựng theo pattern gen_* sẵn có (cùng huyết thống alg.sh/decay.sh): **node.sh + gen_node.py** — vét cạn alphabet Node × type-combo của codegen (lấp ô trống "per-node simulation phi hình thức" PROOF.md, không gian hữu hạn → định lý thật); **flow.sh + gen_flow.py** — generator TỰ VIẾT trong tests/ (không import Csmith 80k C++ — Csmith-the-tool vẫn là homework cho fork), sinh chương trình control-flow+arith nhỏ UB-free có seed, differential checksum vs cc — thế chỗ đúng vùng generative đã hủy nhưng ở class NHANH/seek-được.
  0. **PROOF.md — sổ định lý (trục a, thước đo đứng trên 4 khía dưới)**: mỗi feature một dòng — *thuộc không gian nào / hữu hạn (vét cạn = định lý) hay vô hạn (quy nạp + differential = bằng chứng) / gate nào giữ / phần dư phải thừa nhận*. "Độ phủ toán" = tỷ lệ feature có gate đứng gác, đo được chứ không cảm tính. Vùng trống đã biết cần gate mới: variadic + HFA vào abi.sh (bug 920625-1 lọt đúng ô này), FP (khía 3), linkage (khía 4), codegen simulation per-node (yếu nhất — hiện phi hình thức). **Luật map 1:1 (Vu siết 2026-08-18 tối, sau án B/pr92904)**: mỗi feature phải map 1:1 với đúng MỘT định lý, và nghĩa vụ proof gồm HAI tầng tách bạch — (i) *map đúng*: alphabet của model phủ hết bề mặt construct thật (2 bug 18/8 đều là LỖ MAP chứ không phải lỗ vét: abi.sh thiếu chữ cái aligned(16), không gate nào model type-derivation nên ternary/comma decay lọt); (ii) *chứng minh định lý*: vét cạn khi không gian hữu hạn, quy nạp + differential khi vô hạn. Suite công nghiệp = máy falsify tầng (i); gate = máy chứng minh tầng (ii). Escape mới bắt buộc trả lời "chữ cái nào thiếu trong alphabet nào" trước khi đóng án. Gate mẫu sinh từ luật này: **decay.sh + gen_decay.py (18/8)** — định lý lvalue conversion 6.3.2.1p3 (+6.5.15/6.5.2.2p6/6.5.17), vét SOURCES×CONTEXTS 12×11, differential cc; ngay lượt chạy đầu bắt bug comma thiếu decay (6.5.17).
  1. **Csmith generative fuzzing (trục b)** — sinh hàng loạt chương trình C ngẫu nhiên CAM KẾT không-UB, differential zcc↔gcc/cc (checksum output + crash-khi-compile = bug). Cùng huyết thống alg.sh (trọng tài + generator lọc UB) nhưng generative cả chương trình: vét được tổ hợp struct lồng/pointer sâu mà corpus tĩnh (torture/nginx/redis) không bao giờ chạm. Mục tiêu: harness `tests/csmith.sh` chạy nightly, reduce case bằng creduce khi bắt được. Cùng rổ trục b: **yarpgen** (thế hệ sau Csmith, UB-free by construction, nặng loop/arithmetic — bù đúng vùng Csmith mỏng), **c-testsuite** (~220 case conformance độc lập), đều differential trong box. Suite thương mại (Plum Hall/Perennial — chuẩn công nghiệp thật sự) ghi nhận tồn tại, không mua.
  2. **Valgrind memcheck** — bọc runtest/binary thật để phơi stack lệch/ghi lấn thầm lặng chưa tới mức segfault. LƯU Ý: Valgrind KHÔNG hỗ trợ macOS arm64 → chạy trong box Linux với binary zcc-ELF (M15 mở khóa đúng cửa này); món rẻ hơn trên Darwin: `-fsanitize=address` không có (zcc -O0 không sanitizer) → dùng guard page/malloc debug của libc (`MallocScribble`).
  3. **Numeric rigor — gate `num.sh` + ĐỊNH LÝ QUY GIẢN** (Vu mở rộng 2026-08-18: thiếu suite/định lý cho scientific tool — long long/float/double, overflow/underflow). Định lý quy giản -O0: zcc không tự tính arithmetic lúc runtime — silicon IEEE 754 là tiên đề; nghĩa vụ proof rút về 3 mệnh đề: (i) đúng instruction + đúng width per node (cấm tính thừa precision — double rounding); (ii) đúng chuỗi conversion UAC/6.3 (nối alg.sh); (iii) hai nơi compiler TỰ tính phải bit-identical runtime: constant folder (kể cả inf/NaN/subnormal, unsigned wrap mod 2^n) và literal parser (decimal→double correctly-rounded; literal `f` parse THẲNG f32, cấm qua f64). Tầng gate: (a) **vét cạn f32 một ngôi 2^32** — mọi cast/negate/convert trên toàn bộ 4 tỷ bit pattern, differential gcc trong box = định lý vét cạn thật; (b) hai ngôi: vét không gian cấu trúc cross-product giá trị đặc biệt (±0, subnormal min/max, normal min/max, ±1, ±inf, NaN) + mẫu biên rounding; (c) int: biên overflow per (op × width × sign), unsigned wrap, fold-vs-runtime. Suite ngoài: Paranoia (Kahan), **Berkeley TestFloat** (oracle softfloat độc lập), IBM FPgen vectors, **testbase David Gay** (chuyên strtod/literal). Ghi chú: libc-test math/* (ULP check vector crlibm) CHÍNH LÀ suite scientific đang chạy — cụm fail math trong 77 suspect là đầu vào chiến dịch này. Sổ nợ liên đới: long double=double trên ELF (fp128) là lệch chuẩn CÓ SỔ, scientific tool thật đòi thì thương lượng lại. Mìn ĐÃ XÁC NHẬN chờ gate này xử: literal `f` parse f64→thu hẹp f32 (parser.rs ~2923, double rounding — C89 6.1.3.1 cho latitude 1 ULP nên chưa phạm chuẩn, nhưng lệch gcc/clang ở case hiếm kiểu 7.038531e-26f; fix = parse thẳng f32 từ PTok.raw khi num.sh đổ bộ).
  4. **Linkage & symbol multi-file** — gate `link.sh` mới: tentative definitions rải nhiều TU (C89 3.7.2 — linker gộp), weak symbol, shadow global/local, gọi hàm KHÔNG prototype với default argument promotion (char→int, float→double) link chéo cc↔zcc (mở rộng tự nhiên của abi.sh sang không gian linkage).

- **M19 (dự kiến) — chiến dịch userland: shell + đồ kinh điển** (Vu chốt hướng
  2026-08-18: "build được gần như toàn bộ userland" nằm trong lộ trình
  replenish compiler capability; hợp lưu với Tầm nhìn xa — initramfs musl-static
  boot qemu cần shell mới thành hệ SỐNG). Luật giữ nguyên: test-first, mỗi ext
  mới phải có phần mềm trong rổ đòi; mỗi mục chấm bằng suite chính chủ của nó.
  Thứ tự theo giá/coverage:
  1. **dash** — POSIX sh nhỏ nhất, gần chắc build ngay; mở khóa initramfs
     tương tác. Oracle: chạy chính test POSIX của nó + so gcc.
  2. **busybox** (hoặc sbase+ubase đã nằm rổ M17) — một binary = cả userland
     (init/sh/coreutils/mount); kconfig + GNU ext vừa phải. Đây là "gần như
     toàn bộ userland" trong một mục tiêu.
  3. **bash** — biểu tượng + suite `tests/` chính chủ dày (trụ 3 đúng nghĩa);
     dialect bảo thủ hơn tiếng đồn (autoconf-era C). Kèm readline bundled.
  4. **GNU make** — tự build phần mềm TRÊN hệ đích → userland tự tái sản xuất.
  5. **zsh** — stretch (to, module dlopen → đòi nợ -shared/TLS-in-so trước).
  6. **Chuỗi foundation lib → PostgreSQL 18** (Vu ký 2026-08-18 "chơi postgres
     18 mới nhất luôn"): zlib → libpng (đòi zlib — test luôn chuỗi dep) →
     libjpeg (bản IJG C thuần, không phải turbo-SIMD) → **PG18**. *(SỬA 18/8
     khuya theo "điểm dừng": PG chấm bằng **compile + run được là OK** —
     initdb + start + query sống; KHÔNG lấy pg_regress làm suite, không nhập
     kho suite. Lib chấm bằng smoke chính chủ mức chạy-được, cùng tinh thần.)* Đã trinh sát + TRẢ TRƯỚC 18/8 (52 LOC, local PASS
     19/19 ext + 67/67 cases): __sync_fetch_and_and/or/xor (atomics/
     generic-gcc.h dùng thẳng cả họ khi HAVE_GCC__SYNC_INT32_CAS bật) +
     _Static_assert EXT(c11) hai scope (StaticAssertStmt/Decl). Phần còn lại
     PG18 đã có sẵn từ M12–M17: spinlock TAS (slock_t arm64 = int),
     CAS 32/64, barrier, asm-label, __builtin_expect/constant_p/unreachable.
     KHÔNG cần lib async ngoài: AIO PG18 tự trồng, io_method=worker default,
     io_uring/liburing là opt-in configure — không bật; --without-icu
     --without-readline --without-zlib nếu cần tối giản (PG17+ đã xóa
     --disable-spinlocks nên cụm __sync là bắt buộc — đã có).
  - **systemd: TỪ CHỐI CÓ SỔ** — không phải vì to mà vì sai target kép:
    (a) glibc-only chính thức (musl không được hỗ trợ — Alpine/Void dùng
    runit/openrc vì đúng lý do này), hệ đích của mình là musl-static;
    (b) GNU11 dày đặc `__attribute__((cleanup))` = ngữ nghĩa destructor,
    _Generic, meson — mua bề mặt dialect khổng lồ cho MỘT phần mềm không chạy
    được trên hệ đích. PID 1 cho hệ nhỏ: **sinit/runit** (vài trăm dòng C
    thuần) hoặc init tự viết trong initramfs — đúng khí chất MINIX-inspired.

- **M20 (dự kiến) — dynamic linking musl: loader + .so** (Vu chốt 2026-08-18:
  hệ phải chạy dynamic như distro thật — static toàn phần chỉ là bước đệm
  bootstrap; Alpine cũng ship musl dynamic). Trả trọn dòng nợ M15 "-shared
  chưa prove". Đặc sản musl: **ld-musl CHÍNH LÀ libc.so** — build được libc
  shared là có loader, không có mảnh GNU nào mọc thêm. Các mảnh, theo thứ tự:
  1. **zcc: visibility** — lỗ thật đã soi 2026-08-18: zcc nuốt im
     `__attribute__((visibility("hidden")))`, không phát `.hidden`, và dưới
     -fPIC đẩy MỌI global non-static qua GOT. musl ldso đòi hidden = truy cập
     TRỰC TIẾP (non-preemptible, và đường _dlstart chạy TRƯỚC self-relocation
     không được đụng GOT chưa reloc). Việc: parse visibility → emit `.hidden`
     + adrp/add trực tiếp kể cả khi pic.
  2. **Build musl shared**: compile lại -fPIC (.lo), link `libc.so` (-shared
     -nostdlib, entry _dlstart tự reloc) → `ld-musl-aarch64.so.1` trong
     sysroot. LƯU Ý cache: config.mak cũ đã bake quyết định static — phải
     configure lại khi mở shared.
  3. **Driver sysroot dynamic mode**: PT_INTERP = <sysroot>/lib/libc.so,
     link против libc.so (giữ static làm mặc định/flag — initramfs vẫn cần).
  4. **TLS model cho .so**: local-exec hiện tại chỉ đúng trong exe —
     initial-exec cho .so (dòng nợ TLS-in-so M15 gộp vào đây).
  5. **Gate link.sh mọc chiều dynamic**: interposition exe-đè-so, PLT, GOT
     data, link chéo gcc-main↔zcc-so và ngược (copy-reloc phía gcc vs
     GOT-luôn phía zcc — đúng automaton ABI, vét được).
  - Chấm điểm: libc-test nhánh dynamic + dlopen/dso tests (đang ngoài scope
    static), rồi dash/busybox link dynamic chạy trên ld-musl do zcc build.

- ~~nginx-tests rerun~~ TRẢ XONG 2026-08-17: minimal 493 file/1136 test All PASS; build FULL (ssl+http2+stream+mail, pcre2+openssl@3) 5346 test, fail-chỉ-zcc = RỖNG (trọng tài nginx-cc cùng config fail giống hệt — tests/README.md).
- ~~tcc.sh smoke~~ TRẢ XONG 2026-08-17: 108 pass, 31 skip, 0 fail — baseline rỗng ngay lần đầu.
- ~~TLS thật cho `__thread` Mach-O~~ TRẢ XONG 2026-08-17 (xem M14); phần ELF TLS model chuyển vào M15.
- `tests/suites/musl.sh` (suite 7, VIẾT RỒI, HOÃN): compile-only trên Darwin; box Linux đã có (M15) — thăng cấp thành build thật + libc-test khi mở lại.
- Nợ M15: `-shared` ELF (.so + GOT toàn cục) chưa prove; va_arg composite vắt ngang reg/stack (AAPCS C.11) chưa xử.
- **Mục tiêu C99-ĐỦ (Vu chốt 2026-08-18: userland mượt cần đủ C99, không chỉ ext-theo-đòi; thi hành NGAY KHI M18 mở băng, trước M19).** Đo 18/8 bằng 20 probe differential: **18/20 đã ăn** (mixed decl, for-decl, __VA_ARGS__ + empty arg, inline + extern inline, restrict, _Bool, compound literal cả static, designated init cả nested, flexible array, __func__, long long, UCN, wide string, VLA local/param/ptr, _Complex đại số đủ, hex float, _Pragma; __STDC_VERSION__ đã xưng 199901L). Còn thiếu, xếp giá: (1) `sizeof(VLA)` runtime — FAIL probe, đáng mua nhất; (2) digraphs `<% %> <: :>` — bảng lexer vài dòng, mua cho tròn; (3) `#pragma STDC FP_CONTRACT/FENV_ACCESS/CX_LIMITED_RANGE` — nuốt-và-ghi-nhận là CONFORMING ở -O0 (không bao giờ contract, không tối ưu qua fenv); (4) `tgmath.h` — cần builtin dispatch, phần mềm thật gần như không dùng, mua CUỐI; (5) `long double`=double — VẪN CONFORMING C99 (5.2.4.2.2 chỉ đòi ≥ double; MSVC cùng lựa chọn) — vấn đề fp128 là NỢ ABI ELF interop, sổ riêng, không tính vào C99. **NỢ CÓ CHỦ từ 18/8 khuya**: redis full-suite (rổ đóng băng) chết 86/156 tại moduleapi/misc — module zcc truyền long double 8-byte vào glibc đợi fp128 16-byte, `'0' != '0.00000000000000001'` (đúng schrödinbug thứ 2 trong sổ collapse, ledger R1). ~~Xử theo FREEZE: baseline known-fail + skipunit~~ **ĐẢO ÁN theo luật mới (Vu 18/8 khuya): "suite hiện tại cần mua feature thì BẮT BUỘC phải mua"** — rổ suite đóng băng là ĐỊNH NGHĨA của bề mặt hiện hành, suite trong rổ đòi = nghĩa vụ, không baseline né được (luật này chỉ áp cho RỔ SUITE 4 món nhân chứng wiring; corpus tĩnh known-fail — torture nested-fn… — giữ chế độ triage cũ). fp128 long double ELF thành PURCHASE BẮT BUỘC, vẫn theo thủ tục mở băng: khai PROOF.md + gate (chữ cái Q vào abi.sh + va_arg fp128) trước khi merge. Giá đã trinh sát: TyTab tham số 16/16 theo target (Darwin giữ 8/8) + Ty long double tách khỏi double qua UAC + codegen libcall __addtf3/__extenddftf2/__fixtfsi… (libgcc.a — driver ELF thêm -lgcc, lâu nay không link vì đi as→ld thẳng) + pass/return q-regs NSRN + va_arg q-slot + literal `L` parse qua double+extend (đủ cho ld2string %.17Lf của redis; correctly-rounded fp128 literal = nâng cấp sau nếu suite đòi). Thư viện C99 (snprintf, stdint, wchar…) = musl gánh, đã có. Luật mở băng giữ nguyên: mỗi món vào kèm dòng PROOF.md + gate (sizeof-VLA vào shape.sh, digraph vào gen_lex).
- **fp128 ĐÃ MUA 18/8 đêm — án R1 ĐÓNG** (trước cả mở băng M18, theo luật "suite trong rổ đòi = bắt buộc"): 120 LOC, thiết kế memory=binary128/reg=f64 (rẻ hơn giá trinh sát — không cần __addtf3, số học chạy double hợp lệ C99 vì float.h khai LDBL_MANT_DIG 53). Bug thật: default arg promotion nuốt long double (C99 6.5.2.2p6). Redis 22 unit: họ ldbl ~113 err → 0; referee gcc 5998 ok/0 err chứng minh box sạch → còn đúng 2 án zcc mới (ledger R2 stacktrace ×9, R3 proctitle ×1). Thủ tục PROOF.md + gate chữ Q: CHƯA khai — Vu 18/8 đêm hạ lệnh tối giản quy trình (SOP v4), nợ nếu bị đòi lại.
  **THI HÀNH XONG 18/8 cùng ngày (Vu ký sớm, không chờ mở băng — lệnh "trừ tgmath.h ra đưa toàn bộ c99 vào")**: hiến chương #1 đổi **Strict compliance C99**; 24 marker `EXT(c99)` hạ thành chú thích `C99:` (bề mặt lệch chuẩn còn 88 vendor: 77 gcc + 8 clang + 3 apple); mua (1) `sizeof(VLA)` runtime — local ẩn `.vlasz` chốt byte lúc khai báo, sizeof + alloca cùng đọc (vla_szs key offset, clear mỗi hàm như reg_pins; phủ cả `sizeof(int[n])` typename + `sizeof *p` qua vla_arrs); (2) digraphs — bảng DIGRAPHS 6 mục ánh xạ về punct chính tắc TRƯỚC bảng PUNCTS (C 6.4.6 vô điều kiện, không có luật `<::` C++); (3) `#pragma STDC` — nuốt sẵn, tuyên bố conforming ở -O0. Trọng tài run.sh cases/ nâng `-std=c89`→`-std=c99`, corpus 67/67 (case mới c99_digraph_vla.c differential khớp byte), shape + cpp PASS. **Deviation tuyên bố duy nhất còn lại của C99: tgmath.h** (cần builtin dispatch/_Generic, userland thật không dùng — mua khi có chủ nợ, đường rẻ là header kiểu musl `__typeof__`).
- Driver harness riêng (flag matrix: -nostdinc/-bundle/-undefined/-MMD/-shared… hiện chỉ test gián tiếp qua m9/m13/m14).
- Probe sqlite/zlib/sbase/jemalloc… để đo bề mặt fail (nạp cho M17).

## Tầm nhìn xa

Dùng zcc phối hợp qemu viết một hệ điều hành đơn giản → cần mode freestanding (không libc, assembler directive bare-metal). KHÔNG thiết kế trước — chỉ giữ codegen tách file để thay được (đã là luật kiến trúc).

Bước đệm đã khả thi từ 2026-08-18 (musl sysroot + crt tự build, static ELF
kernel tự map không cần loader): **initramfs Linux nhỏ** — kernel arm64 +
`/init` static-musl-zcc boot trong qemu; M19 cấp shell/tools biến nó thành hệ
sống. Học boot chain/initramfs/syscall surface ở đây trước khi thay kernel
bằng của mình.

**M21 (đích công bố, Vu chốt hướng 2026-08-18)** — distro PoC console-first.
**TIỀN ĐỀ CỨNG (Vu chốt 2026-08-18): KHÔNG compose bất kỳ distro nào khi chưa
pass hết toàn bộ test suite / sci proof hiện hành VÀ các suite+proof tương lai
(M18 trọn vẹn: PROOF.md không ô trống + Csmith/yarpgen/c-testsuite + 4 khía).
Correctness đi trước danh tiếng — thứ tự không thương lượng: M18 → M19/M20 →
M21.** *(SỬA 18/8 khuya theo "điểm dừng": vế "suite tương lai +
Csmith/yarpgen/c-testsuite" HỦY — tiền đề rút về: rổ suite 4 món
nginx/redis/musl/git với baseline known-fail có sổ + PROOF.md không ô trống.)*
Nội dung:
rootfs = musl + ld-musl + init + shell + utils, TẤT CẢ userland compile bằng
zcc (kernel vay như mọi distro; as/ld binutils ở build-time — claim chuẩn:
"zcc là compiler duy nhất trong toolchain"; claim "GNU-free build chain" chỉ
phát biểu SAU khi có assembler/linker riêng — món này để ngỏ, đòi thương
lượng lại trần LOC). Điều kiện công bố theo hiến chương: artifact tái lập
được — script dựng image từ source + zcc seed (binary musl static, không cần
rustc trên hệ), boot qemu, kèm bảng suite (libc-test, redis, git t/, sqlite
byte-diff). Quảng bá bằng lệnh người ta tự chạy lại được, không tự tuyên bố.

**M22 (dự kiến, Vu duyệt ngân sách 2026-08-18) — zas + zld: toolchain không GNU.**
~~Xếp SAU M20~~ **KÉO LÊN (Vu 18/8 khuya: "thôi rồi tiến tới ld, assembly")**
— đi ngay sau khi trục theory M18 củng cố, TRƯỚC PG18 (Vu giao tao chọn thứ
tự as/ld vs postgres; chọn as/ld vì: thu bề mặt ngoại lai = diệt nguồn bug,
assembler = automaton hữu hạn vét cạn được — đúng trục theory, gate =
differential GNU as/ld trên chính corpus .s zcc đã sinh trong rổ đóng băng,
không cần suite mới; PG lại là bề mặt configure mới). **crt0 + loader vay
musl nguyên trạng (Vu: libc là firstborn)** — zld chỉ cần link ĐÚNG với
ld-musl/crt1.o musl, không tự viết loader; điều kiện "học qua GNU ld trước"
thỏa dần bằng chính corpus reloc hiện hành thay vì chờ trọn M20. Ngân sách LOC: **trần compiler 10k KHÔNG đổi**
(src/ nguyên vẹn); zas/zld là binary riêng, sổ riêng — zas ~2k (assembler
aarch64 subset: chỉ nuốt (a) output của chính zcc — tập mnemonic vét bằng grep
codegen, (b) file .s viết tay của musl), zld static ~1.5k + dynamic ~1k (chỉ
tập relocation zas sinh ra: ADR_PREL_PG_HI21, ADD_ABS_LO12, CALL26, ABS64,
cụm GOT/TLS). **Dynamic là ĐIỀU KIỆN HOÀN THÀNH, không phải option (Vu chốt
lần 2, 2026-08-18): zlinux bắt buộc chạy .so + loader — không all-static dù
disk rẻ; zld static-only chỉ là nấc nội bộ, CHƯA đủ tư cách thay GNU ld.** Tổng toolchain mục tiêu **<15k** — vẫn không đối thủ cùng hạng
(tcc 80k x86, cproc mượn QBE). Toán: assembler = automaton bảng tra encoding
(hữu hạn → vét cạn); linker = reachability + đại số relocation. Gate:
differential vs GNU as/ld — cùng .s, diff semantic readelf/objdump; cùng .o,
binary chạy qua suite chính chủ. Món phụ ăn theo: thời gian assemble về tay
mình (Apple clang-as 3.9s vs GNU as 1s trên sqlite .s 742k dòng — đo
2026-08-18). Claim mở khóa: "toolchain không một dòng GNU: zcc → zas → zld →
musl". Mức meta khai trung thực khi công bố: zcc build bằng rustc (seed
binary — mọi toolchain đều cần compiler mồi; giáo trình dây sang Trusting
Trust của Thompson).
**Luật interface (Vu chốt 2026-08-18): zas/zld BẮT BUỘC drop-in với
make/configure/m4 — nối dài luật driver của hiến chương.** Cụ thể: (1) bề
mặt flag mua test-first từ chính log build verbose của rổ (mọi invocation
`as`/`ld` mà driver zcc + build system thật đã phát — V=1 log các overnight
chính là bản ghi spec); (2) TUYỆT ĐỐI không nuốt nhầm flag có tham số;
(3) exit code + stderr format để configure grep được; (4) bẫy libtool/m4:
LT_PATH_LD sniff `ld --version` tìm chuỗi "GNU" và đổi hành vi theo —
quyết định masquerade hay dạy libtool nhận diện riêng sẽ chốt bằng số liệu
khi busybox/bash vào rổ, không đoán trước; (5) hợp đồng tối thiểu của zld =
CHÍNH lệnh ld mà driver zcc đang phát hôm nay (crt1/crti/crtn, -dynamic-linker,
-lc -lm, -shared/-soname khi M20) + đuôi -Wl, mà build system tunnel qua.
(6) Lỗ hổng claim đã điểm danh 2026-08-18: build thật còn gọi **ar/ranlib**
(musl đóng libc.a, redis đóng deps, mọi static lib) — "GNU-free" đòi thêm
**zar** (~200-300 LOC: format ar cổ điển + symbol index /; ranlib = zar -s).
nm/strip/objcopy/readelf KHÔNG bắt buộc cho build chain — tool chẩn đoán,
vay được không mất claim.
Câu chuyện khác biệt: toolchain auditable — "trusting trust" cỡ một học kỳ
sinh viên.

## C99-đủ — bịt 3 lỗ ISO C99 cuối (VLA) 2026-08-20

**torture `1377 pass / 317 not-impl / 0 FAIL` (box, gate giữ).** Phase 2 chốt 3
construct ISO C99 THẬT (không vendor — proof `clang -pedantic-errors` ACCEPT +
spec), mỗi món map định lý, test-first, differential vs gcc:
- **`pr57568` — địa chỉ-hằng 2D** (6.6p9): `gaddr` thêm nhánh `Deref(p)` khi kiểu
  là `Array(..)` → `&arr[i][j]` là hằng địa chỉ hợp lệ (trước reject).
- **`20040411-1` — variably-modified typedef `sizeof`** (6.7.7 + 6.5.3.4p2):
  `typedef int c[i+2]` — size eval MỘT LẦN tại điểm khai báo, chốt byte vào local
  ẩn `.vmtsz`; `sizeof(c)` đọc lại (trước trả 0 = silent-miscompile). Non-ISO
  (VM-typedef ngoài mảng / đa chiều) vẫn reject sạch.
- **`20221006-1` — VLA 2 chiều local** `int M[d0][d1]` (6.7.6.2): hạ xuống con
  trỏ-tới-hàng-VLA `int[d1]` + `alloca(d0·d1·elem)`. TÁI DÙNG nguyên cơ chế
  `(*p)[w]`: đăng ký row-type vào `vla_arrs` (bước-hàng runtime `d1·elem` trong
  local ẩn `.rowsz`) → indexing `M[i][j]` tự đúng qua decay `Deref-Array`
  (arm64_elf 867-875, không thêm codegen). Mỗi declarator sinh TypeId row riêng
  ⇒ `M1`/`M2` khác bề rộng không đụng nhau. `vla_inner: Vec<NodeId>` gom chiều
  trong; ≥3 chiều / chiều-trong-lồng-VLA reject sạch. Differential gcc: sum +
  row-stride khớp kể cả chữ nhật `a≠b` (stride `= b·4` độc lập số hàng).

Kèm: **VLA-in-struct** (GNU non-ISO, `-pedantic-errors` "will never be
supported") giờ reject HONEST tại struct-body (`VLA trong struct/union chưa hỗ
trợ`) thay vì rò state → miscompile (pr82210/20040423-1/pr41935/20040308-1/
20041218-2/20070919-1/align-nest). `align-nest` rớt pass→not-impl: "pass" cũ là
VACUOUS (test chỉ ghi rồi `return 0`, không kiểm giá trị nào — exit-0 vô nghĩa
trên layout undefined) → honest-reject > vacuous-pass. Batch `src ~10124/13000`.

## Chiến dịch torture — nested function (GNU) ~~ĐẠT~~ **ĐẢO ÁN: BỎ 2026-08-20**

**RÚT LẠI (Vu quyết 2026-08-20, sau Phase 1 2-fact):** nested function BỊ GỠ
HẲN — `−240 LOC src (10271→10031)`, full suite 14/14 vẫn PASS, 0 FAIL. Lý do:
(1) **vendor lock-in không chủ nợ** — clang/MSVC đều không có, không app nào
trong rổ (nginx/redis/git/sqlite/musl, kể cả PG18/CPython tương lai — cả hai
cấm vì phải build MSVC) đòi; mua CHỈ để pass torture = SAI CỬA test-first
(torture là corpus chứng nghiệm, không phải app-demand trigger — vi phạm hiến
chương "không tính năng trước khi có file .c thật đòi"). (2) đòi **executable
stack** (trampoline) = cờ đỏ bảo mật, ngược W^X/NX. (3) đơn giản hóa không gian
VLA (pr22061-3/4 nested+2D-VLA tự thành GNU-ext NOT-IMPL sạch). Giá đo được: 23
torture case PASS→NOT-IMPL (reject sạch `nested function (GNU) không hỗ trợ`) —
gate 2-fact = 0 FAIL nên KHÔNG phá gate, chỉ giảm cột pass. Gỡ trọn: 3 Node
(Upvar/Tramp/NlGoto) + 3 Func field (uid/parent_uid/chain) + `nested_funcdef`
+ static-chain x18 + non-local-goto + `.note.GNU-stack` note + IR `Place::Upvar`.
`__label__` giữ parse-and-ignore (same-func label vẫn qua Goto thường).

**Nội dung LỊCH SỬ (đã gỡ, giữ làm sử ký):** ~~+23 torture (95→72 fail, 0 NEW
fail), full suite 14/14 PASS, src 9894/10000 (KHÔNG cần bỏ Mach-O).~~ GNU nested
function = phương ngữ vendor `EXT(gcc)`,
**chỉ ELF** (trampoline đòi executable stack — Darwin W^X từ chối tại parse).
Rút thẳng từ gcc-14 aarch64 asm làm oracle (không đoán trí nhớ) — 3 cơ chế
trực giao, mỗi case là tổ hợp con:
- **T trampoline**: 40B sinh RUNTIME trên stack tại slot local
  (`bti c; ldr x17,.+20; ldr x18,.+24; br x17; dsb sy; isb; .xword fn; .xword chain`),
  patch (fn_addr, chain) + `__clear_cache` (libgcc, driver ELF đã `-lgcc`);
  fnptr = địa chỉ trampoline. Cần stack THỰC THI → phát `.note.GNU-stack,"x"`
  CHỈ khi TU có nested (giữ NX cho mọi chương trình khác). MỌI tham chiếu tên
  nested (gọi/truyền) đều hạ về `Node::Tramp`→`CallPtr(tramp)` — triệt tiêu
  nhánh direct-call-chain, struct-sret/variadic vẫn đúng (trampoline chỉ đụng
  x17/x18, không đụng x8).
- **C static chain qua x18** (STATIC_CHAIN_REGNUM, zcc chưa dùng x18 nên tự do):
  prologue nested lưu x18 vào slot `Func.chain`; upvar = `[chain - off]`, off =
  hằng biên dịch DÙNG CHUNG (chain = x29 hàm bao, biến local hàm bao vốn địa
  chỉ `x29-off`). chain của Tramp: cùng cha ⟹ x29, sibling ⟹ forward chain mình.
- **G non-local goto**: `__label__` của hàm bao + `goto` từ nested → khôi phục
  `(x29,sp)` hàm bao qua chain rồi `b lg_{parent}.{label}` (label = local symbol
  cùng TU, adrp-reachable). sp hàm bao = x29 - frame (đúng khi không VLA).

8/8 case: nestfunc-1..3,5,6,7 + nest-align-1 (over-align 16) + nest-stdar-1
(variadic nested). Giá: +258 LOC src (ast 3 Node + 3 Func field; parser
`nested_funcdef` + upvar/Tramp/NlGoto resolution + `setup_params` tách dùng
chung; codegen ELF Tramp/Upvar/NlGoto + prologue chain-save + GNU-stack note).
Depth-1 (upvar/goto tới hàm bao TRỰC TIẾP) — depth>1 chưa có case đòi, chưa làm.

## Ngã rẽ chiến lược (Vu nêu 2026-08-20) — HOÃN, chưa thực thi

**Ý định**: bỏ hẳn Mach-O/Darwin (arm64_darwin.rs ~1402 LOC) → dùng ngân sách
đó dựng **1 IR layer đơn giản + vài optimization technique chứng minh đúng bằng
toán** → zcc thành compiler "full-fledged" vẫn trong trần 10k (ELF-only).
Phép tính: 9894 − 1402 = 8492 → ~1500 headroom; IR tuyến tính (3-address/SSA-lite)
~300-500 + passes provable (const-fold ~50, DCE ~80, local value numbering ~120,
copy-prop ~60, register allocation linear-scan ~300-500) ≈ 1000-1500 → khả thi.

**LẬT LUẬT TỐI THƯỢNG #2** ("không optimization pass, ngữ nghĩa -O0") — kỷ luật
mới thay thế: "mọi pass phải chứng minh semantics-preserving bằng toán" (hợp
MATHEMATIC FOUNDATION). Thu target về ELF-only.

**ĐIỀU KIỆN CỔNG (Vu chốt 2026-08-20): CHƯA PASS TORTURE SẠCH thì KHOAN đụng IR
hay optimization.** 72 torture fail còn lại phải xử hết (fix thật hoặc chứng
minh ngoài-scope theo luật suy đoán tội) TRƯỚC. Phiên này dừng ở nested-func +
ghi roadmap; không code IR.
