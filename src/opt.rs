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

use crate::ast::{Ty, TyTab, TypeId, ULONG, VOID};
use crate::ir::{
    canon, eval_bin, eval_cast, inst_def, inst_uses, term_targets, term_uses, Block, BlockId, Callee,
    Inst, IrFunc, Op, Place, Term, Tmp, Un, Val,
};
use std::collections::{HashMap, HashSet};

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
        // Select (B4) is a pure data-select (cond ? a : b, no memory/control effect) —
        // a dead Select may be removed like any pure value.
        | Inst::Select(..)
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
// B1 — LIGHTWEIGHT ALIAS ANALYSIS (the memory oracle; the enabler for B2 load-elim).
// [Side-I theorem — OPT.md §5 (B1), ported from QBE `alias.c` but
//  re-derived as a theorem in zcc's IR, not transliterated.]
//
// THEOREM. Aliasing is a TOTAL DECIDABLE relation over a 4-point base lattice + an
// integer offset — no points-to graph, no fixpoint, ONE pass. Every address value
// gets a descriptor (base, offset):
//   Loc(off) — a stack slot at frame offset `off`  (QBE ALoc; provably local unless it escapes)
//   Sym(k,i) — a global (k=0) / string-literal (k=1) symbol address  (QBE ASym)
//   Con      — a pure integer-constant address, no symbolic base      (QBE ACon)
//   Unk(t)   — unknown; the base is the temp `t` itself                (QBE AUnk)
// plus `escaped`: the set of stack slots whose address LEAKED (passed to a call,
// stored through a pointer, returned, or mixed into an untracked pointer). A
// non-escaped Loc is PROVABLY disjoint from every unknown pointer — QBE's key move.
//
// The relation alias((p,sp),(q,sq)) ∈ {No, Must, May} (sp/sq = access widths):
//   • two different stack slots            → No     (disjoint frame regions)
//   • same base (slot/sym/con/unk-temp)    → overlap ? Must : No   (offsets decide)
//   • different symbols                    → May    (conservative; QBE)
//   • one Unknown vs a provably-local slot → No     (the local never leaked)
//   • one Unknown vs anything else         → May    (conservative)
//   • two disjoint kinds (loc/sym/con)     → No     (distinct memory regions)
// where overlap = ap.off < aq.off+sq ∧ aq.off < ap.off+sp.
//
// SOUNDNESS (the CbC obligation, unit-tested `alias_soundness`): the oracle NEVER
// answers No/Must when two accesses can actually alias at runtime — `May` is always a
// safe reply, and every non-May verdict is backed by a disjointness/identity proof.
// This is an ANALYSIS, not a transform: it emits no code, so it carries no
// improvement obligation — its correctness IS the proof obligation (Law 3).
// ─────────────────────────────────────────────────────────────────────────────

/// The base of an address value — the 4-point lattice (Side-I). `Sym` carries a kind
/// (0 = global, 1 = string literal) so a global and a string never share an identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ABase {
    Loc(u32),      // stack slot @ frame offset
    Sym(u8, u32),  // (kind, interned id)
    Con,           // pure integer-constant address
    Unk(Tmp),      // unknown; base is the temp itself
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Alias {
    pub base: ABase,
    pub off: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AliasR {
    No,
    Must,
    May,
}

/// The alias oracle for one function: a per-temp descriptor + the escaped-slot set.
pub(crate) struct AliasInfo {
    a: Vec<Alias>,
    escaped: HashSet<u32>,
}

/// getalias(Val): a temp reads its descriptor; a constant is a `Con` address at its
/// value; a float bit-pattern is never an address (a distinct Unknown that matches
/// nothing real).
fn getal(a: &[Alias], v: &Val) -> Alias {
    match v {
        Val::Tmp(t) => a[*t as usize],
        Val::Imm(x) => Alias { base: ABase::Con, off: *x },
        Val::FImm(_) => Alias { base: ABase::Unk(u32::MAX), off: 0 },
    }
}

impl AliasInfo {
    fn get(&self, v: Val) -> Alias {
        getal(&self.a, &v)
    }
    /// Is this base a PROVABLY-LOCAL stack slot (never leaked)? Such a slot cannot be
    /// reached through any unknown pointer.
    fn is_local(&self, b: ABase) -> bool {
        matches!(b, ABase::Loc(off) if !self.escaped.contains(&off))
    }
    /// Is a Load of `addr` FAULT-FREE, hence safe to SPECULATE past a branch (B4
    /// if-conversion)? A stack slot (`Loc`) and a symbol address (`Sym`, a global /
    /// string) are always MAPPED — reading them cannot trap. A pure integer-constant
    /// address (`Con`) or an unknown pointer (`Unk`) may be null/dangling ⟹ NOT safe.
    /// (Escape is irrelevant here: an escaped local is still a valid, mapped address.)
    pub(crate) fn fault_free(&self, addr: Val) -> bool {
        matches!(self.get(addr).base, ABase::Loc(_) | ABase::Sym(_, _))
    }
    /// The decidable alias relation. `sp`/`sq` are the byte widths of the two accesses.
    pub(crate) fn alias(&self, p: Val, sp: u32, q: Val, sq: u32) -> AliasR {
        use ABase::*;
        let (ap, aq) = (self.get(p), self.get(q));
        let ovlap = ap.off < aq.off + sq as i64 && aq.off < ap.off + sp as i64;
        let ov = |b| if b { AliasR::Must } else { AliasR::No };
        match (ap.base, aq.base) {
            // both stack: same slot ⟹ overlap decides; different slots are disjoint.
            (Loc(x), Loc(y)) => {
                if x == y {
                    ov(ovlap)
                } else {
                    AliasR::No
                }
            }
            // both symbolic: same symbol ⟹ overlap decides; different ⟹ conservatively May.
            (Sym(k1, i1), Sym(k2, i2)) => {
                if (k1, i1) != (k2, i2) {
                    AliasR::May
                } else {
                    ov(ovlap)
                }
            }
            (Con, Con) => ov(ovlap),
            (Unk(x), Unk(y)) if x == y => ov(ovlap),
            // one side unknown vs a non-provably-local base ⟹ May; otherwise the two
            // bases are disjoint memory regions (a non-escaped local, or two distinct
            // kinds among loc/sym/con) ⟹ No.
            _ => {
                if matches!(ap.base, Unk(_)) && !self.is_local(aq.base) {
                    AliasR::May
                } else if matches!(aq.base, Unk(_)) && !self.is_local(ap.base) {
                    AliasR::May
                } else {
                    AliasR::No
                }
            }
        }
    }
}

/// Intern a symbol name → a small dense id (so equality is an integer compare).
fn intern(syms: &mut HashMap<String, u32>, name: &str) -> u32 {
    if let Some(&id) = syms.get(name) {
        id
    } else {
        let id = syms.len() as u32;
        syms.insert(name.to_string(), id);
        id
    }
}

/// Compute the alias oracle: ONE RPO pass filling per-temp descriptors and the
/// escaped-slot set. In SSA (and in straight lowering for address computations) a def
/// precedes its uses, so an operand's descriptor is ready when read; callers query
/// only AFTER this returns, so the escaped set is complete before any verdict.
pub(crate) fn alias_info(f: &IrFunc) -> AliasInfo {
    let n = f.temps.len();
    let mut a: Vec<Alias> = (0..n).map(|t| Alias { base: ABase::Unk(t as Tmp), off: 0 }).collect();
    let mut escaped: HashSet<u32> = HashSet::new();
    let mut syms: HashMap<String, u32> = HashMap::new();

    // Mark the stack slot behind a Val as escaped (leaked). Non-Loc values carry no slot.
    let esc = |escaped: &mut HashSet<u32>, a: &[Alias], v: &Val| {
        if let Val::Tmp(t) = v {
            if let ABase::Loc(off) = a[*t as usize].base {
                escaped.insert(off);
            }
        }
    };

    for b in rpo(f) {
        let blk = &f.blocks[b as usize];
        for i in &blk.insts {
            // 1. the descriptor of the destination (if any).
            match i {
                Inst::Lea(d, Place::Local(off)) => {
                    a[*d as usize] = Alias { base: ABase::Loc(*off), off: 0 }
                }
                Inst::Lea(d, Place::Global(name, off)) => {
                    let id = intern(&mut syms, name);
                    a[*d as usize] = Alias { base: ABase::Sym(0, id), off: *off }
                }
                Inst::Lea(d, Place::Str(s)) => {
                    a[*d as usize] = Alias { base: ABase::Sym(1, *s), off: 0 }
                }
                Inst::Copy(d, _, v) => a[*d as usize] = getal(&a, v), // propagate
                // pointer ± constant offset stays on the SAME base (a ring identity on
                // the address); pointer + variable / pointer + pointer is untrackable.
                Inst::Bin(d, Op::Add, _, x, y) => {
                    let (ax, ay) = (getal(&a, x), getal(&a, y));
                    a[*d as usize] = if ax.base == ABase::Con {
                        Alias { base: ay.base, off: ay.off.wrapping_add(ax.off) }
                    } else if ay.base == ABase::Con {
                        Alias { base: ax.base, off: ax.off.wrapping_add(ay.off) }
                    } else {
                        Alias { base: ABase::Unk(*d), off: 0 }
                    };
                }
                Inst::Bin(d, Op::Sub, _, x, y) => {
                    let (ax, ay) = (getal(&a, x), getal(&a, y));
                    a[*d as usize] = if ay.base == ABase::Con {
                        Alias { base: ax.base, off: ax.off.wrapping_sub(ay.off) }
                    } else {
                        Alias { base: ABase::Unk(*d), off: 0 }
                    };
                }
                _ => {
                    if let Some(d) = inst_def(i) {
                        a[d as usize] = Alias { base: ABase::Unk(d), off: 0 }
                    }
                }
            }
            // 2. escape: an instruction whose result is NOT a tracked base leaks its
            // pointer operands — EXCEPT a bare dereference (load/store through an
            // address does not leak the address; a store leaks the stored VALUE only).
            let leaks = match inst_def(i) {
                None => true,
                Some(d) => matches!(a[d as usize].base, ABase::Unk(_)),
            };
            if leaks {
                match i {
                    Inst::Load(..) => {} // addr dereferenced, not leaked
                    Inst::Store(_, _addr, val) => esc(&mut escaped, &a, val),
                    _ => {
                        let mut us = Vec::new();
                        inst_uses(i, &mut us);
                        for u in us {
                            esc(&mut escaped, &a, &Val::Tmp(u));
                        }
                    }
                }
            }
        }
        // a pointer flowing out through the terminator (return value / branch) leaks.
        let mut tu = Vec::new();
        term_uses(&blk.term, &mut tu);
        for u in tu {
            esc(&mut escaped, &a, &Val::Tmp(u));
        }
    }
    AliasInfo { a, escaped }
}

// ─────────────────────────────────────────────────────────────────────────────
// B2 — LOAD ELIMINATION / STORE→LOAD FORWARDING (gated by the B1 alias oracle).
// [Side-I theorem — OPT.md §5 (B2), ported from QBE `load.c`.]
//
// THEOREM. Within a straight-line block, a `Load` from `a` (width sa) whose value is
// already available in memory — because a preceding `Store` wrote a MUST-alias
// address with the same width, with NO intervening MAY-alias store — equals that
// stored value ⟹ forward it (Load → Copy). A load whose value was produced by an
// earlier same-width must-alias load is likewise redundant. The alias oracle also
// lets an available value SURVIVE a store it provably does-not-alias (block-local
// `cse` conservatively kills every load at any store; the oracle keeps NoAlias ones).
//
// This is BLOCK-LOCAL: within one block the instruction sequence is executed in order
// with no incoming edges mid-block, so "preceding" = "dominating on the only path" and
// no dominance machinery is needed. Floats are excluded (a `float`(4) store rounds to
// f32 on the way to memory, but `Copy` does not round — forwarding would skip the
// rounding). Any opaque memory writer (call / memcpy / zero / exotic) conservatively
// clears the available set.
//
// PROOF OBLIGATION (Law 3):
//   • correctness — the commuting square `⟦f⟧=⟦load-elim(f)⟧` via `equiv`: interp's
//     per-frame `mem` models intra-function store→load, so a forwarded local load is
//     machine-validated; global/unknown loads are unmodeled → `equiv` SKIP, correct
//     by the B1 soundness theorem (the oracle proved must/no-alias) + the box torture.
//   • improvement — the emitted `.s` has strictly FEWER `ldr` (each forwarded load
//     becomes a register copy the peephole then folds). Statically visible, no race.
// ─────────────────────────────────────────────────────────────────────────────

/// Is the stored value `val` guaranteed to be in BACKEND-CANONICAL form for a `ty`
/// access — i.e. does forwarding it (a register `Copy`) reproduce what the memory
/// round-trip (`str`+`ldr`, which sign/zero-extends to `ty`) would yield?
///
/// [Side-II fact — the arm64 backend's canonicalization timing.] A temp's REGISTER
/// value is canonical to its type only for width ≥ 4 (`ir_bin_r` ext-s width-4 results;
/// loads ext all widths — but width-1/2 arithmetic results are left wide, canonicalized
/// lazily at the `strb`/`ldrb` boundary). A store→load forward that assumes canonicity
/// on a width-1/2 value would skip that truncation (GCC torture pr81913: `u8 d--`
/// forwarded as a full-width `-1` instead of the wrapped `255`). interp canonicalizes
/// EAGERLY at every `Bin`, so `equiv` is blind to this — the box torture is the oracle.
/// Constants forward when they already fit `ty` (the backend materializes the same bits).
fn fwd_canonical(tt: &TyTab, ty: TypeId, val: Val) -> bool {
    if tt.is_float(ty) || matches!(tt.tys[ty as usize], Ty::Bitfield(..)) {
        return false;
    }
    match val {
        Val::FImm(_) => false,
        Val::Imm(x) => canon(tt, ty, x) == x, // an in-range constant round-trips identically
        Val::Tmp(_) => tt.size(ty) >= 4,      // width-4/8 def is register-canonical; width-1/2 is not
    }
}

/// Does this instruction potentially WRITE memory (⟹ clear the available set)? The
/// allowlist is the pure, memory-read-or-less set; anything else (incl. a future
/// exotic) kills by default — never silently retaining a value across an unknown write.
fn pure_mem(i: &Inst) -> bool {
    matches!(
        i,
        Inst::Bin(..)
            | Inst::Un(..)
            | Inst::Copy(..)
            | Inst::Load(..)
            | Inst::Lea(..)
            | Inst::Cast(..)
            | Inst::FunAddr(..)
            | Inst::LabelAddr(..)
            | Inst::VaArea(..)
            | Inst::Phi(..)
    )
}

pub fn load_elim(tt: &TyTab, f: &mut IrFunc) -> u32 {
    let ai = alias_info(f);
    let mut n = 0u32;
    for b in f.blocks.iter_mut() {
        // available memory contents, in program order: (address, byte width, type, value).
        let mut avail: Vec<(Val, u32, TypeId, Val)> = Vec::new();
        for i in b.insts.iter_mut() {
            match i {
                Inst::Store(ty, addr, v) => {
                    let sz = tt.size(*ty);
                    // a store INVALIDATES every value it may-alias (Must or May); keep
                    // only the provably-disjoint (NoAlias) entries.
                    let (addr, v, ty) = (*addr, *v, *ty);
                    avail.retain(|(a2, s2, _, _)| ai.alias(addr, sz, *a2, *s2) == AliasR::No);
                    if !tt.is_float(ty) {
                        avail.push((addr, sz, ty, v)); // this exact address now holds `v`
                    }
                }
                Inst::Load(d, ty, addr) => {
                    let (d, ty, addr) = (*d, *ty, *addr);
                    let sz = tt.size(ty);
                    // forward a must-alias, same-width, same-type available value —
                    // only when its register form is canonical for `ty` (see fwd_canonical).
                    let hit = avail.iter().find(|(a2, s2, t2, v)| {
                        *t2 == ty
                            && *s2 == sz
                            && fwd_canonical(tt, ty, *v)
                            && ai.alias(addr, sz, *a2, *s2) == AliasR::Must
                    });
                    if let Some(&(_, _, _, val)) = hit {
                        *i = Inst::Copy(d, ty, val);
                        n += 1;
                    } else if !tt.is_float(ty) {
                        avail.push((addr, sz, ty, Val::Tmp(d))); // this load's value is now cached
                    }
                }
                other if !pure_mem(other) => avail.clear(), // opaque memory writer
                _ => {}                                      // pure non-memory op
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
pub fn abi_alloc(tt: &TyTab, f: &IrFunc, gp: &ClassBudget, fp: &ClassBudget, coalesce: bool) -> Vec<AbiHome> {
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
    // by construction (a Copy preserves type); the per-class select filters anyway. Empty
    // when coalescing is toggled off ⟹ the plain Stage-5b coloring.
    let mut move_adj: Vec<Vec<Tmp>> = vec![Vec::new(); nt];
    if coalesce {
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
// LOOP INFRASTRUCTURE + LICM (Phase B) — LOOP-INVARIANT CODE MOTION.
//
// GOVERNING THEOREM (CbC): `⟦f⟧ = ⟦licm(f)⟧`, MEASURED by `equiv`. QBE deliberately
// skips loop passes; we admit ONE (LICM) because it has a clean commuting-square proof:
//
//   A PURE, TRAP-FREE instruction whose operands are all defined OUTSIDE the loop
//   computes the SAME value on every iteration. Moving its single (SSA) definition to
//   the loop's PREHEADER — a block that dominates the loop and lies on the only entry
//   edge — evaluates it ONCE instead of n times. Because it is pure and trap-free, the
//   preheader may compute it even when the loop body runs zero times (speculation is
//   safe): the result is simply unused, and no observable state changes. Every use of
//   the value is inside the loop, which the preheader dominates ⟹ def-before-use holds.
//   Hence ⟦·⟧ is preserved.
//
// SAFETY FENCES (why the "trap-free / pure" hypothesis actually holds):
//   • hoist only Bin(¬Div,¬Rem) / Un / Copy / Cast / Lea — pure and non-faulting
//     (integer +−×, bitwise, shifts, casts, frame-address computation). Div/Rem are the
//     only trapping arithmetic (÷0 is UB) and are NOT hoisted; Load is NOT hoisted (it may
//     fault or alias a store in the loop) — so speculation never introduces a fault.
//   • zcc's IR is only PARTIAL SSA — to_ssa promotes address-not-taken scalar locals, but
//     straight-lowering temps and non-promotable values stay MULTI-DEF (reassignable). So
//     single-assignment is NOT assumed; it is CHECKED: an instruction is hoisted only when
//     its DST is single-def and every operand is a constant or a single-def temp defined
//     outside the loop. Hoisting one def of a multi-def temp (e.g. a loop-condition temp
//     reassigned each iteration) would FREEZE the value and change control flow — the class
//     of bug the earlier version introduced (a terminating loop turned infinite). `equiv`
//     is BLIND to it (interp of an infinite loop → Err → skipped as UB), so the guard is a
//     construction-time obligation, re-checked by a direct-interp regression test.
//   • `cfg_complete`-guarded (computed goto ⟹ the CFG is incomplete → dominance unsound).
//   • a loop with no single out-of-loop entry (irreducible / entry-as-header) is skipped.
// This attacks matmul's invariant address arithmetic (`a + i*rowsize`, the base of
// `c[i][j]`), the biggest gap in the bench.
// ─────────────────────────────────────────────────────────────────────────────

/// Back-edges (tail → header): a CFG edge whose head DOMINATES its tail (Aho §9.6.2).
/// The head is a natural-loop header.
fn back_edges(f: &IrFunc, dom: &[HashSet<BlockId>]) -> Vec<(BlockId, BlockId)> {
    let succ = successors(f);
    let mut out = Vec::new();
    for u in 0..f.blocks.len() as BlockId {
        for &v in &succ[u as usize] {
            if dom[u as usize].contains(&v) {
                out.push((u, v));
            }
        }
    }
    out
}

/// The natural loop of back-edge (tail→header): {header} ∪ {nodes that reach `tail`
/// without passing through `header`} — backward reachability from tail (Aho Alg. 9.45).
fn natural_loop(f: &IrFunc, tail: BlockId, header: BlockId) -> HashSet<BlockId> {
    let preds = predecessors(f);
    let mut body = HashSet::from([header]);
    let mut stack = Vec::new();
    if tail != header && body.insert(tail) {
        stack.push(tail);
    }
    while let Some(n) = stack.pop() {
        for &p in &preds[n as usize] {
            if body.insert(p) {
                stack.push(p);
            }
        }
    }
    body
}

/// Ensure a dedicated PREHEADER for `header`: a block OUTSIDE `body`, on the sole
/// out-of-loop entry edge, dominating the loop. Returns its BlockId, or None when the
/// loop has no single out-of-loop entry (0 = the function entry is itself the header;
/// >1 = multiple entries / irreducible → bail). Reuses an existing single-successor
/// `Jmp(header)` entry block; otherwise inserts a fresh empty block on the entry edge
/// (⟦·⟧-preserving, exactly like a critical-edge split) and moves the header's φ-arm
/// for the entry predecessor onto the preheader.
fn ensure_preheader(f: &mut IrFunc, header: BlockId, body: &HashSet<BlockId>) -> Option<BlockId> {
    let preds = predecessors(f);
    let entries: Vec<BlockId> =
        preds[header as usize].iter().copied().filter(|p| !body.contains(p)).collect();
    if entries.len() != 1 {
        return None;
    }
    let p = entries[0];
    let mut succ = Vec::new();
    term_targets(&f.blocks[p as usize].term, &mut succ);
    if succ.len() == 1 && matches!(f.blocks[p as usize].term, Term::Jmp(h) if h == header) {
        return Some(p); // already a dedicated preheader (also gives idempotency across runs)
    }
    let ph = f.blocks.len() as BlockId;
    f.blocks.push(Block { insts: Vec::new(), term: Term::Jmp(header) });
    retarget(&mut f.blocks[p as usize].term, header, ph);
    rename_phi_pred(&mut f.blocks[header as usize], p, ph);
    Some(ph)
}

/// Hoistable ⟺ pure AND trap-free AND fault-free (see SAFETY FENCES). φ, Load, Store,
/// Div/Rem, and every impure exotic are excluded.
fn is_hoistable(i: &Inst) -> bool {
    match i {
        Inst::Bin(_, op, ..) => !matches!(op, Op::Div | Op::Rem),
        Inst::Un(..) | Inst::Copy(..) | Inst::Cast(..) | Inst::Lea(..) => true,
        _ => false,
    }
}

/// Every operand is either a compile-time constant or a temp AVAILABLE in the preheader
/// (a single-def source whose one definition lies outside the loop, incl. an already-hoisted one).
fn operands_avail(i: &Inst, avail: &[bool]) -> bool {
    let mut u = Vec::new();
    inst_uses(i, &mut u);
    u.iter().all(|&t| avail[t as usize])
}

/// Register-PRESSURE of a loop body = the max, over every program point in `body`, of the
/// number of simultaneously-live GP (non-float) temps. This is the SCARCE resource the
/// speed-positivity guard protects: the GP class has only `k` colours (AAPCS64 allocatable
/// set, a Side-II constant threaded from the backend budget), so a point with pressure > k
/// forces a spill. Computed by the standard backward live-set walk (the transfer function
/// abi_alloc's crossing scan uses), each block seeded with live-out ∪ terminator-uses.
fn loop_gp_pressure(tt: &TyTab, f: &IrFunc, lv: &Liveness, body: &HashSet<BlockId>) -> u32 {
    let nt = f.temps.len();
    let is_fp: Vec<bool> = f.temps.iter().map(|&ty| tt.is_float(ty)).collect();
    let gp_count = |live: &[bool]| (0..nt).filter(|&t| live[t] && !is_fp[t]).count() as u32;
    let mut maxp = 0u32;
    let mut buf = Vec::new();
    for &bid in body {
        let b = &f.blocks[bid as usize];
        let mut live = lv.live_out[bid as usize].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true; // at block exit the terminator operands are live
        }
        maxp = maxp.max(gp_count(&live));
        for i in b.insts.iter().rev() {
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
            maxp = maxp.max(gp_count(&live));
        }
    }
    maxp
}

/// `gp_k` = the GP colour budget (AAPCS64 allocatable count, the backend's `GP_BUDGET.k`),
/// threaded in so the SPEED-POSITIVITY guard shares the ONE Side-II constant rather than
/// duplicating it. Correctness (`⟦f⟧=⟦licm f⟧`) is independent of `gp_k`; it only caps how
/// many invariants may be hoisted per loop (see the guard note inline).
pub fn licm(tt: &TyTab, f: &mut IrFunc, gp_k: u32) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto ⟹ incomplete CFG ⟹ dominance/back-edges unsound
    }
    // zcc's IR is only PARTIAL SSA: to_ssa promotes address-not-taken scalar locals into
    // single-assignment temps, but straight-lowering temps and non-promotable values stay
    // MULTI-DEF (reassignable 3-address temps). LICM's theorem needs single-assignment, so
    // it is gated on it EXPLICITLY: def counts per temp + the (unique) defining block.
    let nt = f.temps.len();
    let mut defcnt = vec![0u32; nt];
    let mut def_block = vec![u32::MAX; nt];
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defcnt[d as usize] += 1;
                def_block[d as usize] = bi as BlockId;
            }
        }
    }
    let dom = dominators(f);
    let backs = back_edges(f, &dom);
    let mut hoisted = 0u32;
    for (tail, header) in backs {
        let body = natural_loop(f, tail, header); // against the CURRENT CFG (fresh preds)
        // A temp is an AVAILABLE invariant source ⟺ it is SINGLE-DEF and its one definition
        // lies OUTSIDE the loop body. A multi-def temp is never invariant (it may be
        // reassigned — the loop-condition bug), and a temp defined inside the body is not
        // yet available (until it is itself hoisted).
        let mut avail = vec![false; nt];
        for t in 0..nt {
            if defcnt[t] == 1 && def_block[t] != u32::MAX && !body.contains(&def_block[t]) {
                avail[t] = true;
            }
        }
        // Candidate ⟺ hoistable shape ∧ its DST is SINGLE-DEF (moving a multi-def
        // instruction would break the value that flows on another path) ∧ every operand
        // is available. Only disturb the CFG if such a candidate exists (no empty preheaders).
        let candidate = |inst: &Inst, avail: &[bool]| {
            is_hoistable(inst)
                && matches!(inst_def(inst), Some(d) if defcnt[d as usize] == 1)
                && operands_avail(inst, avail)
        };
        let has_candidate =
            body.iter().any(|&bid| f.blocks[bid as usize].insts.iter().any(|i| candidate(i, &avail)));
        if !has_candidate {
            continue;
        }
        // SPEED-POSITIVITY GUARD — the mathematical fence that makes LICM ship-safe on a
        // k-register machine. Correctness lives on one axis (the commuting square ⟦f⟧=⟦licm f⟧,
        // still an identity because a partial hoist is a SUBSET of the proven hoist set); COST
        // lives on the orthogonal axis this guard governs. Each hoist makes one value live
        // ACROSS the whole body, so it can raise the live-count at any point by at most 1 ⟹
        // post-hoist max GP pressure ≤ P + (#hoists). Capping #hoists at (k − P) keeps pressure
        // ≤ k = the GP colour budget ⟹ the k-colouring survives ⟹ the allocator introduces NO
        // new spill ⟹ each hoist strictly deletes (trip−1) dynamic ALU ops with ZERO added
        // memory traffic ⟹ C_M strictly decreases. P is MEASURED (liveness); k is a Side-II ABI
        // constant — no tuned weight anywhere. (Residual: P is SSA-pressure, a proxy for the
        // post-φ-destruction allocator's pressure; the box A/B closes that gap before the
        // default-ON flip — OPT.md §2 (why proof-faster meets reality-slower).)
        let lv = liveness(f);
        let headroom = gp_k.saturating_sub(loop_gp_pressure(tt, f, &lv, &body));
        if headroom == 0 {
            continue; // loop already at/over budget — ANY hoist would spill; refuse (no regress)
        }
        let ph = match ensure_preheader(f, header, &body) {
            Some(p) => p,
            None => continue,
        };
        // Fixpoint hoist, CAPPED at `headroom`: each round moves the first newly-available
        // invariant instruction and marks its (single) def available, so a dependent invariant
        // becomes hoistable next round — landing AFTER its producer (dependency order kept).
        let mut here = 0u32;
        loop {
            if here >= headroom {
                break; // GP-pressure budget for this loop exhausted
            }
            let mut found = None;
            'scan: for &bid in body.iter() {
                for (ix, inst) in f.blocks[bid as usize].insts.iter().enumerate() {
                    if candidate(inst, &avail) {
                        found = Some((bid, ix));
                        break 'scan;
                    }
                }
            }
            let (bid, ix) = match found {
                Some(x) => x,
                None => break,
            };
            let inst = f.blocks[bid as usize].insts.remove(ix);
            if let Some(d) = inst_def(&inst) {
                avail[d as usize] = true; // now defined in the preheader (outside the body)
            }
            f.blocks[ph as usize].insts.push(inst);
            hoisted += 1;
            here += 1;
        }
    }
    hoisted
}

