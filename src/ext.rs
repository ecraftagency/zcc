// EXT — the home for substantial extension logic under the decoupling rule
// (CLAUDE.md): the core (parser/codegen) may only call INTO here; removing this
// file plus the touchpoints marked EXT(...) in the core leaves a pure C89
// compiler. Small touchpoints (1-3 lines) stay in place with a marker; only logic
// of sufficient bulk is moved here.
use crate::ast::SyncOp;

// EXT(gcc): the __sync_* atomic builtin table → (op, correct arg count).
// The list is drawn from real needs on aarch64: nginx (bool_compare_and_swap,
// fetch_and_add, synchronize), redis atomicvar.h HAVE_ATOMIC path
// (add_and_fetch, sub_and_fetch, fetch_and_add, bool_compare_and_swap),
// postgres s_lock (lock_test_and_set, lock_release); val_compare_and_swap and
// fetch_and_sub are added because they share the codegen mold, essentially free.
// EXT(clang): the __has_* operator family in #if (M13 — TargetConditionals.h,
// arm/_types.h calls them BEFORE cdefs.h can fallback-define them). __has_include
// is handled separately (it must look up a real file); these names are already
// "defined" and always evaluate to 0 — "no such feature" is the safe answer
// because the SDK always has a fallback branch.
pub fn has_operator_zero(name: &str) -> bool {
    matches!(
        name,
        "__has_feature" | "__has_extension" | "__has_builtin" | "__has_attribute"
    )
}

// EXT(gcc): the C11-style __atomic_* family (required by hdr_histogram + redis
// atomicvar at M14) — the memorder argument is IGNORED because zcc always emits
// seq_cst; mapped as macros down to __sync_* (statement-expr + __typeof__ are
// available). Note that a load = fetch_add(p,0), so it CANNOT be used on a
// read-only page. fetch_or/and/xor are required only by jemalloc (a MALLOC=libc
// build does not touch them) — not yet implemented.
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
// EXT(gcc): bit-manipulation builtins required by redis (util.h clz, endianconv
// bswap64, keymeta popcount, hyperloglog ctzll) — pure statement-expr, no
// dedicated codegen needed; with -O0 semantics, speed is not a goal. The
// temporary variable names must differ between macros because bswap64 expands a
// nested bswap32 (the arg lives INSIDE the inner block: a name clash would be
// self-referential). clz/ctz with x=0: UB, as in GCC.
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

// EXT(gcc): __ATOMIC_RELAXED..__ATOMIC_SEQ_CST = 0..5 (values as in GCC); the
// EXISTENCE of __ATOMIC_SEQ_CST is what hdr_atomic.h probes to select the
// __atomic path
pub const ATOMIC_ORDERS: &[&str] = &[
    "__ATOMIC_RELAXED",
    "__ATOMIC_CONSUME",
    "__ATOMIC_ACQUIRE",
    "__ATOMIC_RELEASE",
    "__ATOMIC_ACQ_REL",
    "__ATOMIC_SEQ_CST",
];

// EXT(gcc): __builtin_{add,sub,mul}_overflow — ℤ semantics (GCC spec): compute
// a∘b over the infinite integers, *res = the value truncated to the type of *res,
// return 1 if it is NOT representable. Each ≤64-bit operand is embedded as a
// 128-bit two's-complement {hi,lo} according to ITS OWN signedness ⇒ a uniform
// 128-bit operation (add=adds/adc, sub=subs/sbc, mul=mul/umulh/madd — the ah*bh
// term is dropped as it lives at 2^128) ⇒ the representability test is the
// injectivity of ℤ→Tr. All cset, NO branches. Pure-register AArch64, shared by
// darwin+ELF (same ISA). The backend pre-places: x0=al, x1=bl, x9=&res;
// scratch x10..x15. op 0=+ 1=- 2=*; a/b_sg = operand is signed; r_sg,rw = *res.
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
    ); // *res = the low rw bytes
    let wb = rw * 8;
    if !r_sg {
        // unsigned: representable ⟺ rh==0 ∧ (rl>>wb)==0
        _ = writeln!(s, "\tmov x14, x13");
        if wb < 64 {
            _ = writeln!(s, "\torr x14, x14, x12, lsr #{wb}");
        }
        _ = writeln!(s, "\tcmp x14, #0\n\tcset x0, ne");
    } else {
        // signed: representable ⟺ sign-extend(rl, wb) == {rh,rl}
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

// EXT(gcc): `__builtin_<f>` where <f> is a real C library function (abort, memcpy,
// printf…) → GCC lowers it to the libc symbol; zcc strips the prefix and calls it
// directly. A mandatory ALLOWLIST (default-deny): any builtin NOT here is a pure
// compiler intrinsic (clrsb, parity, frame_address, apply, va_arg_pack,
// mul_overflow_p…) — stripping it would emit a call to a NON-existent symbol →
// as/ld chokes (a silent miscompile). Under the 2-fact rule, an unknown builtin
// must be REJECTED CLEANLY, never silently turned into a libc call. The list =
// the C89/C99 library functions + POSIX/GNU-string functions the REAL corpus
// touches (test-first).
pub fn builtin_is_libc(f: &str) -> bool {
    // fortified `__builtin___memcpy_chk` → strips to `__memcpy_chk` (a real musl symbol)
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
        // math.h (libm — the driver links -lm)
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
