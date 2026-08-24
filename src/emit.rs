// MIR(final) → AArch64-ELF assembly text (REARCH.md §9, THEORY II-4).
// R0.8 fills this in; until the MIR layers exist it produces only the data
// sections, so the pipeline is wired end to end from the first commit.
use crate::ast::Ast;
use crate::mir::MModule;

pub fn emit(_ast: &Ast, _m: &MModule) -> String {
    String::new()
}
