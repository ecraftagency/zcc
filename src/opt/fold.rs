// src/opt/fold.rs — value simplification — constant-fold, algebraic-identity, DCE, SCCP, GVN.
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

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
//
// ALGEBRAIC IDENTITIES (one variable + one neutral/absorbing constant, or a self-operand):
// a second family of rewrite-soundness laws, each a ⟦Bin⟧ algebraic identity over ℤ/2^n
// (THEORY §A7), so `algebraic_identity` is faithful by the SAME construction as the folder
// — it returns only rewrites that hold in the interpreted structure `eval_bin` realizes:
//   neutral element   x+0, 0+x, x−0, x*1, 1*x, x|0, 0|x, x^0, 0^x, x<<0, x>>0, x/1  → x
//   absorbing element x*0, 0*x, x&0, 0&x                                             → 0
//   equal operands    x−x, x^x → 0;   x&x, x|x → x   (idempotence / annihilation)
// INTEGER ONLY — float breaks x+0 (−0.0) and x*1 (sNaN); floats carry FImm so the Imm
// patterns already exclude them, the !is_float guard is belt-and-suspenders. Pointer-typed
// Bins are included and benefit (p+0 → p is common). NOT folded: 0−x (a negation, not an
// identity), x/2^k (signed rounds toward 0, shift toward −∞ — unsound), x/−1 (INT_MIN UB).
// gcc/LLVM instcombine; measured sqlite residual = 1,311 `x*1` + 615 `x+0`.
// ─────────────────────────────────────────────────────────────────────────────
pub(crate) fn algebraic_identity(d: Tmp, op: Op, ty: TypeId, a: Val, b: Val) -> Option<Inst> {
    let copy = |v: Val| Some(Inst::Copy(d, ty, v));
    let zero = Some(Inst::Copy(d, ty, Val::Imm(0)));
    let is0 = |v: Val| matches!(v, Val::Imm(0));
    let is1 = |v: Val| matches!(v, Val::Imm(1));
    let same = matches!((a, b), (Val::Tmp(x), Val::Tmp(y)) if x == y);
    match op {
        Op::Add => {
            if is0(b) { return copy(a); }
            if is0(a) { return copy(b); }
        }
        Op::Sub => {
            if is0(b) { return copy(a); }
            if same { return zero; } // x − x = 0
        }
        Op::Mul => {
            if is0(a) || is0(b) { return zero; } // x * 0 = 0 (absorbing)
            if is1(b) { return copy(a); }
            if is1(a) { return copy(b); }
        }
        Op::Div => {
            if is1(b) { return copy(a); } // x / 1 = x  (NOT x/−1: INT_MIN UB)
        }
        Op::And => {
            if is0(a) || is0(b) { return zero; } // x & 0 = 0 (absorbing)
            if same { return copy(a); }          // x & x = x
        }
        Op::Or => {
            if is0(b) { return copy(a); }
            if is0(a) { return copy(b); }
            if same { return copy(a); } // x | x = x
        }
        Op::Xor => {
            if is0(b) { return copy(a); }
            if is0(a) { return copy(b); }
            if same { return zero; } // x ^ x = 0
        }
        Op::Shl | Op::Shr => {
            if is0(b) { return copy(a); } // x << 0 = x, x >> 0 = x
        }
        _ => {}
    }
    None
}


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
                // One-variable algebraic identity (the both-Imm arm above already claimed
                // the constant-fold case) → Copy(x) / Copy(0), ⟦·⟧-preserving.
                Inst::Bin(d, op, ty, a, b) if !tt.is_float(*ty) => algebraic_identity(*d, *op, *ty, *a, *b),
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
/// C99 6.7.3 — is this instruction a VOLATILE ACCESS (a Load/Store through a
/// volatile-qualified type)? A volatile access is an observable side effect of the
/// abstract machine: it must occur exactly as written — never removed, merged,
/// duplicated, reordered across another volatile access, or promoted out of memory.
/// The volatile bit rides the access TypeId (TyTab::vol), set at the decl/pointee/member
/// where the qualifier originated, so no separate IR flag is needed. Every memory pass
/// consults this to PIN volatile accesses while optimizing the volatile-free remainder
/// normally — replacing the old whole-function -O0 fallback (`has_volatile`).
pub fn is_volatile_access(tt: &TyTab, i: &Inst) -> bool {
    match i {
        Inst::Load(_, ty, _) | Inst::Store(ty, _, _) => tt.is_volatile(*ty),
        _ => false,
    }
}


pub(crate) fn is_pure(i: &Inst) -> bool {
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


pub fn dce(tt: &TyTab, f: &mut IrFunc) -> u32 {
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
                // A dead volatile Load is NOT removable (C99 6.7.3 — the access must occur).
                Some(d) if !used[d as usize] && is_pure(i) && !is_volatile_access(tt, i) => false,
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
pub(crate) enum Lat {
    Top,
    Const(i64),
    Bot,
}


/// The meet (greatest lower bound) of two lattice points — used to combine a φ's
/// reachable arms and to lower a temp monotonically.
pub(crate) fn lat_meet(a: Lat, b: Lat) -> Lat {
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

