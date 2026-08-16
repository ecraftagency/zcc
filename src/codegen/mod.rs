// Tầng codegen: mỗi target một file con, hợp đồng chung tối giản:
//   emit(&Ast) -> String   — text assembly cho as của target đó.
// Mọi hiểu biết về ABI/section/asm syntax nằm TRONG file target, không rò ra ngoài.
// Thêm target mới (vd elf x86_64, arm64 freestanding ELF) = thêm file + một nhánh
// match ở đây + nhánh toolchain (as/ld) tương ứng bên driver. Hiện lock arm64 Darwin.
use crate::ast::Ast;

pub mod arm64_darwin;

pub fn emit(ast: &Ast) -> String {
    arm64_darwin::emit(ast)
}
