// The HIR pass ladder (REARCH.md §4) — the tree-SSA half of gcc -O1, re-realized
// on this architecture.
//
// Every pass in here is an HIR→HIR function shipping the commuting square
// ⟦f⟧ = ⟦P f⟧ (Law 3). The square is not a comment: `hir::tests` runs the WHOLE
// battery corpus through both sides of it, so a pass that changes an observable
// value fails `cargo test` before any machine layer exists to hide it.
//
// Order mirrors gcc -O1's `-ftree-*` sequence, bounded to a small fixpoint —
// each pass exposes work for the next (sccp folds a branch, cfg_simplify deletes
// the arm, gvn re-numbers what the deletion merged), and the rounds stop when a
// round changes nothing or the bound is reached.
pub mod cfg;
pub mod dce;
pub mod fold;
pub mod gvn;
pub mod ifconv;
pub mod inline;
pub mod iv;
pub mod licm;
pub mod mem;
pub mod purity;
pub mod rotate;
pub mod scev;
#[cfg(test)]
mod tests;
pub mod sccp;
pub mod sink;
pub mod sroa;

use super::*;

/// Rounds of the ladder. Three is gcc's own practical bound for -O1-class
/// pipelines: the interesting cascades (fold → branch removal → merge →
/// re-number) are two levels deep, and the third is the confirmation round that
/// usually changes nothing. It is a TERMINATION bound, not a correctness one —
/// each pass is individually meaning-preserving, so any number of rounds is
/// sound and this one only decides how much is left on the table.
pub const ROUNDS: u32 = 3;

pub fn run_module(m: &mut Module) {
    run_module_with(m, &std::collections::HashSet::new());
}

/// `pinned` names functions a STATIC INITIALIZER refers to (`static void (*p)() =
/// &f;`). HIR has no view of the data segment, so the caller supplies it — and
/// without it the inliner would delete a function the linker still needs.
pub fn run_module_with(m: &mut Module, pinned: &std::collections::HashSet<String>) {
    // The purity set is INTERPROCEDURAL, so it is computed on the module and
    // handed to the per-function ladder rather than rediscovered inside it. It
    // is recomputed after inlining, which changes both the call graph and the
    // set of functions.
    let ro = readonly(m);
    for f in m.funcs.iter_mut() {
        run_with(f, &ro);
    }
    // Inlining is the one INTERPROCEDURAL row, so it runs between two
    // intra-procedural sweeps rather than inside one: the callee must already be
    // optimized when it is spliced in (its locals promoted, its constants
    // folded), and the caller must be re-optimized afterwards, because a call
    // replaced by a body is exactly the shape the other rows feed on.
    if on("inline") && inline::run_module(m, pinned) {
        let ro = readonly(m);
        for f in m.funcs.iter_mut() {
            run_with(f, &ro);
        }
    }
}

fn readonly(m: &Module) -> std::collections::HashSet<String> {
    match on("purecall") {
        true => {
            let r = purity::readonly_functions(m);
            if licm::residual_wanted() {
                eprintln!("RESIDUAL readonly={} of {}", r.len(), m.funcs.len());
            }
            r
        }
        false => std::collections::HashSet::new(),
    }
}

pub fn run(f: &mut Func) {
    run_with(f, &std::collections::HashSet::new())
}

pub fn run_with(f: &mut Func, ro: &std::collections::HashSet<String>) {
    // Critical edges are split once, up front: sccp and gvn both want to place a
    // value on an edge, and a critical edge offers nowhere to put it.
    dom::split_critical_edges(f);
    for _ in 0..ROUNDS {
        let mut changed = false;
        if on("cfg") {
            changed |= cfg::run(f);
        }
        if on("sroa") {
            changed |= sroa::run(f);
        }
        if on("sccp") {
            changed |= sccp::run(f);
        }
        if on("gvn") {
            changed |= fold::canon(f);
            changed |= gvn::run(f);
        }
        if on("mem") {
            changed |= mem::run(f);
        }
        if on("ifconv") {
            changed |= ifconv::run(f);
        }
        // Rotation runs BEFORE licm, not after: a bottom-tested loop is what
        // makes "the loop runs at least once" structural rather than
        // arithmetic, and that is the fence licm's call hoist was refusing on.
        if on("rotate") {
            changed |= rotate::run(f);
        }
        if on("licm") {
            changed |= licm::run_with(f, ro);
        }
        // AFTER licm and rotation: the loop must already be in its final shape,
        // because the recurrence this reads is a property of that shape. Before
        // dce, which is what removes the address chain it replaces.
        if on("iv") {
            changed |= iv::run(f);
        }
        // Widening is a SEPARATE row from the pointer walk above and is ON: it
        // removes the per-iteration `sxtw` that stands between an `a[i]` loop
        // and gcc's (§13l).
        if on("widen") {
            changed |= iv::widen(f);
        }
        if on("sink") {
            changed |= sink::run(f);
        }
        if on("dce") {
            changed |= dce::run(f);
        }
        if !changed {
            break;
        }
    }
    if on("cfg") {
        cfg::run(f);
    }
}

