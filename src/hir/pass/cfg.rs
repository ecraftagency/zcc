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
//   (e) THREADING A KNOWN CONDITION (R4.5). If S is instruction-free and ends in
//       `br p, X, Y` where `p` is one of S's own parameters, then a predecessor
//       that passes a LITERAL for `p` already knows which edge S will take —
//       ⟦br k,X,Y⟧ is ⟦jmp X⟧ for k≠0 and ⟦jmp Y⟧ for k=0, exactly identity (a),
//       except that the literal is not in S's terminator but on one incoming
//       edge. That predecessor names X (or Y) directly, and the run is the same:
//       S contributes no state transition, and the arguments X/Y receive are
//       S's own, with S's parameters replaced by what that edge passed for them.
//       This is what C's `&&` and `||` produce — one arm computes a relation,
//       the other passes 0 — and it is why §13n row (d) measured 3,707
//       pure-boolean `csel` and 669 `csel → cbnz` against gcc's 9: the two arms
//       met in a φ and the φ was branched on, where gcc branches on flags. After
//       (e) the constant arm skips the merge entirely and (d) folds what is left
//       into the block that computed the relation, which is what lets isel fuse
//       the compare into the branch.
//   (f) A BRANCH ON A SELECT OF TWO LITERALS. `br (c ? k₁ : k₂), X, Y` observes
//       only whether the selected literal is nonzero, so it is `br c` (both
//       literals nonzero/zero collapse to a jump, and a swapped pair swaps the
//       targets). The same shape as (e), reached when the merge has already been
//       if-converted into a value.
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
        changed |= thread_known_condition(f);
        changed |= merge(f);
        any |= changed;
        if !changed {
            break;
        }
    }
    any
}

/// (f) — a branch whose condition is a select between two literals observes only
/// which literal is nonzero.
fn fold_select_conditions(f: &mut Func) -> bool {
    let mut changed = false;
    for b in 0..f.blocks.len() {
        let (v, x, y) = match &f.blocks[b].term {
            Term::Br(Operand::Val(v), x, y) => (*v, x.clone(), y.clone()),
            _ => continue,
        };
        let Def::Inst(db, di) = f.values[v as usize].def else { continue };
        let Some(Inst::Select { c, a, b: fb, .. }) = f.blocks[db as usize].insts.get(di as usize)
        else {
            continue;
        };
        let (c, a, fb) = (*c, *a, *fb);
        let (Operand::Imm(k1), Operand::Imm(k2)) = (a, fb) else { continue };
        f.blocks[b].term = match (k1 != 0, k2 != 0) {
            (true, false) => Term::Br(c, x, y),
            (false, true) => Term::Br(c, y, x),
            (true, true) => Term::Jmp(x),
            (false, false) => Term::Jmp(y),
        };
        changed = true;
    }
    changed
}

