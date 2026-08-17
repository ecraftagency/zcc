# zcc — hiến chương dự án

C89 compiler viết bằng Rust. Tác giả: Vu (xưng hô "mày/tao", trả lời tiếng Việt, thuật ngữ kỹ thuật giữ tiếng Anh).

## 2 yêu cầu tối thượng (mọi quyết định quy về đây)

1. **Strict compliance C89** — hỗ trợ đầy đủ ngôn ngữ C89, ngữ nghĩa đúng spec; giai đoạn 2 mở rộng thành **C89+** (xem thang milestone). Target DUY NHẤT: executable AArch64 trên macOS Apple Silicon (Mach-O). Tương lai xa (không thiết kế trước): ELF Linux arm64 → x86_64.
2. **Ít LOC nhất có thể** — không optimization pass (ngữ nghĩa -O0), không tính năng nào được viết trước khi có một file `.c` test đòi hỏi nó, không abstraction đón đầu, **zero external crate** (dependency cũng là LOC).

Khi 2 yêu cầu xung đột: compliance thắng LOC.

Mục tiêu xa: dùng `zcc` phối hợp qemu để viết một hệ điều hành đơn giản. Hệ quả thiết kế: sau này cần mode freestanding (không libc, có thể cần assembler directive kiểu ELF cho bare-metal) — KHÔNG thiết kế trước cho việc đó, chỉ cần giữ codegen tách riêng một file để thay được.

## Kiến trúc

```
main.rs (driver) → lexer → parser → AST (arena + NodeId(u32), KHÔNG Box/reference chằng chịt) → codegen/<target> → .s text

- **Boundary frontend/backend = `src/ast.rs`** (AST + TyTab). Frontend (lexer, parser) DỰNG, backend (codegen/*) chỉ ĐỌC; hai tầng không được import lẫn nhau, mọi trao đổi qua ast.rs. Layout size/align nằm trong TyTab vì lock họ ABI LP64 (arm64 lẫn x86_64 đều int=4 char=1 long=ptr=8); cần ILP32 thì tham số hóa TyTab.
- Tầng backend: `src/codegen/mod.rs` là cửa duy nhất (`codegen::emit(&Ast) -> String` = .s text), mỗi target một file con (`codegen/arm64_darwin.rs`; tương lai: elf x86_64, arm64 freestanding…). ABI/section/asm syntax nằm trọn trong file target. Thêm target = thêm file + nhánh match trong mod.rs + nhánh toolchain (as/ld) bên driver.
```

- zcc sinh .s text nội bộ rồi tự phối hợp toolchain hệ thống TRỰC TIẾP (không cc driver): `as tmp.s -o tmp.o` → `ld tmp.o -o out -lSystem -syslibroot $(xcrun -sdk macosx --show-sdk-path) -arch arm64`. Không cần crt0.o (dynamic executable entry qua LC_MAIN trong libSystem); ld64 arm64 tự ad-hoc codesign.
- CLI tương thích `cc` để drop-in (`CC=zcc`): `zcc [-c | -S] [-o out] <in.c>` → executable `a.out` mặc định; `-c` dừng ở `<stem>.o`, `-S` ở `<stem>.s`; flag cc khác (`-O` `-g` `-W*` `-std=*`…) nuốt im lặng.
- Single crate, không workspace.

## Đặc sản ABI Darwin/AArch64 (sai là crash khó hiểu — đọc trước khi codegen)

- Symbol C có gạch dưới: `main` → `_main`, gọi libc: `bl _printf`.
- KHÔNG địa chỉ tuyệt đối. Global/string literal truy cập qua: `adrp x0, sym@PAGE` + `add x0, x0, sym@PAGEOFF`.
- **Variadic (printf...): tham số vô danh đi LÊN STACK** (đặc sản Apple, ngược Linux ARM64). Named args vẫn x0–x7.
- Calling convention AAPCS64: args x0–x7, return x0, float v0–v7, stack thẳng hàng 16 byte trước `bl`.
- Sections Mach-O: `.section __TEXT,__text`, string vào `.section __TEXT,__cstring`, globals `.section __DATA,__data`.
- Prologue/epilogue chuẩn: `stp x29, x30, [sp, #-16]!` / `ldp x29, x30, [sp], #16`.
- `char` mặc định signed trên Darwin.
- Đối chiếu đáp án mẫu bất cứ lúc nào bằng: `clang -S -O0 -std=c89 foo.c`.

## Thang milestone (đi tuần tự, không nhảy cóc)

