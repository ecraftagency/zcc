// EXT — nơi tập trung logic extension "có thịt" theo luật decouple (CLAUDE.md):
// core (parser/codegen) chỉ được gọi VÀO đây; cắt file này + các touchpoint đánh
// dấu EXT(...) trong core là còn lại compiler C89 thuần. Touchpoint nhỏ (1-3
// dòng) vẫn nằm tại chỗ với marker, chỉ logic đủ dày mới dọn về đây.
use crate::ast::SyncOp;

// EXT(gcc): bảng builtin atomic __sync_* (M12) → (op, số arg đúng).
// Danh sách rút từ nhu cầu thật trên aarch64: nginx (bool_compare_and_swap,
// fetch_and_add, synchronize), redis atomicvar.h path HAVE_ATOMIC
// (add_and_fetch, sub_and_fetch, fetch_and_add, bool_compare_and_swap),
// postgres s_lock (lock_test_and_set, lock_release); val_compare_and_swap và
// fetch_and_sub thêm vì cùng khuôn codegen, coi như miễn phí.
// EXT(clang): họ operator __has_* trong #if (M13 — TargetConditionals.h,
// arm/_types.h gọi TRƯỚC khi cdefs.h kịp fallback-define). __has_include xử lý
// riêng (phải tra file thật); các tên này "defined" sẵn và luôn eval 0 —
// "không có feature" là câu trả lời an toàn vì SDK luôn có nhánh fallback.
pub fn has_operator_zero(name: &str) -> bool {
    matches!(
        name,
        "__has_feature" | "__has_extension" | "__has_builtin" | "__has_attribute"
    )
}

// EXT(gcc): họ __atomic_* kiểu C11 (hdr_histogram + redis atomicvar đòi ở M14)
// — arg memorder BỎ QUA vì zcc luôn phát seq_cst; map thành macro xuống
// __sync_* (statement-expr + __typeof__ có sẵn). Lưu ý load = fetch_add(p,0)
// nên KHÔNG dùng được trên trang read-only. fetch_or/and/xor chỉ jemalloc đòi
// (build MALLOC=libc không đụng) — chưa làm.
pub const ATOMIC_MACROS: &[(&str, &[&str], &str)] = &[
    (
        "__atomic_load_n",
        &["p", "mo"],
        "__sync_fetch_and_add((p), 0)",
    ),
    (
        "__atomic_store_n",
        &["p", "v", "mo"],
        "((void)__sync_lock_test_and_set((p), (v)))",
    ),
    (
        "__atomic_exchange_n",
        &["p", "v", "mo"],
        "__sync_lock_test_and_set((p), (v))",
    ),
    (
        "__atomic_fetch_add",
        &["p", "v", "mo"],
        "__sync_fetch_and_add((p), (v))",
    ),
    (
        "__atomic_add_fetch",
        &["p", "v", "mo"],
        "__sync_add_and_fetch((p), (v))",
    ),
    (
        "__atomic_fetch_sub",
        &["p", "v", "mo"],
        "__sync_fetch_and_sub((p), (v))",
    ),
    (
        "__atomic_sub_fetch",
        &["p", "v", "mo"],
        "__sync_sub_and_fetch((p), (v))",
    ),
    ("__atomic_thread_fence", &["mo"], "__sync_synchronize()"),
    (
        "__atomic_compare_exchange_n",
        &["p", "e", "d", "w", "s", "f"],
        "({ __typeof__(*(p)) __zcc_old = *(e), \
            __zcc_cur = __sync_val_compare_and_swap((p), __zcc_old, (d)); \
            __zcc_cur == __zcc_old ? 1 : (*(e) = __zcc_cur, 0); })",
    ),
];
// EXT(gcc): builtin bit-manipulation redis đòi (util.h clz, endianconv bswap64,
// keymeta popcount, hyperloglog ctzll) — statement-expr thuần, không cần codegen
// riêng; ngữ nghĩa -O0 nên tốc độ không phải mục tiêu. Tên biến tạm phải khác
// nhau giữa các macro vì bswap64 expand lồng bswap32 (arg nằm TRONG block con:
// trùng tên là tự tham chiếu). clz/ctz với x=0: UB, giống GCC.
pub const BIT_MACROS: &[(&str, &[&str], &str)] = &[
    (
        "__builtin_bswap16",
        &["x"],
        "({ unsigned short __zb16 = (x); (unsigned short)((__zb16 >> 8) | (__zb16 << 8)); })",
    ),
    (
        "__builtin_bswap32",
        &["x"],
        "({ unsigned int __zb32 = (x); (__zb32 >> 24) | ((__zb32 >> 8) & 0xff00u) \
        | ((__zb32 << 8) & 0xff0000u) | (__zb32 << 24); })",
    ),
    (
        "__builtin_bswap64",
        &["x"],
        "({ unsigned long long __zb64 = (x); \
        ((unsigned long long)__builtin_bswap32((unsigned int)__zb64) << 32) \
        | __builtin_bswap32((unsigned int)(__zb64 >> 32)); })",
    ),
    (
        "__builtin_clz",
        &["x"],
        "({ unsigned __zc32 = (x); int __zn32 = 0; \
        while (!(__zc32 >> 31)) { __zn32++; __zc32 <<= 1; } __zn32; })",
    ),
    ("__builtin_clzl", &["x"], "__builtin_clzll(x)"),
    (
        "__builtin_clzll",
        &["x"],
        "({ unsigned long long __zc64 = (x); int __zn64 = 0; \
        while (!(__zc64 >> 63)) { __zn64++; __zc64 <<= 1; } __zn64; })",
    ),
    (
        "__builtin_ctz",
        &["x"],
        "({ unsigned __zt32 = (x); int __zm32 = 0; \
        while (!(__zt32 & 1)) { __zm32++; __zt32 >>= 1; } __zm32; })",
    ),
    ("__builtin_ctzl", &["x"], "__builtin_ctzll(x)"),
    (
        "__builtin_ctzll",
        &["x"],
        "({ unsigned long long __zt64 = (x); int __zm64 = 0; \
        while (!(__zt64 & 1)) { __zm64++; __zt64 >>= 1; } __zm64; })",
    ),
    (
        "__builtin_popcount",
        &["x"],
        "({ unsigned __zp32 = (x); int __zq32 = 0; \
        while (__zp32) { __zq32 += __zp32 & 1; __zp32 >>= 1; } __zq32; })",
    ),
    (
        "__builtin_popcountll",
        &["x"],
        "({ unsigned long long __zp64 = (x); int __zq64 = 0; \
        while (__zp64) { __zq64 += (int)(__zp64 & 1); __zp64 >>= 1; } __zq64; })",
    ),
];

