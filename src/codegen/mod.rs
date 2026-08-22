// Code generation layer: one child file per target, with a minimal shared contract:
//   emit_ir(&Ast) -> String   — textual assembly for that target's assembler.
// All ABI/section/asm-syntax knowledge resides WITHIN the target file; none leaks out.
// Adding a new target = a new file + one dispatch arm here + a toolchain arm in the driver.
use crate::ast::Ast;

pub mod arm64_elf;

// The SOLE path: lower(AST) → IR → passes → asm. The AST-walk emit() has been removed:
// every C99 construct lowers to a typed Inst, and the backend simulates per-inst. See OPT.md §7.
pub fn emit_ir(ast: &Ast) -> String {
    arm64_elf::emit_ir(ast)
}