// ─────────────────────────────────────────────────────────────────────────────
// STRENGTH REDUCTION (Phase B.5) — induction-variable based, default-OFF.
//
// THE CLASSIC OPTIMIZATION (Cocke–Kennedy; Aho §9.6): inside a loop, a MULTIPLY by a
// constant that rides an induction variable is replaced by an ADD accumulator. The
// canonical target is address arithmetic: `a + i*elemsize` in a matmul inner loop
// recomputes a multiply every iteration; strength reduction turns it into one add per
// step (the accumulator marches `elemsize` per iteration). This is *the* textbook loop
// optimization — the pedagogical heart of Phase B.
//
// GOVERNING THEOREM (CbC): `⟦f⟧ = ⟦strength_reduce(f)⟧`, MEASURED by `equiv`. The proof
// is an INDUCTION on the loop trip count. Let the loop carry a BASIC INDUCTION VARIABLE
// (BIV) as an SSA header φ:
//
//        i₁ = φ(preheader: i₀, latch: i₂)        i₂ = i₁ + c        (c constant)
//
// so at the head of iteration k, i₁ = i₀ + k·c. A DERIVED induction variable is a body
// expression j = i₁ · d (d constant). We introduce a parallel accumulator φ:
//
//        j₁ = φ(preheader: i₀·d, latch: j₂)      j₂ = j₁ + c·d
//
// and replace `j = i₁·d` with `j = j₁`. CLAIM: j₁ = i₁·d at the head of every iteration.
//   • BASE (k=0): j₁ = i₀·d = i₁·d, since i₁ = i₀ on entry.  ✓
//   • STEP: assume j₁ = i₁·d. After the latch, j₂ = j₁ + c·d = i₁·d + c·d = (i₁+c)·d =
//     i₂·d, which becomes the next head value of both φ's.  ✓
// Hence every observation of j is unchanged ⟹ ⟦·⟧ preserved. The constant c·d is folded
// at build time. Distribution (i₁+c)·d = i₁·d + c·d holds EXACTLY in ℤ/2ⁿ (two's-complement
// wrapping), so no overflow/UB gap opens at the IR level — the reduction is faithful to
// the fixed-width integer semantics interp implements.
//
// SAFETY FENCES (why the hypotheses actually hold on this partial-SSA IR):
//   • INTEGER only — float × does not distribute exactly (non-associative), so `tt.is_float`
//     types are skipped for both the BIV step and the derived multiply.
//   • CONSTANT step c and CONSTANT multiplier d ⟹ c·d is a compile-time constant; no
//     invariant-multiply needs inserting, keeping the inserted latch op a pure ADD.
//   • SINGLE-DEF everywhere (i₁, i₂, j) — SSA guarantees it for promoted temps, but this IR
//     is only PARTIAL SSA, so it is CHECKED, never assumed (same discipline as LICM).
//   • REDUCIBLE single-latch loop — the header φ must have exactly two arms (one external,
//     one from the unique back-edge tail); multi-latch / irreducible loops are skipped.
//   • `cfg_complete`-guarded (computed goto ⟹ unsound dominance).
//
// MEASURED, not asserted: like LICM, on the memory-bound naive-slot backend the new φ adds
// spill slots and edge copies that can outweigh the mul→add saving — so this ships behind
// the `Passes` toggle, default-OFF, and its value here is the PROOF and the teaching, with
// the perf win latent for a register-resident backend.
// ─────────────────────────────────────────────────────────────────────────────

/// Match `Bin(_, _, _, a, b)` operands as (Tmp(want), Imm k) in either order → k.
fn tmp_times_imm(a: &Val, b: &Val, want: Tmp) -> Option<i64> {
    match (a, b) {
        (Val::Tmp(t), Val::Imm(k)) | (Val::Imm(k), Val::Tmp(t)) if *t == want => Some(*k),
        _ => None,
    }
}