- **M0**: `int main() { return N; }` → .s → `cc` link → exit code đúng. Chứng minh toàn pipeline.
- **M1**: biểu thức `+ - * / %`, ngoặc, unary, so sánh — vẫn chỉ trong return.
- **M2**: biến local (stack slot), `=`, `if/else`, `while`, `for`, block.
- **M3**: định nghĩa + gọi hàm nhiều tham số, đệ quy (fib chạy được).
- **M4**: con trỏ, `&` `*`, mảng, pointer arithmetic, `int`/`char`/`long` + sizeof.
- **M5**: string literal, `char *`, gọi `printf` (nhớ luật varargs-lên-stack). Từ đây test diff được stdout.
- **M6**: struct/union, typedef, enum, global variables, initializer.
- **M7**: preprocessor C89 đầy đủ (`#include #define #if...` — macro expansion/rescan là boss thật của cả dự án).
- **M8** (cúp): compile được `chibicc` hoặc `tcc`, binary sinh ra compile được hello world (kiểm chứng bắc cầu — thay cho self-hosting vì Rust không self-host được). ĐẠT 2026-08-17, lặp lại bằng `tests/m8.sh`.

## Thang milestone giai đoạn 2 — C89+ (đích: nginx + redis trên M1 Mach-O, tổng < 10k LOC)

"C89+" = giữ khung C89, cherry-pick đúng phần C99/C11/GCC-extension mà nginx/redis đạp phải. KHÔNG claim C99 (không VLA, không _Complex trừ khi bị đòi). Mỗi milestone: suite cũ (run.sh, m8.sh) phải giữ xanh.

**Luật decouple extension (vì mục đích sư phạm — người đọc phải phân biệt được đâu là ISO C, đâu là phương ngữ vendor):**
- Logic extension có thịt (bảng `__builtin_*`, eval `__has_*`, skip `__attribute__`, asm-label…) sống trong **`src/ext.rs`**; core chỉ gọi ra qua hàm tên `ext_*`.
- Điểm chạm không tách file được (nhánh parse len trong core) BẮT BUỘC đánh marker **`// EXT(gcc)`** / `// EXT(clang)` / `// EXT(apple)` / `// EXT(c99)` — `grep 'EXT(' src/` phải liệt kê đủ 100% bề mặt lệch chuẩn.
- Tiêu chí kiểm chứng: cắt ext.rs + các nhánh có marker → phần còn lại là pure C89 compiler vẫn pass nguyên suite C89. Decouple chứng minh bằng phép cắt, không tự tuyên bố.
- Test extension để riêng `tests/ext/` (trọng tài `cc` không kèm `-std=c89`), không trộn vào `tests/cases/`.

