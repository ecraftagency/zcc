// cfg_simplify (MECHANISM.md §G4 row 1) — the four control-flow identities.
// THEORY A7b — optimization: this pass ships its commuting square
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
//   (h) THE COMPARE MOVES WITH IT (2026-08-28). (e) and (g) both demand an
//       INSTRUCTION-FREE S, and the shape they are aimed at rarely is one: C
//       gives `if (cmp_helper(a,b) < 0)` a join carrying -1, 0 or 1 and then a
//       block that COMPARES that parameter before branching, so S holds exactly
//       one instruction. With the compare admitted, a predecessor passing a
//       literal decides it outright (`-1 < 0` is a fact, not a computation) and
//       one passing a value takes the compare with it — one instruction cloned
//       into a predecessor that already jumps nowhere else.
//
//       Measured, `n1_btree_page`: the three-way compare of a b-tree cell is
//       built as an integer, joined, and then re-tested twice per binary-search
//       step, on the loop-carried recurrence. Threading the return sites straight
//       to their successors was worth 10.3% of that program — a quarter of which
//       a peephole on the `cset`/`cmp` pair can reach, and the rest of which is
//       this.
//
//   (g) THE BRANCH MOVES TO THE PREDECESSOR (2026-08-28). Same S as in (e) —
//       instruction-free, ending in `br p, X, Y` on its own parameter — but the
//       edge passes a VALUE rather than a literal. A predecessor whose own
//       terminator is `jmp S(a)` runs S immediately after itself and S does
//       nothing but branch on `a[k]`, so the predecessor may take that branch
//       itself: `jmp S(a)` becomes `br a[k], X', Y'` with S's parameters
//       substituted exactly as in (e). One terminator replaces one terminator,
//       so nothing is duplicated, and the state transition is the same pair of
//       steps written as one.
//
//       This is what C's `&&` leaves behind when its result is TESTED rather
//       than stored: `if (f() && g())` builds a join carrying 0 or the second
//       relation, and without this the relation is materialized with `cset`,
//       jumped over, and tested again — four instructions where the flags the
//       compare already set would have done (`n7_nested_subq`, gcc -O1 emits
//       `cbz` at both call sites).
//
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

/// THEORY A7b  SQUARE cfg_simplify_merges_and_threads — the four control-flow identities
pub fn run(f: &mut Func) -> bool {
    let mut any = false;
    // A sweep can expose work for the next one (a merge makes the merged block's
    // successor single-predecessor). The bound is termination insurance: each
    // successful sweep strictly removes a block or an edge.
    // ONE ANALYSIS PER SWEEP, NOT ONE PER IDENTITY. Four of the six below build
    // a control-flow graph on entry and one of them also walks every instruction
    // for the pin vector — and a late sweep, where only one identity still has
    // work, rebuilt all of it five times over an unchanged function. The graph is
    // therefore built on demand and dropped the moment an identity reports a
    // change, so each identity still reads a graph of the function exactly as it
    // stands when it runs. Same analyses, same order, same rewrites.
    let mut c: Option<dom::Cfg> = None;
    let mut pin: Option<Vec<bool>> = None;
    let mut uses: Option<Vec<u32>> = None;
    for _ in 0..f.blocks.len().max(1) {
        let mut changed = fold_terms(f);
        if changed {
            c = None;
            pin = None;
        }
        if changed {
            uses = None;
        }
        // WHAT EACH IDENTITY CAN DISTURB, and nothing more. The graph goes stale
        // on any change. The PIN VECTOR does not: it marks the entry, the blocks
        // whose address is taken and a computed goto's targets, and only the two
        // identities that can DELETE an instruction or retarget a `GotoPtr` —
        // folding a decided terminator, and emptying an unreachable block — can
        // move it. Threading and merging move instructions and rewrite `Jmp`s and
        // `Br`s, which leaves every one of those marks where it was.
        let hit = {
            let cc = cfg_of(f, &mut c);
            let pp = pin_of(f, &mut pin);
            drop_unreachable(f, cc, pp)
        };
        if hit {
            changed = true;
            c = None;
            pin = None;
            uses = None;
        }
        let hit = {
            let cc = cfg_of(f, &mut c);
            let pp = pin_of(f, &mut pin);
            thread(f, cc, pp)
        };
        if hit {
            changed = true;
            c = None;
            uses = None;
        }
        let hit = {
            let cc = cfg_of(f, &mut c);
            let pp = pin_of(f, &mut pin);
            thread_known_condition(f, cc, pp)
        };
        if hit {
            changed = true;
            c = None;
            uses = None;
        }
        let hit = {
            let cc = cfg_of(f, &mut c);
            let pp = pin_of(f, &mut pin);
            let uu = uses_of(f, &mut uses);
            thread_branch_into_pred(f, cc, pp, uu)
        };
        if hit {
            changed = true;
            c = None;
        }
        let hit = {
            let cc = cfg_of(f, &mut c);
            let pp = pin_of(f, &mut pin);
            merge(f, cc, pp)
        };
        if hit {
            changed = true;
            c = None;
            uses = None;
        }
        any |= changed;
        if !changed {
            break;
        }
    }
    any
}

