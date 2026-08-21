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

use crate::ast::{Ty, TyTab, TypeId, ULONG};
use crate::ir::{
    canon, eval_bin, eval_cast, inst_def, inst_uses, term_targets, term_uses, Block, BlockId, Callee,
    Inst, IrFunc, Op, Place, Term, Tmp, Un, Val,
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
        // φ is side-effect-free (it only selects a value): a φ whose dst is unused is
        // dead and may be removed — the `!used[d]` guard protects any LIVE φ. Straight
        // lowering emits no φ, so this only affects the SSA pipeline (SCCP-deadened φ).
        | Inst::Phi(..)
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
/// Only live-OUT is consumed downstream (interference is built at defs, scanning tailward);
/// live-IN is the fixpoint's working set, not exported.
pub struct Liveness {
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
    let _ = live_in; // the fixpoint's working set; not exported (only live_out is consumed)
    Liveness { live_out }
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

// ─────────────────────────────────────────────────────────────────────────────
// Stage 5b — ABI-AWARE register allocation (the backend consumes this coloring).
//
// Extends the interference-invariant bisimulation above with the ABI (THEORY §A7,
// §D2). A machine register belongs to one of two ABI classes (AAPCS64 §6.1.1): a
// CALLER-saved register is clobbered by every `bl`; a CALLEE-saved register is
// preserved across it. Hence the extra proof obligation of the plain interference
// invariant:
//   CALL-CLOBBER SET-DISJOINTNESS — a temp whose value is LIVE ACROSS a call must
//   receive a callee-saved color, else the `bl` overwrites it (⟦·⟧ broken).
// We model this by RESTRICTING such a temp's select-range to the callee-saved colors.
// Colors within a class are ordered [caller … | callee …]; `ncaller` marks the split.
// A non-crossing temp prefers the low (caller) colors (a callee-saved home costs a
// prologue save/restore); a crossing temp is confined to [ncaller, k).
pub struct ClassBudget {
    pub k: u32,       // total colors in the class
    pub ncaller: u32, // colors [0,ncaller) are caller-saved; [ncaller,k) callee-saved
}

/// Chaitin simplify/select over ONE register class (`in_class` selects its temps;
/// interference edges to out-of-class temps are ignored — the two files are disjoint).
/// A `crossing` temp may only take a callee-saved color. Result: per-temp color, None = spill.
///
/// `bias` carries CONSERVATIVE register coalescing (Phase A): `bias[v]` lists the temps
/// move-related to v (a non-interfering `Copy` partner). At SELECT, v prefers a color
/// already held by such a partner, so the copy lowers to a same-register `mov` the
/// peephole elides. This is coalescing WITHOUT node-merge — it only picks among the
/// colors already free & legal for v, so it can never worsen k-colorability and NEVER
/// changes the coloring's validity: the interference invariant (hence the ⟦·⟧-preserving
/// rename-bisimulation) is identical with or without the bias. Correctness therefore
/// rests on the SAME `verify_abi` theorem as Stage 5b — no new proof obligation.
pub fn color_abi(
    adj: &[HashSet<Tmp>],
    in_class: &[bool],
    b: &ClassBudget,
    crossing: &[bool],
    bias: &[Vec<Tmp>],
) -> Vec<Option<u32>> {
    let nt = adj.len();
    let k = b.k as usize;
    // class-local degree: count only in-class, not-yet-removed neighbors
    let mut degree: Vec<usize> = (0..nt)
        .map(|v| {
            if in_class[v] {
                adj[v].iter().filter(|&&u| in_class[u as usize]).count()
            } else {
                0
            }
        })
        .collect();
    let mut removed = vec![false; nt];
    let mut stack: Vec<Tmp> = Vec::new();
    for _ in 0..nt {
        // SIMPLIFY: prefer a class-degree<k node (certainly colorable); else max-degree (potential spill)
        let low = (0..nt).find(|&v| in_class[v] && !removed[v] && degree[v] < k);
        let v = match low.or_else(|| {
            (0..nt)
                .filter(|&v| in_class[v] && !removed[v])
                .max_by_key(|&v| degree[v])
        }) {
            Some(v) => v,
            None => break,
        };
        removed[v] = true;
        stack.push(v as u32);
        for &nb in &adj[v] {
            if in_class[nb as usize] && !removed[nb as usize] {
                degree[nb as usize] -= 1;
            }
        }
    }
    // SELECT: smallest free color in the temp's allowed range; out of range → spill.
    let mut colr = vec![None; nt];
    while let Some(v) = stack.pop() {
        let mut used = vec![false; k];
        for &nb in &adj[v as usize] {
            if in_class[nb as usize]
                && let Some(c) = colr[nb as usize]
            {
                used[c as usize] = true;
            }
        }
        let lo = if crossing[v as usize] { b.ncaller } else { 0 };
        let free = |c: u32| c >= lo && c < b.k && !used[c as usize];
        // biased coalescing: prefer a free, in-range color already held by a same-class
        // move partner (the copy becomes a self-move). Falls back to the smallest free
        // color — so the result is always a valid coloring regardless of the bias.
        let biased = bias[v as usize]
            .iter()
            .filter(|&&p| in_class[p as usize])
            .filter_map(|&p| colr[p as usize])
            .find(|&c| free(c));
        colr[v as usize] = biased.or_else(|| (lo..b.k).find(|&c| free(c)));
    }
    colr
}

/// A temp's HOME after ABI allocation: (is_fp, color-within-class), or None = spill (memory slot).
pub type AbiHome = Option<(bool, u32)>;

/// Stage 5b entry — partition temps by ABI file (GP int/ptr vs FP float), color each
/// against its budget, confining call-crossing temps to callee-saved. Falls back to
/// ALL-SPILL for a function containing inline asm (`Inst::Asm`): its operand pool grows
/// over x9../v16.. without bound and can clobber ANY allocatable register, defeating the
/// disjointness invariant — so no home is safe (the pre-Stage-5b memory model, verbatim).
pub fn abi_alloc(tt: &TyTab, f: &IrFunc, gp: &ClassBudget, fp: &ClassBudget) -> Vec<AbiHome> {
    let nt = f.temps.len();
    let mut home: Vec<AbiHome> = vec![None; nt];
    if f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Asm(..))) {
        return home; // conservative all-spill
    }
    let lv = liveness(f);
    let adj = interference(f, &lv);
    // crossing[t]: t ∈ live-out(call) \ {def(call)} for some call ⟹ its value must survive the bl.
    let mut crossing = vec![false; nt];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true;
        }
        for i in b.insts.iter().rev() {
            // `live` == live-OUT(i) at this point (before the backward transfer)
            if matches!(i, Inst::Call(..) | Inst::CallX(..)) {
                let d = inst_def(i);
                for (t, &alive) in live.iter().enumerate() {
                    if alive && Some(t as u32) != d {
                        crossing[t] = true;
                    }
                }
            }
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
        }
    }
    // Conservative coalescing candidates: a Copy whose dst/src are distinct temps that
    // do NOT interfere (their live ranges are disjoint) can share a register. Same class
    // by construction (a Copy preserves type); the per-class select filters anyway.
    let mut move_adj: Vec<Vec<Tmp>> = vec![Vec::new(); nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Copy(d, _, Val::Tmp(s)) = i
                && d != s
                && !adj[*d as usize].contains(s)
            {
                move_adj[*d as usize].push(*s);
                move_adj[*s as usize].push(*d);
            }
        }
    }
    let is_fp: Vec<bool> = f.temps.iter().map(|&ty| tt.is_float(ty)).collect();
    let gp_in: Vec<bool> = is_fp.iter().map(|&b| !b).collect();
    let gc = color_abi(&adj, &gp_in, gp, &crossing, &move_adj);
    let fc = color_abi(&adj, &is_fp, fp, &crossing, &move_adj);
    for t in 0..nt {
        home[t] = if is_fp[t] { fc[t] } else { gc[t] }.map(|c| (is_fp[t], c));
    }
    home
}

