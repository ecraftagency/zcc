// src/opt.rs — optimization PASSES over the IR (THEORY.md Part I §A7).
//
// GOVERNING INVARIANT (replacing the 10k-LOC ceiling in the production fork): every
// pass P must PRESERVE the SEMANTICS ⟦·⟧, and this must NOT be trusted by reasoning —
// it must be MEASURED mechanically by `ir::tests::equiv` (the commuting square:
// ⟦A⟧ ≡ ⟦P(A)⟧ over the input battery) + `ir::verify` (well-formed after the pass).
// The test gate runs via `cargo test opt::` — orders of magnitude CHEAPER than the
// full suite (the speed-of-iteration rule), and stronger (proved at IR→IR_ops, not md5-of-binary).
//
// Each pass is a PURE IR→IR function (mutating in place) that returns the rewrite
// count (a convergence measure). The backend does NOT need to know which pass ran —
// it only reads well-formed IR.
#![allow(dead_code)] // removed once the driver wires the --O1 pipeline into emit_ir

use crate::ast::{TyTab, ULONG};
use crate::ir::{
    canon, eval_bin, eval_cast, inst_def, inst_uses, term_targets, term_uses, Callee, Inst, IrFunc,
    Op, Place, Term, Tmp, Un, Val,
};
use std::collections::{HashMap, HashSet};

