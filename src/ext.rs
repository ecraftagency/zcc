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

// __ATOMIC_RELAXED..__ATOMIC_SEQ_CST = 0..5 (giá trị như GCC); sự TỒN TẠI của
// __ATOMIC_SEQ_CST là cái hdr_atomic.h dò để chọn path __atomic
pub const ATOMIC_ORDERS: &[&str] = &[
    "__ATOMIC_RELAXED",
    "__ATOMIC_CONSUME",
    "__ATOMIC_ACQUIRE",
    "__ATOMIC_RELEASE",
    "__ATOMIC_ACQ_REL",
    "__ATOMIC_SEQ_CST",
];

pub fn sync_op(name: &str) -> Option<(SyncOp, usize)> {
    Some(match name {
        "__sync_fetch_and_add" => (SyncOp::FetchAdd, 2),
        "__sync_add_and_fetch" => (SyncOp::AddFetch, 2),
        "__sync_fetch_and_sub" => (SyncOp::FetchSub, 2),
        "__sync_sub_and_fetch" => (SyncOp::SubFetch, 2),
        "__sync_val_compare_and_swap" => (SyncOp::ValCas, 3),
        "__sync_bool_compare_and_swap" => (SyncOp::BoolCas, 3),
        "__sync_lock_test_and_set" => (SyncOp::TestSet, 2),
        "__sync_lock_release" => (SyncOp::Release, 1),
        "__sync_synchronize" => (SyncOp::Barrier, 0),
        _ => return None,
    })
}
