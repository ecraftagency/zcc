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