/// `ZCC_NOPASS=gvn,mem` disables the named rows. This is a BISECTION tool, not a
/// tuning knob: when a differential suite reports a wrong answer, the first
/// question is which theorem's square is false, and answering it by rebuilding
/// the compiler six times is the slow path Law 2 warns about. No shipped
/// configuration reads it.
fn on(name: &str) -> bool {
    match std::env::var("ZCC_NOPASS") {
        Ok(v) => !v.split(',').any(|x| x == name),
        Err(_) => true,
    }
}

// ── shared plumbing every pass needs ───────────────────────────────────────

/// Recompute every `ValueInfo::def` from where the definition actually sits.
/// Any pass that moves, deletes or inserts an instruction calls this instead of
/// maintaining def records by hand — the verifier checks the record against the
/// layout, so one mechanical recomputation is both shorter and impossible to get
/// subtly wrong. Values whose definition has disappeared keep a stale record and
/// are simply never reached again (the same status a DCE'd value has).
pub fn refresh_defs(f: &mut Func) {
    for bi in 0..f.blocks.len() {
        for k in 0..f.blocks[bi].params.len() {
            let p = f.blocks[bi].params[k];
            f.values[p as usize].def = Def::Param(bi as BlockId, k as u32);
        }
        for i in 0..f.blocks[bi].insts.len() {
            if let Some(d) = f.blocks[bi].insts[i].dst() {
                f.values[d as usize].def = Def::Inst(bi as BlockId, i as u32);
            }
        }
    }
}

/// Blocks whose IDENTITY is observable, so no pass may delete, merge or thread
/// them. Two EXT(gcc) constructs pin a block: `&&label` takes its address as a
/// datum (`Sym::Label`), and `goto *e` names it as a CFG successor. The entry is
/// pinned because the ABI materializes parameters there.
pub fn pinned(f: &Func) -> Vec<bool> {
    let mut p = vec![false; f.blocks.len()];
    p[f.entry as usize] = true;
    for b in &f.blocks {
        for inst in &b.insts {
            if let Inst::SymAddr { sym: Sym::Label(t), .. } = inst {
                p[*t as usize] = true;
            }
        }
        if let Term::GotoPtr(_, bs) = &b.term {
            for &t in bs {
                p[t as usize] = true;
            }
        }
    }
    p
}

/// Apply a value substitution everywhere: instruction operands, terminator
/// operands and block arguments. `map[v]` is the value `v` becomes; the walk is
/// transitive (a chain v→w→x resolves to x) and stops at a self-map.
pub fn rewrite_values(f: &mut Func, map: &[Option<Operand>]) {
    let resolve = |o: Operand| -> Operand {
        let mut cur = o;
        // A substitution chain is acyclic by construction (a value is only ever
        // mapped to one that already existed), but the bound keeps a corrupted
        // map from hanging the compiler instead of failing a test.
        for _ in 0..64 {
            match cur {
                Operand::Val(v) => match map.get(v as usize).and_then(|x| *x) {
                    Some(n) if n != cur => cur = n,
                    _ => return cur,
                },
                _ => return cur,
            }
        }
        cur
    };
    for b in f.blocks.iter_mut() {
        for inst in b.insts.iter_mut() {
            inst.uses_mut(|o| *o = resolve(*o));
        }
        match &mut b.term {
            Term::Br(c, ..) | Term::Switch(c, ..) | Term::GotoPtr(c, _) => *c = resolve(*c),
            Term::Ret(Some(v)) => *v = resolve(*v),
            _ => {}
        }
        for t in b.term.targets_mut() {
            for a in t.args.iter_mut() {
                *a = resolve(*a);
            }
        }
    }
}
