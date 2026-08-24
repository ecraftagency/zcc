// The single door from the frontend to the layered backend (REARCH.md §2).
// main.rs knows only this function; every layer below is private to the pipeline.
//
//   AST + TyTab
//     ──hir::build (Braun on-the-fly SSA)──► HIR
//     ──hir passes──► HIR
//     ──isel (munch + AAPCS64)──► MIR (SSA, virtual)
//     ──mir passes──► MIR
//     ──regalloc (spill → color → destruct)──► MIR (physical)
//     ──frame / layout──► MIR (final)
//     ──emit──► .s text
use crate::ast::Ast;

pub fn compile(ast: &Ast) -> String {
    let h = crate::hir::build::build(ast);
    let m = backend(&h).unwrap_or_else(|e| panic!("zcc: internal: {}", e));
    crate::emit::emit(ast, &m)
}

/// HIR → final MIR. Separated from `compile` so every battery can drive the
/// exact pipeline the compiler drives, rather than an approximation of it.
pub fn backend(h: &crate::hir::Module) -> Result<crate::mir::MModule, String> {
    let mut m = crate::isel::lower(h);
    crate::regalloc::allocate_module(&mut m)?;
    for f in m.funcs.iter_mut() {
        crate::mir::pass::frame::run(f);
    }
    Ok(m)
}
