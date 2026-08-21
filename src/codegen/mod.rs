// Tầng codegen: mỗi target một file con, hợp đồng chung tối giản:
//   emit_ir(&Ast) -> String   — text assembly cho as của target đó.
// Mọi hiểu biết về ABI/section/asm syntax nằm TRONG file target, không rò ra ngoài.
// Thêm target mới = thêm file + một nhánh dispatch ở đây + nhánh toolchain bên driver.
use crate::ast::Ast;

pub mod arm64_elf;

// Đường DUY NHẤT: lower(AST) → IR → passes → asm. AST-walk emit() đã xoá (seal-ir-10k):
// mọi construct C99 lower thành typed Inst, backend simulate per-inst. Xem IR.md.
pub fn emit_ir(ast: &Ast) -> String {
    arm64_elf::emit_ir(ast)
}