/// Mechanically CHECK the Stage-5b obligations (the P-verify): (1) the interference
/// invariant per class — two same-class simultaneously-live temps get distinct homes;
/// (2) call-clobber — no call-crossing temp received a caller-saved color.
/// Test-only: the theorem is checked over a corpus in `tests`, not on every compile.
#[cfg(test)]
pub fn verify_abi(
    tt: &TyTab,
    f: &IrFunc,
    home: &[AbiHome],
    gp: &ClassBudget,
    fp: &ClassBudget,
) -> Result<(), String> {
    let lv = liveness(f);
    let adj = interference(f, &lv);
    for u in 0..adj.len() {
        if let Some((fu, cu)) = home[u] {
            for &v in &adj[u] {
                if let Some((fv, cv)) = home[v as usize]
                    && fu == fv
                    && cu == cv
                {
                    return Err(format!("interference (t{u},t{v}) share {}-reg {cu}", if fu { "fp" } else { "gp" }));
                }
            }
        }
    }
    // recompute crossing and check no caller-saved home for a crossing temp
    let mut crossing = vec![false; f.temps.len()];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &x in &buf {
            live[x as usize] = true;
        }
        for i in b.insts.iter().rev() {
            if matches!(i, Inst::Call(..) | Inst::CallX(..)) {
                let d = inst_def(i);
                for (t, &al) in live.iter().enumerate() {
                    if al && Some(t as u32) != d {
                        crossing[t] = true;
                    }
                }
            }
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &x in &buf {
                live[x as usize] = true;
            }
        }
    }
    let _ = tt;
    for (t, &h) in home.iter().enumerate() {
        if let Some((is_fp, c)) = h {
            let ncaller = if is_fp { fp.ncaller } else { gp.ncaller };
            if crossing[t] && c < ncaller {
                return Err(format!("call-crossing t{t} got caller-saved {}-color {c}", if is_fp { "fp" } else { "gp" }));
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
//  IR→IR transform. `abi_alloc` is wired into arm64_elf.rs (Stage 5b), gated on the same
//  volatile-free/φ-free IR as opt; -O0 keeps the naive spill-per-node model.)
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

// ─────────────────────────────────────────────────────────────────────────────
// SSA CONSTRUCTION — mem2reg (Braun, Buchwald, Hack, Leißa, Mallon, Zwinkau,
// "Simple and Efficient Construction of Static Single Assignment Form", CC 2013).
// On-the-fly, NO dominance frontier / dom-tree (§2) — slimmer than classic Cytron.
//
// GOVERNING THEOREM (CbC, supreme over the QBE projection): `⟦f⟧ = ⟦to_ssa(f)⟧` —
// a semantics-preserving rewrite, MEASURED by `equiv` over the input battery
// (translation validation), never trusted by reasoning.
//
// It promotes PROMOTABLE locals — scalar, type-consistent, non-parameter, and
// non-address-taken — out of frame MEMORY (Lea/Load/Store) into SSA temporaries
// reconciled at join points by Inst::Phi. Anything else stays in memory untouched;
// the analysis is conservative (when in doubt, leave in memory), so promotion never
// changes observable behavior — only which values live in temps vs the stack.
// ─────────────────────────────────────────────────────────────────────────────

/// Predecessor lists — the inverse of `successors` (the CFG read backwards).
fn predecessors(f: &IrFunc) -> Vec<Vec<BlockId>> {
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
fn rpo(f: &IrFunc) -> Vec<BlockId> {
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
fn is_addr_use(i: &Inst, u: Tmp) -> bool {
    match i {
        Inst::Load(_, _, Val::Tmp(a)) => *a == u,
        // the address slot only; if `u` is ALSO the stored value it escapes.
        Inst::Store(_, Val::Tmp(a), v) => *a == u && !matches!(v, Val::Tmp(t) if *t == u),
        _ => false,
    }
}

/// Record the access type of a local; a second, DIFFERENT type (type punning via a
/// union / reinterpretation) makes it non-promotable (an SSA value has one type).
fn note_ty(ty_of: &mut HashMap<u32, TypeId>, escaped: &mut HashSet<u32>, off: u32, ty: TypeId) {
    match ty_of.get(&off) {
        Some(&prev) if prev != ty => {
            escaped.insert(off);
        }
        None => {
            ty_of.insert(off, ty);
        }
        _ => {}
    }
}

/// What to do with one instruction during the fill walk (computed WITHOUT holding a
/// borrow of the instruction, so the `Keep` arm can move it).
enum Act {
    Drop,                     // a dead Lea of a promoted local
    Keep,                     // untouched
    Store(usize, Val),        // writeVariable(var, block, val); delete the Store
    Load(Tmp, TypeId, usize), // dst = readVariable(var, block); Load → Copy(dst, ty, ·)
}

/// Braun's incremental-construction state (per function).
struct Ssa {
    var_ty: Vec<TypeId>,                     // [var] → the local's scalar type (φ / Copy type)
    current_def: Vec<HashMap<BlockId, Val>>, // [var][block] → the reaching value (Braun's currentDef)
    sealed: Vec<bool>,                       // [block] → all predecessors known?
    incomplete: Vec<Vec<(usize, Tmp)>>,      // [block] → (var, φ) awaiting operands until sealed
    preds: Vec<Vec<BlockId>>,
    phi_block: HashMap<Tmp, BlockId>,       // φ temp → the block it heads
    phi_var: HashMap<Tmp, usize>,           // φ temp → the variable it reconciles
    phi_arms: HashMap<Tmp, Vec<(BlockId, Val)>>, // φ temp → [(pred, value)]
    base: u32,                              // first fresh temp id (= |temps| before construction)
    new_temps: Vec<TypeId>,                 // types of the φ temps appended to Γ
}

impl Ssa {
    fn new_temp(&mut self, ty: TypeId) -> Tmp {
        let t = self.base + self.new_temps.len() as u32;
        self.new_temps.push(ty);
        t
    }
    fn new_phi(&mut self, var: usize, block: BlockId) -> Tmp {
        let ty = self.var_ty[var];
        let t = self.new_temp(ty);
        self.phi_block.insert(t, block);
        self.phi_var.insert(t, var);
        self.phi_arms.insert(t, Vec::new());
        t
    }
    fn write_var(&mut self, var: usize, block: BlockId, val: Val) {
        self.current_def[var].insert(block, val);
    }
    /// readVariable (Braun §2): the value of `var` reaching the START-or-here of
    /// `block`, following the local definition first, else recursing over the CFG.
    fn read_var(&mut self, var: usize, block: BlockId) -> Val {
        if let Some(v) = self.current_def[var].get(&block) {
            return *v;
        }
        self.read_var_recursive(var, block)
    }
    fn read_var_recursive(&mut self, var: usize, block: BlockId) -> Val {
        let val = if !self.sealed[block as usize] {
            // CFG still incomplete here: place an operandless φ, filled at seal time.
            let phi = self.new_phi(var, block);
            self.incomplete[block as usize].push((var, phi));
            Val::Tmp(phi)
        } else if self.preds[block as usize].is_empty() {
            // Undefined read: the recursion reached a block with NO predecessor (the entry,
            // or an unreachable block) without finding a definition — the variable is read
            // before any write on this path. The C program has UB (C99 6.3.2.1p2: an
            // indeterminate value of an object whose address is never taken). Building a φ
            // here would be malformed (a φ needs a predecessor edge) ⟹ broken IR. Any value
            // is permissible under the UB, so materialize a deterministic, well-formed 0
            // (as LLVM lowers `undef`). GCC torture pr43629.
            Val::Imm(0)
        } else if self.preds[block as usize].len() == 1 {
            let p = self.preds[block as usize][0];
            self.read_var(var, p) // no join ⟹ no φ needed (minimal SSA)
        } else {
            // ≥2 predecessors: a φ is required. Write it FIRST to break loops.
            let phi = self.new_phi(var, block);
            self.write_var(var, block, Val::Tmp(phi));
            self.add_phi_operands(var, phi)
        };
        self.write_var(var, block, val);
        val
    }
    fn add_phi_operands(&mut self, var: usize, phi: Tmp) -> Val {
        let block = self.phi_block[&phi];
        for p in self.preds[block as usize].clone() {
            let v = self.read_var(var, p);
            self.phi_arms.get_mut(&phi).unwrap().push((p, v));
        }
        Val::Tmp(phi)
    }
    fn seal(&mut self, block: BlockId) {
        if self.sealed[block as usize] {
            return;
        }
        for (var, phi) in std::mem::take(&mut self.incomplete[block as usize]) {
            self.add_phi_operands(var, phi);
        }
        self.sealed[block as usize] = true;
    }
}

/// CFG-completeness precondition (shared by every dominance/reachability-based pass).
/// A computed goto (`GotoPtr`, EXT gcc) transfers to a data-dependent address-taken
/// label; its edges are NOT modeled by any block terminator (the IR block after it is a
/// dead-end `Ret`). So `predecessors`/`rpo`/`dominators`/reachability see an INCOMPLETE
/// CFG — e.g. a loop closed only by `goto *p` looks acyclic. Any transform that trusts the
/// CFG (mem2reg φ-placement, GVN dominance, SCCP reachability) is then UNSOUND. Passes
/// bail on such a function ⟹ identity transform, leaving it for the naive -O0 backend.
/// GCC torture 920302-1, 920501-3 (mem2reg dropped a loop φ / GVN pruned a live block →
/// wild GotoPtr → SIGSEGV).
fn cfg_complete(f: &IrFunc) -> bool {
    !f.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, Inst::GotoPtr(..))))
}

pub fn to_ssa(tt: &TyTab, f: &mut IrFunc) {
    if !cfg_complete(f) {
        return;
    }
    // ── 1. Promotability analysis ────────────────────────────────────────────
    // Parameters live in ABI-seeded frame slots (never Stored in the body) → keep
    // them in memory. Every Lea(t, Local(off)) makes t a "pointer to off".
    let mut escaped: HashSet<u32> = f.params.iter().map(|&(off, _)| off).collect();
    let mut lea_off: HashMap<Tmp, u32> = HashMap::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Lea(t, Place::Local(off)) = i {
                lea_off.insert(*t, *off);
            }
        }
    }
    // An offset escapes if any Lea of it is used other than as a Load/Store address;
    // its type is the (single) type of its scalar accesses.
    let mut ty_of: HashMap<u32, TypeId> = HashMap::new();
    let mut has_mem: HashSet<u32> = HashSet::new();
    let mut uses = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                Inst::Load(_, ty, Val::Tmp(a)) => {
                    if let Some(&off) = lea_off.get(a) {
                        note_ty(&mut ty_of, &mut escaped, off, *ty);
                        has_mem.insert(off);
                    }
                }
                Inst::Store(ty, Val::Tmp(a), _) => {
                    if let Some(&off) = lea_off.get(a) {
                        note_ty(&mut ty_of, &mut escaped, off, *ty);
                        has_mem.insert(off);
                    }
                }
                _ => {}
            }
            uses.clear();
            inst_uses(i, &mut uses);
            for &u in &uses {
                if let Some(&off) = lea_off.get(&u) {
                    if !is_addr_use(i, u) {
                        escaped.insert(off);
                    }
                }
            }
        }
        uses.clear();
        term_uses(&b.term, &mut uses);
        for &u in &uses {
            if let Some(&off) = lea_off.get(&u) {
                escaped.insert(off);
            }
        }
    }
    // The promotable set: scalar (int/float/pointer, per LP64 TyTab), has real
    // memory traffic, not escaped. A dense var index gives φ/currentDef arrays.
    let mut promotable: Vec<u32> = ty_of
        .iter()
        .filter_map(|(&off, &ty)| {
            let scalar = tt.is_integer(ty)
                || tt.is_float(ty)
                || matches!(tt.tys[ty as usize], Ty::Ptr(_));
            (!escaped.contains(&off) && has_mem.contains(&off) && scalar).then_some(off)
        })
        .collect();
    if promotable.is_empty() {
        return; // no promotion possible ⟹ identity transform (zero perf change)
    }
    promotable.sort_unstable();
    let off2var: HashMap<u32, usize> = promotable.iter().enumerate().map(|(i, &o)| (o, i)).collect();
    let var_ty: Vec<TypeId> = promotable.iter().map(|o| ty_of[o]).collect();

    let nb = f.blocks.len();
    let mut s = Ssa {
        var_ty,
        current_def: vec![HashMap::new(); promotable.len()],
        sealed: vec![false; nb],
        incomplete: vec![Vec::new(); nb],
        preds: predecessors(f),
        phi_block: HashMap::new(),
        phi_var: HashMap::new(),
        phi_arms: HashMap::new(),
        base: f.temps.len() as u32,
        new_temps: Vec::new(),
    };

    // ── 2. Fill (RPO) — Store→writeVar (delete), Load→Copy(readVar), dead Lea→drop.
    // Seal a block on entry once all its predecessors are filled (forward joins seal
    // eagerly ⟹ minimal φ); a loop header's back-edge predecessor is still unfilled,
    // so it stays unsealed and its reads create incomplete φ, resolved in step 3.
    let order = rpo(f);
    let mut filled = vec![false; nb];
    for &bi in &order {
        let blk = bi as usize;
        if !s.sealed[blk] && s.preds[blk].iter().all(|&p| filled[p as usize]) {
            s.seal(bi);
        }
        let mut new_insts: Vec<Inst> = Vec::with_capacity(f.blocks[blk].insts.len());
        for inst in std::mem::take(&mut f.blocks[blk].insts) {
            let act = match &inst {
                Inst::Lea(_, Place::Local(off)) if off2var.contains_key(off) => Act::Drop,
                Inst::Store(_, Val::Tmp(a), val)
                    if lea_off.get(a).is_some_and(|o| off2var.contains_key(o)) =>
                {
                    Act::Store(off2var[&lea_off[a]], *val)
                }
                Inst::Load(d, ty, Val::Tmp(a))
                    if lea_off.get(a).is_some_and(|o| off2var.contains_key(o)) =>
                {
                    Act::Load(*d, *ty, off2var[&lea_off[a]])
                }
                _ => Act::Keep,
            };
            match act {
                Act::Drop => {}
                Act::Store(var, val) => s.write_var(var, bi, val),
                Act::Load(d, ty, var) => {
                    let v = s.read_var(var, bi);
                    // A Store into a float(size 4) cell narrows to f32 and the matching
                    // Load widens f32→f64 (ir.rs Store/Load, backend store_narrow / `fcvt
                    // d,s`), so the store∘load round-trip = round-to-f32, NOT identity.
                    // mem2reg elides both, so that narrowing must be restored explicitly —
                    // else the promoted value keeps illegal f64 precision (C99 6.3.1.5).
                    // A self-Cast float→float narrows (eval_cast / backend `fcvt s,d;fcvt
                    // d,s`). Integer cells round-trip as identity (temps are kept canon'd
                    // to their type), so a plain Copy stays faithful there.
                    if tt.is_float(ty) && tt.size(ty) == 4 {
                        new_insts.push(Inst::Cast(d, ty, ty, v));
                    } else {
                        new_insts.push(Inst::Copy(d, ty, v));
                    }
                }
                Act::Keep => new_insts.push(inst),
            }
        }
        f.blocks[blk].insts = new_insts;
        filled[blk] = true;
    }

    // ── 3. Seal any remaining blocks (loop headers) — now every predecessor is
    // filled, so incomplete φ get their operands.
    for bi in 0..nb as BlockId {
        s.seal(bi);
    }

    // ── 4. Extend Γ with the φ temporaries, then materialize Inst::Phi at each
    // block head (deterministic order by temp id).
    f.temps.extend(s.new_temps.iter().copied());
    let mut per_block: Vec<Vec<(Tmp, TypeId, Vec<(BlockId, Val)>)>> = vec![Vec::new(); nb];
    for (&phi, &blk) in &s.phi_block {
        let ty = s.var_ty[s.phi_var[&phi]];
        per_block[blk as usize].push((phi, ty, s.phi_arms[&phi].clone()));
    }
    for blk in 0..nb {
        let mut ps = std::mem::take(&mut per_block[blk]);
        ps.sort_by_key(|(t, _, _)| *t);
        let mut ni: Vec<Inst> =
            ps.into_iter().map(|(t, ty, arms)| Inst::Phi(t, ty, arms)).collect();
        ni.append(&mut f.blocks[blk].insts);
        f.blocks[blk].insts = ni;
    }

    // ── 5. Trivial-φ elimination (Braun §3.1): a φ whose operands (excluding
    // self-references) are one single value V carries V on every edge → replace it
    // by V everywhere and remove it. Semantics-preserving; cascades to a fixpoint.
    remove_trivial_phis(f);
}

