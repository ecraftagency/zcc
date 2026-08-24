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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};


mod util;
mod fold;
mod scalar;
mod mem;
mod ssa;
mod loops;
mod regalloc;
mod inline;
pub(crate) use util::*;
pub(crate) use fold::*;
pub(crate) use scalar::*;
pub(crate) use mem::*;
pub(crate) use ssa::*;
pub(crate) use loops::*;
pub(crate) use regalloc::*;
pub(crate) use inline::*;

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
        n += cse(tt, f);
        n += dce(tt, f);
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
    pub sroa: bool,     // SROA: split non-escaping aggregate locals into per-field slots (before to_ssa)
    pub hoist_const: bool, // lever 9: hoist loop-invariant expensive immediates (bounds) to preheader (last)
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
            pointer_iv: true, // base-fold SR + LFTR; theorem-precondition base≠∅ (loops.rs::pointer_iv) —
            // matmul 3.44→1.71×, sieve 2.60→2.10× vs O1 at k=10, spill-free (measured box best-of-3)
            coalesce: true,
            peephole: true, // measured win: removes the x0-funnel redundant reg-reg moves
            ldst_pair: true, // B4: adjacent str/ldr → stp/ldp; static win, translation-validated
            if_convert: true, // B4: diamond → csel; proven (equiv), B1-licensed load speculation
            inline: true, // Tier-1 #5: β-reduction; proven (equiv) — measured on the bench before ship
            remat: false, // Tier-5 #26: proven (equiv) but speed-gated on the box A/B (like licm/sr)
            sroa: true,   // SROA: field-split non-escaping aggregates; proven (equiv) — feeds to_ssa
            hoist_const: true, // lever 9: pressure-safe (spill-reload ≤ mov;movk), exec-positive on loop bounds
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
            "sroa" => self.sroa = v,
            "hoist_const" | "hoistconst" | "hc" => self.hoist_const = v,
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
// THE SSA OPTIMIZATION PIPELINE (the QBE-level projection, under CbC). The whole
// point of Stages 1–4: build SSA, run the SSA-strength passes to a fixpoint, then
// return to executable (φ-free) IR and do a final non-SSA cleanup.
//
//   optimize_ssa = [sroa] ▸ to_ssa ▸ (sccp ∘ const_fold ∘ copy_prop ∘ gvn ∘ cse ∘ load_elim ∘ dce ∘ cfg_simplify ∘ licm ∘ strength_reduce ∘ pointer_iv)*
//                  ▸ [if_convert] ▸ out_of_ssa ▸ optimize ▸ [remat ∘ hoist_const]   ([·] = toggled in `Passes`)
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

pub fn optimize_ssa(tt: &TyTab, f: &mut IrFunc, p: &Passes, gp_k: u32) {
    if p.sroa {
        sroa(tt, f); // field-split non-escaping aggregates so to_ssa can promote their fields
    }
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
            n += cse(tt, f); // block-local load reuse (GVN skips memory)
        }
        if p.load_elim {
            n += load_elim(tt, f); // B2: alias-aware store→load forwarding + NoAlias survival
        }
        if p.dce {
            n += dce(tt, f); // reclaim the temps the above passes deadened (incl. φ)
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
            dce(tt, f);
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
    if p.hoist_const {
        // ABSOLUTE-last IR touch: lift loop-invariant expensive immediates (bounds) into a
        // preheader temp. Runs after remat so nothing rematerializes/const-folds it back into
        // the loop; the constant now lives in a register the codegen compares against directly.
        hoist_loop_consts(f);
    }
}



#[cfg(test)]
mod tests;