/// The control-flow graph of `f` as it stands, built once and kept until an
/// identity changes the function.
fn cfg_of<'a>(f: &Func, slot: &'a mut Option<dom::Cfg>) -> &'a dom::Cfg {
    if slot.is_none() {
        *slot = Some(dom::cfg(f));
    }
    slot.as_ref().unwrap()
}

/// Which blocks may not be absorbed or renamed, on the same terms.
fn pin_of<'a>(f: &Func, slot: &'a mut Option<Vec<bool>>) -> &'a [bool] {
    if slot.is_none() {
        *slot = Some(pinned(f));
    }
    slot.as_ref().unwrap()
}

/// How many times each value is read, on the same terms — built once and kept
/// current by the one identity that carries it.
fn uses_of<'a>(f: &Func, slot: &'a mut Option<Vec<u32>>) -> &'a mut Vec<u32> {
    if slot.is_none() {
        let mut u = vec![0u32; f.values.len()];
        for b in &f.blocks {
            for inst in &b.insts {
                inst.uses(|o| {
                    if let Operand::Val(v) = o {
                        u[v as usize] += 1;
                    }
                });
            }
            b.term.uses(|o| {
                if let Operand::Val(v) = o {
                    u[v as usize] += 1;
                }
            });
        }
        *slot = Some(u);
    }
    slot.as_mut().unwrap()
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
/// (g) — the predecessor takes S's branch itself when it passes a value for the
/// condition. Shares (e)'s side conditions: S is instruction-free, branches on
/// its own parameter, and every parameter of S is read nowhere but that
/// terminator, so the substitution below removes every occurrence.
fn thread_branch_into_pred(f: &mut Func, c: &dom::Cfg, pin: &[bool], uses: &mut Vec<u32>) -> bool {
    let n = f.blocks.len();
    let mut changed = false;
    for b in 0..n {
        if !c.reachable(b as BlockId) {
            continue;
        }
        // the predecessor must have exactly one successor: this rewrite replaces
        // its whole terminator, it does not duplicate a block
        let Term::Jmp(t) = f.blocks[b].term.clone() else { continue };
        let s = t.block as usize;
        if s == b || pin[s] || !f.blocks[s].labels.is_empty() {
            continue;
        }
        let Term::Br(Operand::Val(v), x, y) = f.blocks[s].term.clone() else { continue };
        // (h): S may hold the COMPARE that produces its own condition, and
        // nothing else. It is cloned into the predecessor below.
        let cmp = match f.blocks[s].insts.as_slice() {
            [] => None,
            [Inst::Cmp { dst, op, ty, a, b: cb }] if *dst == v => Some((*op, *ty, *a, *cb)),
            _ => continue,
        };
        let Some(k) = (match cmp {
            None => f.blocks[s].params.iter().position(|p| *p == v),
            // with a compare in the way the branch reads its result, and the
            // PARAMETER is whichever side of the compare is one
            Some((_, _, a, cb)) => {
                let par = |o: Operand| {
                    o.val().and_then(|x| f.blocks[s].params.iter().position(|p| *p == x))
                };
                par(a).or_else(|| par(cb))
            }
        }) else {
            continue;
        };
        if x.block as usize == s || y.block as usize == s {
            continue;
        }
        // every parameter of S read only inside S — by its terminator, and by
        // the compare when there is one, since both travel to the predecessor
        let mut here = vec![0u32; f.blocks[s].params.len()];
        let mut tally = |o: Operand, here: &mut Vec<u32>| {
            if let Operand::Val(v) = o {
                if let Some(j) = f.blocks[s].params.iter().position(|p| *p == v) {
                    here[j] += 1;
                }
            }
        };
        f.blocks[s].term.uses(|o| tally(o, &mut here));
        if let Some((_, _, a, cb)) = cmp {
            tally(a, &mut here);
            tally(cb, &mut here);
        }
        // the compare's own result counts as a use of nothing, but the parameter
        // it reads must not escape S by any other route
        if cmp.is_some() && uses[v as usize] != 1 {
            continue;
        }
        if f.blocks[s].params.iter().zip(&here).any(|(p, m)| uses[*p as usize] != *m) {
            continue;
        }
        // The condition this predecessor will branch on. Without a compare it is
        // the argument itself; with one it is the compare re-evaluated on the
        // arguments — decided outright when they are literals, and otherwise
        // cloned into the predecessor.
        let arg = match t.args.get(k) {
            Some(o) => *o,
            None => continue,
        };
        let mut clone_cmp: Option<Inst> = None;
        let cond = match cmp {
            None => match arg {
                Operand::Val(cv) => Operand::Val(cv),
                _ => continue, // a literal is (e)'s case, already handled
            },
            Some((op, ty, a, cb)) => {
                let subst = |o: Operand| -> Operand {
                    match o.val() {
                        Some(x) => match f.blocks[s].params.iter().position(|p| *p == x) {
                            Some(j) => t.args.get(j).copied().unwrap_or(o),
                            None => o,
                        },
                        None => o,
                    }
                };
                let (na, nb) = (subst(a), subst(cb));
                // any parameter left behind names a value the threaded path
                // never defines
                let free = |o: Operand| {
                    o.val().is_none_or(|x| !f.blocks[s].params.contains(&x))
                };
                if !free(na) || !free(nb) {
                    continue;
                }
                match super::fold::fold_inst(&Inst::Cmp { dst: v, op, ty, a: na, b: nb }) {
                    Some(o) => o,
                    None => {
                        let nv = f.new_value(f.ty_of(v), Def::Inst(b as BlockId, 0));
                        clone_cmp = Some(Inst::Cmp { dst: nv, op, ty, a: na, b: nb });
                        Operand::Val(nv)
                    }
                }
            }
        };
        let params = f.blocks[s].params.clone();
        let subst = |dest: &Target| -> Option<Target> {
            let mut d = dest.clone();
            for a in d.args.iter_mut() {
                if let Operand::Val(v) = *a {
                    if let Some(j) = params.iter().position(|p| *p == v) {
                        *a = *t.args.get(j)?;
                    }
                }
            }
            // a half-substituted argument would name a value the threaded path
            // never defines
            d.args
                .iter()
                .all(|a| !matches!(a, Operand::Val(v) if params.contains(v)))
                .then_some(d)
        };
        let (Some(nx), Some(ny)) = (subst(&x), subst(&y)) else { continue };
        // the two edges must stay distinguishable: one predecessor naming one
        // successor twice with different arguments is an edge `Cfg` would dedup
        if nx.block == ny.block && nx.args != ny.args {
            continue;
        }
        let clone_cmp2 = clone_cmp.clone();
        if let Some(ci) = clone_cmp {
            f.blocks[b].insts.push(ci);
            // The clone's `Def` was stamped before its position was known; the
            // verifier reads that record, so it is re-stamped here. Without it,
            // `hir::verify` reports "use of undefined %n" — which it did, on 21
            // torture cases, the moment this identity started cloning compares.
            super::refresh_block_defs(f, b as BlockId);
        }
        f.blocks[b].term = match cond {
            // a decided compare needs no branch at all
            Operand::Imm(k) => Term::Jmp(if k != 0 { nx } else { ny }),
            _ => Term::Br(cond, nx, ny),
        };
        // THE TABLE IS CARRIED, AND THIS REWRITE'S EFFECT ON IT IS EXACT: the old
        // terminator was a `Jmp` whose arguments were its only uses, the clone
        // reads its two operands, and the new terminator reads what it names.
        // Nothing here touches a count the gates above read — S's parameters and
        // its condition are used inside S alone, by the terminator and compare
        // just copied. The caller drops the table whenever another identity
        // changes the function.
        uses.resize(f.values.len(), 0);
        for a in &t.args {
            if let Operand::Val(u) = *a {
                uses[u as usize] -= 1;
            }
        }
        if let Some(ci) = clone_cmp2 {
            ci.uses(|o| {
                if let Operand::Val(u) = o {
                    uses[u as usize] += 1;
                }
            });
        }
        f.blocks[b].term.uses(|o| {
            if let Operand::Val(u) = o {
                uses[u as usize] += 1;
            }
        });

        // ONE REWRITE PER CALL, AND IT STAYS THAT WAY — measured, not assumed.
        //
        // Sweeping instead would remove a full walk of the function per rewrite,
        // worth 1.5 s on the sqlite amalgamation. It also CHANGES THE OUTPUT, and
        // the reason is not a stale use-count table: with the table rebuilt from
        // scratch after every rewrite the assembly changed in exactly the same
        // way. `run` interleaves six identities to a fixpoint and that fixpoint is
        // not confluent — taking several of one identity before the others get
        // their turn lands somewhere else. All 58 corpus programs stayed identical
        // through both attempts; only sqlite could see it.
        return true;
    }
    let _ = changed;
    false
}

