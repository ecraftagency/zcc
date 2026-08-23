// src/opt/util.rs — shared CFG / dataflow plumbing (operand walkers, rpo, dominators, liveness successors).
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

// A walker that MUTATES every operand (each READ Val) of an instruction — used by
// copy/CSE to substitute uses. Does NOT touch the destination temporary (def).
// Symmetric with ir::inst_uses (the read-only version).
pub(crate) fn each_use_mut(i: &mut Inst, mut g: impl FnMut(&mut Val)) {
    match i {
        Inst::Bin(_, _, _, a, b) => {
            g(a);
            g(b);
        }
        Inst::Un(_, _, _, a) | Inst::Copy(_, _, a) | Inst::Load(_, _, a) | Inst::Cast(_, _, _, a) => {
            g(a)
        }
        Inst::Store(_, a, b) | Inst::Memcpy(a, b, _) => {
            g(a);
            g(b);
        }
        Inst::Zero(a, _)
        | Inst::VaStart(a)
        | Inst::VaArg(_, a, _, _)
        | Inst::GotoPtr(a)
        | Inst::Alloca(_, a) => g(a),
        Inst::Overflow(_, _, _, _, _, a, b, rp) => {
            g(a);
            g(b);
            g(rp);
        }
        Inst::Select(_, _, c, a, b) => {
            g(c);
            g(a);
            g(b);
        }
        Inst::Phi(_, _, arms) => {
            for (_, a) in arms {
                g(a)
            }
        }
        Inst::Lea(..)
        | Inst::FunAddr(..)
        | Inst::LabelAddr(..)
        | Inst::Param(..)
        | Inst::VaArea(..) => {}
        Inst::Call(_, c, args, _) => {
            if let Callee::Ptr(p) = c {
                g(p)
            }
            for a in args {
                g(a)
            }
        }
        Inst::CallX(_, c, args, _, _) => {
            if let Callee::Ptr(p) = c {
                g(p)
            }
            for (a, _) in args {
                g(a)
            }
        }
        Inst::Sync(_, _, args, _, _) => {
            for a in args {
                g(a)
            }
        }
        Inst::Asm(_, ops) => {
            for op in ops {
                if let Some(x) = &mut op.inp {
                    g(x)
                }
                if let Some(x) = &mut op.wb {
                    g(x)
                }
            }
        }
    }
}

pub(crate) fn each_use_term_mut(t: &mut Term, mut g: impl FnMut(&mut Val)) {
    match t {
        Term::Br(c, ..) => g(c),
        Term::Ret(Some(r)) => g(r),
        _ => {}
    }
}


// ─────────────────────────────────────────────────────────────────────────────
// Pass 3 — COPY PROPAGATION (Leibniz: substitution of equals).
//
// Theorem (THEORY §A7): for `t = Copy(src)`, replacing every USE of t with src
// preserves ⟦·⟧ PROVIDED the value of src at the use point = its value at the copy
// point. SAFE sufficient conditions (no dominator tree required):
//   • src = Imm/FImm: a CONSTANT — invariant at every program point ⟹ always substitutable.
//   • src = Tmp(s) with s SINGLE-DEF: the value of s is invariant (defined exactly
//     once), and the copy reads s ⟹ def(s) precedes the copy ⟹ precedes every use of
//     t (structured lowering: use-after-def). ⟹ replacing t with s is safe.
// Propagate only a SINGLE-DEF temporary t (a multi-def like the `res` of a Cond
// depends on the path taken → do NOT substitute). Resolve a copy chain (t←u←Imm) back
// to its origin. Do NOT remove the Copy instruction (let DCE clean it up once dead) —
// this pass only rewrites USES. The equiv gate double-checks.
// ─────────────────────────────────────────────────────────────────────────────
pub(crate) fn resolve(subst: &[Option<Val>], v: Val) -> Val {
    let mut cur = v;
    for _ in 0..=subst.len() {
        match cur {
            Val::Tmp(t) => match subst[t as usize] {
                Some(next) if !matches!(next, Val::Tmp(x) if x == t) => cur = next,
                _ => return cur,
            },
            _ => return cur,
        }
    }
    cur
}