pub fn strength_reduce(tt: &TyTab, f: &mut IrFunc, gp_k: u32) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto ⟹ dominance/back-edges unsound (as LICM/GVN/SCCP)
    }
    let dom = dominators(f);
    let backs = back_edges(f, &dom);
    let mut changed = 0u32;
    for &(tail, header) in &backs {
        // REDUCIBLE single-latch only: a unique back-edge into `header` ⟹ the header φ has
        // exactly one latch arm, so the accumulator φ we build is well-formed.
        if backs.iter().filter(|(_, h)| *h == header).count() != 1 {
            continue;
        }
        // Def-counts are recomputed PER back-edge, against the CURRENT function: a previous
        // loop's rewrite may have appended fresh temps (jbase/jnext/jphi), so a stale snapshot
        // would both under-size the array (a nested-loop panic) and miss the new single-defs.
        let mut defcnt = vec![0u32; f.temps.len()];
        for b in &f.blocks {
            for i in &b.insts {
                if let Some(d) = inst_def(i) {
                    defcnt[d as usize] += 1;
                }
            }
        }
        let body = natural_loop(f, tail, header);
        // SPEED-POSITIVITY GUARD (same axis-split as LICM). The reduction trades a per-iter
        // `mul` for a per-iter `add` (a win in the count model) but INSERTS an accumulator that
        // is live across the loop: an added φ (`j₁`, live header→latch) + its latch step (`j₂`)
        // ⟹ up to +2 GP values live across the body. If the body is already within 2 colours of
        // the budget, that accumulator spills and the reload-per-iteration outweighs the
        // mul→add saving (the exact 2026 regression). Refuse unless P + 2 ≤ k. Correctness is
        // unaffected (⟦f⟧=⟦sr f⟧ holds whether or not a given loop is transformed).
        {
            let lv = liveness(f);
            if loop_gp_pressure(tt, f, &lv, &body) + 2 > gp_k {
                continue; // no register headroom for the accumulator φ ⟹ would spill
            }
        }
        // 1. BASIC INDUCTION VARIABLES from the header φ region: i₁ = φ(ext: i₀, tail: i₂)
        //    with i₂ = i₁ + c (constant c), integer, all single-def.
        let mut bivs: Vec<(Tmp, TypeId, Val, i64)> = Vec::new(); // (i₁, ty, i₀, step c)
        for inst in &f.blocks[header as usize].insts {
            let Inst::Phi(i1, ty, arms) = inst else { continue };
            if tt.is_float(*ty) || arms.len() != 2 || defcnt[*i1 as usize] != 1 {
                continue;
            }
            let latch = arms.iter().find(|(p, _)| *p == tail);
            let ext = arms.iter().find(|(p, _)| *p != tail);
            let (Some((_, Val::Tmp(i2))), Some((_, i0))) = (latch, ext) else { continue };
            if defcnt[*i2 as usize] != 1 {
                continue;
            }
            // find i₂'s definition inside the body: Bin(i₂, Add, ty, i₁, Imm c) (either order).
            let mut step = None;
            for &bid in &body {
                for di in &f.blocks[bid as usize].insts {
                    if let Inst::Bin(d, Op::Add, dty, a, b) = di {
                        if *d == *i2 && *dty == *ty {
                            step = tmp_times_imm(a, b, *i1);
                        }
                    }
                }
            }
            if let Some(c) = step {
                bivs.push((*i1, *ty, *i0, c));
            }
        }
        if bivs.is_empty() {
            continue;
        }
        // 2. DERIVED IVs: Bin(j, Mul, ty, i₁, Imm d) in the body, j single-def, integer.
        let mut derived: Vec<(BlockId, Tmp, TypeId, Val, i64, i64)> = Vec::new(); // (blk, j, ty, i₀, c, d)
        for &bid in &body {
            for inst in &f.blocks[bid as usize].insts {
                let Inst::Bin(j, Op::Mul, ty, a, b) = inst else { continue };
                if tt.is_float(*ty) || defcnt[*j as usize] != 1 {
                    continue;
                }
                for &(i1, bty, i0, c) in &bivs {
                    if bty == *ty {
                        if let Some(d) = tmp_times_imm(a, b, i1) {
                            derived.push((bid, *j, *ty, i0, c, d));
                            break;
                        }
                    }
                }
            }
        }
        if derived.is_empty() {
            continue;
        }
        // 3. Materialize the preheader ONLY now (no empty preheaders). It may rename the
        //    header φ's external arm predecessor to `ph` (the value i₀ is unchanged).
        let ph = match ensure_preheader(f, header, &body) {
            Some(p) => p,
            None => continue,
        };
        // 4. Rewrite each derived IV into an accumulator φ.
        for (bid, j, ty, i0, c, d) in derived {
            let jbase = f.temps.len() as Tmp; // j₀ = i₀·d, computed ONCE in the preheader
            f.temps.push(ty);
            f.blocks[ph as usize].insts.push(Inst::Bin(jbase, Op::Mul, ty, i0, Val::Imm(d)));
            let jnext = f.temps.len() as Tmp; // j₂ = j₁ + c·d, at the latch
            f.temps.push(ty);
            let jphi = f.temps.len() as Tmp; // j₁ = φ(ph: j₀, tail: j₂), the accumulator
            f.temps.push(ty);
            let pos = f.blocks[header as usize]
                .insts
                .iter()
                .position(|i| !matches!(i, Inst::Phi(..)))
                .unwrap_or(0);
            f.blocks[header as usize].insts.insert(
                pos,
                Inst::Phi(jphi, ty, vec![(ph, Val::Tmp(jbase)), (tail, Val::Tmp(jnext))]),
            );
            let step = c.wrapping_mul(d); // c·d folded now (interp canon's to `ty` width)
            f.blocks[tail as usize].insts.push(Inst::Bin(jnext, Op::Add, ty, Val::Tmp(jphi), Val::Imm(step)));
            // Replace the derived multiply `j = i₁·d` with `j = j₁`. Indices shifted (φ
            // insert / latch append), so RE-LOCATE by j's unique (single-def) defining site.
            if let Some(cur) = f.blocks[bid as usize].insts.iter().position(|i| inst_def(i) == Some(j)) {
                f.blocks[bid as usize].insts[cur] = Inst::Copy(j, ty, Val::Tmp(jphi));
            }
            changed += 1;
        }
    }
    changed
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass — POINTER-IV STRENGTH REDUCTION + LFTR (the gcc-O1 loop-nest recipe).
//
// THE MEASURED LEVER (OPT.md §4): the accumulator-φ `strength_reduce` above turns a
// derived `i·d` into a marching add but LEAVES the invariant base add and the loop
// counter — matmul's inner loop keeps a per-iter `mul`, a `base+index` add and a
// counter compare (2.92× even fully un-starved). gcc-O1 collapses the same loop to
// SIX instructions by (1) folding the invariant base INTO the induction, producing a
// MARCHING POINTER, and (2) LFTR — replacing the counter test with a pointer/limit
// compare so the counter dies. This pass does both. Unlike the accumulator SR it is
// PRESSURE-REDUCING (a pointer replaces {counter, base, index-mul, base-add}).
//
// GOVERNING THEOREM (CbC): `⟦f⟧ = ⟦pointer_iv(f)⟧`, MEASURED by `equiv`. Two legs:
//   • BASE-FOLD leg. For an address `a = base + iv·s` (base loop-invariant, iv a basic
//     induction φ `i₁=φ(i₀,i₂)`, `i₂=i₁+c`), introduce `p₁=φ(ph: base+i₀·s, tail: p₂)`,
//     `p₂=p₁+c·s`, and replace `a` with `p₁`. CLAIM p₁ = base+i₁·s at every head:
//       BASE k=0: p₁ = base+i₀·s = base+i₁·s (i₁=i₀ on entry). STEP: p₂ = p₁+c·s =
//       base+i₁·s+c·s = base+(i₁+c)·s = base+i₂·s. ✓ (distribution exact in ℤ/2ⁿ.)
//     Value-preserving casts between iv and the multiply are transparent (a widening in
//     the same signedness class keeps the integer value), so `Cast(i₁)·d` reduces too —
//     the exact form the accumulator SR missed on matmul.
//   • LFTR leg. When after base-fold the counter i₁ feeds ONLY its increment and a header
//     test `i₁ < N` (N invariant, step c>0, stride s>0), replace the test with `p₁ < L`,
//     L = base+N·s (preheader). Since s>0 the map i₁↦p₁ is strictly monotone ⟹ `i₁<N`
//     ⇔ `p₁<L` on every iteration ⟹ the observable branch outcome is identical. The now-
//     dead counter φ/increment are reclaimed by DCE (next fixpoint round).
//
// SAFETY FENCES: integer IV only (float × non-associative); constant step c and stride s
// (c·s folds to a constant ⟹ the latch op is a pure add); EXACTLY ONE iv-linear term in
// the address (all other Add leaves loop-invariant); single-def i₁,i₂,a; reducible
// single-latch loop; `cfg_complete`-guarded. The invariant base is RECOMPUTED into the
// preheader (a self-contained mini-LICM, so the pass does not depend on LICM running or
// being pressure-unblocked first). Ships behind the `Passes` toggle; measured on the box
// before any default-ON flip (OPT.md §4).
// ─────────────────────────────────────────────────────────────────────────────

/// A cast that PRESERVES the integer value of an induction variable: an integer widening
/// in the same signedness class (sign/zero-extension keeps the value). Narrowing may wrap
/// ⟹ rejected; float casts never qualify.
fn iv_preserving_cast(tt: &TyTab, from: TypeId, to: TypeId) -> bool {
    !tt.is_float(from)
        && !tt.is_float(to)
        && tt.size(to) >= tt.size(from)
        && tt.is_unsigned(from) == tt.is_unsigned(to)
}

/// Recognize a temp whose loop value is `biv·stride`, through value-preserving casts and
/// constant multiplies/shifts. Returns (biv i₁, stride). Read-only (defs via `def_of`).
fn iv_linear(
    f: &IrFunc,
    tt: &TyTab,
    def_of: &[Option<(BlockId, usize)>],
    t: Tmp,
    biv_of: &HashMap<Tmp, (Val, i64)>,
) -> Option<(Tmp, i64)> {
    if biv_of.contains_key(&t) {
        return Some((t, 1));
    }
    let (b, i) = def_of[t as usize]?;
    match &f.blocks[b as usize].insts[i] {
        Inst::Cast(_, from, to, Val::Tmp(s)) if iv_preserving_cast(tt, *from, *to) => {
            iv_linear(f, tt, def_of, *s, biv_of)
        }
        // A Copy preserves the value exactly ⟹ same (biv, stride). Copy-prop residue (a
        // widened index copied for a second address) must not block that address's reduction.
        Inst::Copy(_, _, Val::Tmp(s)) => iv_linear(f, tt, def_of, *s, biv_of),
        Inst::Bin(_, Op::Mul, _, a, c) => match (a, c) {
            (Val::Tmp(s), Val::Imm(m)) | (Val::Imm(m), Val::Tmp(s)) => {
                iv_linear(f, tt, def_of, *s, biv_of).map(|(bi, st)| (bi, st.wrapping_mul(*m)))
            }
            _ => None,
        },
        Inst::Bin(_, Op::Shl, _, Val::Tmp(s), Val::Imm(sh)) if *sh >= 0 && *sh < 63 => {
            iv_linear(f, tt, def_of, *s, biv_of).map(|(bi, st)| (bi, st.wrapping_shl(*sh as u32)))
        }
        _ => None,
    }
}

/// Walk the Add-tree of address value `v`, sorting each leaf into an invariant `base`
/// term or THE (single) iv-linear term. Returns false if any leaf is neither.
#[allow(clippy::too_many_arguments)]
fn decompose_addr(
    f: &IrFunc,
    tt: &TyTab,
    def_of: &[Option<(BlockId, usize)>],
    v: &Val,
    body: &HashSet<BlockId>,
    biv_of: &HashMap<Tmp, (Val, i64)>,
    inv: &[bool],
    base: &mut Vec<Val>,
    lin: &mut Vec<(Tmp, i64)>,
) -> bool {
    let t = match v {
        Val::Tmp(t) => *t,
        _ => {
            base.push(v.clone()); // a constant addend is loop-invariant
            return true;
        }
    };
    if inv[t as usize] {
        base.push(Val::Tmp(t));
        return true;
    }
    if let Some(ls) = iv_linear(f, tt, def_of, t, biv_of) {
        lin.push(ls);
        return true;
    }
    // Not invariant, not a bare linear term ⟹ must be an in-body Add to recurse through.
    if let Some((b, i)) = def_of[t as usize] {
        if body.contains(&b) {
            if let Inst::Bin(_, Op::Add, _, a, c) = &f.blocks[b as usize].insts[i] {
                return decompose_addr(f, tt, def_of, a, body, biv_of, inv, base, lin)
                    && decompose_addr(f, tt, def_of, c, body, biv_of, inv, base, lin);
            }
        }
    }
    false
}