fn thread_known_condition(f: &mut Func, c: &dom::Cfg, pin: &[bool]) -> bool {
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

fn drop_unreachable(f: &mut Func, c: &dom::Cfg, pin: &[bool]) -> bool {
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
fn thread(f: &mut Func, c: &dom::Cfg, pin: &[bool]) -> bool {
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
///
/// ONE SUBSTITUTION PER SWEEP, not one per merge. The first cut rebuilt the CFG
/// and the pin vector, allocated a value-wide map, rewrote every use in the
/// function and re-stamped every definition — and then restarted from block
/// zero — for EACH block absorbed. That is a full walk of the function per
/// merge, and on the sqlite amalgamation this pass and its neighbour in `run`
/// were 3.3 s of a 6.6 s ladder.
///
/// The sweep is safe to carry on, and the substitution is safe to accumulate:
///
///   NOTHING EARLIER BECOMES MERGEABLE. Absorbing S into B re-parents S's
///   successors from S to B, so no other block's predecessor COUNT changes, and
///   a merge is gated on that count being one. The restart therefore could only
///   ever find the block the sweep is already standing on, which is why the
///   inner loop lets B keep absorbing.
///
///   THE PENDING MAP DOES NOT CHANGE A DECISION. `rewrite_values` resolves
///   chains, so an argument that is itself a mapped parameter lands where it
///   would have. The one test that reads a value is the type check below, and it
///   is what makes that sound: a parameter is only substituted by an argument of
///   the SAME type, so a pending substitution cannot change the answer.
///
/// Stale reachability is likewise harmless: a block emptied by a merge is left
/// with `Term::Unreachable`, which the `Jmp` match below simply does not take.
fn merge(f: &mut Func, c0: &dom::Cfg, pin0: &[bool]) -> bool {
    let mut changed = false;
    let mut map: Vec<Option<Operand>> = Vec::new();
    // The sweep already holds a graph and a pin vector for the function as it
    // stands; the first pass here reads those rather than building its own. Most
    // sweeps merge nothing and return on this pass, so the pair it used to build
    // on entry was built to be thrown away.
    let (mut own_c, mut own_pin) = (None::<dom::Cfg>, None::<Vec<bool>>);
    loop {
        let c: &dom::Cfg = own_c.as_ref().unwrap_or(c0);
        let pin: &[bool] = own_pin.as_deref().unwrap_or(pin0);
        map.clear();
        map.resize(f.values.len(), None);
        let mut done = true;
        for b in 0..f.blocks.len() {
            if !c.reachable(b as BlockId) {
                continue;
            }
            loop {
            let s = match &f.blocks[b].term {
                Term::Jmp(t) if t.block as usize != b => t.block,
                _ => break,
            };
            // A labelled successor may NOT be absorbed: `emit` writes the
            // `lg_<func>.<label>` symbol at the head of the block that carries
            // the label, and a static initializer may hold that address — after
            // a merge the symbol would name B's first instruction instead of
            // S's, moving a program point the linker can see.
            if pin[s as usize] || c.preds[s as usize].len() != 1 || !f.blocks[s as usize].labels.is_empty() {
                break;
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
                break;
            }
            let succ = std::mem::replace(
                &mut f.blocks[s as usize],
                Block { params: Vec::new(), insts: Vec::new(), term: Term::Unreachable, labels: Vec::new(), weight: 1 },
            );
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
            if !done {
                break;
            }
        }
        if done {
            return changed;
        }
    }
}