pub(crate) fn successors(f: &IrFunc) -> Vec<Vec<u32>> {
    let mut out = Vec::with_capacity(f.blocks.len());
    let mut buf = Vec::new();
    for b in &f.blocks {
        buf.clear();
        term_targets(&b.term, &mut buf);
        out.push(buf.clone());
    }
    out
}


/// Predecessor lists — the inverse of `successors` (the CFG read backwards).
pub(crate) fn predecessors(f: &IrFunc) -> Vec<Vec<BlockId>> {
    let mut preds = vec![Vec::new(); f.blocks.len()];
    for (bi, ss) in successors(f).iter().enumerate() {
        for &s in ss {
            preds[s as usize].push(bi as BlockId);
        }
    }
    preds
}


/// Reverse post-order from the entry (DFS finish order, reversed): a forward edge's
/// source precedes its target, so a join block is filled after all its forward
/// predecessors. Blocks unreachable from the entry are appended last (interp never
/// visits them). Iterative DFS (no host-stack recursion on the CFG).
pub(crate) fn rpo(f: &IrFunc) -> Vec<BlockId> {
    let n = f.blocks.len();
    let succ = successors(f);
    let mut seen = vec![false; n];
    let mut post = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new(); // (block, next-successor index)
    if n > 0 {
        seen[0] = true;
        stack.push((0, 0));
    }
    while let Some(&(b, i)) = stack.last() {
        if i < succ[b].len() {
            stack.last_mut().unwrap().1 += 1;
            let s = succ[b][i] as usize;
            if !seen[s] {
                seen[s] = true;
                stack.push((s, 0));
            }
        } else {
            post.push(b as BlockId);
            stack.pop();
        }
    }
    post.reverse();
    for b in 0..n {
        if !seen[b] {
            post.push(b as BlockId);
        }
    }
    post
}


/// Does temp `u` appear in `i` ONLY as the ADDRESS operand of a Load/Store? Any
/// other appearance (an arithmetic operand, a call arg, a stored VALUE, …) means the
/// address escaped ⟹ the local it points at is not promotable.
pub(crate) fn is_addr_use(i: &Inst, u: Tmp) -> bool {
    match i {
        Inst::Load(_, _, Val::Tmp(a)) => *a == u,
        // the address slot only; if `u` is ALSO the stored value it escapes.
        Inst::Store(_, Val::Tmp(a), v) => *a == u && !matches!(v, Val::Tmp(t) if *t == u),
        _ => false,
    }
}


pub(crate) fn val_eq(a: Val, b: Val) -> bool {
    match (a, b) {
        (Val::Tmp(x), Val::Tmp(y)) => x == y,
        (Val::Imm(x), Val::Imm(y)) => x == y,
        (Val::FImm(x), Val::FImm(y)) => x == y,
        _ => false,
    }
}


/// Chase a value through the trivial-φ substitution to its final representative. `subst`
/// never maps a temp to itself (self-refs are excluded when a φ collapses) and forms a DAG,
/// so the walk terminates; the self-map guard is pure defence.
pub(crate) fn resolve_subst(mut r: Val, subst: &HashMap<Tmp, Val>) -> Val {
    while let Val::Tmp(t) = r {
        match subst.get(&t) {
            Some(&nv) => {
                if matches!(nv, Val::Tmp(t2) if t2 == t) {
                    break;
                }
                r = nv;
            }
            None => break,
        }
    }
    r
}


/// Replace, in a terminator, the target BlockId `from` by `to` (edge redirection).
pub(crate) fn retarget(term: &mut Term, from: BlockId, to: BlockId) {
    match term {
        Term::Jmp(t) => {
            if *t == from {
                *t = to;
            }
        }
        Term::Br(_, a, b) => {
            if *a == from {
                *a = to;
            }
            if *b == from {
                *b = to;
            }
        }
        Term::Ret(_) | Term::Unreachable => {}
    }
}


