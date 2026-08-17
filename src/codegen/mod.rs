// Tầng codegen: mỗi target một file con, hợp đồng chung tối giản:
//   emit(&Ast) -> String   — text assembly cho as của target đó.
// Mọi hiểu biết về ABI/section/asm syntax nằm TRONG file target, không rò ra ngoài.
// Thêm target mới = thêm file + một nhánh match ở đây + nhánh toolchain bên driver.
use crate::ast::{Ast, Target};

pub mod arm64_darwin;
pub mod arm64_elf;

pub fn emit(ast: &Ast) -> String {
    match ast.tgt {
        Target::Arm64Darwin => arm64_darwin::emit(ast),
        Target::Arm64Elf => arm64_elf::emit(ast),
    }
}