// EXT(gcc): __ATOMIC_RELAXED..__ATOMIC_SEQ_CST = 0..5 (giá trị như GCC); sự TỒN TẠI của
// __ATOMIC_SEQ_CST là cái hdr_atomic.h dò để chọn path __atomic
pub const ATOMIC_ORDERS: &[&str] = &[
    "__ATOMIC_RELAXED",
    "__ATOMIC_CONSUME",
    "__ATOMIC_ACQUIRE",
    "__ATOMIC_RELEASE",
    "__ATOMIC_ACQ_REL",
    "__ATOMIC_SEQ_CST",
];

// EXT(gcc): __builtin_{add,sub,mul}_overflow — ngữ nghĩa ℤ (spec GCC): tính
// a∘b trong số nguyên vô hạn, *res = giá trị cắt về kiểu *res, trả 1 nếu KHÔNG
// biểu diễn được. Nhúng mỗi toán hạng ≤64-bit thành two's-complement 128-bit
// {hi,lo} theo dấu RIÊNG của nó ⇒ phép toán 128-bit uniform (add=adds/adc,
// sub=subs/sbc, mul=mul/umulh/madd — bỏ term ah*bh vì ở 2^128) ⇒ test biểu
// diễn được = đơn ánh ℤ→Tr. Toàn cset, KHÔNG nhánh. AArch64 thuần register,
// dùng chung darwin+ELF (cùng ISA). Backend đặt sẵn: x0=al, x1=bl, x9=&res;
// scratch x10..x15. op 0=+ 1=- 2=*; a/b_sg = toán hạng signed; r_sg,rw = *res.
pub fn overflow_emit(s: &mut String, op: u8, a_sg: bool, b_sg: bool, r_sg: bool, rw: u32) {
    use std::fmt::Write as _;
    let ext = |s: &mut String, hi: &str, lo: &str, sg: bool| {
        _ = if sg {
            writeln!(s, "\tasr {hi}, {lo}, #63")
        } else {
            writeln!(s, "\tmov {hi}, #0")
        };
    };
    ext(s, "x10", "x0", a_sg); // ah
    ext(s, "x11", "x1", b_sg); // bh
    _ = match op {
        0 => writeln!(s, "\tadds x12, x0, x1\n\tadc x13, x10, x11"),
        1 => writeln!(s, "\tsubs x12, x0, x1\n\tsbc x13, x10, x11"),
        _ => writeln!(
            s,
            "\tmul x12, x0, x1\n\tumulh x13, x0, x1\n\tmadd x13, x0, x11, x13\n\tmadd x13, x10, x1, x13"
        ),
    }; // {rh:x13, rl:x12} = a∘b (128-bit)
    _ = writeln!(
        s,
        "\t{}",
        match rw {
            8 => "str x12, [x9]",
            4 => "str w12, [x9]",
            2 => "strh w12, [x9]",
            _ => "strb w12, [x9]",
        }
    ); // *res = rw byte thấp
    let wb = rw * 8;
    if !r_sg {
        // unsigned: biểu diễn được ⟺ rh==0 ∧ (rl>>wb)==0
        _ = writeln!(s, "\tmov x14, x13");
        if wb < 64 {
            _ = writeln!(s, "\torr x14, x14, x12, lsr #{wb}");
        }
        _ = writeln!(s, "\tcmp x14, #0\n\tcset x0, ne");
    } else {
        // signed: ⟺ sign-extend(rl, wb) == {rh,rl}
        _ = if wb < 64 {
            writeln!(s, "\tsbfx x14, x12, #0, #{wb}")
        } else {
            writeln!(s, "\tmov x14, x12")
        };
        _ = writeln!(
            s,
            "\tasr x15, x14, #63\n\tcmp x14, x12\n\tcset x0, ne\n\tcmp x15, x13\n\tcset x14, ne\n\torr x0, x0, x14"
        );
    }
}

