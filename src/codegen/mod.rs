// Tầng codegen: mỗi target một file con, hợp đồng chung tối giản:
//   emit(&Ast) -> String   — text assembly cho as của target đó.
// Mọi hiểu biết về ABI/section/asm syntax nằm TRONG file target, không rò ra ngoài.
// Thêm target mới = thêm file + một nhánh dispatch ở đây + nhánh toolchain bên driver.
use crate::ast::Ast;

pub mod arm64_elf;

pub fn emit(ast: &Ast) -> String {
    arm64_elf::emit(ast)
}

// Đường IR (migrate, --ir): lower(AST) → IR → asm. Song song emit() tới khi phủ
// hết suite; rồi thay emit() + xoá đường AST-walk. Xem IR.md, arm64_elf::emit_ir.
pub fn emit_ir(ast: &Ast) -> String {
    arm64_elf::emit_ir(ast)
}