- **M9 — driver đúng chuẩn cc** (điều kiện sống còn để tích hợp toolchain): nhiều input một lệnh (`zcc a.c b.o c.o -o app` — compile phần .c, forward tất cả .o cho ld), `-l`/`-L`, `-U`, `-MMD -MF` (make của redis cần), `-v`, exit code + diagnostic format `file:line:` chuẩn (configure grep stderr), flag lạ nuốt im lặng nhưng KHÔNG nuốt nhầm flag có tham số đi kèm (`-o` `-I`…). **Gate: build tcc bằng chính Makefile gốc của nó (không ONE_SOURCE) — nhiều .c → .o → link — rồi m8.sh vẫn pass.** ĐẠT 2026-08-17, lặp lại bằng `tests/m9.sh` (hack `-Dinline=__inline` hết cần).
- **M10 — ngôn ngữ C89+**: mixed declarations + decl trong `for(...)`, `long long` thật + `_Bool`, variadic macros `__VA_ARGS__` (+ named `args...`, `,##__VA_ARGS__`), `__typeof__`, flexible array member, `inline`/`__restrict`/`__extension__` no-op, `__attribute__` parse-skip + honor `aligned`/`packed`, `__builtin_expect/unreachable`, designated init C99, `#warning`. **Gate: mỗi feature một case trong tests/ext/ (`tests/run.sh ext`).** ĐẠT 2026-08-17 — đa số đã có sẵn từ thời torture, chỉ phải thêm `__typeof__`, named variadic, comma-deletion, `#warning`. Case range `1 ... 5`, `$` trong ident, `__has_include`, asm-label: CHƯA làm (chưa ai đòi / thuộc M11).
- **M11 — nuốt header THẬT của SDK** (bỏ dần stub — trả nợ library): `_Nullable` family, `__attribute__((availability...))`, blocks `^` (parse-skip ở vị trí declarator), `__asm("_rename")` (đổi symbol khi emit), `__has_include/__has_feature/__has_builtin`, `#pragma` skip. Driver mặc định `-I $SDK/usr/include` khi include không có trong embedded. **Gate: một file include `pthread.h`, `sys/socket.h`, `netinet/in.h`, `sys/event.h`, `signal.h` từ SDK thật — compile, tạo socket + kqueue, chạy đúng.** ĐẠT 2026-08-17, lặp lại bằng `tests/m11.sh`. Quyết định hệ trọng: **zcc xưng `__GNUC__ 4.2.1`** (như clang) vì SDK chỉ viết nhánh arm64 dưới `#ifdef __GNUC__` — nhánh non-GNUC là x86-only và thiếu định nghĩa. KHÔNG xưng `__clang__`, KHÔNG define `__BLOCKS__` (block `^` tự biến mất sau #if guard — khỏi parse). `__has_feature/__has_builtin` KHÔNG implement — cdefs.h tự fallback về 0. `defined(...)` phải resolve cả SAU macro expansion (pthread.h expand macro ra defined). `__uint128_t` = storage 16-byte align-16 (mcontext), không arithmetic.
- **M12 — atomics + đa luồng (ĐẠT 2026-08-17)**: họ `__sync_*` (fetch_and_add/sub + chiều ngược, val/bool_compare_and_swap, lock_test_and_set, lock_release, synchronize) hạ xuống `Node::Sync` → vòng LL/SC ldaxr/stlxr (acquire+release = seq_cst), operand integer/pointer 4|8 byte; bảng tên ở `src/ext.rs` (file khai sinh từ đây theo luật decouple). `__atomic_*` chưa ai đòi — chưa làm. **Gate `tests/m12.sh`: 4 thread × 100000 băm counter fetch_add + spinlock CAS + macro trích nguyên văn `atomicvar.h` redis, kết quả chính xác 3/3; ngữ nghĩa đơn luồng khóa ở `tests/ext/gcc_sync_atomics.c`.**
- **M13 — nginx (ĐẠT 2026-08-17)**: configure + make + chạy thật, lặp lại bằng `tests/m13.sh` (clone sạch → serve → curl khớp cả file 200KB). Vá để đạt: họ `__has_feature/extension/builtin/attribute` thành operator #if trả 0 (bảng ở ext.rs — arm/_types.h gọi trước khi cdefs.h kịp fallback), `__APPLE_CC__ 6000` (TargetConditionals chọn nhánh GNUC bằng nó), `##` paste dùng raw spelling (199506L từng rớt suffix → sai tên macro cdefs), **inline definition hạ về static** (gnu89 `extern __inline` của SDK hết phát duplicate symbol — đúng nhánh `static __inline` cdefs dành cho compiler lạ), stub teo 5 cái: sys/time.h, unistd.h, fcntl.h, time.h, errno.h → SDK thật (probe nginx cần NULL/timespec/ENOSPC/openat), stdio.h stub thêm `sys_nerr`. Diff probe configure với cc chỉ còn 2 lệch vô hại (-Wl,-E nuốt-nên-found, __builtin_bswap64 thiếu-có-fallback).
- **M14 — redis** (cúp giai đoạn 2): `make CC=zcc MALLOC=libc` — kéo theo vendored deps (lua, hiredis, linenoise) cũng compile bằng zcc. **Gate: `redis-server` chạy, `redis-cli` PING→PONG + SET/GET đúng.**

Ngân sách LOC suy từ hiện trạng 5281 (2026-08-17): M9 ~150, M10 ~350, M11 ~250, M12 ~150, M13+M14 là vá không đoán trước — tổng dự kiến ~6.6k, trần cứng 10k. Stub headers (473 dòng) teo dần từ M11.

## Vòng lặp phát triển & test

- Test harness: `tests/run.sh` — mỗi case `tests/cases/*.c` compile bằng cả `cc -std=c89 -O0` (trọng tài) lẫn zcc, chạy hai binary, diff exit code (sau M5: diff cả stdout).
- Compile chương trình test TRƯỚC, chạy, vỡ ở construct nào thì mới implement construct đó — đây là cơ chế ép LOC tối thiểu.
- Quy tắc của Vu: mọi con số/quyết định phải suy ra được từ tiền đề đã tuyên bố, không magic number không nguồn gốc.