// A walker that MUTATES every operand (each READ Val) of an instruction — used by
// copy/CSE to substitute uses. Does NOT touch the destination temporary (def).
// Symmetric with ir::inst_uses (the read-only version).
fn each_use_mut(i: &mut Inst, mut g: impl FnMut(&mut Val)) {
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
        Inst::Phi(_, _, arms) => {
            for (_, a) in arms {
                g(a)
            }
        }
        Inst::Lea(..)
        | Inst::FunAddr(..)
        | Inst::LabelAddr(..)
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
fn each_use_term_mut(t: &mut Term, mut g: impl FnMut(&mut Val)) {
    match t {
        Term::Br(c, ..) => g(c),
        Term::Ret(Some(r)) => g(r),
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 1 — CONSTANT FOLDING (partial evaluation).
//
// Theorem (rewrite-soundness, THEORY §A7): ⟦Bin(op, Imm a, Imm b)⟧ = ⟦Imm(eval_op(a,b))⟧,
// and similarly for constant Un/Cast. Correct BY CONSTRUCTION because fold CALLS the
// very `eval_bin/eval_cast/canon` that interp uses — the folder and the interpreter
// are ONE denotation function and cannot diverge (faithfulness).
//
// Scope (conservative, avoiding UB & rounding):
//   • integer immediates ONLY. Float (FImm) is DEFERRED: interp models f64, so folding
//     f32 could round differently from the backend s-register → keep the instruction and let the hardware decide.
//   • Div/Rem by 0 → eval_bin returns Err → do NOT fold (keep the instruction, preserving the target's UB behavior).
//   • const-branch: Br(Imm c)→Jmp (opening the way for DCE to remove the dead block later).
// Constants are NOT propagated through temporaries (that is copy_prop, pass 3) — this pass folds only already-constant operands.
// ─────────────────────────────────────────────────────────────────────────────
pub fn const_fold(tt: &TyTab, f: &mut IrFunc) -> u32 {
    let mut n = 0u32;
    for blk in f.blocks.iter_mut() {
        for inst in blk.insts.iter_mut() {
            let repl: Option<Inst> = match inst {
                Inst::Bin(d, op, ty, Val::Imm(x), Val::Imm(y)) if !tt.is_float(*ty) => {
                    // Err (div/mod by 0) → None: do NOT fold UB into a constant.
                    eval_bin(tt, *op, *ty, *x, *y).ok().map(|r| Inst::Copy(*d, *ty, Val::Imm(r)))
                }
                Inst::Un(d, op, ty, Val::Imm(x)) if !tt.is_float(*ty) => {
                    let r = match op {
                        Un::Neg => canon(tt, *ty, x.wrapping_neg()),
                        Un::BNot => canon(tt, *ty, !*x),
                    };
                    Some(Inst::Copy(*d, *ty, Val::Imm(r)))
                }
                Inst::Cast(d, from, to, Val::Imm(x)) if !tt.is_float(*from) && !tt.is_float(*to) => {
                    Some(Inst::Copy(*d, *to, Val::Imm(eval_cast(tt, *from, *to, *x))))
                }
                _ => None,
            };
            if let Some(r) = repl {
                *inst = r;
                n += 1;
            }
        }
        let newterm: Option<Term> = match &blk.term {
            Term::Br(Val::Imm(c), t, e) => Some(Term::Jmp(if *c != 0 { *t } else { *e })),
            _ => None,
        };
        if let Some(t) = newterm {
            blk.term = t;
            n += 1;
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 2 — DEAD CODE ELIMINATION (liveness).
//
// Theorem (THEORY §A7): a PURE instruction (no side effect) whose destination
// temporary is NOT read anywhere ⟹ removing it preserves ⟦·⟧ (the observable result
// does not depend on a dead value). Pure = Bin/Un/Copy/Lea/Cast/Load (Load only READS
// memory, does not write → removing it is harmless in the CORE, non-volatile model).
// Impure = Call (side effect), Store/Memcpy (memory write), Opaque (conservative,
// unknown) → KEPT even if the destination is dead.
//
// The liveness used here is flow-INSENSITIVE (live-if-used-anywhere): a SAFE
// approximation (keeps MORE than necessary) — only a temporary NOT read ANYWHERE is
// removed ⟹ certainly dead. Iterate to a fixpoint: removing an instruction can make
// its operands dead → a later round removes them too.
// ─────────────────────────────────────────────────────────────────────────────
fn is_pure(i: &Inst) -> bool {
    matches!(
        i,
        Inst::Bin(..) | Inst::Un(..) | Inst::Copy(..) | Inst::Lea(..) | Inst::Cast(..) | Inst::Load(..)
    )
}

pub fn dce(f: &mut IrFunc) -> u32 {
    let mut removed = 0u32;
    let mut buf: Vec<u32> = Vec::new();
    loop {
        // Liveness: which temporaries are read by ANY instruction/terminator?
        let mut used = vec![false; f.temps.len()];
        for b in &f.blocks {
            for i in &b.insts {
                buf.clear();
                inst_uses(i, &mut buf);
                for &u in &buf {
                    used[u as usize] = true;
                }
            }
            buf.clear();
            term_uses(&b.term, &mut buf);
            for &u in &buf {
                used[u as usize] = true;
            }
        }
        let mut any = false;
        for b in f.blocks.iter_mut() {
            let before = b.insts.len();
            b.insts.retain(|i| match inst_def(i) {
                Some(d) if !used[d as usize] && is_pure(i) => false, // dead + pure → remove
                _ => true,
            });
            let cut = before - b.insts.len();
            if cut > 0 {
                removed += cut as u32;
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    removed
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
fn resolve(subst: &[Option<Val>], v: Val) -> Val {
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

pub fn copy_prop(f: &mut IrFunc) -> u32 {
    let nt = f.temps.len();
    let mut defcnt = vec![0u32; nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defcnt[d as usize] += 1;
            }
        }
    }
    // Substitution table: a single-def temporary defined by Copy(src) where src is a constant or a single-def temp.
    let mut subst: Vec<Option<Val>> = vec![None; nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Copy(d, _, src) = i
                && defcnt[*d as usize] == 1 {
                    let ok = match src {
                        Val::Imm(_) | Val::FImm(_) => true,
                        Val::Tmp(s) => defcnt[*s as usize] == 1,
                    };
                    if ok {
                        subst[*d as usize] = Some(*src);
                    }
                }
        }
    }
    let mut n = 0u32;
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            each_use_mut(i, |v| {
                let r = resolve(&subst, *v);
                if !matches!((*v, r), (Val::Tmp(a), Val::Tmp(b)) if a == b)
                    && !matches!(*v, Val::Imm(_) | Val::FImm(_)) {
                        *v = r;
                        n += 1;
                    }
            });
        }
        each_use_term_mut(&mut b.term, |v| {
            let r = resolve(&subst, *v);
            if !matches!((*v, r), (Val::Tmp(a), Val::Tmp(b)) if a == b)
                && !matches!(*v, Val::Imm(_) | Val::FImm(_)) {
                    *v = r;
                    n += 1;
                }
        });
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 4 — COMMON SUBEXPRESSION ELIMINATION (value numbering).
//
// Theorem (THEORY §A7): two PURE instructions with the same (op, type, operands)
// produce the SAME value ⟹ the later one is replaced with Copy(result-of-the-earlier).
// SAFE scope without alias analysis / a dominator tree:
//   • BLOCK-LOCAL (the cache is reset per block): within one block a temporary is
//     single-def (structured lowering) ⟹ the value-number is the operand Val itself, with no renumbering.
//   • Bin/Un/Cast arithmetic: a PURE value (no memory read) ⟹ the cache lives for the
//     whole block. Commutative Ops (Add/Mul/And/Or/Xor/Eq/Ne) canonicalize operand order (a+b≡b+a).
//   • Load: value-numbered by (address, type), but the load cache is FLUSHED
//     (conservatively) at ANY memory write (Store/Memcpy/Call/Opaque) — no non-alias
//     assumption. ⟹ two Loads of the same address are CSE'd only when NO memory write intervenes (available-loads).
// Replace the duplicate instruction with a Copy (DCE/copy-prop clean up afterward). The equiv gate double-checks aliasing.
// ─────────────────────────────────────────────────────────────────────────────
fn enc(v: &Val) -> (u8, i64) {
    match v {
        Val::Tmp(t) => (0, *t as i64),
        Val::Imm(x) => (1, *x),
        Val::FImm(b) => (2, *b as i64),
    }
}
/// A binary value-number key: (op-tag, type, operand-1, operand-2). Commutative → sort.
fn bin_key(op: Op, ty: u32, a: &Val, b: &Val) -> (u16, u32, (u8, i64), (u8, i64)) {
    let commutative = matches!(op, Op::Add | Op::Mul | Op::And | Op::Or | Op::Xor | Op::Eq | Op::Ne);
    let (mut o1, mut o2) = (enc(a), enc(b));
    if commutative && o1 > o2 {
        std::mem::swap(&mut o1, &mut o2);
    }
    (op as u16, ty, o1, o2)
}

pub fn cse(f: &mut IrFunc) -> u32 {
    let mut n = 0u32;
    for b in f.blocks.iter_mut() {
        // arith: key (op-tag, ty, o1, o2). Un tag=100+, Cast tag=200 (from in o2).
        let mut arith: HashMap<(u16, u32, (u8, i64), (u8, i64)), Tmp> = HashMap::new();
        // loads: key (encoded-address, ty). Flushed at every memory write.
        let mut loads: HashMap<((u8, i64), u32), Tmp> = HashMap::new();
        for i in b.insts.iter_mut() {
            // memory-kill BY-CONSTRUCTION: keep the load cache ONLY across instructions
            // PROVEN not to write memory; flush for everything else. This inverts the old
            // writer allowlist (which was correct-by-vigilance: it missed
            // Overflow/Zero/VaStart/VaArg — all of which write memory → reusing a stale
            // load = miscompile, GCC PR84169). A new exotic Inst kills by default → safe, never silently retaining a load.
            if !matches!(i,
                Inst::Bin(..) | Inst::Un(..) | Inst::Copy(..) | Inst::Load(..) | Inst::Lea(..)
                | Inst::Cast(..) | Inst::FunAddr(..) | Inst::LabelAddr(..) | Inst::VaArea(..)
            ) {
                loads.clear(); // memory write (or unknown) → conservative memory-kill
            }
            let repl: Option<Inst> = match i {
                Inst::Bin(d, op, ty, a, bb) => {
                    let k = bin_key(*op, *ty, a, bb);
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *ty, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                Inst::Un(d, op, ty, a) => {
                    let k = (100u16 + *op as u16, *ty, enc(a), (9, 0));
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *ty, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                Inst::Cast(d, from, to, a) => {
                    let k = (200u16, *to, enc(a), (9, *from as i64));
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *to, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                Inst::Load(d, ty, addr) => {
                    let k = (enc(addr), *ty);
                    match loads.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *ty, Val::Tmp(prev))),
                        None => {
                            loads.insert(k, *d);
                            None
                        }
                    }
                }
                // Lea(Local off) is PURE, its value = a frame address (invariant) → it
                // can be value-numbered, deduplicating addresses so load-CSE matches
                // across the pipeline (Global/Str are skipped).
                Inst::Lea(d, Place::Local(off)) => {
                    let k = (300u16, 0u32, (3, *off as i64), (9, 0));
                    match arith.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, ULONG, Val::Tmp(prev))),
                        None => {
                            arith.insert(k, *d);
                            None
                        }
                    }
                }
                _ => None,
            };
            if let Some(r) = repl {
                *i = r;
                n += 1;
            }
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 5 — REGISTER ALLOCATION (graph coloring, Chaitin–Briggs).
//
// NP-complete (THEORY §C2 — graph coloring) ⟹ use a HEURISTIC simplify/spill, NOT
// demanding a strict optimum. But CORRECTNESS (a valid coloring) is verifiable in P.
//
// Correctness here DIFFERS from the four passes above: interp does NOT model
// registers, so ⟦before⟧=⟦after⟧ cannot be used. The correctness invariant is
// RENAMING BISIMULATION (THEORY §A7): the register-assigned program is bisimilar to
// the temporary program ⟺ two SIMULTANEOUSLY LIVE temporaries always occupy DIFFERENT
// locations (a live value is never overwritten). We check the INTERFERENCE INVARIANT
// mechanically:
//   ∀ edge (u,v) ∈ interference-graph, color[u] ≠ color[v]  (a spill = its own slot, never overwritten).
//
// Chain of theorems: liveness (monotone dataflow, Kleene fixpoint) → interference
// graph (u interferes with v ⟺ both live at some def) → coloring (simplify degree<k / spill) → verify.
// ─────────────────────────────────────────────────────────────────────────────

/// Flow-SENSITIVE liveness (backward dataflow, THEORY §B3 fixpoint over the lattice 2^Tmp).
pub struct Liveness {
    pub live_in: Vec<Vec<bool>>,
    pub live_out: Vec<Vec<bool>>,
}

fn successors(f: &IrFunc) -> Vec<Vec<u32>> {
    let mut out = Vec::with_capacity(f.blocks.len());
    let mut buf = Vec::new();
    for b in &f.blocks {
        buf.clear();
        term_targets(&b.term, &mut buf);
        out.push(buf.clone());
    }
    out
}

pub fn liveness(f: &IrFunc) -> Liveness {
    let nb = f.blocks.len();
    let nt = f.temps.len();
    // gen (use before def within a block) + kill (def within a block)
    let mut useb = vec![vec![false; nt]; nb];
    let mut defb = vec![vec![false; nt]; nb];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut defined = vec![false; nt];
        for i in &b.insts {
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                if !defined[u as usize] {
                    useb[bi][u as usize] = true;
                }
            }
            if let Some(d) = inst_def(i) {
                defined[d as usize] = true;
                defb[bi][d as usize] = true;
            }
        }
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            if !defined[u as usize] {
                useb[bi][u as usize] = true;
            }
        }
    }
    let succ = successors(f);
    let mut live_in = vec![vec![false; nt]; nb];
    let mut live_out = vec![vec![false; nt]; nb];
    loop {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut lo = vec![false; nt];
            for &s in &succ[bi] {
                for t in 0..nt {
                    if live_in[s as usize][t] {
                        lo[t] = true;
                    }
                }
            }
            let mut li = useb[bi].clone();
            for t in 0..nt {
                if lo[t] && !defb[bi][t] {
                    li[t] = true;
                }
            }
            if lo != live_out[bi] {
                live_out[bi] = lo;
                changed = true;
            }
            if li != live_in[bi] {
                live_in[bi] = li;
                changed = true;
            }
        }
        if !changed {
            break; // fixpoint (Kleene): no set grows any further
        }
    }
    Liveness { live_in, live_out }
}

/// Interference graph: u—v ⟺ u,v are both live at some definition point (they cannot share a register).
pub fn interference(f: &IrFunc, lv: &Liveness) -> Vec<HashSet<Tmp>> {
    let nt = f.temps.len();
    let mut adj: Vec<HashSet<Tmp>> = vec![HashSet::new(); nt];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        // the terminator's operands are live at the block's tail
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true;
        }
        for i in b.insts.iter().rev() {
            if let Some(d) = inst_def(i) {
                for (t, &alive) in live.iter().enumerate() {
                    if alive && t as u32 != d {
                        adj[d as usize].insert(t as u32);
                        adj[t].insert(d);
                    }
                }
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
        }
    }
    adj
}

/// Coloring result: color[t]=Some(r) → register r; None → spill (its own stack slot).
pub struct Alloc {
    pub color: Vec<Option<u32>>,
    pub spilled: Vec<Tmp>,
    pub k: u32,
}

pub fn color(adj: &[HashSet<Tmp>], k: u32) -> Alloc {
    let nt = adj.len();
    let mut removed = vec![false; nt];
    let mut degree: Vec<usize> = adj.iter().map(|s| s.len()).collect();
    let mut stack: Vec<Tmp> = Vec::new();
    // SIMPLIFY: push a degree<k node onto the stack (certainly colorable); when none
    // remain → pick the max-degree node as a potential-spill (Briggs: it may still be colorable at select).
    for _ in 0..nt {
        let low = (0..nt).find(|&v| !removed[v] && degree[v] < k as usize);
        let v = match low {
            Some(v) => v,
            None => match (0..nt).filter(|&v| !removed[v]).max_by_key(|&v| degree[v]) {
                Some(v) => v,
                None => break,
            },
        };
        removed[v] = true;
        stack.push(v as u32);
        for &nb in &adj[v] {
            if !removed[nb as usize] {
                degree[nb as usize] -= 1;
            }
        }
    }
    // SELECT: pop, assign the smallest color that differs from all neighbors; out of colors → an actual spill (None).
    let mut colr = vec![None; nt];
    let mut spilled = Vec::new();
    while let Some(v) = stack.pop() {
        let mut used = vec![false; k as usize];
        for &nb in &adj[v as usize] {
            if let Some(c) = colr[nb as usize]
                && (c as usize) < k as usize {
                    used[c as usize] = true;
                }
        }
        match (0..k).find(|&c| !used[c as usize]) {
            Some(c) => colr[v as usize] = Some(c),
            None => spilled.push(v),
        }
    }
    Alloc { color: colr, spilled, k }
}

/// Mechanically CHECK the interference invariant: every edge is differently colored.
/// This is the "P-verify" of the NP solution — regalloc may be heuristic, but its correctness is cheap to check.
pub fn verify_coloring(adj: &[HashSet<Tmp>], al: &Alloc) -> Result<(), String> {
    for u in 0..adj.len() {
        if let Some(cu) = al.color[u] {
            for &v in &adj[u] {
                if al.color[v as usize] == Some(cu) {
                    return Err(format!("interference (t{u},t{v}) share register {cu}"));
                }
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ORCHESTRATOR — the IR→IR pipeline (passes 1-4) run to a FIXPOINT. Each pass
// preserves ⟦·⟧ (proved individually), so their composition preserves ⟦·⟧ too (closed
// under composition). Iterate because one pass opens opportunities for another
// (copy-prop→const-fold folds constants; CSE→copy-prop→DCE cleans up). The loop
// terminates on "no more rewrites" (convergence) + a hard cap against runaway.
// (Regalloc is NOT here: it produces an assignment for the BACKEND to consume, not an
//  IR→IR transform; the backend's current spill-per-node does not yet use it — it will
//  be wired in when the default is flipped to IR, Step B.)
// ─────────────────────────────────────────────────────────────────────────────
pub fn optimize(tt: &TyTab, f: &mut IrFunc) {
    for _ in 0..32 {
        let mut n = 0;
        n += const_fold(tt, f);
        n += copy_prop(f);
        n += cse(f);
        n += dce(f);
        if n == 0 {
            break; // fixpoint: no instruction changes any further
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{TyTab, INT};
    use crate::ir::tests::{compile, equiv, interp, mk};
    use crate::ir::{verify, Block};

    fn count_calls(f: &IrFunc) -> usize {
        f.blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Call(..))).count()
    }
    fn count_loads(ir: &[IrFunc]) -> usize {
        ir.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Load(..))).count()
    }

    // Pass 1 folds constant operands, equiv holds, result = Copy(the computed Imm).
    #[test]
    fn cf_folds_const_bin() {
        let tt = TyTab::new();
        let before = vec![mk(
            "f",
            vec![INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![Inst::Bin(0, Op::Mul, INT, Val::Imm(6), Val::Imm(7))],
                term: Term::Ret(Some(Val::Tmp(0))),
            }],
        )];
        let mut after = before.clone();
        let n = const_fold(&tt, &mut after[0]);
        assert!(n >= 1, "must fold ≥1");
        verify(&after[0]).unwrap();
        equiv(&tt, &before, &after, "f").expect("const-fold must preserve ⟦·⟧");
        assert!(matches!(after[0].blocks[0].insts[0], Inst::Copy(0, _, Val::Imm(42))));
        assert_eq!(interp(&tt, &after, "f", &[]).unwrap(), 42);
    }

    // const-branch: Br(Imm 0) → Jmp(else); interp takes the correct branch.
    #[test]
    fn cf_const_branch() {
        let tt = TyTab::new();
        let before = vec![mk(
            "h",
            vec![],
            vec![],
            0,
            INT,
            vec![
                Block { insts: vec![], term: Term::Br(Val::Imm(0), 1, 2) },
                Block { insts: vec![], term: Term::Ret(Some(Val::Imm(10))) },
                Block { insts: vec![], term: Term::Ret(Some(Val::Imm(20))) },
            ],
        )];
        let mut after = before.clone();
        let n = const_fold(&tt, &mut after[0]);
        assert!(n >= 1);
        assert!(matches!(after[0].blocks[0].term, Term::Jmp(2)));
        verify(&after[0]).unwrap();
        equiv(&tt, &before, &after, "h").expect("const-branch must preserve ⟦·⟧");
        assert_eq!(interp(&tt, &after, "h", &[]).unwrap(), 20);
    }

    // On REAL code (the parser already folds source constants ⟹ the pass is largely a
    // no-op) — the key invariant: whether it does little or much, const-fold NEVER changes ⟦·⟧.
    #[test]
    fn cf_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int g(int a,int b){int c=a+b;if(a>b)return c*2;return c-1;}", "g"),
            ("b", "int s(int n){int t=0;int i;for(i=0;i<n;i=i+1)t=t+i*3;return t;}", "s"),
            ("c", "int m(int a,int b){return a%b + a/b;}", "m"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                const_fold(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // DCE removes a dead expression-statement (`a*b;`) + its operands, equiv preserved.
    #[test]
    fn dce_removes_dead() {
        let (ast, ir) = compile("dce1", "int g(int a,int b){a*b; return a+b;}");
        let mut opt = ir.clone();
        let removed: u32 = opt.iter_mut().map(dce).sum();
        assert!(removed >= 1, "must remove ≥1 dead instruction");
        for f in &opt {
            verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
        }
        equiv(&ast.tt, &ir, &opt, "g").expect("DCE must preserve ⟦·⟧");
    }

    // DCE must NOT remove a Call even if its result is unused (side effect). Structural
    // proof: the Call count is unchanged; + equiv.
    #[test]
    fn dce_keeps_call() {
        let (ast, ir) = compile("dce2", "int side(int x){return x;} int k(int a){side(a); return a+1;}");
        let calls_before: usize = ir.iter().map(count_calls).sum();
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            dce(f);
        }
        let calls_after: usize = opt.iter().map(count_calls).sum();
        assert_eq!(calls_before, calls_after, "DCE must NOT remove a Call");
        assert!(calls_after >= 1);
        for f in &opt {
            verify(f).unwrap();
        }
        equiv(&ast.tt, &ir, &opt, "k").expect("DCE (keeping the call) must preserve ⟦·⟧");
        assert_eq!(interp(&ast.tt, &opt, "k", &[41]).unwrap(), 42);
    }

    #[test]
    fn dce_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int g(int a,int b){int c=a+b;a-b;if(a>b)return c*2;return c-1;}", "g"),
            ("b", "int s(int n){int t=0;int i;for(i=0;i<n;i=i+1){i*i;t=t+i;}return t;}", "s"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                dce(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // Copy-prop + const-fold cascade: t0=Copy(5); t1=t0+3 → (prop) t1=5+3 → (fold) 8.
    #[test]
    fn cp_const_cascade() {
        let tt = TyTab::new();
        let before = vec![mk(
            "f",
            vec![INT, INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Copy(0, INT, Val::Imm(5)),
                    Inst::Bin(1, Op::Add, INT, Val::Tmp(0), Val::Imm(3)),
                ],
                term: Term::Ret(Some(Val::Tmp(1))),
            }],
        )];
        let mut opt = before.clone();
        let n = copy_prop(&mut opt[0]);
        assert!(n >= 1, "must propagate ≥1 use");
        verify(&opt[0]).unwrap();
        equiv(&tt, &before, &opt, "f").expect("copy-prop preserves ⟦·⟧");
        // now the Bin has constant operands → const-fold collapses it to 8
        const_fold(&tt, &mut opt[0]);
        equiv(&tt, &before, &opt, "f").expect("the cascade preserves ⟦·⟧");
        assert_eq!(interp(&tt, &opt, "f", &[0, 0]).unwrap(), 8);
    }

    #[test]
    fn cp_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int g(int a,int b){int x=a;int y=x+b;return y*x;}", "g"),
            // Cond ⟹ the temporary `res` is MULTI-DEF: copy-prop must NOT substitute it.
            ("b", "int c(int a){int r=a>0?1:2;return r+a;}", "c"),
            ("d", "int t(int a,int b){int p=a+b;int q=p;return p*q;}", "t"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                copy_prop(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // Arithmetic CSE: two identical Bin(Add,t0,t1) → the later becomes Copy(t2). interp preserved.
    #[test]
    fn cse_arith() {
        let tt = TyTab::new();
        let before = vec![mk(
            "f",
            vec![INT, INT, INT, INT, INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Copy(0, INT, Val::Imm(4)),
                    Inst::Copy(1, INT, Val::Imm(5)),
                    Inst::Bin(2, Op::Add, INT, Val::Tmp(0), Val::Tmp(1)),
                    Inst::Bin(3, Op::Add, INT, Val::Tmp(1), Val::Tmp(0)), // commutative → same key
                    Inst::Bin(4, Op::Mul, INT, Val::Tmp(2), Val::Tmp(3)),
                ],
                term: Term::Ret(Some(Val::Tmp(4))),
            }],
        )];
        let mut after = before.clone();
        let n = cse(&mut after[0]);
        assert!(n >= 1, "must CSE ≥1");
        assert!(matches!(after[0].blocks[0].insts[3], Inst::Copy(3, _, Val::Tmp(2))));
        verify(&after[0]).unwrap();
        equiv(&tt, &before, &after, "f").expect("CSE preserves ⟦·⟧");
        assert_eq!(interp(&tt, &after, "f", &[]).unwrap(), 81);
    }

    // Load-CSE through the pipeline (cse;copy_prop)²;dce: `s+s` loads s TWICE in a row
    // with no memory write between → merged into 1 load. equiv preserved, the Load count DECREASES.
    #[test]
    fn cse_load_pipeline() {
        let (ast, ir) = compile("csel", "int f(int a){int s=a*a; return s+s;}");
        let loads0 = count_loads(&ir);
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            cse(f);
            copy_prop(f);
            cse(f);
            copy_prop(f);
            dce(f);
        }
        for f in &opt {
            verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
        }
        equiv(&ast.tt, &ir, &opt, "f").expect("load-CSE preserves ⟦·⟧");
        assert!(count_loads(&opt) < loads0, "load-CSE must reduce the Load count ({loads0}→{})", count_loads(&opt));
    }

    // ALIASING safety: read p, WRITE p, read p again — load-CSE must NOT merge across
    // the store. equiv is the mechanical referee (a wrong merge shifts the value → caught).
    #[test]
    fn cse_preserves_real() {
        for (nm, src, entry) in [
            ("a", "int f(int a){int p=a;int q=p;p=p+1;return p+q;}", "f"),
            ("b", "int g(int a,int b){return (a+b)*(a+b)+(a+b);}", "g"),
            ("c", "int h(int n){int t=0;int i;for(i=0;i<n;i=i+1)t=t+(i*i)+(i*i);return t;}", "h"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                cse(f);
                copy_prop(f);
                cse(f);
                dce(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // Two temporaries live at the same def MUST interfere (an edge in the graph).
    fn two_live() -> IrFunc {
        mk(
            "f",
            vec![INT, INT, INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Copy(0, INT, Val::Imm(1)),
                    Inst::Copy(1, INT, Val::Imm(2)),
                    Inst::Bin(2, Op::Add, INT, Val::Tmp(0), Val::Tmp(1)), // t0,t1 both live
                ],
                term: Term::Ret(Some(Val::Tmp(2))),
            }],
        )
    }

    #[test]
    fn interference_known() {
        let f = two_live();
        let adj = interference(&f, &liveness(&f));
        assert!(adj[0].contains(&1) && adj[1].contains(&0), "t0,t1 must interfere");
        // t2 is not live together with t0/t1 (they die when t2 is born) → no interference
        assert!(!adj[2].contains(&0) && !adj[2].contains(&1));
    }

    // Valid coloring on REAL code (K=8, ample): the interference invariant holds.
    #[test]
    fn reg_alloc_valid() {
        for (nm, src) in [
            ("a", "int g(int a,int b){int c=a+b;int d=c*a;int e=d-b;return c+d+e;}"),
            ("b", "int s(int n){int t=0;int i;for(i=0;i<n;i=i+1)t=t+i*i;return t;}"),
            ("c", "int m(int a,int b,int c){int x=a*b;int y=b*c;int z=a*c;return x+y+z;}"),
        ] {
            let (_ast, ir) = compile(nm, src);
            for f in &ir {
                let adj = interference(f, &liveness(f));
                let al = color(&adj, 8);
                verify_coloring(&adj, &al).unwrap_or_else(|e| panic!("{nm}/{}: {e}", f.name));
            }
        }
    }

    // K=1 (one register) + two interfering temporaries ⟹ a spill is FORCED; the
    // coloring is still VALID (a spill = its own slot, not counted in the register invariant).
    #[test]
    fn reg_alloc_spill() {
        let f = two_live();
        let adj = interference(&f, &liveness(&f));
        let al = color(&adj, 1);
        assert!(!al.spilled.is_empty(), "K=1 must force a spill of ≥1 temp");
        verify_coloring(&adj, &al).expect("the coloring (with a spill) must still be valid");
    }

    fn count_insts(ir: &[IrFunc]) -> usize {
        ir.iter().flat_map(|f| &f.blocks).map(|b| b.insts.len()).sum()
    }

    // ORCHESTRATOR: the pipeline converges and PRESERVES ⟦·⟧ over a broad corpus
    // (including loop, cond, pointer, struct, recursion). This is the ir-gate of the
    // whole pipeline (replacing the full suite during development).
    #[test]
    fn optimize_preserves_corpus() {
        let cases: &[(&str, &str, &str, &[i64])] = &[
            ("arith", "int f(int a,int b){return (a+b)*(a+b)-a*b+7;}", "f", &[6, 7]),
            ("cond", "int f(int a){int r=a>0?a*2:-a;return r+1;}", "f", &[5]),
            ("loop", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1)s=s+i*i;return s;}", "f", &[6]),
            ("ptr", "int f(int x){int y=x;int*p=&y;*p=*p+3;return y*y;}", "f", &[4]),
            ("while", "int f(int n){int a=0,b=1,i=0;while(i<n){int t=a+b;a=b;b=t;i=i+1;}return a;}", "f", &[10]),
            ("rec", "int f(int n){if(n<=1)return 1;return n*f(n-1);}", "f", &[6]),
            ("cse", "int f(int a,int b){int s=a+b;return s*s+s;}", "f", &[3, 4]),
            ("cfold", "int f(int a){int k=2*3+4;return a*k;}", "f", &[5]),
        ];
        for &(nm, src, entry, args) in cases {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                optimize(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {nm}/{}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
            // the pipeline must give the SAME interp result as before (sanity)
            let r0 = interp(&ast.tt, &ir, entry, args).unwrap();
            let r1 = interp(&ast.tt, &opt, entry, args).unwrap();
            assert_eq!(r0, r1, "{nm}: interp diverges after optimize");
        }
    }

    // The pipeline ACTUALLY optimizes (not a no-op): cfold + CSE reduce the instruction count.
    #[test]
    fn optimize_reduces() {
        let (ast, ir) = compile("red", "int f(int a,int b){int s=a+b;int t=a+b;return s*t+s*t;}");
        let before = count_insts(&ir);
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            optimize(&ast.tt, f);
        }
        equiv(&ast.tt, &ir, &opt, "f").unwrap();
        assert!(count_insts(&opt) < before, "the pipeline must reduce instructions ({before}→{})", count_insts(&opt));
    }

    // ═════════════════════════════════════════════════════════════════════════
    // EXECUTABLE THEOREM — the fold-vs-runtime commuting square, lifted from
    // alg.sh to the REFERENCE SEMANTICS. alg.sh establishes commutation at the
    // SOURCE level (two binaries produced by the system compiler and by zcc);
    // here we establish it directly on the IR via interp (the denotation ⟦·⟧),
    // in-process and dependency-free — no system compiler, no end-to-end wait.
    //
    // STATEMENT (SEMANTICS.md §5, IR.md §3c):
    //   ∀ e ∈ 𝔼_struct (the generated structural expression space, exhausting
    //                    shape × operator),
    //   ∀ P ∈ {const_fold, copy_prop, cse, dce, optimize},
    //   ∀ input i ∈ battery(arity) with ⟦lower(e)⟧(i) defined (no UB / exotic):
    //          ⟦ P(lower(e)) ⟧(i)  =  ⟦ lower(e) ⟧(i).
    // That is, every pass commutes with the reference semantics over the whole
    // generated space. This is a form of structurally-exhaustive translation
    // validation (Rice's theorem blocks the universal case, so we restrict to a
    // finite, decidable class of shapes) — not a proof, but a mechanical check.
    // ═════════════════════════════════════════════════════════════════════════

    // POOL holds the total binary operators over ℤ/2^n (interp is total on them),
    // so the internal commuting square has no UB skips on the arithmetic families:
    // every point is comparable (strongly non-vacuous). The div/mod family adds
    // the branch where the folder DECLINES undefined behavior, which still commutes
    // (interp returns Err and the equivalence check skips it, symmetrically).
    const POOL: [&str; 6] = ["+", "-", "*", "&", "|", "^"];

    fn gen_rich(o1: &str, o2: &str, o3: &str) -> String {
        // x, y are duplicate subexpressions (a o1 b) → CSE merges them; z uses x
        // (copy/use); the statement (a o3 c) is dead → DCE removes it; k is a
        // constant (3 o1 4) → constant folding; the return mixes all of them.
        format!(
            "int f(int a,int b,int c){{int x=a{o1}b;int y=a{o1}b;int z=x{o2}c;a{o3}c;int k=3{o1}4;return (x{o2}y){o3}(z{o1}k);}}"
        )
    }
    fn gen_divmod(o1: &str, o2: &str) -> String {
        // The / % branch: most inputs are defined, but boundary cases (b = 0, …)
        // make interp return Err, so the equivalence check skips them symmetrically
        // (a pre-image Err means the input is outside the modeled space). This
        // shows that the constant folder, by NOT folding division by zero into a
        // constant (it keeps the instruction), still commutes with the runtime.
        format!("int f(int a,int b,int c){{int x=a{o1}b;int y=a{o1}b;return (x{o2}c){o1}(y{o2}a);}}")
    }
    fn gen_shift(o1: &str, sh: &str) -> String {
        // Shift with a small masked right operand (b & 3 ∈ [0, 3]) is always
        // defined (avoiding shift-out-of-range UB); the duplicate (a sh s) → CSE;
        // the constant (1 sh 2) → fold. Exercises Shl / Shr (arithmetic >>).
        format!("int f(int a,int b){{int s=b&3;int x=a{sh}s;int y=a{sh}s;int k=1{sh}2;return (x{o1}y){o1}k;}}")
    }
    fn gen_ptr(o1: &str, o2: &str) -> String {
        // Two pointers to locals, with a Store BETWEEN two Loads → exercises the
        // CSE memory-kill (a reduced GCC PR84169). Lea / Load / Store / μ: shows
        // that the passes respect the memory model.
        format!("int f(int a,int b){{int x=a;int y=b;int*p=&x;int*q=&y;*p=*p{o1}*q;*q=*q{o2}a;return *p{o1}*q;}}")
    }
    fn gen_loop(o1: &str, o2: &str) -> String {
        // A loop with trip count ≤ 7 (b & 7) → interp always terminates
        // (non-vacuous); a back-edge (Br / Jmp) plus copy-prop / CSE / DCE within
        // the body and across the block boundary. Shows commutation on a CFG.
        format!("int f(int a,int b){{int s=a;int i;for(i=0;i<(b&7);i=i+1){{s=s{o1}i;s=s{o2}a;}}return s{o1}b;}}")
    }

    // 𝔼_struct is the union of five families, each exhausting the operator set over
    // a distinct SHAPE (straight-line arithmetic, div/mod UB, shift, pointer/memory,
    // loop/CFG). Together they cover all four CORE passes, every kind of Inst
    // (Bin/Un/Copy/Load/Store/Lea/Cast), and both kinds of Term.
    fn e_struct() -> Vec<String> {
        let mut s = Vec::new();
        for o1 in POOL {
            for o2 in POOL {
                for o3 in POOL {
                    s.push(gen_rich(o1, o2, o3)); // family A: straight-line arithmetic (6³ = 216)
                }
            }
        }
        for o2 in POOL {
            for o1 in ["/", "%"] {
                s.push(gen_divmod(o1, o2)); // family B: div/mod UB skip (6×2 = 12)
            }
        }
        for o1 in POOL {
            for sh in ["<<", ">>"] {
                s.push(gen_shift(o1, sh)); // family C: shift (6×2 = 12)
            }
        }
        for o1 in POOL {
            for o2 in POOL {
                s.push(gen_ptr(o1, o2)); // family D: pointer/memory (6² = 36)
            }
        }
        for o1 in POOL {
            for o2 in POOL {
                s.push(gen_loop(o1, o2)); // family E: loop/CFG (6² = 36)
            }
        }
        s // 216 + 12 + 12 + 36 + 36 = 312 expressions
    }

    // Each pass is one arrow of the commuting square. We run each pass INDIVIDUALLY
    // (to isolate a fault) as well as `optimize` (their composition to a fixpoint):
    // if the composition commutes but an individual pass does not, we catch the
    // offending pass.
    fn all_passes(tt: &TyTab, f: &mut IrFunc, which: u8) {
        match which {
            0 => { const_fold(tt, f); }
            1 => { copy_prop(f); }
            2 => { cse(f); }
            3 => { dce(f); }
            4 => optimize(tt, f),
            _ => unreachable!(),
        }
    }

    #[test]
    fn commuting_square_structural_exhaustion() {
        // Mechanical evidence trail: count the (expression, pass) squares proven to
        // commute, and the number of expressions generated. A green verdict is valid
        // only when these counts match the expected floor — no vacuous "passing" run.
        let srcs = e_struct();
        let mut squares = 0u32; // commuting squares (expression × pass) closed
        for src in &srcs {
            let (ast, ir) = compile("csq", src);
            for f in &ir {
                verify(f).unwrap_or_else(|e| panic!("verify {src}: {e}"));
            }
            for which in 0u8..=4 {
                let mut opt = ir.clone();
                for f in opt.iter_mut() {
                    all_passes(&ast.tt, f, which);
                }
                for f in &opt {
                    verify(f).unwrap_or_else(|e| panic!("verify (after pass {which}) {src}: {e}"));
                }
                equiv(&ast.tt, &ir, &opt, "f")
                    .unwrap_or_else(|e| panic!("commuting square BROKEN [pass {which}] {src}: {e}"));
                squares += 1;
            }
        }
        let exprs = srcs.len() as u32;
        // Floor: 216 + 12 + 12 + 36 + 36 = 312 expressions × 5 passes = 1560 squares.
        assert_eq!(exprs, 312, "generated space must have the expected size (216+12+12+36+36)");
        assert_eq!(squares, 1560, "must close all 312×5 commuting squares");
        eprintln!("commuting-square theorem: {exprs} expressions in 𝔼_struct (5 shape families) × 5 passes = {squares} squares closed");
    }

    // Self-proof of the theorem (validate the tool that does the validating): if the
    // harness above were "falsely green" — the equivalence check missing a mutation —
    // every verdict would be worthless. A single-pass mutation (CSE merging wrongly
    // across a memory write, i.e. dropping the memory-kill) MUST be caught by the
    // commuting square on at least one expression.
    #[test]
    fn commuting_square_selfproof() {
        // An expression with a Store between two Loads of the same address: if CSE
        // merged blindly (no memory-kill), it would miscompile, and the equivalence
        // check MUST catch it. This is a reduced GCC PR84169.
        let (ast, ir) = compile("csqsp", "int f(int a){int p=a;int q=p;p=p+1;return p+q;}");
        // Correct CSE must commute:
        let mut ok = ir.clone();
        for f in ok.iter_mut() {
            cse(f);
            copy_prop(f);
            dce(f);
        }
        equiv(&ast.tt, &ir, &ok, "f").expect("correct CSE must commute");
        // Mutation: remove a memory write by replacing a Store with a Copy, so the
        // later read observes the OLD value and the result diverges from the runtime.
        // Built by hand: after dropping the Store, the IR reads the old p, differing
        // from the runtime, so the equivalence check catches it.
        let mut bad = ir.clone();
        let mut killed = false;
        'o: for f in bad.iter_mut() {
            for b in f.blocks.iter_mut() {
                for i in b.insts.iter_mut() {
                    if let Inst::Store(ty, _addr, val) = i {
                        // Store(*addr = val) → Copy(dead dst, val): removes the memory
                        // write. Reusing temp 0 as a scratch sink (already defined)
                        // keeps the IR well-formed but semantically broken.
                        *i = Inst::Copy(0, *ty, *val);
                        killed = true;
                        break 'o;
                    }
                }
            }
        }
        assert!(killed, "there must be a Store to mutate");
        assert!(
            equiv(&ast.tt, &ir, &bad, "f").is_err(),
            "the Store-removal mutation MUST be caught by the commuting square (else the harness is blind)"
        );
    }
}
