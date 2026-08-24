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
#[cfg(test)]
mod tests;
pub mod sccp;

use super::*;

/// Rounds of the ladder. Three is gcc's own practical bound for -O1-class
/// pipelines: the interesting cascades (fold → branch removal → merge →
/// re-number) are two levels deep, and the third is the confirmation round that
/// usually changes nothing. It is a TERMINATION bound, not a correctness one —
/// each pass is individually meaning-preserving, so any number of rounds is
/// sound and this one only decides how much is left on the table.
pub const ROUNDS: u32 = 3;

pub fn run_module(m: &mut Module) {
    for f in m.funcs.iter_mut() {
        run(f);
    }
}

pub fn run(f: &mut Func) {
    // Critical edges are split once, up front: sccp and gvn both want to place a
    // value on an edge, and a critical edge offers nowhere to put it.
    dom::split_critical_edges(f);
    for _ in 0..ROUNDS {
        let mut changed = false;
        changed |= cfg::run(f);
        changed |= sccp::run(f);
        changed |= gvn::run(f);
        changed |= dce::run(f);
        if !changed {
            break;
        }
    }
    cfg::run(f);
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