/// Blocks reachable from the entry (a forward DFS over successors).
pub(crate) fn reachable_blocks(f: &IrFunc) -> Vec<bool> {
    let succ = successors(f);
    let mut seen = vec![false; f.blocks.len()];
    if f.blocks.is_empty() {
        return seen;
    }
    seen[0] = true;
    let mut stack = vec![0usize];
    while let Some(b) = stack.pop() {
        for &s in &succ[b] {
            if !seen[s as usize] {
                seen[s as usize] = true;
                stack.push(s as usize);
            }
        }
    }
    seen
}


/// Dominator SETS by the classic iterative data-flow fixpoint (Allen–Cocke):
/// dom(b) = {b} ∪ (⋂ dom(p) over reachable predecessors p); dom(entry) = {entry}.
/// `db ∈ dom(b)` ⟺ db dominates b (every path from entry to b passes through db).
pub(crate) fn dominators(f: &IrFunc) -> Vec<HashSet<BlockId>> {
    let nb = f.blocks.len();
    let preds = predecessors(f);
    let reach = reachable_blocks(f);
    let allr: HashSet<BlockId> = (0..nb as BlockId).filter(|&b| reach[b as usize]).collect();
    let mut dom = vec![allr; nb];
    if nb > 0 {
        dom[0] = HashSet::from([0]);
    }
    let order: Vec<BlockId> =
        rpo(f).into_iter().filter(|&b| reach[b as usize] && b != 0).collect();
    loop {
        let mut changed = false;
        for &b in &order {
            let rp: Vec<BlockId> =
                preds[b as usize].iter().copied().filter(|&p| reach[p as usize]).collect();
            let mut newd = match rp.split_first() {
                Some((first, rest)) => {
                    let mut acc = dom[*first as usize].clone();
                    for &p in rest {
                        acc = acc.intersection(&dom[p as usize]).copied().collect();
                    }
                    acc
                }
                None => HashSet::new(),
            };
            newd.insert(b);
            if newd != dom[b as usize] {
                dom[b as usize] = newd;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    dom
}


// ─────────────────────────────────────────────────────────────────────────────
// CFG SIMPLIFICATION (Phase A) — structural cleanup that removes JUMPS and BLOCKS
// without touching any instruction's value. Two ⟦·⟧-trivial rewrites:
//   (1) straight-line MERGE — a block S whose SOLE predecessor P ends in Jmp(S)
//       (so P's only successor is S) is spliced into P: append S's instructions,
//       adopt S's terminator. No edge enters or leaves between P and S, so the
//       executed instruction SEQUENCE (P then S) is identical ⟹ ⟦·⟧ unchanged.
//       S's successors' φ-arms that name S are renamed to P (P now owns that edge).
//       A φ in S would need a single arm (S has one pred) — Braun never builds one,
//       but if present it degenerates to a Copy of that arm.
//   (2) UNREACHABLE elimination — a block never reached from the entry is deleted
//       and the survivors renumbered (BlockId = index); interp never visits it ⟹
//       ⟦·⟧ unchanged. Dead φ-arms naming a removed predecessor are dropped.
// SCCP folds Br(const)→Jmp, orphaning the not-taken block; (2) removes it and (1)
// then collapses the resulting straight line. Guarded by `cfg_complete` (computed-goto
// edges are unmodeled → reachability/predecessors incomplete). Returns the count of
// structural rewrites (a change-metric for the optimize_ssa fixpoint). MEASURED by
// `equiv`, never trusted. Side I (algorithm: CFG graph rewrite preserving the
// walk); no spec-constant involved.
// ─────────────────────────────────────────────────────────────────────────────
pub(crate) fn rename_phi_pred(b: &mut Block, from: BlockId, to: BlockId) {
    for i in b.insts.iter_mut() {
        if let Inst::Phi(_, _, arms) = i {
            for (p, _) in arms.iter_mut() {
                if *p == from {
                    *p = to;
                }
            }
        }
    }
}


pub(crate) fn remap_term(t: &mut Term, map: &[u32]) {
    match t {
        Term::Jmp(a) => *a = map[*a as usize],
        Term::Br(_, a, b) => {
            *a = map[*a as usize];
            *b = map[*b as usize];
        }
        Term::Ret(_) | Term::Unreachable => {}
    }
}


/// Rewrite an instruction's destination temp (only the pure invariant producers this pass
/// clones into the preheader).
pub(crate) fn set_dst(i: &mut Inst, d: Tmp) {
    match i {
        Inst::Bin(x, ..)
        | Inst::Un(x, ..)
        | Inst::Copy(x, ..)
        | Inst::Cast(x, ..)
        | Inst::Lea(x, ..)
        | Inst::FunAddr(x, ..) => *x = d,
        _ => unreachable!("set_dst on a non-clonable inst: {i:?}"),
    }
}


/// (Block, index) locator of every temp's single def, sized to the CURRENT temp count.
/// pointer_iv materialization INSERTS instructions (header φ) and PUSHES temps, which
/// invalidates any previously-cached locator (a stale index reads the wrong instruction —
/// or indexes past a freshly-created temp, the s0272 OOB). Rebuild this after every such
/// mutation so `clone_inv_to_ph` always reads the instruction it means to.
pub(crate) fn def_locations(f: &IrFunc) -> Vec<Option<(BlockId, usize)>> {
    let mut d = vec![None; f.temps.len()];
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, inst) in b.insts.iter().enumerate() {
            if let Some(dd) = inst_def(inst) {
                d[dd as usize] = Some((bi as BlockId, ii));
            }
        }
    }
    d
}


// The mutable-def companion to ir::inst_def — used to relocate a clone's destination
// temporary into the caller's temp space (mirror of the read-only each_use_mut).
pub(crate) fn each_def_mut(i: &mut Inst, mut g: impl FnMut(&mut Tmp)) {
    match i {
        Inst::Bin(d, ..)
        | Inst::Un(d, ..)
        | Inst::Copy(d, ..)
        | Inst::Load(d, ..)
        | Inst::Lea(d, ..)
        | Inst::Cast(d, ..)
        | Inst::FunAddr(d, ..)
        | Inst::LabelAddr(d, ..)
        | Inst::VaArg(d, ..)
        | Inst::Overflow(d, ..)
        | Inst::VaArea(d, ..)
        | Inst::Param(d, ..)
        | Inst::Alloca(d, ..)
        | Inst::Select(d, ..)
        | Inst::Phi(d, ..) => g(d),
        Inst::Call(d, ..) | Inst::CallX(d, ..) | Inst::Sync(d, ..) => {
            if let Some(d) = d {
                g(d)
            }
        }
        Inst::Store(..)
        | Inst::Memcpy(..)
        | Inst::Zero(..)
        | Inst::VaStart(..)
        | Inst::GotoPtr(..)
        | Inst::Asm(..) => {}
    }
}


// Relocate one cloned instruction into the caller: temp uses/def += tb, and a static
// local address += fb (the only raw frame offset among the whitelisted kinds).
pub(crate) fn relocate_inst(i: &mut Inst, tb: Tmp, fb: u32) {
    each_use_mut(i, |v| {
        if let Val::Tmp(t) = v {
            *t += tb
        }
    });
    each_def_mut(i, |d| *d += tb);
    if let Inst::Lea(_, Place::Local(off)) = i {
        *off += fb;
    }
}

pub(crate) fn relocate_term(t: &mut Term, tb: Tmp, bb: BlockId) {
    each_use_term_mut(t, |v| {
        if let Val::Tmp(x) = v {
            *x += tb
        }
    });
    match t {
        Term::Jmp(x) => *x += bb,
        Term::Br(_, a, b) => {
            *a += bb;
            *b += bb;
        }
        Term::Ret(_) | Term::Unreachable => {}
    }
}