fn val_eq(a: Val, b: Val) -> bool {
    match (a, b) {
        (Val::Tmp(x), Val::Tmp(y)) => x == y,
        (Val::Imm(x), Val::Imm(y)) => x == y,
        (Val::FImm(x), Val::FImm(y)) => x == y,
        _ => false,
    }
}

fn remove_trivial_phis(f: &mut IrFunc) {
    loop {
        // Find one trivial φ: its arms, minus self-references, reduce to a single value.
        let mut trivial: Option<(Tmp, Val)> = None;
        'scan: for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Phi(d, _, arms) = i {
                    let mut uniq: Option<Val> = None;
                    let mut same = true;
                    for (_, v) in arms {
                        if matches!(v, Val::Tmp(t) if *t == *d) {
                            continue; // a self-reference does not count
                        }
                        match uniq {
                            None => uniq = Some(*v),
                            Some(u) => {
                                if !val_eq(u, *v) {
                                    same = false;
                                    break;
                                }
                            }
                        }
                    }
                    // uniq==None ⟹ only self-refs (undefined / unreachable): leave it.
                    if same {
                        if let Some(u) = uniq {
                            trivial = Some((*d, u));
                            break 'scan;
                        }
                    }
                }
            }
        }
        let Some((d, v)) = trivial else { break };
        for b in f.blocks.iter_mut() {
            b.insts.retain(|i| !matches!(i, Inst::Phi(dd, ..) if *dd == d));
            for i in b.insts.iter_mut() {
                each_use_mut(i, |x| {
                    if matches!(x, Val::Tmp(t) if *t == d) {
                        *x = v;
                    }
                });
            }
            each_use_term_mut(&mut b.term, |x| {
                if matches!(x, Val::Tmp(t) if *t == d) {
                    *x = v;
                }
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OUT-OF-SSA — φ-destruction (Stage 3). The INVERSE of to_ssa's join reconciliation:
// every Inst::Phi is replaced by explicit Inst::Copy on the incoming control edges,
// leaving IR the backend can consume directly (φ is an SSA artifact with no machine
// form — see ir.rs Inst::Phi).
//
// GOVERNING THEOREM (CbC, supreme over the QBE projection): `⟦f⟧ = ⟦out_of_ssa(f)⟧`
// for f in SSA form — MEASURED by `equiv` (translation validation), never trusted.
// Composed with Stage 2 this closes the round trip ⟦to_ssa(f)⟧ = ⟦out_of_ssa(to_ssa(f))⟧.
//
// TWO CLASSIC MISCOMPILE TRAPS (csmith bait), both handled by construction:
//   • critical edges — a φ-block has ≥2 preds; if a predecessor also has ≥2
//     successors, copies appended to it would leak onto its OTHER edge. Such an edge
//     is SPLIT: a fresh block on the edge holds the copies (`split_edge`).
//   • the swap / lost-copy problem — φ-nodes at a block are PARALLEL (simultaneous).
//     Sequentializing {a←b, b←a} naively yields a=b; b=b. `seq_pcopy` orders the
//     copies (a leaf whose dst is read by no pending copy is emitted first) and breaks
//     any remaining cycle by saving one value into a fresh temp.
// ─────────────────────────────────────────────────────────────────────────────

/// Sequentialize a PARALLEL copy set {dst ← src} (dsts distinct) into an ordered list
/// of copies with identical net effect. `fresh(ty)` mints a temp to break cycles.
fn seq_pcopy(pc: &[(Tmp, TypeId, Val)], fresh: &mut impl FnMut(TypeId) -> Tmp) -> Vec<(Tmp, TypeId, Val)> {
    // Identity copies (d ← d) carry no information — drop them.
    let mut pending: Vec<(Tmp, TypeId, Val)> =
        pc.iter().cloned().filter(|(d, _, s)| !matches!(s, Val::Tmp(t) if t == d)).collect();
    let mut out = Vec::new();
    while !pending.is_empty() {
        // A copy is safe to emit now iff its dst is read by no OTHER pending copy
        // (emitting it cannot clobber a value still needed by the parallel set).
        let leaf = pending
            .iter()
            .position(|(d, _, _)| !pending.iter().any(|(d2, _, s)| d2 != d && matches!(s, Val::Tmp(t) if t == d)));
        match leaf {
            Some(i) => out.push(pending.remove(i)),
            None => {
                // All remaining copies form cycles (each dst is read by another): break
                // one by saving the current value of a dst into a fresh temp, then
                // redirect readers to it — the cycle becomes a chain.
                let (d, ty, _) = pending[0];
                let t = fresh(ty);
                out.push((t, ty, Val::Tmp(d))); // t ← d (preserve d's incoming value)
                for (_, _, s) in pending.iter_mut() {
                    if matches!(s, Val::Tmp(x) if *x == d) {
                        *s = Val::Tmp(t);
                    }
                }
            }
        }
    }
    out
}

/// Replace, in a terminator, the target BlockId `from` by `to` (edge redirection).
fn retarget(term: &mut Term, from: BlockId, to: BlockId) {
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

pub fn out_of_ssa(f: &mut IrFunc) {
    let preds = predecessors(f);
    let succ_cnt: Vec<usize> = successors(f).iter().map(|s| s.len()).collect();

    // Copies to append at the END of a single-successor predecessor (before its term).
    let mut append_to: HashMap<BlockId, Vec<(Tmp, TypeId, Val)>> = HashMap::new();
    // Critical edges to split: (pred, φ-block, the parallel copy set on that edge).
    let mut splits: Vec<(BlockId, BlockId, Vec<(Tmp, TypeId, Val)>)> = Vec::new();

    for b in 0..f.blocks.len() as BlockId {
        // The φ-nodes heading this block (dst, ty, arms), in program order.
        let phis: Vec<(Tmp, TypeId, Vec<(BlockId, Val)>)> = f.blocks[b as usize]
            .insts
            .iter()
            .filter_map(|i| match i {
                Inst::Phi(d, ty, arms) => Some((*d, *ty, arms.clone())),
                _ => None,
            })
            .collect();
        if phis.is_empty() {
            continue;
        }
        // For each DISTINCT predecessor edge, gather the parallel copy set {dst ← arm(P)}.
        let mut seen: HashSet<BlockId> = HashSet::new();
        for &p in &preds[b as usize] {
            if !seen.insert(p) {
                continue; // a multi-edge (Br to the same block twice) — copies are identical
            }
            let pc: Vec<(Tmp, TypeId, Val)> = phis
                .iter()
                .map(|(d, ty, arms)| {
                    let v = arms
                        .iter()
                        .find(|(pp, _)| *pp == p)
                        .map(|(_, v)| *v)
                        .expect("out_of_ssa: φ missing an arm for a predecessor");
                    (*d, *ty, v)
                })
                .collect();
            if succ_cnt[p as usize] == 1 {
                append_to.entry(p).or_default().extend(pc); // safe: p's only edge is p→b
            } else {
                splits.push((p, b, pc)); // critical edge → split
            }
        }
    }

    // Fresh temps for cycle-breaking are appended to Γ.
    let mut new_temps: Vec<TypeId> = Vec::new();
    let base = f.temps.len() as u32;
    let mut fresh = |ty: TypeId| -> Tmp {
        let t = base + new_temps.len() as u32;
        new_temps.push(ty);
        t
    };

    // Apply single-successor appends: insert the sequentialized copies before the term.
    for (p, pc) in append_to {
        let seq = seq_pcopy(&pc, &mut fresh);
        let insts = &mut f.blocks[p as usize].insts;
        for (d, ty, s) in seq {
            insts.push(Inst::Copy(d, ty, s));
        }
    }

    // Apply critical-edge splits: a new block E = {copies; Jmp(b)} on the edge p→b.
    for (p, b, pc) in splits {
        let seq = seq_pcopy(&pc, &mut fresh);
        let insts = seq.into_iter().map(|(d, ty, s)| Inst::Copy(d, ty, s)).collect();
        let e = f.blocks.len() as BlockId;
        f.blocks.push(Block { insts, term: Term::Jmp(b) });
        retarget(&mut f.blocks[p as usize].term, b, e);
    }

    f.temps.extend(new_temps);

    // Every φ has been replaced by edge copies — remove them all.
    for b in f.blocks.iter_mut() {
        b.insts.retain(|i| !matches!(i, Inst::Phi(..)));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SCCP — Sparse Conditional Constant Propagation (Wegman & Zadeck, TOPLAS 1991).
// An SSA pass (runs AFTER to_ssa, BEFORE out_of_ssa). Strictly stronger than
// const_fold + copy_prop: it folds a value that is constant on every REACHABLE path
// (through φ-joins), simultaneously discovering which CFG edges are dead — a branch
// on a proven constant makes one successor unreachable, which can in turn prove more
// φ constant. The two facts (reachability × constant lattice) reinforce each other in
// one fixpoint; running const-prop and dead-branch-elimination separately would miss it.
//
// LATTICE (per SSA temp): Top ⊒ Const(c) ⊒ Bot. Top = "not yet known" (optimistic),
// Const = one integer value on all reachable paths, Bot = "overdefined" (varies /
// unknown). SSA gives each temp a SINGLE definition, so a temp's lattice value is just
// the (monotone) evaluation of that one instruction — no cross-definition meet needed;
// only φ MEETS its reachable arms. Monotone descent (Top→Const→Bot, reachable grows) ⟹
// the round-robin fixpoint converges.
//
// THEOREM (CbC): `⟦f⟧ = ⟦sccp(f)⟧`. FAITHFUL by construction — the transfer function
// evaluates constants with the SAME `eval_bin/eval_cast/canon` interp uses, and DECLINES
// div/rem-by-0 (eval_bin→Err ⟹ Bot, keeping the instruction), matching interp's UB.
// Floats stay Bot (interp models f64; the backend uses hardware regs — as in const_fold).
// MEASURED by `equiv`, never trusted.
// ─────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
enum Lat {
    Top,
    Const(i64),
    Bot,
}

/// The meet (greatest lower bound) of two lattice points — used to combine a φ's
/// reachable arms and to lower a temp monotonically.
fn lat_meet(a: Lat, b: Lat) -> Lat {
    match (a, b) {
        (Lat::Top, x) | (x, Lat::Top) => x,
        (Lat::Const(x), Lat::Const(y)) if x == y => Lat::Const(x),
        _ => Lat::Bot,
    }
}

pub fn sccp(tt: &TyTab, f: &mut IrFunc) -> u32 {
    if !cfg_complete(f) {
        return 0; // CFG-reachability is unsound with computed goto — see cfg_complete
    }
    let nt = f.temps.len();
    let nb = f.blocks.len();
    // Float temps are never folded (rounding / hardware register); seed them Bot.
    let mut lat: Vec<Lat> =
        (0..nt).map(|t| if tt.is_float(f.temps[t]) { Lat::Bot } else { Lat::Top }).collect();
    let mut reach_b = vec![false; nb];
    let mut reach_e: HashSet<(BlockId, BlockId)> = HashSet::new();
    if nb > 0 {
        reach_b[0] = true;
    }

    // The value of an operand under the current lattice (an immediate is a constant;
    // a float immediate cannot be integer-folded → Bot).
    let vlat = |lat: &[Lat], v: &Val| -> Lat {
        match v {
            Val::Imm(x) => Lat::Const(*x),
            Val::Tmp(t) => lat[*t as usize],
            Val::FImm(_) => Lat::Bot,
        }
    };
    // The lattice value PRODUCED by one instruction (its transfer function). A φ meets
    // its arms whose predecessor edge is currently reachable; anything not integer-
    // foldable (Load/Call/Lea/exotic, a float result) is Bot.
    let transfer = |lat: &[Lat], reach_e: &HashSet<(BlockId, BlockId)>, b: BlockId, i: &Inst| -> Lat {
        match i {
            Inst::Copy(_, ty, a) => match vlat(lat, a) {
                Lat::Const(x) => Lat::Const(canon(tt, *ty, x)),
                o => o,
            },
            Inst::Bin(_, op, ty, a, bb) => match (vlat(lat, a), vlat(lat, bb)) {
                (Lat::Bot, _) | (_, Lat::Bot) => Lat::Bot,
                (Lat::Const(x), Lat::Const(y)) => match eval_bin(tt, *op, *ty, x, y) {
                    Ok(v) => Lat::Const(v),
                    Err(_) => Lat::Bot, // div/rem by 0 → keep the instruction (UB), do not fold
                },
                _ => Lat::Top, // an operand still unknown (optimistic)
            },
            Inst::Un(_, op, ty, a) => match vlat(lat, a) {
                Lat::Const(x) => {
                    let r = match op {
                        Un::Neg => canon(tt, *ty, x.wrapping_neg()),
                        Un::BNot => canon(tt, *ty, !x),
                    };
                    Lat::Const(r)
                }
                o => o,
            },
            Inst::Cast(_, from, to, a) => {
                if tt.is_float(*from) || tt.is_float(*to) {
                    Lat::Bot // i↔f folding deferred (like float arithmetic)
                } else {
                    match vlat(lat, a) {
                        Lat::Const(x) => Lat::Const(eval_cast(tt, *from, *to, x)),
                        o => o,
                    }
                }
            }
            Inst::Phi(_, ty, arms) => {
                let mut m = Lat::Top;
                for (pb, v) in arms {
                    if reach_e.contains(&(*pb, b)) {
                        let a = match vlat(lat, v) {
                            Lat::Const(x) => Lat::Const(canon(tt, *ty, x)),
                            o => o,
                        };
                        m = lat_meet(m, a);
                    }
                }
                m
            }
            _ => Lat::Bot, // Load/Call/Lea/Alloca/exotic define an unknown value
        }
    };

    loop {
        let mut changed = false;
        let mark = |reach_b: &mut Vec<bool>, reach_e: &mut HashSet<(BlockId, BlockId)>, from: BlockId, to: BlockId, ch: &mut bool| {
            if reach_e.insert((from, to)) {
                *ch = true;
            }
            if !reach_b[to as usize] {
                reach_b[to as usize] = true;
                *ch = true;
            }
        };
        for b in 0..nb {
            if !reach_b[b] {
                continue;
            }
            for i in &f.blocks[b].insts {
                if let Some(d) = inst_def(i) {
                    let nv = transfer(&lat, &reach_e, b as BlockId, i);
                    let m = lat_meet(lat[d as usize], nv);
                    if m != lat[d as usize] {
                        lat[d as usize] = m;
                        changed = true;
                    }
                }
            }
            match &f.blocks[b].term {
                Term::Jmp(t) => mark(&mut reach_b, &mut reach_e, b as BlockId, *t, &mut changed),
                Term::Br(c, t, e) => match vlat(&lat, c) {
                    Lat::Const(0) => mark(&mut reach_b, &mut reach_e, b as BlockId, *e, &mut changed),
                    Lat::Const(_) => mark(&mut reach_b, &mut reach_e, b as BlockId, *t, &mut changed),
                    Lat::Bot => {
                        mark(&mut reach_b, &mut reach_e, b as BlockId, *t, &mut changed);
                        mark(&mut reach_b, &mut reach_e, b as BlockId, *e, &mut changed);
                    }
                    Lat::Top => {} // condition still undetermined — no edge yet (optimistic)
                },
                _ => {}
            }
        }
        if !changed {
            break;
        }
    }

    // ── Apply. Every temp proven Const(c) has that value on all reachable paths
    // (SSA single-def + monotone lattice) → substitute Imm(c) for its uses. A branch
    // on a now-constant condition collapses to a Jmp (dead successor pruned; a later
    // DCE reclaims the unreachable block).
    let mut n = 0u32;
    let subst = |x: &mut Val| {
        if let Val::Tmp(t) = x {
            if let Lat::Const(c) = lat[*t as usize] {
                *x = Val::Imm(c);
            }
        }
    };
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            each_use_mut(i, |x| {
                let before = matches!(x, Val::Tmp(_));
                subst(x);
                if before && matches!(x, Val::Imm(_)) {
                    n += 1;
                }
            });
        }
        each_use_term_mut(&mut b.term, &subst);
        if let Term::Br(Val::Imm(c), t, e) = &b.term {
            b.term = Term::Jmp(if *c != 0 { *t } else { *e });
            n += 1;
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// GVN — Global Value Numbering (dominator-based, Simpson/Briggs). An SSA pass: the
// global generalization of block-local `cse`. Two PURE instructions with the same
// (op, type, operand value-numbers) compute the same value — and in SSA a temporary
// has ONE definition, so its value is invariant along any path. Thus a redundant
// computation whose value was already produced in a DOMINATING block can be replaced
// by a Copy of that earlier temp (the dominating def is available on every path here).
//
// SOUND ONLY on SSA form (non-SSA reassignment would make same-operand-temp ≠ same-value)
// — runs after `to_ssa`. Restricted to pure arithmetic (Bin/Un/Cast/Lea-Local); Loads
// stay with block-local `cse` (cross-block load reuse needs memory-availability analysis,
// deliberately omitted — the QBE "fraction of the complexity" ethos). Operand value-numbers
// are the SSA temp ids themselves (single-def); run `copy_prop` first to collapse copies.
//
// THEOREM (CbC): `⟦f⟧ = ⟦gvn(f)⟧` for f in SSA form, MEASURED by `equiv`.
// ─────────────────────────────────────────────────────────────────────────────

/// Blocks reachable from the entry (a forward DFS over successors).
fn reachable_blocks(f: &IrFunc) -> Vec<bool> {
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
fn dominators(f: &IrFunc) -> Vec<HashSet<BlockId>> {
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

pub fn gvn(f: &mut IrFunc) -> u32 {
    if !cfg_complete(f) {
        return 0; // dominance is unsound with computed goto — see cfg_complete
    }
    let dom = dominators(f);
    let reach = reachable_blocks(f);
    let order: Vec<BlockId> = rpo(f).into_iter().filter(|&b| reach[b as usize]).collect();
    // value-number key → (representative temp, its defining block).
    let mut table: HashMap<(u16, u32, (u8, i64), (u8, i64)), (Tmp, BlockId)> = HashMap::new();
    let mut n = 0u32;
    for b in order {
        for i in f.blocks[b as usize].insts.iter_mut() {
            let (key, d, ty) = match i {
                Inst::Bin(d, op, ty, a, bb) => (bin_key(*op, *ty, a, bb), *d, *ty),
                Inst::Un(d, op, ty, a) => ((100u16 + *op as u16, *ty, enc(a), (9, 0)), *d, *ty),
                Inst::Cast(d, from, to, a) => ((200u16, *to, enc(a), (9, *from as i64)), *d, *to),
                Inst::Lea(d, Place::Local(off)) => ((300u16, 0u32, (3, *off as i64), (9, 0)), *d, ULONG),
                _ => continue,
            };
            match table.get(&key) {
                // reuse only when the earlier def DOMINATES here (available on every path).
                Some(&(prev, db)) if dom[b as usize].contains(&db) => {
                    *i = Inst::Copy(d, ty, Val::Tmp(prev));
                    n += 1;
                }
                Some(_) => {} // same value but incomparable block → cannot safely reuse
                None => {
                    table.insert(key, (d, b));
                }
            }
        }
    }
    n
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
fn rename_phi_pred(b: &mut Block, from: BlockId, to: BlockId) {
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

fn remap_term(t: &mut Term, map: &[u32]) {
    match t {
        Term::Jmp(a) => *a = map[*a as usize],
        Term::Br(_, a, b) => {
            *a = map[*a as usize];
            *b = map[*b as usize];
        }
        Term::Ret(_) | Term::Unreachable => {}
    }
}

pub fn cfg_simplify(f: &mut IrFunc) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto: the CFG is incomplete → merge/reachability unsound
    }
    let mut changed = 0u32;
    // (1) straight-line merges to a fixpoint (recompute predecessors each step).
    loop {
        let preds = predecessors(f);
        let mut pair = None;
        for p in 0..f.blocks.len() as BlockId {
            if let Term::Jmp(s) = f.blocks[p as usize].term
                && s != 0 // never merge the entry away
                && s != p // not a self-loop
                && preds[s as usize].len() == 1
            {
                pair = Some((p, s)); // s's sole predecessor is p
                break;
            }
        }
        let (p, s) = match pair {
            Some(x) => x,
            None => break,
        };
        let mut succs = Vec::new();
        term_targets(&f.blocks[s as usize].term, &mut succs);
        for t in succs {
            if t != s {
                rename_phi_pred(&mut f.blocks[t as usize], s, p);
            }
        }
        let sb = std::mem::replace(
            &mut f.blocks[s as usize],
            Block { insts: Vec::new(), term: Term::Unreachable },
        );
        for inst in sb.insts {
            match inst {
                Inst::Phi(d, ty, arms) => {
                    let v = arms.iter().find(|(pp, _)| *pp == p).map(|(_, v)| *v).unwrap_or(Val::Imm(0));
                    f.blocks[p as usize].insts.push(Inst::Copy(d, ty, v));
                }
                other => f.blocks[p as usize].insts.push(other),
            }
        }
        f.blocks[p as usize].term = sb.term;
        changed += 1; // s is now an isolated Unreachable block; step (2) deletes it
    }
    // (2) unreachable-block elimination + renumber.
    let reach = reachable_blocks(f);
    if reach.iter().any(|&r| !r) {
        let mut map = vec![0u32; f.blocks.len()];
        let mut next = 0u32;
        for (b, &r) in reach.iter().enumerate() {
            if r {
                map[b] = next;
                next += 1;
            }
        }
        let old = std::mem::take(&mut f.blocks);
        for (b, mut blk) in old.into_iter().enumerate() {
            if !reach[b] {
                continue; // deleted (already counted as a merge, or genuinely dead)
            }
            remap_term(&mut blk.term, &map);
            for i in blk.insts.iter_mut() {
                if let Inst::Phi(_, _, arms) = i {
                    arms.retain(|(pp, _)| reach[*pp as usize]);
                    for (pp, _) in arms.iter_mut() {
                        *pp = map[*pp as usize];
                    }
                }
            }
            f.blocks.push(blk);
        }
        for (_, b) in f.labels.iter_mut() {
            *b = map[*b as usize];
        }
    }
    changed
}

// ─────────────────────────────────────────────────────────────────────────────
// THE SSA OPTIMIZATION PIPELINE (the QBE-level projection, under CbC). The whole
// point of Stages 1–4: build SSA, run the SSA-strength passes to a fixpoint, then
// return to executable (φ-free) IR and do a final non-SSA cleanup.
//
//   optimize_ssa = to_ssa ▸ (sccp ∘ const_fold ∘ copy_prop ∘ gvn ∘ cse ∘ dce)* ▸ out_of_ssa ▸ optimize
//
// Each stage is an INDIVIDUALLY-PROVEN semantics-preserving rewrite (⟦·⟧-invariant,
// gated by `equiv`); the COMPOSITE is therefore semantics-preserving, and this is
// re-checked end-to-end by `optimize_ssa_preserves` — composition of commuting squares
// is a commuting square, but we MEASURE it anyway (never trust by reasoning). This is
// the artifact Stage 5 wires into the backend behind an optimization flag.
// ─────────────────────────────────────────────────────────────────────────────
pub fn optimize_ssa(tt: &TyTab, f: &mut IrFunc) {
    to_ssa(tt, f);
    for _ in 0..32 {
        let mut n = 0;
        n += sccp(tt, f); // conditional constants + dead-branch pruning (through φ)
        n += const_fold(tt, f); // fold newly-constant operands
        n += copy_prop(f); // collapse copies so GVN's operand value-numbers are canonical
        n += gvn(f); // global redundant-expression elimination (dominator-based)
        n += cse(f); // block-local load reuse (GVN skips memory)
        n += dce(f); // reclaim the temps the above passes deadened (incl. φ)
        n += cfg_simplify(f); // collapse the straight lines / dead blocks SCCP exposed
        if n == 0 {
            break; // fixpoint
        }
    }
    out_of_ssa(f); // φ → edge copies (swap/critical-edge safe) → executable IR
    optimize(tt, f); // the proven non-SSA cleanup (folds the φ-destruction copies)
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

    // Stage 5b — the ABI budgets the backend uses (arm64_elf.rs): GP = 10 callee-saved
    // (x19–x28), NO caller-saved (the emitter's scratch spans x0–x15); FP = 16 caller
    // (v16–v31) ⊕ 8 callee (v8–v15).
    fn gp_budget() -> ClassBudget {
        ClassBudget { k: 10, ncaller: 0 }
    }
    fn fp_budget() -> ClassBudget {
        ClassBudget { k: 24, ncaller: 16 }
    }

    // ABI allocation VALIDATES over a corpus mixing calls (crossing temps) and floats:
    // both Stage-5b obligations (interference invariant + call-clobber) hold.
    #[test]
    fn abi_alloc_valid() {
        for (nm, src) in [
            ("a", "int g(int a,int b){int c=a+b;int d=c*a;int e=d-b;return c+d+e;}"),
            ("call", "int h(int);int f(int a){int x=a*a;int y=a+7;return h(a)+x-y;}"),
            ("flt", "double f(double a,double b){double c=a*b;double d=c+a;return d-b;}"),
            ("loop", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1)s=s+i*i;return s;}"),
        ] {
            let (ast, ir) = compile(nm, src);
            for f in &ir {
                let home = abi_alloc(&ast.tt, f, &gp_budget(), &fp_budget());
                verify_abi(&ast.tt, f, &home, &gp_budget(), &fp_budget())
                    .unwrap_or_else(|e| panic!("{nm}/{}: {e}", f.name));
            }
        }
    }

    // Phase A — conservative register COALESCING (biased coloring). Its correctness is
    // the SAME theorem as Stage 5b: the bias only chooses among already-free, already-legal
    // colors, so the coloring stays valid ⟹ rename-bisimulation ⟹ ⟦·⟧ preserved — proven
    // by `verify_abi` (which now runs WITH the bias in `abi_alloc_valid`). This test proves
    // the pass is NON-VACUOUS: a non-interfering `Copy` pair (the φ-destruction edge copies
    // out_of_ssa emits) actually ends up in the SAME register, so the copy lowers to a
    // self-move the peephole elides.
    #[test]
    fn coalesce_shares_register_for_moves() {
        // The fib swap loop: out_of_ssa emits edge copies for a,b,i — the coalescing targets.
        let (ast, ir) = compile(
            "coal",
            "int f(int n){int a=0,b=1,i=0;while(i<n){int t=a+b;a=b;b=t;i=i+1;}return a;}",
        );
        let mut f = ir[0].clone();
        to_ssa(&ast.tt, &mut f);
        out_of_ssa(&mut f); // emits the Copy insts coalescing bites on
        verify(&f).unwrap();
        let (gp, fp) = (gp_budget(), fp_budget());
        let home = abi_alloc(&ast.tt, &f, &gp, &fp);
        verify_abi(&ast.tt, &f, &home, &gp, &fp).expect("biased coloring must still be valid");
        let mut move_pairs = 0u32;
        let mut coalesced = 0u32;
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Copy(d, _, Val::Tmp(s)) = i {
                    move_pairs += 1;
                    if home[*d as usize].is_some() && home[*d as usize] == home[*s as usize] {
                        coalesced += 1; // shares a register ⟹ self-move ⟹ elidable
                    }
                }
            }
        }
        assert!(move_pairs >= 1, "out_of_ssa must have produced edge copies to coalesce");
        assert!(coalesced >= 1, "a non-interfering move pair MUST be coalesced ({coalesced}/{move_pairs})");
    }

    // TEETH for call-clobber: with ZERO callee-saved colors (k==ncaller), a value LIVE
    // ACROSS a call has no legal register → it MUST spill, never silently occupy a
    // caller-saved reg the `bl` would clobber. Proves the restriction actually bites.
    #[test]
    fn abi_alloc_no_clobber() {
        let gp = ClassBudget { k: 2, ncaller: 2 }; // all caller-saved, no callee-saved
        let fp = ClassBudget { k: 16, ncaller: 16 };
        let (ast, ir) = compile("x", "int h(int);int f(int a){int x=a*a;return h(a)+x;}");
        let f = ir.iter().find(|f| f.name == "f").unwrap();
        let home = abi_alloc(&ast.tt, f, &gp, &fp);
        verify_abi(&ast.tt, f, &home, &gp, &fp).expect("no-callee budget must still verify (via spills)");
        assert!(
            home.iter().any(|h| h.is_none()),
            "a call-crossing temp must SPILL when no callee-saved register exists"
        );
    }

    // asm ⟹ conservative all-spill: its operand pool (x9../v16..) can clobber any
    // allocatable register, so no register home is safe — regalloc disables per function.
    #[test]
    fn abi_alloc_asm_all_spill() {
        let (ast, ir) = compile(
            "s",
            "int f(int a){int b=0; __asm__(\"lsl %w0, %w1, #1\" : \"=r\"(b) : \"r\"(a)); return b;}",
        );
        let f = ir.iter().find(|f| f.name == "f").unwrap();
        let home = abi_alloc(&ast.tt, f, &gp_budget(), &fp_budget());
        assert!(
            home.iter().all(|h| h.is_none()),
            "a function containing inline asm must fall back to all-spill"
        );
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

    // ═════════════════════════════════════════════════════════════════════════
    // SSA construction — the Stage-2 theorem: ⟦f⟧ = ⟦to_ssa(f)⟧ (Braun 2013).
    // Same mechanical translation-validation as the pass squares above: to_ssa is
    // one arrow, and it must commute with the reference semantics over 𝔼_struct.
    // ═════════════════════════════════════════════════════════════════════════

    fn count_phis(ir: &[IrFunc]) -> usize {
        ir.iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Phi(..)))
            .count()
    }

    #[test]
    fn to_ssa_semantics_preserved() {
        // ∀ e ∈ 𝔼_struct: ⟦lower(e)⟧ = ⟦to_ssa(lower(e))⟧, and the result is
        // well-formed. Anti-vacuous: the loop/branch shapes MUST actually introduce
        // φ (a no-op to_ssa would trivially "commute" and prove nothing).
        let srcs = e_struct();
        let mut proven = 0u32;
        let mut with_phi = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("ssa", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
            }
            for f in &ssa {
                verify(f).unwrap_or_else(|e| panic!("verify to_ssa {src}: {e}"));
            }
            equiv(&ast.tt, &ir, &ssa, "f")
                .unwrap_or_else(|e| panic!("⟦f⟧ ≠ ⟦to_ssa(f)⟧ for {src}: {e}"));
            proven += 1;
            if count_phis(&ssa) > 0 {
                with_phi += 1;
            }
        }
        assert_eq!(proven, 312, "must prove to_ssa over the whole generated space");
        // The 36 loop shapes each reconcile a mutated accumulator + index at the
        // header ⟹ φ. (gen_ptr's locals are address-taken → stay in memory, no φ.)
        assert!(with_phi >= 36, "loop/branch shapes must introduce φ (anti-vacuous), got {with_phi}");
        eprintln!("to_ssa theorem: {proven} exprs proven ⟦f⟧=⟦to_ssa(f)⟧; {with_phi} introduced φ");
    }

    #[test]
    fn to_ssa_diamond_and_loop() {
        // Diamond: r written in both arms → one φ at the merge; the promoted value
        // is selected per the edge taken.
        let (ast, ir) = compile("ssad", "int f(int a){int r;if(a<10)r=100;else r=200;return r;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        for f in &ssa {
            verify(f).unwrap();
        }
        assert!(count_phis(&ssa) >= 1, "the diamond merge needs a φ");
        equiv(&ast.tt, &ir, &ssa, "f").expect("diamond: ⟦f⟧=⟦to_ssa(f)⟧");
        assert_eq!(interp(&ast.tt, &ssa, "f", &[5]).unwrap(), 100);
        assert_eq!(interp(&ast.tt, &ssa, "f", &[50]).unwrap(), 200);

        // Loop: an accumulator mutated across the back-edge → φ at the header.
        // mem2reg removes the promoted scalars' Load/Store (only the param `n`
        // — kept in memory by design — still Loads).
        let (a2, ir2) =
            compile("ssal", "int f(int n){int s=0;int i;for(i=1;i<=n;i=i+1)s=s+i;return s;}");
        let mut ssa2 = ir2.clone();
        for f in ssa2.iter_mut() {
            to_ssa(&a2.tt, f);
        }
        for f in &ssa2 {
            verify(f).unwrap();
        }
        assert!(count_phis(&ssa2) >= 1, "the loop header needs a φ");
        assert!(count_loads(&ssa2) < count_loads(&ir2), "promotion must remove Loads");
        equiv(&a2.tt, &ir2, &ssa2, "f").expect("loop: ⟦f⟧=⟦to_ssa(f)⟧");
        assert_eq!(interp(&a2.tt, &ssa2, "f", &[5]).unwrap(), 15);
        assert_eq!(interp(&a2.tt, &ssa2, "f", &[10]).unwrap(), 55);
    }

    #[test]
    fn to_ssa_respects_address_taken() {
        // &x escapes ⟹ x is NOT promoted (stays in memory); to_ssa still commutes.
        let (ast, ir) = compile("ssaesc", "int f(int a){int x=a;int*p=&x;*p=*p+5;return x;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        for f in &ssa {
            verify(f).unwrap();
        }
        equiv(&ast.tt, &ir, &ssa, "f").expect("address-taken: ⟦f⟧=⟦to_ssa(f)⟧");
        assert_eq!(interp(&ast.tt, &ssa, "f", &[7]).unwrap(), 12);
    }

    // Self-proof (clean-input law): the to_ssa gate must have TEETH on φ. Corrupt a
    // φ's predecessor→value mapping (swap the two arms' BlockIds) so the wrong value
    // is selected on each edge; equiv MUST catch it. If this is green, every to_ssa
    // "pass" verdict above is worthless.
    #[test]
    fn to_ssa_gate_has_teeth() {
        let (ast, ir) = compile("ssateeth", "int f(int a){int r;if(a<10)r=100;else r=200;return r;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        equiv(&ast.tt, &ir, &ssa, "f").expect("correct to_ssa must commute");
        let mut bad = ssa.clone();
        let mut mutated = false;
        'o: for f in bad.iter_mut() {
            for b in f.blocks.iter_mut() {
                for i in b.insts.iter_mut() {
                    if let Inst::Phi(_, _, arms) = i {
                        if arms.len() == 2 && !val_eq(arms[0].1, arms[1].1) {
                            let b0 = arms[0].0; // swap which predecessor feeds which value
                            arms[0].0 = arms[1].0;
                            arms[1].0 = b0;
                            mutated = true;
                            break 'o;
                        }
                    }
                }
            }
        }
        assert!(mutated, "a 2-arm φ with distinct values must exist to mutate");
        assert!(
            equiv(&ast.tt, &ir, &bad, "f").is_err(),
            "a mis-wired φ (swapped predecessor edges) MUST be caught (else the gate is blind)"
        );
    }

    // Float(size 4) promotion regression (C99 6.3.1.5 / FLT_EVAL_METHOD=0). A `float`
    // local's Store narrows f64→f32 and its Load widens back, so Store∘Load = round-to-f32,
    // NOT identity. A naive mem2reg (promoted Load → plain Copy) DROPS that rounding, leaving
    // illegal f64 precision — ⟦f⟧≠⟦to_ssa(f)⟧. The promoted Load must be a self-narrowing Cast.
    #[test]
    fn to_ssa_narrows_promoted_float() {
        // x = 123456.78f * 9.87f : a float×float whose f64 product is NOT f32-representable,
        // so the store-narrow is observable. x is a scalar non-address-taken local ⟹ promoted.
        let (ast, ir) = compile("fnar", "float f(void){ float x = 123456.78f * 9.87f; return x; }");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        for f in &ssa {
            verify(f).unwrap();
        }
        // TEETH #1 (structural): the promoted float(size 4) Load must appear as a
        // self-narrowing Cast (from == to == float, size 4), never a plain Copy.
        let has_narrow_cast = ssa.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).any(
            |i| matches!(i, Inst::Cast(_, from, to, _) if from == to && ast.tt.is_float(*from) && ast.tt.size(*from) == 4),
        );
        assert!(has_narrow_cast, "a promoted float(size 4) Load must narrow via a self-Cast");
        // ⟦f⟧ = ⟦to_ssa(f)⟧ (interp narrows at the Store; the promoted form at the Cast).
        equiv(&ast.tt, &ir, &ssa, "f").expect("float promotion must commute");
        // TEETH #2 (value): the result is the f32-narrowed product, NOT the raw f64 one —
        // a plain-Copy promotion would return the second, distinct, bit pattern.
        let (a, b) = (123456.78f32 as f64, 9.87f32 as f64);
        let got = interp(&ast.tt, &ssa, "f", &[]).unwrap();
        assert_eq!(got, ((a * b) as f32 as f64).to_bits() as i64, "must be f32-narrowed");
        assert_ne!(got, (a * b).to_bits() as i64, "must NOT keep the raw f64 product");
    }

    // Bitfield truncation through const-fold (C99 6.7.2.1). GCC torture 921016-1:
    // signed m:11, `(l.m = 1081)` wraps to −967 (1081 > 2^10−1), so `== 1081` is FALSE.
    // to_ssa promotes j to the constant 1081, exposing the bitfield Cast to const-fold;
    // `canon` must truncate to the DECLARED width (11), not the container's 32 bits.
    #[test]
    fn sccp_truncates_signed_bitfield() {
        let (ast, ir) = compile(
            "bf11",
            "int f(void){ struct { signed int m : 11; } l; int j = 1081; return (l.m = j) == j; }",
        );
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            optimize_ssa(&ast.tt, f);
        }
        for f in &ssa {
            verify(f).unwrap();
        }
        equiv(&ast.tt, &ir, &ssa, "f").expect("bitfield fold must commute");
        // 1081 truncated to signed:11 = −967 ≠ 1081 ⟹ 0. A 32-bit-wide canon would keep
        // 1081 == 1081 ⟹ 1 (the miscompile).
        assert_eq!(interp(&ast.tt, &ssa, "f", &[]).unwrap(), 0, "signed:11 wraps 1081→−967");
    }

    // Undefined-variable read must yield WELL-FORMED IR (not a predecessor-less φ). GCC
    // torture pr43629: `int x; if(flag) x=-1; else x&=0xff;` reads x uninitialized on the
    // else path — UB (C99 6.3.2.1p2, address-not-taken). Regression: read_var built a φ in
    // the entry block ⟹ a use with no def ⟹ `verify` rejected it and the compiler panicked.
    #[test]
    fn to_ssa_undefined_read_is_wellformed() {
        let (ast, ir) =
            compile("undef", "int f(int flag){ int x; if(flag) x=-1; else x&=0xff; return x & ~0xff; }");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        // TEETH: the crux — this `verify` panicked before the fix (broken SSA).
        for f in &ssa {
            verify(f).expect("to_ssa on an undefined read must produce well-formed IR");
        }
        // The read-before-write x resolves to a deterministic 0 on the else path ⟹
        // 0 & ~0xff = 0. (UB ⟹ any value legal; 0 is the well-formed choice.)
        assert_eq!(interp(&ast.tt, &ssa, "f", &[0]).unwrap(), 0);
    }

    // CFG-completeness guard: mem2reg/GVN/SCCP must bail on a computed goto (its edges to
    // address-taken labels are unmodeled ⟹ incomplete CFG). GCC torture 920501-3 / 920302-1:
    // a loop closed by `goto *p` looks acyclic, so a loop-carried local would be promoted
    // with no φ → miscompile / SIGSEGV. Teeth: the local stays in MEMORY (unpromoted).
    #[test]
    fn to_ssa_bails_on_computed_goto() {
        let (ast, ir) =
            compile("cgoto", "int f(int n){ int x=0; void*t=&&L; L: x++; if(x<n) goto *t; return x; }");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        for f in &ssa {
            verify(f).unwrap();
        }
        let gotoptr = ssa.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).any(|i| matches!(i, Inst::GotoPtr(..)));
        assert!(gotoptr, "test must actually contain a computed goto (guard precondition)");
        // Bailed ⟹ identity: the Stores stay (x/t in memory), no promotion happened.
        let stores = ssa.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Store(..))).count();
        assert!(stores >= 1, "a computed-goto function must be left in memory (unpromoted)");
    }

    // ═════════════════════════════════════════════════════════════════════════
    // STAGE 3 — out_of_ssa / φ-destruction. THEOREM ⟦to_ssa(f)⟧ = ⟦out_of_ssa(to_ssa(f))⟧.
    // ═════════════════════════════════════════════════════════════════════════

    fn roundtrip(tt: &TyTab, f: &IrFunc) -> IrFunc {
        let mut g = f.clone();
        to_ssa(tt, &mut g);
        out_of_ssa(&mut g);
        g
    }

    #[test]
    fn out_of_ssa_semantics_preserved() {
        // ∀ e ∈ 𝔼_struct: ⟦to_ssa(lower e)⟧ = ⟦out_of_ssa(to_ssa(lower e))⟧, and the
        // result is well-formed AND φ-free (the backend can consume it). Anti-vacuous:
        // the same ≥36 loop/branch shapes that grew φ in Stage 2 must round-trip.
        let srcs = e_struct();
        let mut proven = 0u32;
        let mut had_phi = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("oos", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
            }
            let phis_before = count_phis(&ssa);
            let mut back = ssa.clone();
            for f in back.iter_mut() {
                out_of_ssa(f);
            }
            for f in &back {
                verify(f).unwrap_or_else(|e| panic!("verify out_of_ssa {src}: {e}"));
            }
            assert_eq!(count_phis(&back), 0, "out_of_ssa must remove every φ for {src}");
            // The theorem: SSA form ≡ destructed form (the round trip preserves ⟦·⟧).
            equiv(&ast.tt, &ssa, &back, "f")
                .unwrap_or_else(|e| panic!("⟦to_ssa(f)⟧ ≠ ⟦out_of_ssa(to_ssa(f))⟧ for {src}: {e}"));
            // And end-to-end vs the original (transitivity, guarding against a shared bug).
            equiv(&ast.tt, &ir, &back, "f")
                .unwrap_or_else(|e| panic!("⟦f⟧ ≠ ⟦out_of_ssa(to_ssa(f))⟧ for {src}: {e}"));
            proven += 1;
            if phis_before > 0 {
                had_phi += 1;
            }
        }
        assert_eq!(proven, 312, "must prove out_of_ssa over the whole generated space");
        assert!(had_phi >= 36, "loop/branch shapes must exercise φ-destruction (anti-vacuous), got {had_phi}");
        eprintln!("out_of_ssa theorem: {proven} exprs proven ⟦to_ssa(f)⟧=⟦out_of_ssa(to_ssa(f))⟧; {had_phi} had φ");
    }

    #[test]
    fn out_of_ssa_diamond_and_loop() {
        // Diamond: φ at the merge → one copy on each arm's edge.
        let (ast, ir) = compile("oosd", "int f(int a){int r;if(a<10)r=100;else r=200;return r;}");
        let back = roundtrip(&ast.tt, &ir[0]);
        verify(&back).unwrap();
        assert_eq!(count_phis(std::slice::from_ref(&back)), 0);
        let bk = vec![back];
        equiv(&ast.tt, &ir, &bk, "f").expect("diamond round trip");
        assert_eq!(interp(&ast.tt, &bk, "f", &[5]).unwrap(), 100);
        assert_eq!(interp(&ast.tt, &bk, "f", &[50]).unwrap(), 200);

        // Loop: the header φ (accumulator + index) → copies on the entry edge and the
        // back-edge; the sum must still be correct.
        let (a2, ir2) =
            compile("oosl", "int f(int n){int s=0;int i;for(i=1;i<=n;i=i+1)s=s+i;return s;}");
        let back2 = vec![roundtrip(&a2.tt, &ir2[0])];
        verify(&back2[0]).unwrap();
        assert_eq!(count_phis(&back2), 0);
        equiv(&a2.tt, &ir2, &back2, "f").expect("loop round trip");
        assert_eq!(interp(&a2.tt, &back2, "f", &[5]).unwrap(), 15);
        assert_eq!(interp(&a2.tt, &back2, "f", &[10]).unwrap(), 55);
    }

    // The swap / lost-copy trap (csmith bait): two variables PERMUTED across a loop
    // back-edge produce mutually-referential φ (a←…,b←… where a's back-arm is b and
    // b's back-arm is a). Naive sequential copies corrupt the swap; seq_pcopy must
    // break the cycle. If out_of_ssa were wrong here, the fib-style values would diverge.
    #[test]
    fn out_of_ssa_swap() {
        let (ast, ir) =
            compile("oosw", "int f(int n){int a=0,b=1,i=0;while(i<n){int t=a+b;a=b;b=t;i=i+1;}return a;}");
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        // The header reconciles a, b, i via φ — the a/b permutation is the swap.
        assert!(count_phis(std::slice::from_ref(&ssa)) >= 2, "the fib loop header needs ≥2 φ");
        let mut back = ssa.clone();
        out_of_ssa(&mut back);
        verify(&back).unwrap();
        assert_eq!(count_phis(std::slice::from_ref(&back)), 0);
        let (ssa_v, back_v) = (vec![ssa], vec![back]);
        equiv(&ast.tt, &ssa_v, &back_v, "f").expect("swap: ⟦to_ssa(f)⟧=⟦out_of_ssa(·)⟧");
        // Concrete Fibonacci check (n → F(n)): 10 → 55, guards against a silent swap bug.
        for (n, fib) in [(0, 0), (1, 1), (2, 1), (5, 5), (10, 55)] {
            assert_eq!(interp(&ast.tt, &back_v, "f", &[n]).unwrap(), fib, "F({n})");
        }
    }

    // Critical edge: a Br whose taken side lands on a multi-pred φ-block. Copies must
    // NOT be appended to the shared predecessor (they would leak onto its other edge)
    // — the edge is split. Here `m` is assigned only under the if, so the merge φ has a
    // critical edge from the condition block.
    #[test]
    fn out_of_ssa_critical_edge() {
        let (ast, ir) =
            compile("oosc", "int f(int a,int b){int m=a;if(a<b)m=b;m=m+1;return m;}");
        let ssa = {
            let mut g = ir[0].clone();
            to_ssa(&ast.tt, &mut g);
            g
        };
        let blocks_before = ssa.blocks.len();
        let mut back = ssa.clone();
        out_of_ssa(&mut back);
        verify(&back).unwrap();
        assert_eq!(count_phis(std::slice::from_ref(&back)), 0);
        // A split introduces a new block on the critical edge (evidence the trap fired).
        assert!(back.blocks.len() >= blocks_before, "edge split may add a block");
        let (ssa_v, back_v) = (vec![ssa], vec![back]);
        equiv(&ast.tt, &ssa_v, &back_v, "f").expect("critical edge round trip");
        assert_eq!(interp(&ast.tt, &back_v, "f", &[3, 7]).unwrap(), 8); // max(3,7)+1
        assert_eq!(interp(&ast.tt, &back_v, "f", &[9, 2]).unwrap(), 10); // max(9,2)+1
    }

    // Self-proof (clean-input law): the out_of_ssa gate must have TEETH. A parallel
    // copy sequentialized WRONG (dropping the cycle break) corrupts a swap; equiv must
    // catch such a divergence. We test seq_pcopy directly against a reference.
    #[test]
    fn seq_pcopy_swap_is_faithful() {
        // Parallel {t0←t1, t1←t0} over an initial register file must swap.
        let pc = vec![(0u32, INT, Val::Tmp(1)), (1u32, INT, Val::Tmp(0))];
        let mut next = 2u32;
        let seq = seq_pcopy(&pc, &mut |_ty| {
            let t = next;
            next += 1;
            t
        });
        assert!(next > 2, "a 2-cycle swap MUST allocate a fresh temp (else it corrupts)");
        // Execute the sequential copies on a register file and confirm the swap.
        let mut reg = vec![0i64; next as usize];
        reg[0] = 10;
        reg[1] = 20;
        for (d, _, s) in &seq {
            reg[*d as usize] = match s {
                Val::Tmp(t) => reg[*t as usize],
                Val::Imm(x) => *x,
                Val::FImm(b) => *b as i64,
            };
        }
        assert_eq!((reg[0], reg[1]), (20, 10), "seq_pcopy must realize the parallel swap");
    }

    // ═════════════════════════════════════════════════════════════════════════
    // STAGE 4 — SCCP (Wegman–Zadeck). THEOREM ⟦to_ssa(f)⟧ = ⟦sccp(to_ssa(f))⟧.
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn sccp_semantics_preserved() {
        // ∀ e ∈ 𝔼_struct: SCCP on SSA form commutes with ⟦·⟧, and is well-formed.
        // Anti-vacuous: SCCP must actually fold on a healthy fraction (the constant
        // sub-expression `3 o 4` in gen_rich, dead-branch prunes, …).
        let srcs = e_struct();
        let mut proven = 0u32;
        let mut folded = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("sccp", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
            }
            let mut opt = ssa.clone();
            let mut changes = 0u32;
            for f in opt.iter_mut() {
                changes += sccp(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify sccp {src}: {e}"));
            }
            equiv(&ast.tt, &ssa, &opt, "f")
                .unwrap_or_else(|e| panic!("⟦to_ssa(f)⟧ ≠ ⟦sccp(to_ssa(f))⟧ for {src}: {e}"));
            proven += 1;
            if changes > 0 {
                folded += 1;
            }
        }
        assert_eq!(proven, 312, "must prove sccp over the whole generated space");
        assert!(folded >= 36, "SCCP must actually fold on a healthy fraction (anti-vacuous), got {folded}");
        eprintln!("sccp theorem: {proven} exprs proven ⟦to_ssa(f)⟧=⟦sccp(·)⟧; {folded} folded");
    }

    // The SCCP-SPECIFIC win over const_fold+copy_prop: a branch on a proven constant
    // makes one arm UNREACHABLE, so a φ that merges (reachable-const, dead) collapses to
    // the constant. Plain const-fold cannot do this (it has no reachability). Here `b`
    // is constant 1 ⟹ the else is dead ⟹ c is provably 10 ⟹ the whole function is const.
    #[test]
    fn sccp_folds_through_reachability() {
        let (ast, ir) = compile("sccpr", "int f(int a){int b=1;int c;if(b)c=10;else c=a;return c;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        let mut opt = ssa.clone();
        for f in opt.iter_mut() {
            sccp(&ast.tt, f);
        }
        for f in &opt {
            verify(f).unwrap();
        }
        equiv(&ast.tt, &ssa, &opt, "f").expect("reachability fold: ⟦to_ssa(f)⟧=⟦sccp(·)⟧");
        // The dead-branch condition collapsed → no Br survives in f (the else is pruned).
        let ff = opt.iter().find(|f| f.name == "f").unwrap();
        assert!(
            ff.blocks.iter().all(|b| !matches!(b.term, Term::Br(..))),
            "SCCP must resolve the constant branch to a Jmp"
        );
        // c is provably 10 regardless of a.
        for a in [-3, 0, 7, 1000] {
            assert_eq!(interp(&ast.tt, &opt, "f", &[a]).unwrap(), 10, "f({a}) must fold to 10");
        }
    }

    // Self-proof (clean-input law): the gate must catch a WRONG SCCP result. Corrupt one
    // constant that SCCP produced; equiv must reject. If green, every sccp verdict above
    // is worthless.
    #[test]
    fn sccp_gate_has_teeth() {
        let (ast, ir) = compile("sccpt", "int f(int a){int b=2*3+4;return a+b;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        let mut opt = ssa.clone();
        for f in opt.iter_mut() {
            sccp(&ast.tt, f);
            dce(f); // strip the now-dead pre-fold arithmetic so the surviving Imm is LIVE
        }
        equiv(&ast.tt, &ssa, &opt, "f").expect("correct sccp must commute");
        // Corrupt a folded immediate (b was 10) and confirm the gate bites.
        let mut bad = opt.clone();
        let mut mutated = false;
        'o: for f in bad.iter_mut().filter(|f| f.name == "f") {
            for blk in f.blocks.iter_mut() {
                each_use_term_mut(&mut blk.term, |x| {
                    if let Val::Imm(c) = x {
                        *x = Val::Imm(*c + 1);
                        mutated = true;
                    }
                });
                for i in blk.insts.iter_mut() {
                    each_use_mut(i, |x| {
                        if let Val::Imm(c) = x {
                            *x = Val::Imm(*c + 1);
                            mutated = true;
                        }
                    });
                    if mutated {
                        break 'o;
                    }
                }
                if mutated {
                    break 'o;
                }
            }
        }
        assert!(mutated, "SCCP must have produced a constant to corrupt");
        assert!(equiv(&ast.tt, &ssa, &bad, "f").is_err(), "a corrupted SCCP constant MUST be caught");
    }

    // ═════════════════════════════════════════════════════════════════════════
    // GVN — global value numbering (dominator-based). THEOREM ⟦to_ssa(f)⟧=⟦gvn(·)⟧.
    // ═════════════════════════════════════════════════════════════════════════

    fn count_copies(ir: &[IrFunc]) -> usize {
        ir.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Copy(..))).count()
    }

    #[test]
    fn gvn_semantics_preserved() {
        // ∀ e ∈ 𝔼_struct: GVN on SSA form commutes with ⟦·⟧, well-formed. copy_prop
        // first (so operand value-numbers are canonical). Anti-vacuous: GVN must fire.
        let srcs = e_struct();
        let mut proven = 0u32;
        let mut fired = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("gvn", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
                copy_prop(f);
            }
            let mut opt = ssa.clone();
            let mut changes = 0u32;
            for f in opt.iter_mut() {
                changes += gvn(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify gvn {src}: {e}"));
            }
            equiv(&ast.tt, &ssa, &opt, "f")
                .unwrap_or_else(|e| panic!("⟦to_ssa(f)⟧ ≠ ⟦gvn(·)⟧ for {src}: {e}"));
            proven += 1;
            if changes > 0 {
                fired += 1;
            }
        }
        assert_eq!(proven, 312, "must prove gvn over the whole generated space");
        assert!(fired >= 36, "GVN must actually eliminate redundancy (anti-vacuous), got {fired}");
        eprintln!("gvn theorem: {proven} exprs proven ⟦to_ssa(f)⟧=⟦gvn(·)⟧; {fired} fired");
    }

    // The GVN-SPECIFIC win over block-local cse: a redundant computation in a block
    // DOMINATED by an earlier one is eliminated ACROSS the block boundary. Here `s*s` is
    // computed at entry (u) and again in the then-block; entry dominates then ⟹ GVN
    // replaces the second with a copy of u. (`s` is a promoted SSA temp — one value.)
    #[test]
    fn gvn_eliminates_across_dominating_block() {
        let (ast, ir) =
            compile("gvnd", "int f(int a,int b){int s=a+b;int u=s*s;if(a>b){return s*s+u;}return u;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
            copy_prop(f);
        }
        let muls_before: usize = ssa
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Bin(_, Op::Mul, ..)))
            .count();
        let mut opt = ssa.clone();
        let mut n = 0u32;
        for f in opt.iter_mut() {
            n += gvn(f);
        }
        for f in &opt {
            verify(f).unwrap();
        }
        assert!(n > 0, "GVN must fire on the cross-block redundant s*s");
        let muls_after: usize = opt
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Bin(_, Op::Mul, ..)))
            .count();
        assert!(muls_after < muls_before, "the redundant multiply must be gone ({muls_before}→{muls_after})");
        assert!(count_copies(&opt) > count_copies(&ssa), "a redundant op becomes a Copy");
        equiv(&ast.tt, &ssa, &opt, "f").expect("cross-block GVN: ⟦to_ssa(f)⟧=⟦gvn(·)⟧");
        // s=a+b, u=s*s; a>b → (s*s)+u = 2u; else u. Concrete:
        assert_eq!(interp(&ast.tt, &opt, "f", &[3, 1]).unwrap(), 32); // s=4,u=16,a>b→32
        assert_eq!(interp(&ast.tt, &opt, "f", &[1, 3]).unwrap(), 16); // s=4,u=16,a≤b→16
    }

    // Soundness guard for the DOMINANCE condition: a value computed in a block that does
    // NOT dominate the use must NOT be reused. Two branches each compute s*s; neither
    // dominates the other, so GVN must leave both (a wrong GVN would merge them and, via
    // equiv over both edges, diverge). This proves the dominance test carries weight.
    #[test]
    fn gvn_respects_dominance() {
        let (ast, ir) =
            compile("gvndom", "int f(int a,int b){int s=a+b;int t;if(a>0)t=s*s;else t=s*s+1;return t;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
            copy_prop(f);
        }
        let mut opt = ssa.clone();
        for f in opt.iter_mut() {
            gvn(f);
        }
        for f in &opt {
            verify(f).unwrap();
        }
        // Both branch multiplies survive (incomparable blocks) — equiv confirms no
        // unsound merge slipped through regardless.
        equiv(&ast.tt, &ssa, &opt, "f").expect("dominance-respecting GVN: ⟦to_ssa(f)⟧=⟦gvn(·)⟧");
        assert_eq!(interp(&ast.tt, &opt, "f", &[2, 1]).unwrap(), 9); // a>0: s=3 → 9
        assert_eq!(interp(&ast.tt, &opt, "f", &[-1, 2]).unwrap(), 2); // a≤0: s=1 → 1+1=2
    }

    // ═════════════════════════════════════════════════════════════════════════
    // CFG-SIMPLIFICATION — Phase A. THEOREM ⟦f⟧ = ⟦cfg_simplify(f)⟧ (a structural
    // rewrite of the CFG that preserves the executed instruction sequence). Proven
    // over 𝔼_struct in the SSA context (where merges/dead blocks actually arise), and
    // on the SCCP-then-simplify path where it does real work.
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn cfg_simplify_semantics_preserved() {
        // ∀ e ∈ 𝔼_struct: to_ssa ▸ cfg_simplify commutes with ⟦·⟧, stays well-formed,
        // and never introduces a φ. Anti-vacuous: the branch/loop families give blocks
        // to merge/prune.
        let srcs = e_struct();
        let mut proven = 0u32;
        let mut shrank = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("cfgs", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
            }
            let blocks_before: usize = ssa.iter().map(|f| f.blocks.len()).sum();
            let mut opt = ssa.clone();
            for f in opt.iter_mut() {
                cfg_simplify(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify cfg_simplify {src}: {e}"));
            }
            equiv(&ast.tt, &ssa, &opt, "f")
                .unwrap_or_else(|e| panic!("⟦to_ssa(f)⟧ ≠ ⟦cfg_simplify(·)⟧ for {src}: {e}"));
            let blocks_after: usize = opt.iter().map(|f| f.blocks.len()).sum();
            if blocks_after < blocks_before {
                shrank += 1;
            }
            proven += 1;
        }
        assert_eq!(proven, 312, "must prove cfg_simplify over the whole generated space");
        assert!(shrank >= 36, "the branch/loop shapes must yield mergeable blocks (anti-vacuous), got {shrank}");
        eprintln!("cfg_simplify theorem: {proven} exprs proven ⟦to_ssa(f)⟧=⟦cfg_simplify(·)⟧; {shrank} shrank");
    }

    // TEETH: SCCP folds a constant branch to Jmp, orphaning the not-taken block; then
    // cfg_simplify must PRUNE it and MERGE the straight line — and the result must still
    // compute the same value. If cfg_simplify silently did nothing, `shrank` above and
    // this block-count drop would both be zero.
    #[test]
    fn cfg_simplify_prunes_dead_branch() {
        // `c` is a runtime-opaque local the frontend does NOT fold, but SCCP promotes it
        // to Const(1) and rewrites the branch → Jmp, orphaning the else block.
        let (ast, ir) =
            compile("cfgd", "int f(int a){int c=1;int r;if(c)r=a+1;else r=a-1;return r+r;}");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
            sccp(&ast.tt, f); // fold the constant branch → Jmp, orphaning the else block
        }
        let blocks_before: usize = ssa.iter().map(|f| f.blocks.len()).sum();
        let mut opt = ssa.clone();
        let mut n = 0u32;
        for f in opt.iter_mut() {
            n += cfg_simplify(f);
        }
        for f in &opt {
            verify(f).unwrap();
        }
        assert!(n > 0, "cfg_simplify must fire after the constant branch is folded");
        let blocks_after: usize = opt.iter().map(|f| f.blocks.len()).sum();
        assert!(blocks_after < blocks_before, "dead block + straight line collapse ({blocks_before}→{blocks_after})");
        equiv(&ast.tt, &ssa, &opt, "f").expect("dead-branch prune: ⟦sccp∘to_ssa(f)⟧=⟦cfg_simplify(·)⟧");
        assert_eq!(interp(&ast.tt, &opt, "f", &[10]).unwrap(), 22); // (10+1)*2
    }

    // Self-proof (clean-input law): the gate has TEETH. Corrupt a merged instruction's
    // value and equiv MUST catch it — proving the battery actually exercises the
    // spliced code, not a vacuous pass.
    #[test]
    fn cfg_simplify_gate_has_teeth() {
        let (ast, ir) =
            compile("cfgt", "int f(int a){int s=0;int i;for(i=0;i<a;i=i+1)s=s+i;return s;}");
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        let mut opt = ssa.clone();
        cfg_simplify(&mut opt);
        verify(&opt).unwrap();
        let base = vec![ssa];
        equiv(&ast.tt, &base, &vec![opt.clone()], "f").expect("identity: cfg_simplify preserves ⟦·⟧");
        // Mutate one Add→Sub in the simplified function; equiv must diverge.
        let mut bad = opt.clone();
        let mut mutated = false;
        'o: for b in bad.blocks.iter_mut() {
            for i in b.insts.iter_mut() {
                if let Inst::Bin(_, op @ Op::Add, ..) = i {
                    *op = Op::Sub;
                    mutated = true;
                    break 'o;
                }
            }
        }
        assert!(mutated, "there must be an Add to mutate in the merged body");
        assert!(
            equiv(&ast.tt, &base, &vec![bad], "f").is_err(),
            "an Add→Sub mutation in the merged code MUST be caught (else the gate is blind)"
        );
    }

    // ═════════════════════════════════════════════════════════════════════════
    // THE SSA PIPELINE — the composite QBE-level optimizer. THEOREM ⟦f⟧=⟦optimize_ssa(f)⟧.
    // ═════════════════════════════════════════════════════════════════════════

    #[test]
    fn optimize_ssa_preserves() {
        // ∀ e ∈ 𝔼_struct: the WHOLE pipeline commutes with ⟦·⟧, is well-formed, and the
        // result is φ-free (backend-consumable). This is the end-to-end gate that Stage 5
        // wiring relies on. Also cross-checked against the plain interp value.
        let srcs = e_struct();
        let mut proven = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("pipe", src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                optimize_ssa(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify optimize_ssa {src}: {e}"));
            }
            assert_eq!(count_phis(&opt), 0, "optimize_ssa must return φ-free IR for {src}");
            equiv(&ast.tt, &ir, &opt, "f")
                .unwrap_or_else(|e| panic!("⟦f⟧ ≠ ⟦optimize_ssa(f)⟧ for {src}: {e}"));
            proven += 1;
        }
        assert_eq!(proven, 312, "must prove the SSA pipeline over the whole generated space");
        eprintln!("optimize_ssa theorem: {proven} exprs proven ⟦f⟧=⟦optimize_ssa(f)⟧, φ-free");
    }

    #[test]
    fn optimize_ssa_preserves_corpus_and_reduces() {
        // Real programs (loop, cond, pointer, struct-ish, recursion): value-correct AND
        // the pipeline actually shrinks the code (non-vacuous). Compared vs the plain
        // interp result for a hard sanity check.
        let cases: &[(&str, &str, &[i64], i64)] = &[
            ("arith", "int f(int a,int b){int x=a+b;int y=a+b;return x*y+x*y;}", &[3, 4], 98),
            ("loop", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1)s=s+i*i;return s;}", &[5], 30),
            ("cond", "int f(int a){int t=1;int r;if(t)r=a*a;else r=0;return r+1;}", &[6], 37),
            ("fib", "int f(int n){int a=0,b=1,i=0;while(i<n){int t=a+b;a=b;b=t;i=i+1;}return a;}", &[10], 55),
            ("rec", "int f(int n){if(n<=1)return 1;return n*f(n-1);}", &[5], 120),
            ("cfold", "int f(int a){int k=2*3+4;int m=k*2;return a+m;}", &[7], 27),
        ];
        for &(nm, src, args, want) in cases {
            let (ast, ir) = compile(nm, src);
            let before = count_insts(&ir);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                optimize_ssa(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {nm}: {e}"));
            }
            assert_eq!(count_phis(&opt), 0, "{nm}: φ-free");
            equiv(&ast.tt, &ir, &opt, "f").unwrap_or_else(|e| panic!("{nm}: {e}"));
            assert_eq!(interp(&ast.tt, &opt, "f", args).unwrap(), want, "{nm}: wrong value");
            assert_eq!(interp(&ast.tt, &ir, "f", args).unwrap(), want, "{nm}: baseline wrong (test bug)");
            // At least one case must demonstrably shrink (the arith/cfold ones do).
            if matches!(nm, "arith" | "cfold") {
                assert!(count_insts(&opt) < before, "{nm}: pipeline must reduce ({before}→{})", count_insts(&opt));
            }
        }
    }
}
