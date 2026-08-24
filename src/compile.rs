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
    let mut h = phase("hir::build", || crate::hir::build::build(ast));
    if optimize() {
        phase("hir::pass", || crate::hir::pass::run_module(&mut h));
    }
    let h = h;
    let m = backend(&h).unwrap_or_else(|e| panic!("zcc: internal: {}", e));
    phase("emit", || crate::emit::emit(ast, &m))
}

/// The HIR pass ladder is ON unless `ZCC_O0` says otherwise. The switch exists
/// for ONE reason: `tests/opt-parity.sh` compiles every torture program twice and
/// compares the two runs, which is the whole-compiler confirmation that the
/// ladder preserves meaning (REARCH §10 — it CONFIRMS; the batteries discover).
pub fn optimize() -> bool {
    std::env::var_os("ZCC_O0").is_none()
}

/// Wall-clock per pipeline stage, printed when `ZCC_TIME` is set. A performance
/// claim is a measurement (Article E), and the measurement has to be available
/// without rebuilding the compiler.
pub fn phase<T>(name: &str, f: impl FnOnce() -> T) -> T {
    if std::env::var_os("ZCC_TIME").is_none() {
        return f();
    }
    let t = std::time::Instant::now();
    let r = f();
    eprintln!("[time] {:<16} {:>8.1} ms", name, t.elapsed().as_secs_f64() * 1e3);
    r
}

/// HIR → final MIR. Separated from `compile` so every battery can drive the
/// exact pipeline the compiler drives, rather than an approximation of it.
pub fn backend(h: &crate::hir::Module) -> Result<crate::mir::MModule, String> {
    let mut m = allocated(h)?;
    finish(&mut m);
    Ok(m)
}

/// HIR → physical MIR, stopping BEFORE frame lowering. Split out so the
/// frame/layout square (`⟦mir_p⟧ = ⟦mir_final⟧`) has two sides to compare.
pub fn allocated(h: &crate::hir::Module) -> Result<crate::mir::MModule, String> {
    let mut m = phase("isel", || crate::isel::lower(h));
    phase("regalloc", || crate::regalloc::allocate_module(&mut m))?;
    Ok(m)
}

/// Frame lowering and block layout: physical MIR → final MIR.
pub fn finish(m: &mut crate::mir::MModule) {
    phase("frame+layout", || {
        for f in m.funcs.iter_mut() {
            crate::mir::pass::frame::run(f);
            crate::mir::pass::legalize::run(f);
            crate::mir::pass::layout::run(f);
        }
    });
}