/// (e) — a predecessor that passes a literal for the parameter a forwarding
/// block branches on already knows which way that branch goes.
fn thread_known_condition(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let pin = pinned(f);
    let n = f.blocks.len();
    // THE SIDE CONDITION THAT MAKES THIS A PROOF. Skipping S skips the
    // DEFINITION of S's parameters, and a parameter may be read far below S —
    // every block S dominates could use it, which is precisely what SSA
    // licences. Substituting the parameters into the arguments of the target
    // reaches only the first block; a use one level deeper would be left naming
    // a value that the threaded path never defines. So S is threadable only
    // when every parameter it defines is used NOWHERE but its own terminator,
    // and the substitution below therefore removes every occurrence.
    //
    // (Nothing else about S can lose dominance: a strict dominator D of S
    // dominates every predecessor P of S — extend any path entry→P by the edge
    // P→S and D must lie on it, and D ≠ S — so the values the threaded edge
    // still names are all defined above P.)
    //
    // Found by `hir::verify` in one run: without it, `t: %24 used in bb6 but
    // defined in bb2` — a loop header whose induction parameter the body read
    // directly (torture pr54937, pr109925, pr116799, and sqlite `unixLock`).
    let mut uses = vec![0u32; f.values.len()];
    for b in &f.blocks {
        for inst in &b.insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    uses[v as usize] += 1;
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                uses[v as usize] += 1;
            }
        });
    }
    // `known[s] = Some((param index, X, Y))` for a block whose whole content is
    // a branch on one of its own parameters.
    let mut known: Vec<Option<(usize, Target, Target)>> = vec![None; n];
    for s in 0..n {
        if pin[s] || !c.reachable(s as BlockId) || !f.blocks[s].labels.is_empty() {
            continue;
        }
        let blk = &f.blocks[s];
        if !blk.insts.is_empty() {
            continue;
        }
        let Term::Br(Operand::Val(v), x, y) = &blk.term else { continue };
        // …and it must be S's OWN parameter: a value defined elsewhere is not
        // decided by the incoming edge.
        let Some(k) = blk.params.iter().position(|p| p == v) else { continue };
        if x.block as usize == s || y.block as usize == s {
            continue; // a self-edge would thread into the block being skipped
        }
        // every parameter used only here, so the substitution below is total
        let mut here = vec![0u32; blk.params.len()];
        blk.term.uses(|o| {
            if let Operand::Val(v) = o {
                if let Some(j) = blk.params.iter().position(|p| p == &v) {
                    here[j] += 1;
                }
            }
        });
        if blk.params.iter().zip(&here).any(|(p, n)| uses[*p as usize] != *n) {
            continue;
        }
        known[s] = Some((k, x.clone(), y.clone()));
    }
    if known.iter().all(|x| x.is_none()) {
        return false;
    }
    let mut changed = false;
    for b in 0..n {
        if !c.reachable(b as BlockId) {
            continue;
        }
        let mut term = f.blocks[b].term.clone();
        {
            // As in `thread`: a predecessor must not name one successor twice
            // with different arguments — `Cfg` dedups successors, so the second
            // edge would vanish from every analysis while ⟦·⟧ still takes it.
            let mut seen: Vec<BlockId> = term.targets().iter().map(|t| t.block).collect();
            for (i, t) in term.targets_mut().into_iter().enumerate() {
                let Some((k, x, y)) = &known[t.block as usize] else { continue };
                let taken = match t.args.get(*k) {
                    Some(Operand::Imm(v)) => {
                        if *v != 0 {
                            x
                        } else {
                            y
                        }
                    }
                    _ => continue,
                };
                if taken.block as usize == b
                    || seen.iter().enumerate().any(|(j, &sb)| j != i && sb == taken.block)
                {
                    continue;
                }
                // S's parameters, as this edge binds them, substituted into the
                // arguments S would have passed on.
                let params = &f.blocks[t.block as usize].params;
                let mut dest = taken.clone();
                for a in dest.args.iter_mut() {
                    if let Operand::Val(v) = *a {
                        if let Some(j) = params.iter().position(|p| *p == v) {
                            match t.args.get(j) {
                                Some(o) => *a = *o,
                                None => {}
                            }
                        }
                    }
                }
                // …but only if every parameter it names was actually bound: a
                // half-substituted argument would name a value defined in a
                // block this edge no longer passes through.
                let ok = dest.args.iter().all(|a| match a {
                    Operand::Val(v) => !params.contains(v),
                    _ => true,
                });
                if !ok {
                    continue;
                }
                seen[i] = dest.block;
                *t = dest;
                changed = true;
            }
        }
        f.blocks[b].term = term;
    }
    changed
}

/// (a) + (b) + (f)
fn fold_terms(f: &mut Func) -> bool {
    let mut changed = fold_select_conditions(f);
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
            // Substituting a parameter by its argument is only a renaming when
            // the two have the SAME type. `build` occasionally hands an edge a
            // value wider than the parameter it feeds (a promoted `char`), which
            // is harmless while the parameter exists to narrow it and ill-typed
            // the moment it does not.
            let typed = f.blocks[s as usize]
                .params
                .iter()
                .zip(args.iter())
                .all(|(p, a)| match a.val() {
                    Some(v) => f.ty_of(v) == f.ty_of(*p),
                    None => true,
                });
            if !typed {
                continue;
            }
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