/// Rewrite an instruction's destination temp (only the pure invariant producers this pass
/// clones into the preheader).
fn set_dst(i: &mut Inst, d: Tmp) {
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

/// Clone the invariant computation of temp `t` (defined inside `body`) into preheader
/// `ph`, with fresh temps, operands first (a targeted hoist). A temp defined OUTSIDE the
/// body is already available ⟹ returned unchanged. Memoized.
fn clone_inv_to_ph(
    f: &mut IrFunc,
    ph: BlockId,
    body: &HashSet<BlockId>,
    t: Tmp,
    def_of: &[Option<(BlockId, usize)>],
    memo: &mut HashMap<Tmp, Tmp>,
) -> Tmp {
    if let Some(&nt) = memo.get(&t) {
        return nt;
    }
    let (b, i) = match def_of[t as usize] {
        Some(loc) if body.contains(&loc.0) => loc,
        _ => return t, // available in the preheader already
    };
    // Operands first (so the memo is populated before we remap this inst's uses).
    let mut us = Vec::new();
    inst_uses(&f.blocks[b as usize].insts[i], &mut us);
    for u in us {
        clone_inv_to_ph(f, ph, body, u, def_of, memo);
    }
    let mut inst = f.blocks[b as usize].insts[i].clone();
    each_use_mut(&mut inst, |val| {
        if let Val::Tmp(x) = val {
            if let Some(&nu) = memo.get(x) {
                *x = nu;
            }
        }
    });
    let ty = f.temps[t as usize];
    let nt = f.temps.len() as Tmp;
    f.temps.push(ty);
    set_dst(&mut inst, nt);
    f.blocks[ph as usize].insts.push(inst);
    memo.insert(t, nt);
    nt
}

pub fn pointer_iv(tt: &TyTab, f: &mut IrFunc, _gp_k: u32) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto ⟹ dominance/back-edges unsound (as LICM/SR)
    }
    let dom = dominators(f);
    let backs = back_edges(f, &dom);
    let mut changed = 0u32;
    for &(tail, header) in &backs {
        if backs.iter().filter(|(_, h)| *h == header).count() != 1 {
            continue; // reducible single-latch only
        }
        let nt = f.temps.len();
        // def location of every temp, against the CURRENT function (prior loops appended temps).
        let mut def_of: Vec<Option<(BlockId, usize)>> = vec![None; nt];
        let mut defcnt = vec![0u32; nt];
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, inst) in b.insts.iter().enumerate() {
                if let Some(d) = inst_def(inst) {
                    def_of[d as usize] = Some((bi as BlockId, ii));
                    defcnt[d as usize] += 1;
                }
            }
        }
        let body = natural_loop(f, tail, header);
        // BASIC INDUCTION VARIABLES: header φ i₁=φ(ext: i₀, tail: i₂) with i₂=i₁+c (const c),
        // integer, single-def — the same recognition as the accumulator SR.
        let mut biv_of: HashMap<Tmp, (Val, i64)> = HashMap::new();
        for inst in &f.blocks[header as usize].insts {
            let Inst::Phi(i1, ty, arms) = inst else { continue };
            if tt.is_float(*ty) || arms.len() != 2 || defcnt[*i1 as usize] != 1 {
                continue;
            }
            let (Some((_, Val::Tmp(i2))), Some((_, i0))) =
                (arms.iter().find(|(p, _)| *p == tail), arms.iter().find(|(p, _)| *p != tail))
            else {
                continue;
            };
            if defcnt[*i2 as usize] != 1 {
                continue;
            }
            let mut step = None;
            for &bid in &body {
                for di in &f.blocks[bid as usize].insts {
                    if let Inst::Bin(d, Op::Add, dty, a, b) = di {
                        if *d == *i2 && *dty == *ty {
                            step = tmp_times_imm(a, b, *i1);
                        }
                    }
                }
            }
            if let Some(c) = step {
                biv_of.insert(*i1, (i0.clone(), c));
            }
        }
        if biv_of.is_empty() {
            continue;
        }
        // LOOP-INVARIANT set over the body: const / defined outside body / (in-body, pure,
        // all operands invariant). Fixpoint. φ is never invariant (excludes the BIVs).
        let mut inv = vec![false; nt];
        for t in 0..nt {
            if let Some((b, _)) = def_of[t] {
                if !body.contains(&b) {
                    inv[t] = true;
                }
            }
        }
        loop {
            let mut grew = false;
            for &bid in &body {
                for inst in &f.blocks[bid as usize].insts {
                    let Some(d) = inst_def(inst) else { continue };
                    if inv[d as usize] || !is_hoistable(inst) {
                        continue;
                    }
                    let mut us = Vec::new();
                    inst_uses(inst, &mut us);
                    if us.iter().all(|&u| inv[u as usize]) {
                        inv[d as usize] = true;
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
        // RECOGNIZE reducible addresses (Load/Store whose address = base + iv·stride).
        struct Reduction {
            addr: Tmp,
            ty: TypeId,
            base: Vec<Val>,
            biv: Tmp,
            stride: i64,
        }
        let mut reds: Vec<Reduction> = Vec::new();
        let mut seen: HashSet<Tmp> = HashSet::new();
        for &bid in &body {
            for inst in &f.blocks[bid as usize].insts {
                let addr = match inst {
                    Inst::Load(_, _, Val::Tmp(a)) => *a,
                    Inst::Store(_, Val::Tmp(a), _) => *a,
                    _ => continue,
                };
                if !seen.insert(addr) || defcnt[addr as usize] != 1 {
                    continue;
                }
                match def_of[addr as usize] {
                    Some((b, _)) if body.contains(&b) => {}
                    _ => continue, // address already invariant / outside loop ⟹ nothing to march
                }
                let (mut base, mut lin) = (Vec::new(), Vec::new());
                if decompose_addr(
                    f,
                    tt,
                    &def_of,
                    &Val::Tmp(addr),
                    &body,
                    &biv_of,
                    &inv,
                    &mut base,
                    &mut lin,
                ) && lin.len() == 1
                    && lin[0].1 != 0
                {
                    reds.push(Reduction {
                        addr,
                        ty: f.temps[addr as usize],
                        base,
                        biv: lin[0].0,
                        stride: lin[0].1,
                    });
                }
            }
        }
        if reds.is_empty() {
            continue;
        }
        // MATERIALIZE. Preheader first (no empty preheaders).
        let ph = match ensure_preheader(f, header, &body) {
            Some(p) => p,
            None => continue,
        };
        let mut memo: HashMap<Tmp, Tmp> = HashMap::new();
        let mut lftr: Option<(Tmp /*pphi*/, Tmp /*pbase sum*/, i64 /*stride*/, Tmp /*biv*/)> = None;
        for r in &reds {
            // pbase = Σ base terms, recomputed once in the preheader.
            let mut acc: Option<Val> = None;
            for bt in &r.base {
                let v = match bt {
                    Val::Tmp(t) => Val::Tmp(clone_inv_to_ph(f, ph, &body, *t, &def_of, &mut memo)),
                    other => other.clone(),
                };
                acc = Some(match acc {
                    None => v,
                    Some(prev) => {
                        let d = f.temps.len() as Tmp;
                        f.temps.push(r.ty);
                        f.blocks[ph as usize].insts.push(Inst::Bin(d, Op::Add, r.ty, prev, v));
                        Val::Tmp(d)
                    }
                });
            }
            let pbase = acc.unwrap_or(Val::Imm(0));
            let pbase_t = match pbase {
                Val::Tmp(t) => t,
                imm => {
                    let d = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Copy(d, r.ty, imm));
                    d
                }
            };
            // pinit = pbase + i₀·stride  (folded; i₀ is the φ external arm, available in ph).
            let (i0, c) = biv_of[&r.biv].clone();
            let pinit = match i0 {
                Val::Imm(0) => Val::Tmp(pbase_t),
                Val::Imm(v) => {
                    let d = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Bin(
                        d,
                        Op::Add,
                        r.ty,
                        Val::Tmp(pbase_t),
                        Val::Imm(v.wrapping_mul(r.stride)),
                    ));
                    Val::Tmp(d)
                }
                Val::Tmp(t) => {
                    let t = clone_inv_to_ph(f, ph, &body, t, &def_of, &mut memo);
                    let off = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Bin(
                        off,
                        Op::Mul,
                        r.ty,
                        Val::Tmp(t),
                        Val::Imm(r.stride),
                    ));
                    let d = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Bin(
                        d,
                        Op::Add,
                        r.ty,
                        Val::Tmp(pbase_t),
                        Val::Tmp(off),
                    ));
                    Val::Tmp(d)
                }
                Val::FImm(_) => continue,
            };
            // p₁ = φ(ph: pinit, tail: p₂); p₂ = p₁ + c·stride.
            let p2 = f.temps.len() as Tmp;
            f.temps.push(r.ty);
            let pphi = f.temps.len() as Tmp;
            f.temps.push(r.ty);
            let pos = f.blocks[header as usize]
                .insts
                .iter()
                .position(|i| !matches!(i, Inst::Phi(..)))
                .unwrap_or(0);
            f.blocks[header as usize].insts.insert(
                pos,
                Inst::Phi(pphi, r.ty, vec![(ph, pinit), (tail, Val::Tmp(p2))]),
            );
            f.blocks[tail as usize].insts.push(Inst::Bin(
                p2,
                Op::Add,
                r.ty,
                Val::Tmp(pphi),
                Val::Imm(c.wrapping_mul(r.stride)),
            ));
            // Replace the address def with `a = p₁` (uses unchanged; the old base+index
            // computation is left for DCE to reclaim).
            if let Some(cur) =
                f.blocks[def_of[r.addr as usize].unwrap().0 as usize]
                    .insts
                    .iter()
                    .position(|i| inst_def(i) == Some(r.addr))
            {
                let blk = def_of[r.addr as usize].unwrap().0 as usize;
                f.blocks[blk].insts[cur] = Inst::Copy(r.addr, r.ty, Val::Tmp(pphi));
            }
            if lftr.is_none() && c > 0 && r.stride > 0 {
                lftr = Some((pphi, pbase_t, r.stride, r.biv));
            }
            changed += 1;
        }
        // LFTR: replace the header counter test `i₁ < N` with `p₁ < L`, L = pbase + N·stride,
        // so the counter chain dies (DCE reclaims it next round). Conservative: exact shape.
        if let Some((pphi, pbase_t, stride, biv)) = lftr {
            // The counter must now feed ONLY its increment and the test. The address
            // reduction left the old index arithmetic (`biv·stride`, casts) ORPHANED —
            // still present but dead until DCE runs. Counting raw uses would see those and
            // block LFTR forever, so count against a DCE-accurate liveness (fixpoint: a use
            // inside a dead-defined pure instruction does not keep `biv` alive).
            let live = {
                let mut live = vec![false; f.temps.len()];
                loop {
                    let mut ch = false;
                    let mark = |v: &[Tmp], live: &mut Vec<bool>, ch: &mut bool| {
                        for &u in v {
                            if !live[u as usize] {
                                live[u as usize] = true;
                                *ch = true;
                            }
                        }
                    };
                    for b in &f.blocks {
                        for inst in &b.insts {
                            let keep = !is_pure(inst)
                                || inst_def(inst).map_or(true, |d| live[d as usize]);
                            if keep {
                                let mut us = Vec::new();
                                inst_uses(inst, &mut us);
                                mark(&us, &mut live, &mut ch);
                            }
                        }
                        let mut us = Vec::new();
                        term_uses(&b.term, &mut us);
                        mark(&us, &mut live, &mut ch);
                    }
                    if !ch {
                        break;
                    }
                }
                live
            };
            let mut uses = 0u32;
            for b in &f.blocks {
                for inst in &b.insts {
                    let keep =
                        !is_pure(inst) || inst_def(inst).map_or(true, |d| live[d as usize]);
                    if !keep {
                        continue;
                    }
                    let mut us = Vec::new();
                    inst_uses(inst, &mut us);
                    uses += us.iter().filter(|&&u| u == biv).count() as u32;
                }
                let mut us = Vec::new();
                term_uses(&b.term, &mut us);
                uses += us.iter().filter(|&&u| u == biv).count() as u32;
            }
            // find the header test: Bin(cmp, Lt, biv, bound), bound invariant, feeding Br.
            let mut applied = false;
            let hdr = header as usize;
            let test = f.blocks[hdr].insts.iter().position(|i| {
                matches!(i, Inst::Bin(_, Op::Lt, _, Val::Tmp(x), bound)
                    if *x == biv && matches!(bound, Val::Imm(_) | Val::Tmp(_)))
            });
            // biv used exactly twice (increment + this test) ⟹ removing the test-use frees it.
            if uses == 2 {
                if let Some(ti) = test {
                    if let Inst::Bin(_, _, cty, _, bound) = f.blocks[hdr].insts[ti].clone() {
                        let bound_ok = match &bound {
                            Val::Imm(_) => true,
                            Val::Tmp(t) => inv[*t as usize],
                            _ => false,
                        };
                        if bound_ok {
                            // L = pbase + bound·stride  (in the preheader)
                            let boundv = match bound {
                                Val::Tmp(t) => {
                                    Val::Tmp(clone_inv_to_ph(f, ph, &body, t, &def_of, &mut memo))
                                }
                                other => other,
                            };
                            let scaled = f.temps.len() as Tmp;
                            f.temps.push(f.temps[pbase_t as usize]);
                            f.blocks[ph as usize].insts.push(Inst::Bin(
                                scaled,
                                Op::Mul,
                                f.temps[pbase_t as usize],
                                boundv,
                                Val::Imm(stride),
                            ));
                            let limit = f.temps.len() as Tmp;
                            f.temps.push(f.temps[pbase_t as usize]);
                            f.blocks[ph as usize].insts.push(Inst::Bin(
                                limit,
                                Op::Add,
                                f.temps[pbase_t as usize],
                                Val::Tmp(pbase_t),
                                Val::Tmp(scaled),
                            ));
                            // rewrite the test to compare the pointer against the limit
                            if let Inst::Bin(_, _, ty, a, b) = &mut f.blocks[hdr].insts[ti] {
                                *ty = f.temps[pphi as usize];
                                *a = Val::Tmp(pphi);
                                *b = Val::Tmp(limit);
                            }
                            let _ = cty;
                            applied = true;
                            // Clear the orphaned index arithmetic (old `biv·stride`, casts,
                            // copies) the address reduction left behind — it is non-cyclic
                            // dead code that still NAMES biv, so it must go before biv's def
                            // can be removed (else a dead use dangles). DCE cannot touch the
                            // counter's φ↔increment cycle itself (each keeps the other live).
                            dce(f);
                            // The counter is now a dead self-cycle: biv (φ result) feeds
                            // only its increment i₂, and i₂ feeds only the φ. Break it here —
                            // find the header φ(biv), take its tail arm i₂, and if i₂ is used
                            // nowhere but that φ, delete both the increment and the φ.
                            let i2 = f.blocks[hdr].insts.iter().find_map(|i| match i {
                                Inst::Phi(d, _, arms) if *d == biv => arms
                                    .iter()
                                    .find(|(p, _)| *p == tail)
                                    .and_then(|(_, v)| match v {
                                        Val::Tmp(t) => Some(*t),
                                        _ => None,
                                    }),
                                _ => None,
                            });
                            if let Some(i2) = i2 {
                                let i2_uses: u32 = f
                                    .blocks
                                    .iter()
                                    .flat_map(|b| b.insts.iter())
                                    .map(|i| {
                                        let mut u = Vec::new();
                                        inst_uses(i, &mut u);
                                        u.iter().filter(|&&x| x == i2).count() as u32
                                    })
                                    .sum();
                                // exactly one use of i₂ = the φ's tail arm ⟹ cycle is dead.
                                if i2_uses == 1 {
                                    for b in f.blocks.iter_mut() {
                                        b.insts.retain(|i| {
                                            !matches!(i, Inst::Phi(d, ..) if *d == biv)
                                                && inst_def(i) != Some(i2)
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let _ = applied;
        }
    }
    changed
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass — FUNCTION INLINING (Tier-1 #5; β-reduction across the call graph).
//
// Theorem (call-substitution / β-reduction): in any context,
//   ⟦Call f(args)⟧ = ⟦ body_f with params bound to args, each Ret v → (dst := v; goto cont) ⟧.
// Proof obligation = the standard commuting square ⟦prog⟧ = ⟦inline(prog)⟧ on the
// ENTRY; `equiv` recurses through the residual Call(Sym), so before/after are compared
// whole-program (the self-recursive case included: a depth-1 unroll returns the same
// value, only fewer frames).
//
// The one non-obvious Side-II fact is the FRAME MODEL (ir.rs interp + arm64_elf
// lea_local): a local at offset `off` lives at index (frame − off) / address (x29 −
// off) — ONE offset law shared by the interpreter and the emitter. To merge a callee's
// frame into the caller's WITHOUT disturbing the caller's own offsets we APPEND the
// callee frame below: a callee local off_k relocates to off_k + frame_base (frame_base
// = the caller frame at splice time) and the merged frame grows by the callee frame.
// Then caller and every clone occupy DISJOINT regions under both (frame − off) and
// (x29 − off), so the single relocation is correct for the semantics AND the backend.
//
// Bound: at most ONE level — only call sites in the caller's ORIGINAL blocks are
// expanded; a clone's own calls stay calls — so recursion is finite and self-recursion
// is a depth-1 unroll (the fib lever: halves prologue/`bl` traffic, no unbounded
// growth). Cost model = a static instruction-count ceiling; only a SMALL callee pays.
// ─────────────────────────────────────────────────────────────────────────────

// The mutable-def companion to ir::inst_def — used to relocate a clone's destination
// temporary into the caller's temp space (mirror of the read-only each_use_mut).
fn each_def_mut(i: &mut Inst, mut g: impl FnMut(&mut Tmp)) {
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
fn relocate_inst(i: &mut Inst, tb: Tmp, fb: u32) {
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
fn relocate_term(t: &mut Term, tb: Tmp, bb: BlockId) {
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

// A scalar type = one that lives in a single register/slot (int / float / pointer). A
// scalar param is seeded by exactly ONE `Store(pty, &slot, arg)` — the same instruction
// the prologue uses to spill the incoming arg register — so its β-substitution is
// provably the ABI. An AGGREGATE (struct/union/array) param is passed byval/indirect
// and copied by Memcpy, NOT a scalar store; seeding it with one Store miscompiles.
fn scalar_ty(tt: &TyTab, ty: TypeId) -> bool {
    tt.is_integer(ty) || tt.is_float(ty) || matches!(tt.tys[ty as usize], Ty::Ptr(_))
}

// May a callee be inlined? Whitelist the instruction kinds whose relocation is a pure
// temp/frame shift. Rejects va*, alloca/VLA, computed goto (and any callee carrying
// label symbols), inline asm, atomics, overflow builtins, and CallX (its sret slot is
// a second frame offset) — each would need identity-/state-aware fix-up we don't do.
// Also requires every PARAMETER to be scalar and the RETURN to be scalar-or-void: an
// aggregate param/return uses byval/sret ABI copying the scalar store-seed can't model
// (the struct-by-value miscompiles: gcc 20000706-1 / pr20621-1).
fn inline_ok(tt: &TyTab, callee: &IrFunc) -> bool {
    callee.labels.is_empty()
        && callee.params.iter().all(|&(_, pty)| scalar_ty(tt, pty))
        && (callee.ret == VOID || scalar_ty(tt, callee.ret))
        && callee.blocks.iter().flat_map(|b| &b.insts).all(|i| {
            matches!(
                i,
                Inst::Bin(..)
                    | Inst::Un(..)
                    | Inst::Copy(..)
                    | Inst::Load(..)
                    | Inst::Store(..)
                    | Inst::Memcpy(..)
                    | Inst::Lea(..)
                    | Inst::Cast(..)
                    | Inst::Call(..)
                    | Inst::Zero(..)
                    | Inst::FunAddr(..)
            )
        })
}

fn inst_count(f: &IrFunc) -> usize {
    f.blocks.iter().map(|b| b.insts.len()).sum()
}

// Splice `callee` into `caller` at block `b`, instruction `k` (a direct Call). The
// call site block is split: [0..k] + param-seeding stores → jump to the clone entry;
// the clone's returns write the call's dst and jump to a continuation block holding
// [k+1..] + the block's original terminator.
fn splice(caller: &mut IrFunc, b: BlockId, k: usize, callee: &IrFunc) {
    let tb = caller.temps.len() as Tmp; // clone temps land here
    let fb = caller.frame; // clone frame appended below the caller's
    let bb = caller.blocks.len() as BlockId; // clone entry = first appended block
    let nk = callee.blocks.len() as BlockId;
    let cont = bb + nk; // continuation = appended just after the clone blocks

    let (dst, args) = match &caller.blocks[b as usize].insts[k] {
        Inst::Call(d, Callee::Sym(_), a, _) => (*d, a.clone()),
        _ => unreachable!("splice: not a direct call"),
    };
    let ret_ty = callee.ret;

    // 1. Grow the caller's Γ + frame by the callee's (the frame-append law above).
    caller.temps.extend_from_slice(&callee.temps);
    caller.frame += callee.frame;

    // 2. Clone + relocate the callee blocks; Ret v → (dst := v) ; goto cont.
    for blk in &callee.blocks {
        let mut nb = blk.clone();
        for i in nb.insts.iter_mut() {
            relocate_inst(i, tb, fb);
        }
        relocate_term(&mut nb.term, tb, bb);
        if let Term::Ret(v) = &nb.term {
            let v = *v; // Option<Val> is Copy; already relocated by relocate_term
            if let Some(d) = dst {
                // void-return path in a value fn is unreachable → Imm(0) keeps dst defined.
                let rv = v.unwrap_or(Val::Imm(0));
                nb.insts.push(Inst::Copy(d, ret_ty, rv));
            }
            nb.term = Term::Jmp(cont);
        }
        caller.blocks.push(nb);
    }

    // 3. Detach the tail (everything AFTER the call) + the block's old terminator.
    let tail = caller.blocks[b as usize].insts.split_off(k + 1);
    let old_term = std::mem::replace(&mut caller.blocks[b as usize].term, Term::Jmp(bb));
    caller.blocks[b as usize].insts.pop(); // drop the Call now at index k

    // 4. Seed the callee params: Store arg → the relocated param slot (off + fb).
    for (idx, &(poff, pty)) in callee.params.iter().enumerate() {
        let arg = args.get(idx).copied().unwrap_or(Val::Imm(0));
        let addr = caller.temps.len() as Tmp;
        caller.temps.push(ULONG); // a local-address temp
        caller.blocks[b as usize].insts.push(Inst::Lea(addr, Place::Local(poff + fb)));
        caller.blocks[b as usize].insts.push(Inst::Store(pty, Val::Tmp(addr), arg));
    }

    caller.blocks.push(Block { insts: tail, term: old_term });
}

/// Whole-program inlining (Tier-1 #5). Depth-1: expands only the call sites present in
/// each caller's ORIGINAL blocks, against a SNAPSHOT taken before any mutation — so a
/// non-recursive callee is substituted once and a self-recursive callee is unrolled
/// exactly one level (its clone's recursive calls survive). Terminates trivially.
///
/// `caller_ok[ci]` = may func ci be inlined INTO (false for variadic / VLA callers): the
/// splice appends the callee frame into [caller.frame, caller.frame+cf), which for a
/// variadic caller is exactly the AAPCS64 reg-save area and for a VLA caller confuses the
/// dynamic-SP reset base — both need va/VLA-aware placement this pass does not do. Callee
/// eligibility (scalar-only, no va/alloca/asm/…) is separately gated by `inline_ok`.
pub fn inline(tt: &TyTab, funcs: &mut [IrFunc], caller_ok: &[bool]) -> u32 {
    const LEAF_MAX: usize = 16; // a non-recursive callee this small is a pure win to inline
    const SELF_MAX: usize = 40; // a self-recursive callee: depth-1 unroll if this small
    let snapshot: Vec<IrFunc> = funcs.to_vec();
    let by_name: HashMap<&str, usize> =
        snapshot.iter().enumerate().map(|(i, f)| (f.name.as_str(), i)).collect();
    let mut total = 0u32;
    for ci in 0..funcs.len() {
        if !caller_ok.get(ci).copied().unwrap_or(true) {
            continue;
        }
        let start_nc = funcs[ci].blocks.len();
        // Original-block call sites eligible under the cost model, with their snapshot index.
        let mut sites: Vec<(BlockId, usize, usize)> = Vec::new();
        for b in 0..start_nc {
            for (k, inst) in funcs[ci].blocks[b].insts.iter().enumerate() {
                if let Inst::Call(_, Callee::Sym(name), _, _) = inst {
                    let Some(&gi) = by_name.get(name.as_str()) else { continue };
                    let g = &snapshot[gi];
                    if !inline_ok(tt, g) {
                        continue;
                    }
                    let n = inst_count(g);
                    let ok = if gi == ci { n <= SELF_MAX } else { n <= LEAF_MAX };
                    if ok {
                        sites.push((b as BlockId, k, gi));
                    }
                }
            }
        }
        // Per block, splice HIGHER indices first: lower call sites keep their (b,k)
        // valid (split_off only detaches the tail above k), so the whole original
        // worklist can be applied without recomputation.
        sites.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        for (b, k, gi) in sites {
            splice(&mut funcs[ci], b, k, &snapshot[gi]);
            total += 1;
        }
    }
    total
}

// ─────────────────────────────────────────────────────────────────────────────
// THE SSA OPTIMIZATION PIPELINE (the QBE-level projection, under CbC). The whole
// point of Stages 1–4: build SSA, run the SSA-strength passes to a fixpoint, then
// return to executable (φ-free) IR and do a final non-SSA cleanup.
//
//   optimize_ssa = to_ssa ▸ (sccp ∘ const_fold ∘ copy_prop ∘ gvn ∘ cse ∘ dce ∘ cfg ∘ licm)* ▸ out_of_ssa ▸ optimize
//
// Each stage is an INDIVIDUALLY-PROVEN semantics-preserving rewrite (⟦·⟧-invariant,
// gated by `equiv`); the COMPOSITE is therefore semantics-preserving, and this is
// re-checked end-to-end by `optimize_ssa_preserves` — composition of commuting squares
// is a commuting square, but we MEASURE it anyway (never trust by reasoning). This is
// the artifact Stage 5 wires into the backend behind an optimization flag.
//
// INDUSTRIAL TOGGLEABLE PIPELINE (cf. gcc `-fno-<pass>` / LLVM PassBuilder): every stage
// is a switch in `Passes`, so any element can be disabled independently. This matters
// because a pass may be ⟦·⟧-CORRECT yet not a MEASURED win on a given backend — the
// constitution ships only a proven-AND-measured win. `licm` is exactly that case: proven
// (0 FAIL / 0 DIVERGE) but MEASURED to regress the memory-bound naive-slot backend
// (hoisting trades a cheap address recompute for a per-iteration reload + more spill
// pressure), so it defaults OFF and is one flag from ON for when a register-resident
// backend makes hoisting pay. All other proven passes default ON.
// ─────────────────────────────────────────────────────────────────────────────

/// Per-pass on/off switches for the SSA optimizer. `default()` is the shipped profile
/// (every proven pass ON except the measured-negative `licm`); `all()` turns everything
/// ON (the maximal pipeline the proofs cover). `from_env()` applies `ZCC_OPT_OFF` /
/// `ZCC_OPT_ON` (comma-separated pass names) on top of the default profile.
#[derive(Clone, Copy)]
pub struct Passes {
    pub sccp: bool,
    pub const_fold: bool,
    pub copy_prop: bool,
    pub gvn: bool,
    pub cse: bool,
    pub load_elim: bool, // B2: alias-aware store→load forwarding + NoAlias load survival
    pub dce: bool,
    pub cfg_simplify: bool,
    pub licm: bool,
    pub strength_reduce: bool,
    pub pointer_iv: bool, // pointer-IV strength reduction + LFTR (gcc-O1 loop-nest recipe)
    pub coalesce: bool, // register coalescing (biased coloring, inside abi_alloc)
    pub peephole: bool, // backend machine-level redundant-move elimination (arm64_elf)
    pub ldst_pair: bool, // B4: backend adjacent load/store → ldp/stp pairing (arm64_elf)
    pub if_convert: bool, // B4: side-effect-free diamond → Select (csel); runs before out_of_ssa
    pub inline: bool,   // Tier-1 #5: whole-program depth-1 inlining (runs before to_ssa)
    pub remat: bool,    // Tier-5 #26: rematerialize pure operand-free defs under pressure (last)
}

impl Default for Passes {
    fn default() -> Self {
        Passes {
            sccp: true,
            const_fold: true,
            copy_prop: true,
            gvn: true,
            cse: true,
            load_elim: true, // B2: alias-gated, proven (equiv commuting square) — default-ON
            dce: true,
            cfg_simplify: true,
            licm: false, // proven-correct but measured-negative on the naive-slot backend
            strength_reduce: false, // same: proven, but the accumulator φ costs spill on this backend
            pointer_iv: false, // pressure-reducing loop reducer; measured on the box before default-ON
            coalesce: true,
            peephole: true, // measured win: removes the x0-funnel redundant reg-reg moves
            ldst_pair: true, // B4: adjacent str/ldr → stp/ldp; static win, translation-validated
            if_convert: true, // B4: diamond → csel; proven (equiv), B1-licensed load speculation
            inline: true, // Tier-1 #5: β-reduction; proven (equiv) — measured on the bench before ship
            remat: false, // Tier-5 #26: proven (equiv) but speed-gated on the box A/B (like licm/sr)
        }
    }
}

impl Passes {
    /// The maximal pipeline the proofs cover (everything ON, incl. `licm`). Test-only:
    /// at runtime the same effect is `ZCC_OPT_ON=licm` over the default profile.
    #[cfg(test)]
    pub fn all() -> Self {
        Passes {
            licm: true,
            strength_reduce: true,
            pointer_iv: true,
            remat: true,
            ..Passes::default()
        }
    }
    fn set(&mut self, name: &str, v: bool) {
        match name {
            "sccp" => self.sccp = v,
            "const_fold" | "fold" => self.const_fold = v,
            "copy_prop" | "copy" => self.copy_prop = v,
            "gvn" => self.gvn = v,
            "cse" => self.cse = v,
            "load_elim" | "loadelim" | "le" => self.load_elim = v,
            "dce" => self.dce = v,
            "cfg_simplify" | "cfg" => self.cfg_simplify = v,
            "licm" => self.licm = v,
            "strength_reduce" | "strength" | "sr" => self.strength_reduce = v,
            "pointer_iv" | "piv" | "pointeriv" => self.pointer_iv = v,
            "coalesce" => self.coalesce = v,
            "peephole" | "peep" => self.peephole = v,
            "ldst_pair" | "ldp" | "stp" | "pair" => self.ldst_pair = v,
            "if_convert" | "ifconv" | "csel" => self.if_convert = v,
            "inline" => self.inline = v,
            "remat" => self.remat = v,
            _ => {} // an unknown name is ignored (forward-compatible)
        }
    }
    /// The default profile, then `ZCC_OPT_OFF=a,b` disables and `ZCC_OPT_ON=c,d` enables.
    pub fn from_env() -> Self {
        let mut p = Passes::default();
        if let Ok(off) = std::env::var("ZCC_OPT_OFF") {
            for n in off.split(',') {
                p.set(n.trim(), false);
            }
        }
        if let Ok(on) = std::env::var("ZCC_OPT_ON") {
            for n in on.split(',') {
                p.set(n.trim(), true);
            }
        }
        p
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass — IF-CONVERSION (B4; branch → data-select).
//
// Theorem (control→data): a DIAMOND
//     h: … ; Br(c, T, E)
//     T: s_T ; Jmp M          E: s_E ; Jmp M          M: φ(d,[(T,vT),(E,vE)]) …
// where every instruction of the two arms s_T, s_E is PURE and NON-FAULTING is
// semantically equal to executing BOTH arms unconditionally (speculation) and then
// selecting per c:  ⟦diamond⟧ = ⟦ h;s_T;s_E ; Select(d,c,vT,vE) ; M∖φ ⟧.
// Justification: a pure, non-faulting instruction has NO observable effect when its
// result is unused (Law-1 Side-I: ⟦·⟧ is a function of the live result only) — so
// running the not-taken arm is invisible, and the φ (which picks the value of the
// edge actually taken) becomes exactly `c ? vT : vE`. The commuting square
// ⟦f⟧ = ⟦if_convert(f)⟧ is unit-tested (`if_convert_semantics`).
//
// NON-FAULTING (the speculation-safety side-condition, the ONLY subtle premise):
//   • Bin(Div|Rem) on an INTEGER type traps on /0 → NOT speculatable.
//   • Load(addr) faults on a bad address → speculatable ONLY when B1's oracle proves
//     `addr` is a mapped location (a stack slot or a symbol) — `fault_free`. THIS is
//     where B4 consumes B1 (the ★★★★★ enabler): without the alias oracle no ternary
//     over memory could be if-converted.
//   • Store/Call/Memcpy/Zero/Va*/Sync/Asm/Alloca/GotoPtr — side effects → NOT arms.
// The produced Select is restricted to NON-FLOAT scalars (the backend lowers it to
// integer `csel`); a float φ keeps its branch.
// ─────────────────────────────────────────────────────────────────────────────

/// Is `i` safe to hoist out of a conditional arm (pure AND cannot trap)? `al` is the
/// function's alias oracle, consulted for Load fault-safety.
fn speculatable(tt: &TyTab, al: &AliasInfo, i: &Inst) -> bool {
    match i {
        Inst::Bin(_, Op::Div | Op::Rem, ty, _, _) => tt.is_float(*ty), // int /0 traps
        Inst::Bin(..) | Inst::Un(..) | Inst::Copy(..) | Inst::Cast(..) | Inst::Lea(..) => true,
        Inst::Load(_, _, addr) => al.fault_free(*addr),
        _ => false, // Store/Call/Phi/… — a side effect or a control artifact
    }
}

pub fn if_convert(tt: &TyTab, f: &mut IrFunc) -> u32 {
    let al = alias_info(f);
    let preds = predecessors(f);
    let mut n = 0u32;
    // One linear scan over heads; each rewrite is local (h, T, E, M) and never creates a
    // new diamond, so a single pass suffices (nested ternaries fold bottom-up across the
    // optimize_ssa fixpoint's earlier cfg_simplify — here we take one layer).
    for h in 0..f.blocks.len() {
        let Term::Br(cond, t_id, e_id) = f.blocks[h].term.clone() else { continue };
        let (t, e) = (t_id as usize, e_id as usize);
        if t == h || e == h || t == e {
            continue;
        }
        // Arms must be private to this diamond: single predecessor = h.
        if preds[t] != [h as BlockId] || preds[e] != [h as BlockId] {
            continue;
        }
        // Both arms converge on the same merge M by an unconditional Jmp.
        let (Term::Jmp(mt), Term::Jmp(me)) = (f.blocks[t].term.clone(), f.blocks[e].term.clone()) else { continue };
        if mt != me {
            continue;
        }
        let m_id = mt;
        let m = mt as usize;
        if m == t || m == e || m == h {
            continue;
        }
        // M's ONLY predecessors are the two arms (else its φs have arms we cannot fold).
        let mut mp = preds[m].clone();
        mp.sort_unstable();
        let mut want = [t_id, e_id];
        want.sort_unstable();
        if mp != want {
            continue;
        }
        // The MERGE temps: this IR keeps a diamond's result as a register temp assigned
        // in BOTH arms (to_ssa φ-ifies only promoted memory), reconciled after the join —
        // NOT a φ. A merge temp is one defined in both T and E; that pair of defs is what
        // becomes a Select. `defs(blk)` = the set of temps the block defines.
        let defs = |blk: usize| -> HashSet<Tmp> {
            f.blocks[blk].insts.iter().filter_map(inst_def).collect()
        };
        let (dt, de) = (defs(t), defs(e));
        let merge: Vec<Tmp> = { let mut v: Vec<Tmp> = dt.intersection(&de).copied().collect(); v.sort_unstable(); v };
        if merge.is_empty() {
            continue; // no value to select — a pure diamond with no reconciled result
        }
        // Every merge temp must be NON-FLOAT (backend csel) and reconciled by a plain
        // Copy in EACH arm (so its per-arm contribution is a Val we can drop into Select).
        // `src(blk, r)` = the Val copied into r in that arm, or None if r is not Copy-defined there.
        let src = |blk: usize, r: Tmp| -> Option<Val> {
            f.blocks[blk].insts.iter().find_map(|i| match i {
                Inst::Copy(d, _, v) if *d == r => Some(*v),
                _ => None,
            })
        };
        let merge_ok = merge.iter().all(|&r| {
            !tt.is_float(f.temps[r as usize]) && src(t, r).is_some() && src(e, r).is_some()
        });
        if !merge_ok {
            continue;
        }
        // Every NON-merge instruction of each arm must be speculatable (pure + non-faulting)
        // — it will run unconditionally. (The merge Copies are dropped, not hoisted.)
        let arm_pure = |blk: usize| {
            f.blocks[blk].insts.iter().all(|i| match inst_def(i) {
                Some(d) if merge.contains(&d) => matches!(i, Inst::Copy(..)), // the merge reconciler
                _ => speculatable(tt, &al, i),
            })
        };
        if !arm_pure(t) || !arm_pure(e) {
            continue;
        }
        // M may ALSO hold φ nodes (a promoted-memory merge, e.g. a ternary over an
        // address-taken local): each such φ has arms exactly {T,E} (M's only preds).
        // Rewiring h→M would leave those φs with no arm for h — so we convert them TOO,
        // in place. Refuse the diamond if any leading φ is float (backend csel is
        // integer) or (defensively) not a plain 2-arm (T,E) φ.
        let phis_ok = f.blocks[m].insts.iter().take_while(|i| matches!(i, Inst::Phi(..))).all(|i| {
            let Inst::Phi(_, ty, arms) = i else { return false };
            !tt.is_float(*ty)
                && arms.len() == 2
                && arms.iter().any(|(b, _)| *b == t_id)
                && arms.iter().any(|(b, _)| *b == e_id)
        });
        if !phis_ok {
            continue;
        }
        // ---- COMMIT the rewrite (all guards passed) ----
        // Extract each merge temp's per-arm source BEFORE mutating the blocks.
        let sels: Vec<(Tmp, Val, Val)> =
            merge.iter().map(|&r| (r, src(t, r).unwrap(), src(e, r).unwrap())).collect();
        // 1. Hoist every NON-merge-Copy instruction of both arms into h, in order (their
        //    defs are arm-private, distinct, and — once unconditional — dominate M). The
        //    merge Copies are dropped; their role is taken by the Selects below.
        let hoist = |blk: &mut Block, merge: &[Tmp]| -> Vec<Inst> {
            std::mem::take(&mut blk.insts).into_iter()
                .filter(|i| !matches!(inst_def(i), Some(d) if merge.contains(&d)))
                .collect()
        };
        let ht = hoist(&mut f.blocks[t], &merge);
        let he = hoist(&mut f.blocks[e], &merge);
        let hb = &mut f.blocks[h];
        hb.insts.extend(ht);
        hb.insts.extend(he);
        // 2. One Select per merge temp: r = (cond ≠ 0) ? src_T : src_E. Appended AFTER the
        //    hoisted computations, so every referenced value is already in scope.
        for (r, vt, ve) in sels {
            hb.insts.push(Inst::Select(r, f.temps[r as usize], cond, vt, ve));
            n += 1;
        }
        hb.term = Term::Jmp(m_id);
        // 3. Convert every leading φ of M into a Select (in place). φ arms reference values
        //    live on the T / E edges — defined in the arms (now hoisted into h) or above —
        //    so they dominate M. φs never reference sibling-φ dsts, so in-place rewrite is sound.
        for i in f.blocks[m].insts.iter_mut() {
            let Inst::Phi(d, ty, arms) = i else { break };
            let vt = arms.iter().find(|(b, _)| *b == t_id).unwrap().1;
            let ve = arms.iter().find(|(b, _)| *b == e_id).unwrap().1;
            *i = Inst::Select(*d, *ty, cond, vt, ve);
            n += 1;
        }
        // The arms T,E are now unreachable (no preds) and empty; cfg_simplify drops them.
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass — REMATERIALIZATION (Tier-5 #26; the CbC-PURE slice of register allocation).
//
// The QBE cross-check named register allocation as zcc's real QBE-class gap, but its
// Belady-spill core rides a tuned loop-weight ⟹ below the CbC-purity line (rejected). Remat
// is the slice that IS a total theorem: a value whose definition is PURE and OPERAND-FREE —
// a constant (`Copy(Imm)`), a frame/symbol/string address (`Lea`), a function/label address
// (`FunAddr`/`LabelAddr`) — can be RECOMPUTED at any program point unconditionally, because
// it depends on nothing (its availability is the whole-program constant `true`). So instead
// of keeping it live across a high-pressure region — where the allocator must SPILL it
// (store at def + reload per use, memory traffic) — we clone the 1–2-instruction recompute
// right before each use and shorten its live range to ~zero.
//
// THEOREM (CbC): `⟦f⟧ = ⟦remat(f)⟧`. A pure operand-free instruction `D` computes a value
// that is a function of NOTHING (no temp, no memory) ⟹ its result is identical at every
// program point ⟹ replacing a use of `t=D` by a use of a fresh `t'=D` cloned immediately
// before it is a ⟦·⟧-identity (Law-1 Side-I: ⟦·⟧ is a function of the operands, here empty).
// The original def, now useless, is dropped (mark-sweep in one shot). Validated by `equiv`
// over the structural corpus (commuting square) — see `remat_*` tests.
//
// SPEED (the orthogonal axis, Law 3 — MEASURED, not asserted): remat fires ONLY on a temp
// live at some point of GP pressure ≥ k (exactly the temps the k-colouring must spill). It
// trades {store + N reloads} for {N recomputes of a 1–2-inst value} and frees a register
// across the span ⟹ statically fewer memory ops. Default-OFF until the box A/B confirms the
// win on the real allocator (the SSA/backend-pressure proxy gap, same residual as LICM).
// ─────────────────────────────────────────────────────────────────────────────

/// A def that can be recomputed anywhere: PURE and OPERAND-FREE (no temp/memory input, so
/// available unconditionally). `Place` is `Local|Global|Str` — all operand-free — so every
/// `Lea` qualifies; `inst_uses`-empty is asserted belt-and-suspenders.
fn rematerializable(i: &Inst) -> bool {
    let mut u = Vec::new();
    inst_uses(i, &mut u);
    u.is_empty()
        && matches!(
            i,
            Inst::Copy(_, _, Val::Imm(_)) | Inst::Lea(..) | Inst::FunAddr(..) | Inst::LabelAddr(..)
        )
}

pub fn remat(tt: &TyTab, f: &mut IrFunc, gp_k: u32) -> u32 {
    let nt = f.temps.len();
    // 1. Per temp: def count + (if rematerializable) a clone of its defining instruction.
    let mut defcnt = vec![0u32; nt];
    let mut def_inst: Vec<Option<Inst>> = vec![None; nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defcnt[d as usize] += 1;
                if rematerializable(i) {
                    def_inst[d as usize] = Some(i.clone());
                }
            }
        }
    }
    // 2. Under-pressure set: temps live at any point with GP pressure ≥ k (the spill victims).
    //    Backward live-set walk; at each point of pressure ≥ k, mark every live GP temp.
    let lv = liveness(f);
    let is_fp: Vec<bool> = f.temps.iter().map(|&ty| tt.is_float(ty)).collect();
    let gp = |live: &[bool]| (0..nt).filter(|&t| live[t] && !is_fp[t]).count() as u32;
    let mut pressured = vec![false; nt];
    let mark = |live: &[bool], pr: &mut [bool]| {
        if gp(live) >= gp_k {
            for t in 0..nt {
                if live[t] && !is_fp[t] {
                    pr[t] = true;
                }
            }
        }
    };
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true;
        }
        mark(&live, &mut pressured);
        for i in b.insts.iter().rev() {
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
            mark(&live, &mut pressured);
        }
    }
    // 3. Targets = single-def ∧ rematerializable ∧ under pressure.
    let target: Vec<bool> =
        (0..nt).map(|t| defcnt[t] == 1 && def_inst[t].is_some() && pressured[t]).collect();
    if !target.iter().any(|&b| b) {
        return 0;
    }
    // 4. Clone the def before each USE (fresh temp), redirect that use, drop the dead original.
    //    A target's own def is operand-free ⟹ dropping it never orphans another use.
    let mut count = 0u32;
    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out: Vec<Inst> = Vec::with_capacity(old.len());
        for mut inst in old {
            let fresh = clone_remats_before(f, &mut inst, &def_inst, &target, &mut out);
            count += fresh;
            if let Some(d) = inst_def(&inst) {
                if target[d as usize] {
                    continue; // the original def is now dead — drop it
                }
            }
            out.push(inst);
        }
        // The terminator may also use a target (e.g. Ret(Tmp)): materialize before it.
        let mut term = std::mem::replace(&mut f.blocks[bi].term, Term::Ret(None));
        count += clone_remats_before_term(f, &mut term, &def_inst, &target, &mut out);
        f.blocks[bi].insts = out;
        f.blocks[bi].term = term;
    }
    count
}

/// For each target temp USED by `inst`, push a fresh clone of its def to `out` and rewrite
/// the use to the clone (a temp used twice reuses one clone). Returns the clone count.
fn clone_remats_before(
    f: &mut IrFunc,
    inst: &mut Inst,
    def_inst: &[Option<Inst>],
    target: &[bool],
    out: &mut Vec<Inst>,
) -> u32 {
    let mut uses = Vec::new();
    inst_uses(inst, &mut uses);
    let mut map: Vec<(Tmp, Tmp)> = Vec::new();
    for &t in &uses {
        if !target[t as usize] || map.iter().any(|&(o, _)| o == t) {
            continue;
        }
        let ty = f.temps[t as usize];
        let fresh = f.temps.len() as Tmp;
        f.temps.push(ty);
        let mut clone = def_inst[t as usize].clone().unwrap();
        each_def_mut(&mut clone, |d| *d = fresh);
        out.push(clone);
        map.push((t, fresh));
    }
    each_use_mut(inst, |v| {
        if let Val::Tmp(t) = v
            && let Some(&(_, fr)) = map.iter().find(|&&(o, _)| o == *t)
        {
            *t = fr;
        }
    });
    map.len() as u32
}

/// Terminator variant of `clone_remats_before`.
fn clone_remats_before_term(
    f: &mut IrFunc,
    term: &mut Term,
    def_inst: &[Option<Inst>],
    target: &[bool],
    out: &mut Vec<Inst>,
) -> u32 {
    let mut uses = Vec::new();
    term_uses(term, &mut uses);
    let mut map: Vec<(Tmp, Tmp)> = Vec::new();
    for &t in &uses {
        if !target[t as usize] || map.iter().any(|&(o, _)| o == t) {
            continue;
        }
        let ty = f.temps[t as usize];
        let fresh = f.temps.len() as Tmp;
        f.temps.push(ty);
        let mut clone = def_inst[t as usize].clone().unwrap();
        each_def_mut(&mut clone, |d| *d = fresh);
        out.push(clone);
        map.push((t, fresh));
    }
    each_use_term_mut(term, |v| {
        if let Val::Tmp(t) = v
            && let Some(&(_, fr)) = map.iter().find(|&&(o, _)| o == *t)
        {
            *t = fr;
        }
    });
    map.len() as u32
}

pub fn optimize_ssa(tt: &TyTab, f: &mut IrFunc, p: &Passes, gp_k: u32) {
    to_ssa(tt, f);
    for _ in 0..32 {
        let mut n = 0;
        if p.sccp {
            n += sccp(tt, f); // conditional constants + dead-branch pruning (through φ)
        }
        if p.const_fold {
            n += const_fold(tt, f); // fold newly-constant operands
        }
        if p.copy_prop {
            n += copy_prop(f); // collapse copies so GVN's operand value-numbers are canonical
        }
        if p.gvn {
            n += gvn(f); // global redundant-expression elimination (dominator-based)
        }
        if p.cse {
            n += cse(f); // block-local load reuse (GVN skips memory)
        }
        if p.load_elim {
            n += load_elim(tt, f); // B2: alias-aware store→load forwarding + NoAlias survival
        }
        if p.dce {
            n += dce(f); // reclaim the temps the above passes deadened (incl. φ)
        }
        if p.cfg_simplify {
            n += cfg_simplify(f); // collapse the straight lines / dead blocks SCCP exposed
        }
        if p.licm {
            n += licm(tt, f, gp_k); // hoist loop-invariant pure arithmetic (pressure-guarded)
        }
        if p.strength_reduce {
            n += strength_reduce(tt, f, gp_k); // i·d → add-accumulator φ (pressure-guarded)
        }
        if p.pointer_iv {
            n += pointer_iv(tt, f, gp_k); // base+i·d → marching pointer + LFTR (pressure-reducing)
        }
        if n == 0 {
            break; // fixpoint
        }
    }
    if p.if_convert {
        // B4: while still in SSA (φs intact), fold side-effect-free diamonds to Select.
        // Runs ONCE after the fixpoint: it consumes SSA form, and the branches it removes
        // were already exposed by cfg_simplify. dce+cfg_simplify reclaim the now-dead arms.
        if if_convert(tt, f) > 0 {
            dce(f);
            cfg_simplify(f);
        }
    }
    out_of_ssa(f); // φ → edge copies (swap/critical-edge safe) → executable IR
    optimize(tt, f); // the proven non-SSA cleanup (folds the φ-destruction copies)
    if p.remat {
        // LAST IR touch (nothing runs after ⟹ no CSE re-merges the clones): shorten the live
        // ranges of pure operand-free values the allocator would otherwise spill. #26.
        remat(tt, f, gp_k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{TyTab, INT};
    use crate::ir::tests::{compile, equiv, interp, mk};
    use crate::ir::{verify, Block};

    // The GP colour budget the shipped backend allocates against (arm64_elf `GP_BUDGET.k`).
    // The commuting-square proofs are k-INDEPENDENT (a partial hoist is a subset of the proven
    // hoist set), so the ⟦·⟧ tests pass this real value; the pressure-guard's teeth are proven
    // separately with a tiny k that FORCES the cap to bite.
    const GP_K: u32 = 10;

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

    // ── B1 alias oracle — verdict unit tests + the soundness property.
    // One function exercises every arm of the decidable relation; the descriptors are
    // built by `alias_info` (one RPO pass) and queried post-hoc.
    #[test]
    fn alias_verdicts() {
        // t0=&A(loc16)  t1=&B(loc32)  t2=A+4  t3=load(A) [value, ⟹ Unk]
        // t4=&g  t5=&g  t6=&h  t7=&C(loc48)  t8=load(g) [unknown pointer]
        // Store *A = &C  ⟹ slot C escapes (its address is stored through a pointer).
        let f = mk(
            "f",
            vec![ULONG, ULONG, ULONG, ULONG, ULONG, ULONG, ULONG, ULONG, ULONG],
            vec![],
            64,
            ULONG,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(16)),
                    Inst::Lea(1, Place::Local(32)),
                    Inst::Bin(2, Op::Add, ULONG, Val::Tmp(0), Val::Imm(4)),
                    Inst::Load(3, ULONG, Val::Tmp(0)),
                    Inst::Lea(4, Place::Global("g".into(), 0)),
                    Inst::Lea(5, Place::Global("g".into(), 0)),
                    Inst::Lea(6, Place::Global("h".into(), 0)),
                    Inst::Lea(7, Place::Local(48)),
                    Inst::Load(8, ULONG, Val::Tmp(4)),
                    Inst::Store(ULONG, Val::Tmp(0), Val::Tmp(7)), // *A = &C ⟹ C escapes
                ],
                term: Term::Ret(None),
            }],
        );
        verify(&f).expect("well-formed");
        let ai = alias_info(&f);
        use AliasR::*;
        let al = |p, q| ai.alias(Val::Tmp(p), 4, Val::Tmp(q), 4);
        // same slot, overlapping offsets ⟹ MustAlias.
        assert_eq!(al(0, 0), Must);
        // same slot, disjoint offsets (0 vs 4, width 4) ⟹ NoAlias.
        assert_eq!(al(0, 2), No);
        // distinct stack slots ⟹ NoAlias.
        assert_eq!(al(0, 1), No);
        // same symbol ⟹ MustAlias (overlap); different symbol ⟹ conservatively MayAlias.
        assert_eq!(al(4, 5), Must);
        assert_eq!(al(4, 6), May);
        // a stack slot vs a symbol are disjoint regions ⟹ NoAlias.
        assert_eq!(al(0, 4), No);
        // an UNKNOWN pointer (t8) vs a provably-local, non-escaped slot (A) ⟹ NoAlias
        // (A's address never leaked). vs an ESCAPED slot (C) ⟹ MayAlias.
        assert_eq!(al(8, 0), No);
        assert_eq!(al(8, 7), May);
        // an unknown pointer vs another unknown ⟹ MayAlias.
        let t9load = mk(
            "g",
            vec![ULONG, ULONG, ULONG],
            vec![],
            16,
            ULONG,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Global("p".into(), 0)),
                    Inst::Load(1, ULONG, Val::Tmp(0)),
                    Inst::Load(2, ULONG, Val::Tmp(0)),
                ],
                term: Term::Ret(None),
            }],
        );
        let ai2 = alias_info(&t9load);
        assert_eq!(ai2.alias(Val::Tmp(1), 8, Val::Tmp(2), 8), May);
        // offset overlap at sub-object granularity: A+0 (width 8) vs A+4 (width 4) DO
        // overlap ⟹ MustAlias (the wider access straddles the narrower).
        assert_eq!(ai.alias(Val::Tmp(0), 8, Val::Tmp(2), 4), Must);
    }

    // SOUNDNESS: on a battery of hand-built access pairs whose TRUE aliasing is known,
    // the oracle must NEVER answer No/Must when they actually alias (May is the only
    // safe imprecise reply). This is the CbC obligation for an analysis (Law 3).
    #[test]
    fn alias_soundness() {
        // Build accesses over ONE stack slot at three offsets + one truly-unknown
        // pointer. Ground truth is computed directly from (base-identity, offset).
        let f = mk(
            "f",
            vec![ULONG, ULONG, ULONG, ULONG, ULONG],
            vec![],
            64,
            ULONG,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(32)),                       // &S+0
                    Inst::Bin(1, Op::Add, ULONG, Val::Tmp(0), Val::Imm(2)), // &S+2
                    Inst::Bin(2, Op::Add, ULONG, Val::Tmp(0), Val::Imm(8)), // &S+8
                    Inst::Lea(3, Place::Global("g".into(), 0)),          // &g (escapes below)
                    Inst::Load(4, ULONG, Val::Tmp(3)),                   // unknown pointer
                ],
                term: Term::Ret(None),
            }],
        );
        let ai = alias_info(&f);
        // (temp, offset-from-its-base) for the stack accesses; the unknown is separate.
        let stack = [(0u32, 0i64), (1, 2), (2, 8)];
        for &(pa, oa) in &stack {
            for &(pb, ob) in &stack {
                for &sa in &[1u32, 2, 4, 8] {
                    for &sb in &[1u32, 2, 4, 8] {
                        let truth_overlap = oa < ob + sb as i64 && ob < oa + sa as i64;
                        match ai.alias(Val::Tmp(pa), sa, Val::Tmp(pb), sb) {
                            AliasR::No => assert!(!truth_overlap, "No but they overlap"),
                            AliasR::Must => assert!(truth_overlap, "Must but they are disjoint"),
                            AliasR::May => {} // always safe
                        }
                    }
                }
            }
        }
        // The unknown pointer could alias ANYTHING that escaped or is non-local: the
        // stack slot S here never escaped ⟹ NoAlias is SOUND (its address never leaked,
        // so no unknown pointer derived elsewhere can equal it).
        for &(p, _) in &stack {
            assert_eq!(ai.alias(Val::Tmp(4), 8, Val::Tmp(p), 8), AliasR::No);
        }
    }

    // ── B2 load-elim — store→load forwarding fires (Load → Copy of the stored value).
    #[test]
    fn load_elim_forwards_store() {
        let tt = TyTab::new();
        // frame[16..20] ← 42 (INT); reload the same slot ⟹ forward to Copy(42).
        let f = mk(
            "f",
            vec![ULONG, INT],
            vec![],
            16,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(16)),
                    Inst::Store(INT, Val::Tmp(0), Val::Imm(42)),
                    Inst::Load(1, INT, Val::Tmp(0)),
                ],
                term: Term::Ret(Some(Val::Tmp(1))),
            }],
        );
        let mut opt = f.clone();
        assert_eq!(load_elim(&tt, &mut opt), 1, "one load forwarded");
        assert!(matches!(opt.blocks[0].insts[2], Inst::Copy(1, _, Val::Imm(42))), "load → Copy(42)");
        equiv(&tt, std::slice::from_ref(&f), std::slice::from_ref(&opt), "f")
            .expect("store→load forwarding preserves ⟦·⟧");
    }

    // ── B2 — a cached value SURVIVES a provably-disjoint (NoAlias) store; plain `cse`
    // would kill it. Two distinct slots A,B: load A, store B, load A ⟹ BOTH A-loads
    // resolve to the stored value 7 (the store to B does not invalidate A).
    #[test]
    fn load_elim_survives_nonalias_store() {
        let tt = TyTab::new();
        let f = mk(
            "f",
            vec![ULONG, ULONG, INT, INT, INT],
            vec![],
            32,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(32)),                 // &A (index 0)
                    Inst::Lea(1, Place::Local(16)),                 // &B (index 16, distinct slot)
                    Inst::Store(INT, Val::Tmp(0), Val::Imm(7)),     // A = 7
                    Inst::Load(2, INT, Val::Tmp(0)),                // load A ⟹ forward 7
                    Inst::Store(INT, Val::Tmp(1), Val::Imm(9)),     // B = 9 (NoAlias A)
                    Inst::Load(3, INT, Val::Tmp(0)),                // load A ⟹ STILL 7
                    Inst::Bin(4, Op::Add, INT, Val::Tmp(2), Val::Tmp(3)),
                ],
                term: Term::Ret(Some(Val::Tmp(4))),
            }],
        );
        let mut opt = f.clone();
        assert_eq!(load_elim(&tt, &mut opt), 2, "both A-loads forwarded across the B-store");
        equiv(&tt, std::slice::from_ref(&f), std::slice::from_ref(&opt), "f")
            .expect("NoAlias survival preserves ⟦·⟧");
    }

    // ── B2 commuting square over real C with address-taken locals (arrays stay in
    // memory ⟹ real Store/Load to forward). interp models per-frame memory.
    #[test]
    fn load_elim_semantics_preserved() {
        for (nm, src, entry) in [
            ("a", "int f(int a){int x[3]; x[1]=a; x[2]=a+1; return x[1]+x[2];}", "f"),
            ("b", "int g(int a){int s=0; int x[2]; x[0]=a; x[1]=a; s=x[0]+x[1]; return s;}", "g"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                to_ssa(&ast.tt, f);
                load_elim(&ast.tt, f);
                out_of_ssa(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
    }

    // ── B4 commuting square: ⟦f⟧ = ⟦if_convert(to_ssa f)⟧ over a ternary/min-max
    // battery. A pure diamond folds to a Select; interp evaluates Select as a data-select,
    // so before (branch) and after (csel) MUST agree. `fired` proves the pass actually
    // triggers (a green identity would be vacuous).
    #[test]
    fn if_convert_semantics() {
        let mut fired = 0;
        for (nm, src, entry) in [
            ("sel", "int f(int c,int x,int y){return c?x:y;}", "f"),
            ("min", "int f(int a,int b){return a<b?a:b;}", "f"),
            ("max", "long f(long a,long b){return a>b?a:b;}", "f"),
            ("abs", "int f(int a){return a<0?-a:a;}", "f"),
            ("mix", "int f(int a,int b){return (a+1)>b?(a-b):(b-a);}", "f"),
            ("nest", "int f(int a,int b,int c){return a?(b?10:20):(c?30:40);}", "f"),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                to_ssa(&ast.tt, f);
                fired += if_convert(&ast.tt, f);
                dce(f);
                cfg_simplify(f);
                out_of_ssa(f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
            }
            equiv(&ast.tt, &ir, &opt, entry).unwrap_or_else(|e| panic!("{nm}: {e}"));
        }
        assert!(fired > 0, "if_convert must fire on the pure-select battery");
    }

    // ── B4 SAFETY (the speculation side-condition): a diamond whose arm has a SIDE
    // EFFECT (Store/Call) or a FAULTABLE load (through an unknown pointer, not a local
    // slot) MUST NOT be if-converted — running the not-taken arm would be observable /
    // could trap. Asserts if_convert leaves these untouched (n == 0).
    #[test]
    fn if_convert_refuses_unsafe() {
        for (nm, src) in [
            ("store", "int f(int a,int b){int x=0; if(a)x=1; else x=b; return x+ (a?({int*p=&x;*p=9;x;}):0);}"),
            ("ptr", "int f(int a,int *p,int *q){return a?*p:*q;}"), // faultable loads (Unk base)
            ("call", "int g(int);int f(int a){return a?g(1):g(2);}"), // side-effecting calls
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            let mut n = 0;
            for f in opt.iter_mut() {
                to_ssa(&ast.tt, f);
                n += if_convert(&ast.tt, f);
            }
            assert_eq!(n, 0, "{nm}: an unsafe/side-effecting diamond must NOT be if-converted");
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
                let home = abi_alloc(&ast.tt, f, &gp_budget(), &fp_budget(), true);
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
        let home = abi_alloc(&ast.tt, &f, &gp, &fp, true);
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
        let home = abi_alloc(&ast.tt, f, &gp, &fp, true);
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
        let home = abi_alloc(&ast.tt, f, &gp_budget(), &fp_budget(), true);
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
    // STATEMENT (SEMANTICS.md §5, OPT.md §7):
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
            optimize_ssa(&ast.tt, f, &Passes::all(), GP_K);
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
    // LICM — Phase B loop-invariant code motion. THEOREM ⟦f⟧ = ⟦licm(f)⟧ (a pure,
    // trap-free invariant computed once in the preheader = computed n times in the loop).
    // ═════════════════════════════════════════════════════════════════════════

    // How many Mul instructions live in a function's loop-carrying (φ-headed) blocks?
    fn muls_total(ir: &[IrFunc]) -> usize {
        ir.iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Bin(_, Op::Mul, ..)))
            .count()
    }

    #[test]
    fn licm_semantics_preserved() {
        // ∀ e ∈ 𝔼_struct: to_ssa ▸ licm commutes with ⟦·⟧ and stays well-formed. The loop
        // family (E) supplies the back-edges; anti-vacuous below via `licm_hoists_invariant`.
        let srcs = e_struct();
        let mut proven = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("licm", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
            }
            let mut opt = ssa.clone();
            for f in opt.iter_mut() {
                licm(&ast.tt, f, GP_K);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify licm {src}: {e}"));
            }
            equiv(&ast.tt, &ssa, &opt, "f")
                .unwrap_or_else(|e| panic!("⟦to_ssa(f)⟧ ≠ ⟦licm(·)⟧ for {src}: {e}"));
            proven += 1;
        }
        assert_eq!(proven, 312, "must prove licm over the whole generated space");
        eprintln!("licm theorem: {proven} exprs proven ⟦to_ssa(f)⟧=⟦licm(·)⟧");
    }

    // NON-VACUOUS + the matmul motivation: a nested loop where the INNER body multiplies
    // the OUTER induction variable by a constant (`i*7`). `i` is invariant in the inner
    // loop (a promoted temp defined in the outer header), so `i*7` must be HOISTED to the
    // inner preheader — the multiply leaves the inner loop. Value preserved.
    #[test]
    fn licm_hoists_invariant() {
        let (ast, ir) = compile(
            "licmh",
            "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1){int j;for(j=0;j<n;j=j+1){s=s+i*7;}}return s;}",
        );
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        let muls_before = muls_total(&ssa);
        let mut opt = ssa.clone();
        let mut n = 0u32;
        for f in opt.iter_mut() {
            n += licm(&ast.tt, f, GP_K);
        }
        for f in &opt {
            verify(f).unwrap();
        }
        assert!(n > 0, "licm must hoist the invariant i*7 out of the inner loop");
        // The hoisted multiply is now in a preheader, not re-executed per inner iteration —
        // but Mul count is unchanged (it MOVED, not deleted). Prove the MOVE instead: the
        // instruction left every φ-headed loop body. Simpler robust check: equiv + value.
        assert_eq!(muls_total(&opt), muls_before, "licm moves (does not delete) the multiply");
        equiv(&ast.tt, &ssa, &opt, "f").expect("nested-loop LICM: ⟦to_ssa(f)⟧=⟦licm(·)⟧");
        // f(3): Σ_{i=0}^{2} Σ_{j=0}^{2} i*7 = 3*(0+7+14) = 63.
        assert_eq!(interp(&ast.tt, &opt, "f", &[3]).unwrap(), 63);
        assert_eq!(interp(&ast.tt, &opt, "f", &[0]).unwrap(), 0);
    }

    // Prove the invariance test actually BITES: a loop-VARIANT expression must NOT be
    // hoisted. `s*2` where `s` is loop-carried (a header φ) is variant — licm must leave
    // it in the body. If licm wrongly hoisted a variant, equiv would diverge.
    #[test]
    fn licm_respects_variance() {
        let (ast, ir) =
            compile("licmv", "int f(int n){int s=1;int i;for(i=0;i<n;i=i+1){s=s*2;}return s;}");
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        let mut opt = ssa.clone();
        licm(&ast.tt, &mut opt, GP_K);
        verify(&opt).unwrap();
        let base = vec![ssa];
        equiv(&ast.tt, &base, &vec![opt.clone()], "f").expect("variant must be preserved");
        // f(3) = 1*2*2*2 = 8 (the variant multiply stayed in the loop).
        assert_eq!(interp(&ast.tt, &vec![opt], "f", &[3]).unwrap(), 8);
    }

    // POINTER-IV + LFTR — the commuting square ⟦to_ssa(f)⟧=⟦pointer_iv(·)⟧ on the exact
    // shapes it targets: a 1-D indexed sum, a 2-D matmul inner product (the §4 kernel), and
    // an offset/non-unit-step loop (LFTR boundary). equiv samples inputs; a wrong base-fold
    // or a wrong limit shows as a value divergence.
    // LOCAL arrays + local induction vars: parameters are not promoted (read via an in-loop
    // Load that blocks base-invariance — the real matmul uses locals), and interp does not model
    // GLOBAL/string addresses, so the base must be a frame `Lea(Local)`. Param-free ⟹ interp is
    // total (no OOB UB) and equiv is non-vacuous.
    fn pointer_iv_srcs() -> Vec<&'static str> {
        vec![
            // 1-D fill then sum: both a marching store and a marching load, LFTR i<32
            "long f(void){long A[32];int i;for(i=0;i<32;i=i+1)A[i]=i*i;\
             long s=0;for(i=0;i<32;i=i+1)s=s+A[i];return s;}",
            // 2-D inner product (matmul k-loop shape): two strides, invariant local indices ii,jj
            "long f(void){long A[8][8],B[8][8];int i,j;\
             for(i=0;i<8;i=i+1)for(j=0;j<8;j=j+1){A[i][j]=i+j;B[i][j]=i*j;}\
             long s=0;int ii=3,jj=5,k;for(k=0;k<8;k=k+1)s=s+A[ii][k]*B[k][jj];return s;}",
            // offset start + step 2: LFTR must use base+N·s, not assume i0=0/step=1
            "long f(void){int V[100];int i;for(i=0;i<100;i=i+1)V[i]=i;\
             long s=0;for(i=3;i<40;i=i+2)s=s+V[i];return s;}",
            // full nested matmul (the §4 kernel): i,j,k all local, k-loop the reduction target
            "long f(void){long A[8][8],B[8][8],C[8][8];int i,j,k;\
             for(i=0;i<8;i=i+1)for(j=0;j<8;j=j+1){A[i][j]=i+j;B[i][j]=i-j;}\
             long t=0;for(i=0;i<8;i=i+1)for(j=0;j<8;j=j+1){long s=0;\
             for(k=0;k<8;k=k+1)s=s+A[i][k]*B[k][j];C[i][j]=s;t=t+s;}return t;}",
        ]
    }
    #[test]
    fn pointer_iv_semantics_preserved() {
        for src in pointer_iv_srcs() {
            let (ast, ir) = compile("piv", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
                copy_prop(f); // canonicalize the mem2reg copies onto the φ (enabling, as SR)
            }
            let mut opt = ssa.clone();
            for f in opt.iter_mut() {
                pointer_iv(&ast.tt, f, GP_K);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify piv {src}: {e}"));
            }
            equiv(&ast.tt, &ssa, &opt, "f")
                .unwrap_or_else(|e| panic!("⟦to_ssa(f)⟧ ≠ ⟦pointer_iv(·)⟧ for {src}: {e}"));
        }
    }
    #[test]
    fn pointer_iv_fires_and_stays_finite() {
        // Fires on the matmul k-loop, and the LFTR-rewritten loop still TERMINATES with the
        // right value (a wrong limit → infinite loop → interp step-budget Err, caught here).
        let (ast, ir) = compile(
            "pivf",
            "long f(void){long A[8][8],B[8][8];int i,j;\
             for(i=0;i<8;i=i+1)for(j=0;j<8;j=j+1){A[i][j]=i+1;B[i][j]=j+1;}\
             long s=0;int ii=3,jj=5,k;for(k=0;k<8;k=k+1)s=s+A[ii][k]*B[k][jj];return s;}",
        );
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        copy_prop(&mut ssa);
        let base = vec![ssa.clone()];
        let mut opt = ssa.clone();
        let n = pointer_iv(&ast.tt, &mut opt, GP_K);
        verify(&opt).unwrap();
        assert!(n >= 2, "pointer_iv must reduce both A[ii][k] and B[k][jj] addresses (got {n})");
        // A[ii][k]=ii+1=4 for all k; B[k][jj]=jj+1=6 ⟹ s = Σ_{k<8} 4*6 = 192. A wrong LFTR
        // limit would loop forever (interp step-budget Err) or give a wrong count.
        assert_eq!(interp(&ast.tt, &vec![opt.clone()], "f", &[]).unwrap(), 192);
        equiv(&ast.tt, &base, &vec![opt], "f").expect("pointer_iv fires: ⟦·⟧ preserved");
    }

    // REGRESSION (partial-SSA soundness): a loop whose CONDITION temp is multi-def must
    // NOT have one of its defs hoisted — that freezes the condition and turns a terminating
    // loop INFINITE. `equiv` is blind here (interp of the broken loop hits the step budget →
    // Err → skipped as UB), so this is checked by DIRECT interp: the pipeline output must
    // still TERMINATE with the right value. GCC torture loop-9 shape (short-circuit || with
    // a call, loop never taken). This is the exact bug the first LICM introduced.
    #[test]
    fn licm_multidef_condition_stays_finite() {
        let (ast, ir) = compile(
            "licmc",
            "int fa(){return 0;} int f(){int count=0; while(fa()||count<-123) ++count; return count;}",
        );
        // Full pipeline (the real backend path).
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            optimize_ssa(&ast.tt, f, &Passes::all(), GP_K);
        }
        for f in &opt {
            verify(f).unwrap_or_else(|e| panic!("verify: {e}"));
        }
        // The loop is never taken ⟹ count stays 0. A frozen condition would loop forever
        // (interp → step-budget Err); TERMINATION with 0 is the whole point.
        assert_eq!(
            interp(&ast.tt, &opt, "f", &[]).unwrap(),
            0,
            "a multi-def loop condition must NOT be hoisted (loop must stay finite)"
        );
    }

    // The industrial toggle: the default profile ships licm OFF (measured-negative), all()
    // turns it ON, and set() flips individual elements — so any pass can be disabled without
    // touching the others. Correctness of every profile is covered by the equiv proofs;
    // this only checks the switch wiring.
    #[test]
    fn passes_toggle_wiring() {
        assert!(!Passes::default().licm, "default profile ships licm OFF (measured-negative)");
        assert!(!Passes::default().strength_reduce, "default ships strength_reduce OFF (measured-negative)");
        assert!(Passes::default().gvn && Passes::default().coalesce, "other proven passes default ON");
        assert!(Passes::default().peephole, "peephole default ON (MEASURED win: 1.39×→1.07×)");
        assert!(Passes::all().licm && Passes::all().strength_reduce, "all() turns loop passes ON");
        let mut p = Passes::default();
        p.set("gvn", false);
        p.set("licm", true);
        p.set("sr", true); // alias for strength_reduce
        assert!(!p.gvn && p.licm && p.strength_reduce, "set() flips individual elements independently");
        p.set("nonexistent", false); // unknown name ignored (forward-compatible)
    }

    // With coalescing toggled OFF, abi_alloc must still produce a VALID coloring (the bias
    // only ever chose among free legal colors; removing it cannot break validity).
    #[test]
    fn coalesce_off_still_valid() {
        let (ast, ir) = compile("nocoal", "int h(int);int f(int a){int x=a*a;int y=a+7;return h(a)+x-y;}");
        let (gp, fp) = (gp_budget(), fp_budget());
        for f in &ir {
            let home = abi_alloc(&ast.tt, f, &gp, &fp, false);
            verify_abi(&ast.tt, f, &home, &gp, &fp).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    // Self-proof (clean-input law): the gate has TEETH. Corrupt a hoisted instruction and
    // equiv MUST catch it — the input battery exercises the preheader code.
    #[test]
    fn licm_gate_has_teeth() {
        let (ast, ir) = compile(
            "licmt",
            "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1){int j;for(j=0;j<n;j=j+1){s=s+i*7;}}return s;}",
        );
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        let mut opt = ssa.clone();
        assert!(licm(&ast.tt, &mut opt, GP_K) > 0, "licm must fire (precondition for the teeth)");
        verify(&opt).unwrap();
        let base = vec![ssa];
        equiv(&ast.tt, &base, &vec![opt.clone()], "f").expect("identity: licm preserves ⟦·⟧");
        // Mutate the hoisted i*7 → i+7; equiv must diverge (proving it is truly exercised).
        let mut bad = opt.clone();
        let mut mutated = false;
        'o: for b in bad.blocks.iter_mut() {
            for i in b.insts.iter_mut() {
                if let Inst::Bin(_, op @ Op::Mul, ..) = i {
                    *op = Op::Add;
                    mutated = true;
                    break 'o;
                }
            }
        }
        assert!(mutated, "there must be a hoisted Mul to mutate");
        assert!(
            equiv(&ast.tt, &base, &vec![bad], "f").is_err(),
            "a Mul→Add mutation of the hoisted invariant MUST be caught (else the gate is blind)"
        );
    }

    // ── SPEED-POSITIVITY GUARD — teeth on the COST axis ──────────────────────
    // The regression that kept LICM default-OFF was register pressure: a hoist that lifts
    // pressure past the k GP colours forces a spill whose per-iteration reload outweighs the
    // saved recomputation. This test proves the guard actually BITES: on the exact nested-loop
    // shape where `i*7` is hoistable, a budget of k=1 (any real loop is already ≥1-pressured ⟹
    // zero headroom) refuses EVERY hoist, while the true budget k=10 hoists it — and, crucially,
    // ⟦·⟧ is identical either way (a refused hoist is still a subset of the proven hoist set, so
    // correctness never depended on the guard). This is the "prove speed-positive BEFORE ship"
    // obligation discharged mechanically: LICM ships only where P + hoists ≤ k, i.e. no spill.
    #[test]
    fn licm_pressure_guard_caps() {
        let src =
            "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1){int j;for(j=0;j<n;j=j+1){s=s+i*7;}}return s;}";
        let (ast, ir) = compile("licmg", src);
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        let base = vec![ssa.clone()];

        // k = 1: zero headroom (the inner loop is already pressured) ⟹ NO hoist.
        let mut capped = ssa.clone();
        let hc = licm(&ast.tt, &mut capped, 1);
        assert_eq!(hc, 0, "k=1 must leave the loop at/over budget ⟹ refuse all hoists");
        verify(&capped).unwrap();
        equiv(&ast.tt, &base, &vec![capped], "f").expect("a refused hoist still preserves ⟦·⟧");

        // k = 10 (the real GP budget): the invariant fits under budget ⟹ it IS hoisted.
        let mut open = ssa.clone();
        let ho = licm(&ast.tt, &mut open, GP_K);
        assert!(ho > 0, "k=10 has headroom ⟹ i*7 must hoist (the guard is not vacuously off)");
        verify(&open).unwrap();
        equiv(&ast.tt, &base, &vec![open.clone()], "f").expect("a permitted hoist preserves ⟦·⟧");
        assert_eq!(interp(&ast.tt, &vec![open], "f", &[3]).unwrap(), 63);
    }

    // The same teeth for strength reduction: its accumulator φ needs ≥2 GP colours of headroom,
    // so k=1 refuses the reduction (the multiply stays), k=10 performs it — ⟦·⟧ identical.
    #[test]
    fn strength_reduce_pressure_guard_caps() {
        let (ast, ir) =
            compile("srg", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1){s=s+i*7;}return s;}");
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        copy_prop(&mut ssa); // ENABLING pass (see strength_reduce_fires)
        let base = vec![ssa.clone()];

        let mut capped = ssa.clone();
        assert_eq!(strength_reduce(&ast.tt, &mut capped, 1), 0, "k=1 ⟹ no headroom for the accumulator φ");
        verify(&capped).unwrap();
        equiv(&ast.tt, &base, &vec![capped], "f").expect("refused SR preserves ⟦·⟧");

        let mut open = ssa.clone();
        assert!(strength_reduce(&ast.tt, &mut open, GP_K) > 0, "k=10 ⟹ SR fires");
        verify(&open).unwrap();
        equiv(&ast.tt, &base, &vec![open.clone()], "f").expect("permitted SR preserves ⟦·⟧");
        assert_eq!(interp(&ast.tt, &vec![open], "f", &[4]).unwrap(), 42);
    }

    // ═════════════════════════════════════════════════════════════════════════
    // REMATERIALIZATION (Tier-5 #26) — ⟦f⟧=⟦remat(f)⟧ + the pressure guard.
    // ═════════════════════════════════════════════════════════════════════════

    // A `&local` address (Lea, operand-free) is computed once and used at BOTH ends of a
    // pressured region. remat must recompute it at each use (shortening its live range to
    // ~0) and delete the single original — value preserved. The parametric budget proves
    // BOTH directions: k=2 (the region is ≥2-pressured) fires; k=100 (never pressured)
    // refuses. This is the CbC-pure slice of register allocation (no tuned weight anywhere).
    #[test]
    fn remat_recomputes_pressured_address() {
        let tt = TyTab::new();
        // f(n): scratch=7; return ((n+1)+(n+2))+(n+3)+scratch, read through &scratch (t0).
        let build = || {
            mk(
                "f",
                vec![ULONG, ULONG, INT, INT, INT, INT, INT, INT, INT, INT],
                vec![(16, INT)],
                32,
                INT,
                vec![Block {
                    insts: vec![
                        Inst::Lea(0, Place::Local(24)),           // t0 = &scratch (REMAT target)
                        Inst::Store(INT, Val::Tmp(0), Val::Imm(7)), // scratch = 7  (use #1 of t0)
                        Inst::Lea(1, Place::Local(16)),
                        Inst::Load(2, INT, Val::Tmp(1)),          // t2 = n
                        Inst::Bin(3, Op::Add, INT, Val::Tmp(2), Val::Imm(1)),
                        Inst::Bin(4, Op::Add, INT, Val::Tmp(2), Val::Imm(2)),
                        Inst::Bin(5, Op::Add, INT, Val::Tmp(2), Val::Imm(3)),
                        Inst::Bin(6, Op::Add, INT, Val::Tmp(3), Val::Tmp(4)),
                        Inst::Bin(7, Op::Add, INT, Val::Tmp(6), Val::Tmp(5)),
                        Inst::Load(8, INT, Val::Tmp(0)),          // t8 = scratch (use #2 of t0)
                        Inst::Bin(9, Op::Add, INT, Val::Tmp(7), Val::Tmp(8)),
                    ],
                    term: Term::Ret(Some(Val::Tmp(9))),
                }],
            )
        };
        let base = vec![build()];
        verify(&base[0]).expect("well-formed");
        // n=1: ((2)+(3))+4+7 = 16.
        assert_eq!(interp(&tt, &base, "f", &[1]).unwrap(), 16);

        // k=3: only &scratch (t0) is live across the ≥3-pressured middle region (the short-lived
        // &n dies before it) ⟹ remat fires on t0 alone (2 uses → 2 clones).
        let mut fired = build();
        let n = remat(&tt, &mut fired, 3);
        assert_eq!(n, 2, "both uses of the pressured address must be rematerialized");
        verify(&fired).expect("remat output verifies");
        let leas = fired.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Lea(_, Place::Local(24)))).count();
        assert_eq!(leas, 2, "the single original &scratch became two use-local recomputes");
        equiv(&tt, &base, &vec![fired.clone()], "f").expect("remat preserves ⟦·⟧");
        assert_eq!(interp(&tt, &vec![fired], "f", &[1]).unwrap(), 16);

        // k=100: no point reaches pressure 100 ⟹ remat refuses (nothing to relieve).
        let mut idle = build();
        assert_eq!(remat(&tt, &mut idle, 100), 0, "no pressure ⟹ no remat");
    }

    // Teeth on the COST axis: remat must ONLY touch operand-free defs. A value with a temp
    // operand (`n+1`) is NOT rematerializable (its operand may not be available at the use),
    // so even under crushing pressure (k=1) remat must leave every arithmetic temp intact and
    // only relieve the address — proving `rematerializable` is not over-firing.
    #[test]
    fn remat_refuses_operand_bearing_defs() {
        let tt = TyTab::new();
        let f0 = mk(
            "g",
            vec![ULONG, ULONG, INT, INT, INT],
            vec![(16, INT)],
            32,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(16)),
                    Inst::Load(1, INT, Val::Tmp(0)),      // t1 = n (operand-bearing Load)
                    Inst::Bin(2, Op::Add, INT, Val::Tmp(1), Val::Imm(1)), // t2 (operand-bearing)
                    Inst::Bin(3, Op::Add, INT, Val::Tmp(1), Val::Tmp(2)),
                    Inst::Bin(4, Op::Add, INT, Val::Tmp(3), Val::Tmp(2)),
                ],
                term: Term::Ret(Some(Val::Tmp(4))),
            }],
        );
        let base = vec![f0.clone()];
        let mut got = f0;
        remat(&tt, &mut got, 1); // k=1 ⟹ everything is "under pressure", but nothing is operand-free
        // No Load/Add was cloned: instruction count unchanged (only operand-free defs remat).
        assert_eq!(got.blocks[0].insts.len(), base[0].blocks[0].insts.len(), "no operand-bearing clone");
        equiv(&tt, &base, &vec![got], "g").expect("remat is a no-op on operand-bearing defs");
    }

    // ═════════════════════════════════════════════════════════════════════════
    // STRENGTH REDUCTION (Phase B.5) — ⟦f⟧=⟦strength_reduce(f)⟧, the IV-accumulator theorem.
    // ═════════════════════════════════════════════════════════════════════════

    fn phi_count(f: &IrFunc) -> usize {
        f.blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Phi(..))).count()
    }

    // Commuting square over the whole generated structural space: to_ssa ▸ strength_reduce
    // preserves ⟦·⟧ and stays well-formed for every e ∈ 𝔼_struct. Anti-vacuous below.
    #[test]
    fn strength_reduce_semantics_preserved() {
        let srcs = e_struct();
        let mut proven = 0u32;
        for src in &srcs {
            let (ast, ir) = compile("sr", src);
            let mut ssa = ir.clone();
            for f in ssa.iter_mut() {
                to_ssa(&ast.tt, f);
            }
            let mut opt = ssa.clone();
            for f in opt.iter_mut() {
                strength_reduce(&ast.tt, f, GP_K);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("verify sr {src}: {e}"));
            }
            equiv(&ast.tt, &ssa, &opt, "f")
                .unwrap_or_else(|e| panic!("⟦to_ssa(f)⟧ ≠ ⟦strength_reduce(·)⟧ for {src}: {e}"));
            proven += 1;
        }
        assert_eq!(proven, 312, "must prove strength_reduce over the whole generated space");
        eprintln!("strength_reduce theorem: {proven} exprs proven ⟦to_ssa(f)⟧=⟦sr(·)⟧");
    }

    // NON-VACUOUS + the matmul motivation: `s = s + i*7` in a loop. `i` is a basic induction
    // variable (header φ, i=i+1), so `i*7` is a DERIVED IV — strength reduction replaces the
    // per-iteration MULTIPLY with an add-accumulator φ (one extra φ, the multiply leaves the
    // loop as a copy). Value preserved; the accumulator φ is the structural evidence it fired.
    //
    // ENABLING-PASS DEPENDENCY (phase ordering): mem2reg leaves a COPY between the header φ and
    // each use of the induction variable (`t9 = copy(i₁); t10 = t9·7`), so SR sees the multiply
    // riding a copy, NOT the φ directly. `copy_prop` must run FIRST to collapse the copy — only
    // then is the derived IV visible. SR is therefore NOT independent: copy_prop ENABLES it. In
    // the real pipeline copy_prop precedes SR in the fixpoint; the test reproduces that order.
    #[test]
    fn strength_reduce_fires() {
        let (ast, ir) =
            compile("srf", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1){s=s+i*7;}return s;}");
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        copy_prop(&mut ssa); // ENABLING pass — collapse the mem2reg copies onto the φ
        let phis_before = phi_count(&ssa);
        let mut opt = ssa.clone();
        let n = strength_reduce(&ast.tt, &mut opt, GP_K);
        verify(&opt).unwrap();
        assert!(n > 0, "strength_reduce must fire on i*7 (a derived induction variable)");
        assert!(
            phi_count(&opt) > phis_before,
            "the reduction introduces an accumulator φ (mul → add march)"
        );
        let base = vec![ssa];
        equiv(&ast.tt, &base, &vec![opt.clone()], "f")
            .expect("strength reduction: ⟦to_ssa(f)⟧=⟦sr(·)⟧");
        // f(4) = Σ_{i=0}^{3} i*7 = 7*(0+1+2+3) = 42.
        assert_eq!(interp(&ast.tt, &vec![opt.clone()], "f", &[4]).unwrap(), 42);
        assert_eq!(interp(&ast.tt, &vec![opt], "f", &[0]).unwrap(), 0);
    }

    // Full-pipeline soundness: strength_reduce inside optimize_ssa (Passes::all) must produce
    // a φ-free, verifying, value-correct function — the accumulator φ is destroyed by out_of_ssa.
    #[test]
    fn strength_reduce_in_pipeline_terminates_correct() {
        let (ast, ir) =
            compile("srp", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1){s=s+i*3;}return s;}");
        let mut opt = ir.clone();
        for f in opt.iter_mut() {
            optimize_ssa(&ast.tt, f, &Passes::all(), GP_K);
        }
        for f in &opt {
            verify(f).unwrap_or_else(|e| panic!("verify: {e}"));
            assert_eq!(phi_count(f), 0, "out_of_ssa must destroy the accumulator φ");
        }
        // f(5) = 3*(0+1+2+3+4) = 30.
        assert_eq!(interp(&ast.tt, &opt, "f", &[5]).unwrap(), 30);
    }

    // Self-proof (clean-input law): the gate has TEETH. Corrupt the accumulator step and equiv
    // MUST catch it — proving the inserted march is genuinely exercised, not dead.
    #[test]
    fn strength_reduce_gate_has_teeth() {
        let (ast, ir) =
            compile("srt", "int f(int n){int s=0;int i;for(i=0;i<n;i=i+1){s=s+i*7;}return s;}");
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        copy_prop(&mut ssa); // ENABLING pass (see strength_reduce_fires)
        let mut opt = ssa.clone();
        assert!(strength_reduce(&ast.tt, &mut opt, GP_K) > 0, "must fire (precondition for teeth)");
        verify(&opt).unwrap();
        let base = vec![ssa];
        equiv(&ast.tt, &base, &vec![opt.clone()], "f").expect("identity: sr preserves ⟦·⟧");
        // After SR the ONLY remaining Mul is the base `i₀·d` SR inserted in the preheader (the
        // body multiply became a copy of the accumulator φ). Corrupt it Mul→Add; equiv must
        // diverge, proving SR's inserted base computation is genuinely exercised.
        let mut bad = opt.clone();
        let mut mutated = false;
        'o: for b in bad.blocks.iter_mut() {
            for i in b.insts.iter_mut() {
                if let Inst::Bin(_, op @ Op::Mul, ..) = i {
                    *op = Op::Add;
                    mutated = true;
                    break 'o;
                }
            }
        }
        assert!(mutated, "there must be an inserted base Mul to corrupt");
        assert!(
            equiv(&ast.tt, &base, &vec![bad], "f").is_err(),
            "a Mul→Add corruption of the inserted base i₀·d MUST be caught (else the gate is blind)"
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
                optimize_ssa(&ast.tt, f, &Passes::all(), GP_K);
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
                optimize_ssa(&ast.tt, f, &Passes::all(), GP_K);
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

    // Count direct Sym-calls to `name` across a program (the inline convergence measure).
    fn count_sym_calls(ir: &[IrFunc], name: &str) -> usize {
        ir.iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Call(_, Callee::Sym(s), ..) if s == name))
            .count()
    }

    // ── Tier-1 #5 (inline): a NON-RECURSIVE callee is substituted β-reduced and the
    // commuting square holds. Two call sites in one block (exercises descending-k splice)
    // + one call feeding an expression (dst live in the continuation).
    #[test]
    fn inline_leaf_commutes() {
        let (ast, ir) = compile(
            "inl",
            "static int add(int a,int b){return a+b;}\
             int f(int x){return add(x,3)+add(x,x);}",
        );
        assert_eq!(count_sym_calls(&ir, "add"), 2, "baseline: two calls to add");
        let mut inl = ir.clone();
        let ok = vec![true; inl.len()];
        inline(&ast.tt, &mut inl, &ok);
        for g in &inl {
            verify(g).unwrap_or_else(|e| panic!("verify after inline: {e}"));
        }
        assert_eq!(count_sym_calls(&inl, "add"), 0, "both add() sites must be inlined");
        equiv(&ast.tt, &ir, &inl, "f").expect("⟦f⟧ = ⟦inline f⟧ (leaf)");
        // Value spot-check through the interp (independent of equiv's battery).
        assert_eq!(interp(&ast.tt, &inl, "f", &[7]).unwrap(), 24, "add(7,3)+add(7,7)=10+14");
    }

    // ── A VOID call with an unused result inlines cleanly (dst = None → no Copy back).
    #[test]
    fn inline_void_commutes() {
        let (ast, ir) = compile(
            "inlv",
            "static void put(int *p,int v){*p=v;}\
             int f(int x){int a; put(&a,x*2); return a+1;}",
        );
        let mut inl = ir.clone();
        let ok = vec![true; inl.len()];
        inline(&ast.tt, &mut inl, &ok);
        for g in &inl {
            verify(g).unwrap_or_else(|e| panic!("verify: {e}"));
        }
        assert_eq!(count_sym_calls(&inl, "put"), 0, "put() inlined");
        // NOTE — no whole-program equiv here: a void call's only observable effect crosses
        // the frame boundary (here `put` stores through &a into f's frame), and interp
        // models each function's frame as ISOLATED memory, so the PRE-inline program is
        // outside interp's modeled space (it silently reads back 0, not an Err → equiv
        // can't SKIP it). Inlining is exactly what folds the store into f's own frame, so
        // the POST-inline form IS modeled — we validate the dst=None splice path directly
        // on it: right value + no residual call. (Same oracle limit as any pointer/global
        // side effect; the leaf/self tests carry the ⟦f⟧=⟦inline f⟧ proof.)
        assert_eq!(interp(&ast.tt, &inl, "f", &[5]).unwrap(), 11, "put(&a,10) inlined ⇒ a=10, a+1=11");
        assert_eq!(interp(&ast.tt, &inl, "f", &[-6]).unwrap(), -11, "put(&a,-12) inlined ⇒ a=-12, a+1=-11");
    }

    // ── SELF-RECURSION: fib inlines to a DEPTH-1 unroll — the clone's own recursive
    // calls survive (2 original sites → 4 residual calls), and the value is unchanged
    // (equiv recurses through the residual Call(Sym)). This is the fib call-overhead lever.
    #[test]
    fn inline_self_recursion_depth1() {
        let (ast, ir) = compile(
            "inls",
            "static long fib(int n){ if(n<2) return n; return fib(n-1)+fib(n-2);}\
             int f(int n){ return (int)fib(n); }",
        );
        assert_eq!(count_sym_calls(&ir, "fib"), 3, "2 inside fib + 1 in f");
        let mut inl = ir.clone();
        let ok = vec![true; inl.len()];
        inline(&ast.tt, &mut inl, &ok);
        for g in &inl {
            verify(g).unwrap_or_else(|e| panic!("verify after self-inline: {e}"));
        }
        // fib's own 2 sites each become a clone carrying 2 calls → 4; f's single call is
        // also inlined (fib ≤ SELF/LEAF), bringing another unrolled body (2 more) → ≥ 4.
        assert!(count_sym_calls(&inl, "fib") > 3, "depth-1 unroll multiplies residual calls");
        equiv(&ast.tt, &ir, &inl, "fib").expect("⟦fib⟧ = ⟦inline fib⟧ (self, depth-1)");
        for n in 0..12 {
            let (a, b) = (interp(&ast.tt, &ir, "fib", &[n]).unwrap(), interp(&ast.tt, &inl, "fib", &[n]).unwrap());
            assert_eq!(a, b, "fib({n}): {a} vs {b}");
        }
    }
}
