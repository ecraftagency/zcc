// cfg_simplify (REARCH §4 row 1) — the four control-flow identities.
//
// Each is a commuting square proven by inspection of ⟦hir⟧'s terminator rule,
// and none of them touches a value:
//
//   (a) CONSTANT TERMINATOR. ⟦br k, x, y⟧ takes `x` iff k≠0 — with k a literal
//       the other edge is never taken, so `jmp` computes the same successor.
//       Likewise a `switch` on a literal.
//   (b) DEGENERATE BRANCH. `br c, t(a), t(a)` reaches t with the same arguments
//       either way; ⟦·⟧ never observes c, and c carries no effect (it is an
//       operand, and operands are effect-free by construction).
//   (c) THREADING. An empty parameterless block whose terminator is `jmp T(a)`
//       contributes no state transition, so a predecessor may name T directly.
//       The arguments stay legal because a value dominating that block also
//       dominates every predecessor of it (a path entry→P extends to entry→P→B,
//       so a definition on the second path lies on the first).
//   (d) MERGE. If B ends in `jmp S(a)` and S's only predecessor is B, the two
//       blocks are always executed in sequence, and S's parameters take `a` on
//       the one edge that exists — so substituting a for them and concatenating
//       is the same run.
//
// UNREACHABLE blocks are emptied rather than removed, because a block INDEX is
// observable in two places (`Sym::Label`, `goto *`'s edge set) and renumbering
// would invalidate both. An emptied block is never entered, so ⟦·⟧ is unchanged.
use super::*;

pub fn run(f: &mut Func) -> bool {
    let mut any = false;
    // A sweep can expose work for the next one (a merge makes the merged block's
    // successor single-predecessor). The bound is termination insurance: each
    // successful sweep strictly removes a block or an edge.
    for _ in 0..f.blocks.len().max(1) {
        let mut changed = fold_terms(f);
        changed |= drop_unreachable(f);
        changed |= thread(f);
        changed |= merge(f);
        any |= changed;
        if !changed {
            break;
        }
    }
    any
}

/// (a) + (b)
fn fold_terms(f: &mut Func) -> bool {
    let mut changed = false;
    for b in f.blocks.iter_mut() {
        let new = match &b.term {
            Term::Br(Operand::Imm(k), x, y) => {
                Some(Term::Jmp(if *k != 0 { x.clone() } else { y.clone() }))
            }
            Term::Br(_, x, y) if x.block == y.block && x.args == y.args => Some(Term::Jmp(x.clone())),
            Term::Switch(Operand::Imm(k), ty, arms, d) => {
                let k = crate::hir::interp::sext(*k as u64, *ty);
                Some(Term::Jmp(match arms.iter().find(|(v, _)| *v == k) {
                    Some((_, t)) => t.clone(),
                    None => d.clone(),
                }))
            }
            Term::Switch(_, _, arms, d) if arms.iter().all(|(_, t)| t.block == d.block && t.args == d.args) => {
                Some(Term::Jmp(d.clone()))
            }
            _ => None,
        };
        if let Some(t) = new {
            b.term = t;
            changed = true;
        }
    }
    changed
}

fn drop_unreachable(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let pin = pinned(f);
    let mut changed = false;
    for b in 0..f.blocks.len() {
        if c.reachable(b as BlockId) || pin[b] {
            continue;
        }
        let blk = &mut f.blocks[b];
        if blk.insts.is_empty() && blk.params.is_empty() && matches!(blk.term, Term::Unreachable) {
            continue;
        }
        blk.insts.clear();
        blk.params.clear();
        blk.term = Term::Unreachable;
        changed = true;
    }
    changed
}

/// (c) — redirect every edge into an empty forwarding block.
fn thread(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let pin = pinned(f);
    let n = f.blocks.len();
    // `fwd[b] = Some(target)` when b is a pure forwarder.
    let mut fwd: Vec<Option<Target>> = vec![None; n];
    for b in 0..n {
        if pin[b] || !c.reachable(b as BlockId) || !f.blocks[b].labels.is_empty() {
            continue;
        }
        let blk = &f.blocks[b];
        if !blk.insts.is_empty() || !blk.params.is_empty() {
            continue;
        }
        if let Term::Jmp(t) = &blk.term {
            if t.block as usize != b {
                fwd[b] = Some(t.clone());
            }
        }
    }
    if fwd.iter().all(|x| x.is_none()) {
        return false;
    }
    let mut changed = false;
    for b in 0..n {
        if !c.reachable(b as BlockId) {
            continue;
        }
        let mut term = f.blocks[b].term.clone();
        {
            // A predecessor must not end up naming the same successor twice with
            // different arguments: `Cfg` dedups successors, so the second edge
            // would vanish from every analysis while ⟦·⟧ still takes it.
            let mut seen: Vec<BlockId> = term.targets().iter().map(|t| t.block).collect();
            for (i, t) in term.targets_mut().into_iter().enumerate() {
                // one hop only; a chain of forwarders collapses over sweeps
                if let Some(dest) = &fwd[t.block as usize] {
                    if dest.block as usize == b || seen.iter().enumerate().any(|(j, &s)| j != i && s == dest.block) {
                        continue;
                    }
                    seen[i] = dest.block;
                    *t = dest.clone();
                    changed = true;
                }
            }
        }
        f.blocks[b].term = term;
    }
    changed
}

/// (d) — concatenate a block with its only successor.
fn merge(f: &mut Func) -> bool {
    let mut changed = false;
    loop {
        let c = dom::cfg(f);
        let pin = pinned(f);
        let mut done = true;
        for b in 0..f.blocks.len() {
            if !c.reachable(b as BlockId) {
                continue;
            }
            let s = match &f.blocks[b].term {
                Term::Jmp(t) if t.block as usize != b => t.block,
                _ => continue,
            };
            // A labelled successor may NOT be absorbed: `emit` writes the
            // `lg_<func>.<label>` symbol at the head of the block that carries
            // the label, and a static initializer may hold that address — after
            // a merge the symbol would name B's first instruction instead of
            // S's, moving a program point the linker can see.
            if pin[s as usize] || c.preds[s as usize].len() != 1 || !f.blocks[s as usize].labels.is_empty() {
                continue;
            }
            let args = match &f.blocks[b].term {
                Term::Jmp(t) => t.args.clone(),
                _ => unreachable!(),
            };
            let succ = std::mem::replace(
                &mut f.blocks[s as usize],
                Block { params: Vec::new(), insts: Vec::new(), term: Term::Unreachable, labels: Vec::new(), weight: 1 },
            );
            let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
            for (p, a) in succ.params.iter().zip(args.iter()) {
                map[*p as usize] = Some(*a);
            }
            f.blocks[b].insts.extend(succ.insts);
            f.blocks[b].term = succ.term;
            f.blocks[b].weight = f.blocks[b].weight.max(succ.weight);
            rewrite_values(f, &map);
            refresh_defs(f);
            changed = true;
            done = false;
            break;
        }
        if done {
            return changed;
        }
    }
}
