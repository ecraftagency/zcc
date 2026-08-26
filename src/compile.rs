// The single door from the frontend to the layered backend (REARCH.md §2).
// THEORY A5 — the isel/ABI seam; THEORY A6 — HIR; THEORY A6b — MIR
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
        let pinned = pinned_symbols(ast);
        phase("hir::pass", || crate::hir::pass::run_module_with(&mut h, &pinned));
    }
    // Law 3 at the cheapest layer: the HIR verifier decides dominance, edge
    // arity and type agreement on the IR alone, so a pass that breaks one of
    // them is named HERE instead of surfacing as a wrong answer — or, worse, as
    // an allocator that runs out of registers three layers down.
    phase("hir::verify", || {
        for f in &h.funcs {
            if let Err(e) = crate::hir::verify::verify(f) {
                panic!("zcc: internal: {}", e);
            }
        }
    });
    let h = h;
    let m = backend(&h).unwrap_or_else(|e| panic!("zcc: internal: {}", e));
    phase("emit", || crate::emit::emit(ast, &m))
}

/// Every symbol a STATIC INITIALIZER or an ALIAS names. A function whose address
/// a data object holds, or that an `__attribute__((alias))` resolves to, is
/// referenced by the linker, not by any instruction, so HIR cannot tell that it
/// is still needed — and the inliner must not delete it.
pub fn pinned_symbols(ast: &Ast) -> std::collections::HashSet<String> {
    fn walk(i: &crate::ast::GInit, out: &mut std::collections::HashSet<String>) {
        match i {
            crate::ast::GInit::Addr(n, _) => {
                out.insert(n.trim_start_matches('\u{1}').to_string());
            }
            crate::ast::GInit::Diff(a, b) => {
                out.insert(a.trim_start_matches('\u{1}').to_string());
                out.insert(b.trim_start_matches('\u{1}').to_string());
            }
            crate::ast::GInit::List(items) => {
                for (_, _, x) in items {
                    walk(x, out);
                }
            }
            _ => {}
        }
    }
    let mut out = std::collections::HashSet::new();
    for g in &ast.globals {
        walk(&g.init, &mut out);
    }
    // EXT(gcc) `__attribute__((alias("old")))` — musl's `weak_alias(dummy, _init)`
    // over a `static void dummy(void){}`. `.set _init, dummy` is the ONLY reference
    // to `dummy` and it lives in the emitted text, not in HIR, so without this the
    // inliner deletes the target and the link fails on an undefined `dummy`.
    for (_, old, _) in &ast.aliases {
        out.insert(old.trim_start_matches('\u{1}').to_string());
    }
    out
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
    // MIR passes on SSA, before allocation (REARCH §8, the pre-allocation half).
    phase("mir::pass", || {
        for f in m.funcs.iter_mut() {
            crate::mir::pass::ext::run(f);
            crate::mir::pass::cmpelim::run(f);
            crate::mir::pass::cmpelim::branch_on_flags(f);
            crate::mir::pass::const_share::run(f);
            crate::mir::pass::autoinc::run(f);
        }
    });
    // R4.18's instrument: the TIME model, read beside the size model. Placed
    // here because the recurrence is a property of SSA MIR — the loop-carried
    // edges are the header parameters, and `regalloc` is about to destroy them.
    for f in m.funcs.iter() {
        crate::mir::cost::report(f);
    }
    phase("mir::verify", || {
        for f in &m.funcs {
            crate::mir::verify::verify(f)?;
        }
        Ok::<(), String>(())
    })?;
    phase("regalloc", || crate::regalloc::allocate_module(&mut m))?;
    Ok(m)
}

/// Frame lowering and block layout: physical MIR → final MIR.
pub fn finish(m: &mut crate::mir::MModule) {
    phase("frame+layout", || {
        for f in m.funcs.iter_mut() {
            crate::mir::pass::frame::drop_dead_spills(f);
            crate::mir::pass::frame::run(f);
            crate::mir::pass::shrink_wrap::run(f);
            crate::mir::pass::frame::merge_epilogues(f);
            crate::mir::pass::legalize::run(f);
            crate::mir::pass::ldstp::run(f);
            crate::mir::pass::frame_fold::run(f);
            crate::mir::pass::layout::run(f);
            crate::mir::pass::layout::drop_dead_copies(f);
        }
    });
}
