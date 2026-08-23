// src/opt/scalar.rs — scalar redundancy — copy-propagation, common-subexpression elimination.
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

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
pub(crate) fn enc(v: &Val) -> (u8, i64) {
    match v {
        Val::Tmp(t) => (0, *t as i64),
        Val::Imm(x) => (1, *x),
        Val::FImm(b) => (2, *b as i64),
    }
}

/// A binary value-number key: (op-tag, type, operand-1, operand-2). Commutative → sort.
pub(crate) fn bin_key(op: Op, ty: u32, a: &Val, b: &Val) -> (u16, u32, (u8, i64), (u8, i64)) {
    let commutative = matches!(op, Op::Add | Op::Mul | Op::And | Op::Or | Op::Xor | Op::Eq | Op::Ne);
    let (mut o1, mut o2) = (enc(a), enc(b));
    if commutative && o1 > o2 {
        std::mem::swap(&mut o1, &mut o2);
    }
    (op as u16, ty, o1, o2)
}


pub fn cse(tt: &TyTab, f: &mut IrFunc) -> u32 {
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
                | Inst::Param(..)
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
                Inst::Load(d, ty, addr) if !tt.is_volatile(*ty) => {
                    let k = (enc(addr), *ty);
                    match loads.get(&k) {
                        Some(&prev) => Some(Inst::Copy(*d, *ty, Val::Tmp(prev))),
                        None => {
                            loads.insert(k, *d);
                            None
                        }
                    }
                }
                // A volatile Load is a memory barrier: it must EXECUTE (never reuse a cached
                // value) and never be reused later ⟹ clear the load cache and record nothing.
                Inst::Load(_, _, _) => {
                    loads.clear();
                    None
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