// EXT(gcc): `__builtin_<f>` mà <f> là hàm thư viện C thật (abort, memcpy, printf…)
// → GCC đổ về symbol libc; zcc strip prefix rồi gọi thẳng. DANH SÁCH TRẮNG bắt
// buộc (default-deny): builtin nào KHÔNG có ở đây = intrinsic thuần compiler
// (clrsb, parity, frame_address, apply, va_arg_pack, mul_overflow_p…) — strip
// sẽ đẻ call tới symbol KHÔNG tồn tại → as/ld nghẹn (silent-miscompile). Luật
// 2-fact: builtin lạ phải REJECT SẠCH, không được âm thầm hoá libc-call. Danh
// sách = hàm thư viện C89/C99 + POSIX/GNU-string mà corpus THẬT đụng (test-first).
pub fn builtin_is_libc(f: &str) -> bool {
    // fortified `__builtin___memcpy_chk` → strip còn `__memcpy_chk` (symbol musl thật)
    if let Some(core) = f.strip_prefix("__").and_then(|x| x.strip_suffix("_chk")) {
        return builtin_is_libc(core);
    }
    matches!(
        f,
        // string.h + GNU string
        "memcpy" | "memmove" | "memset" | "memcmp" | "memchr" | "mempcpy"
        | "strcpy" | "strncpy" | "stpcpy" | "stpncpy" | "strcat" | "strncat"
        | "strcmp" | "strncmp" | "strcoll" | "strxfrm" | "strlen" | "strnlen"
        | "strchr" | "strrchr" | "strstr" | "strpbrk" | "strspn" | "strcspn"
        | "strdup" | "strndup" | "strtok" | "strerror" | "memrchr"
        | "ffs" | "ffsl" | "ffsll"  // POSIX strings.h — __builtin_ffs ≡ ffs
        // stdio.h
        | "printf" | "fprintf" | "sprintf" | "snprintf" | "vprintf" | "vfprintf"
        | "vsprintf" | "vsnprintf" | "scanf" | "sscanf" | "puts" | "fputs"
        | "putchar" | "fputc" | "putc" | "fwrite" | "fread" | "fopen" | "fflush"
        | "perror" | "fputs_unlocked"
        // stdlib.h
        | "malloc" | "calloc" | "realloc" | "free" | "abort" | "exit" | "_exit"
        | "_Exit" | "atexit" | "abs" | "labs" | "llabs" | "imaxabs" | "atoi"
        | "atol" | "atoll" | "atof" | "qsort" | "bsearch" | "getenv"
        | "strtol" | "strtoul" | "strtoll" | "strtoull" | "strtod"
        // math.h (libm — driver link -lm)
        | "fabs" | "fabsf" | "fabsl" | "sqrt" | "sqrtf" | "sqrtl"
        | "copysign" | "copysignf" | "copysignl" | "fmax" | "fmin" | "fmod"
        | "floor" | "ceil" | "round" | "trunc" | "pow" | "exp" | "log"
        | "sin" | "cos" | "tan"
    )
}

pub fn sync_op(name: &str) -> Option<(SyncOp, usize)> {
    Some(match name {
        "__sync_fetch_and_add" => (SyncOp::FetchAdd, 2),
        "__sync_add_and_fetch" => (SyncOp::AddFetch, 2),
        "__sync_fetch_and_sub" => (SyncOp::FetchSub, 2),
        "__sync_sub_and_fetch" => (SyncOp::SubFetch, 2),
        "__sync_val_compare_and_swap" => (SyncOp::ValCas, 3),
        "__sync_bool_compare_and_swap" => (SyncOp::BoolCas, 3),
        "__sync_fetch_and_and" => (SyncOp::FetchAnd, 2), // postgres18 generic-gcc.h
        "__sync_fetch_and_or" => (SyncOp::FetchOr, 2),
        "__sync_fetch_and_xor" => (SyncOp::FetchXor, 2),
        "__sync_lock_test_and_set" => (SyncOp::TestSet, 2),
        "__sync_lock_release" => (SyncOp::Release, 1),
        "__sync_synchronize" => (SyncOp::Barrier, 0),
        _ => return None,
    })
}
