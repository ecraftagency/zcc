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
            pointer_iv: true, // base-fold SR + LFTR; theorem-precondition base≠∅ (opt.rs:2933) —
            // matmul 3.44→1.71×, sieve 2.60→2.10× vs O1 at k=10, spill-free (measured box best-of-3)
            coalesce: true,
            peephole: true, // measured win: removes the x0-funnel redundant reg-reg moves
            ldst_pair: true, // B4: adjacent str/ldr → stp/ldp; static win, translation-validated
            if_convert: true, // B4: diamond → csel; proven (equiv), B1-licensed load speculation
            inline: true, // Tier-1 #5: β-reduction; proven (equiv) — measured on the bench before ship
            remat: false, // Tier-5 #26: proven (equiv) but speed-gated on the box A/B (like licm/sr)
            sroa: true,   // SROA: field-split non-escaping aggregates; proven (equiv) — feeds to_ssa
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

    // Algebraic identities: each one-variable rewrite preserves ⟦·⟧ (commuting square
    // over a function exercising every law) AND the interpreted result is unchanged for
    // a probe input, AND the pass actually FIRES (anti-vacuous). x is param temp 0.
    #[test]
    fn cf_algebraic_identities() {
        let tt = TyTab::new();
        // param x in frame slot 16, loaded into t1. Then one Bin per identity law:
        //   keep-x:  t2=x+0  t3=t2*1  t6=x^0  t8=x|0  t9=x<<0  t10=x>>0  t11=x/1
        //   zero:    t4=x*0  t5=x-x   t7=x&0
        // Sum the six keep-x temps (⟹ 6x) + the three zeros (⟹ 0)  ⟹  f(x) = 6x.
        let x = Val::Tmp(1);
        let before = vec![mk(
            "f",
            vec![INT; 20],
            vec![(16, INT)],
            16,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(16)),
                    Inst::Load(1, INT, Val::Tmp(0)), // t1 = x
                    Inst::Bin(2, Op::Add, INT, x, Val::Imm(0)),           // x+0 = x
                    Inst::Bin(3, Op::Mul, INT, Val::Tmp(2), Val::Imm(1)), // *1  = x
                    Inst::Bin(4, Op::Mul, INT, x, Val::Imm(0)),           // x*0 = 0
                    Inst::Bin(5, Op::Sub, INT, x, x),                     // x-x = 0
                    Inst::Bin(6, Op::Xor, INT, x, Val::Imm(0)),           // x^0 = x
                    Inst::Bin(7, Op::And, INT, x, Val::Imm(0)),           // x&0 = 0
                    Inst::Bin(8, Op::Or, INT, x, Val::Imm(0)),            // x|0 = x
                    Inst::Bin(9, Op::Shl, INT, x, Val::Imm(0)),           // x<<0 = x
                    Inst::Bin(10, Op::Shr, INT, x, Val::Imm(0)),          // x>>0 = x
                    Inst::Bin(11, Op::Div, INT, x, Val::Imm(1)),          // x/1 = x
                    // sum the six keep-x temps ⟹ 6x
                    Inst::Bin(12, Op::Add, INT, Val::Tmp(3), Val::Tmp(6)),
                    Inst::Bin(13, Op::Add, INT, Val::Tmp(12), Val::Tmp(8)),
                    Inst::Bin(14, Op::Add, INT, Val::Tmp(13), Val::Tmp(9)),
                    Inst::Bin(15, Op::Add, INT, Val::Tmp(14), Val::Tmp(10)),
                    Inst::Bin(16, Op::Add, INT, Val::Tmp(15), Val::Tmp(11)),
                    // sum the three zeros ⟹ 0
                    Inst::Bin(17, Op::Add, INT, Val::Tmp(4), Val::Tmp(5)),
                    Inst::Bin(18, Op::Add, INT, Val::Tmp(17), Val::Tmp(7)),
                    Inst::Bin(19, Op::Add, INT, Val::Tmp(16), Val::Tmp(18)),
                ],
                term: Term::Ret(Some(Val::Tmp(19))),
            }],
        )];
        let mut after = before.clone();
        let n = const_fold(&tt, &mut after[0]);
        assert!(n >= 10, "all 10 identity Bins must be rewritten (got {n})");
        verify(&after[0]).unwrap();
        equiv(&tt, &before, &after, "f").expect("algebraic identities must preserve ⟦·⟧");
        assert_eq!(interp(&tt, &after, "f", &[7]).unwrap(), 42);   // 6·7
        assert_eq!(interp(&tt, &after, "f", &[-5]).unwrap(), -30); // 6·(−5)
        // x+0 (t2) and x*1 (t3) are now plain Copies — the sieve `mul x,x,1` / sqlite lever.
        assert!(matches!(after[0].blocks[0].insts[2], Inst::Copy(2, _, _)));
        assert!(matches!(after[0].blocks[0].insts[3], Inst::Copy(3, _, _)));
        assert!(matches!(after[0].blocks[0].insts[4], Inst::Copy(4, _, Val::Imm(0)))); // x*0
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
        let removed: u32 = opt.iter_mut().map(|f| dce(&ast.tt, f)).sum();
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
            dce(&ast.tt, f);
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
                dce(&ast.tt, f);
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
        let n = cse(&TyTab::new(), &mut after[0]);
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
            cse(&ast.tt, f);
            copy_prop(f);
            cse(&ast.tt, f);
            copy_prop(f);
            dce(&ast.tt, f);
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
                cse(&ast.tt, f);
                copy_prop(f);
                cse(&ast.tt, f);
                dce(&ast.tt, f);
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
                dce(&ast.tt, f);
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

    // Guards the SPARSE interference walk (iterate only live members, not a full 0..nt scan):
    // t0 is defined first and used only at the tail, so it is live ACROSS the defs of t1,t2,t3
    // and must interfere with every one of them. A sparse walk that forgot a long-lived temp
    // would drop these edges (a coloring bug); a dense scan and a sparse scan must agree.
    #[test]
    fn interference_long_live_range() {
        let f = mk(
            "g",
            vec![INT, INT, INT, INT, INT],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Copy(0, INT, Val::Imm(1)),
                    Inst::Copy(1, INT, Val::Imm(2)),
                    Inst::Copy(2, INT, Val::Imm(3)),
                    Inst::Copy(3, INT, Val::Imm(4)),
                    Inst::Bin(4, Op::Add, INT, Val::Tmp(0), Val::Tmp(3)), // t0 live to here
                ],
                term: Term::Ret(Some(Val::Tmp(4))),
            }],
        );
        let adj = interference(&f, &liveness(&f));
        for t in [1u32, 2, 3] {
            assert!(adj[0].contains(&t) && adj[t as usize].contains(&0), "t0 must interfere with t{t}");
        }
    }

    // Stage 5b — the ABI budgets the backend uses (arm64_elf.rs): GP = 10 callee-saved
    // (x19–x28), NO caller-saved (the emitter's scratch spans x0–x15); FP = 16 caller
    // (v16–v31) ⊕ 8 callee (v8–v15).
    fn gp_budget() -> ClassBudget {
        ClassBudget { k: 10, ncaller: 0, narg: 0 }
    }
    fn fp_budget() -> ClassBudget {
        ClassBudget { k: 24, ncaller: 16, narg: 0 }
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
        let gp = ClassBudget { k: 2, ncaller: 2, narg: 0 }; // all caller-saved, no callee-saved
        let fp = ClassBudget { k: 16, ncaller: 16, narg: 0 };
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

    // PERF backstop (abi_alloc's 60000-temp ceiling): a function above the cap must return
    // the all-spill baseline — no super-linear allocator path runs — and that baseline is
    // trivially verify_abi-valid (no colored temp ⟹ the interference invariant cannot be
    // violated). Certifies the Side-II policy constant at the middle (Law 3), not the binary.
    #[test]
    fn abi_alloc_size_backstop_all_spills() {
        let tt = TyTab::new();
        let n = 60_001; // one over the default ZCC_MAXTEMPS ceiling
        let f = mk(
            "big",
            vec![INT; n],
            vec![],
            0,
            INT,
            vec![Block {
                insts: vec![Inst::Copy(0, INT, Val::Imm(1))],
                term: Term::Ret(Some(Val::Tmp(0))),
            }],
        );
        let home = abi_alloc(&tt, &f, &gp_budget(), &fp_budget(), true);
        assert_eq!(home.len(), n);
        assert!(home.iter().all(|h| h.is_none()), "above the backstop every temp must spill");
        verify_abi(&tt, &f, &home, &gp_budget(), &fp_budget()).expect("all-spill must verify");
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
            2 => { cse(tt, f); }
            3 => { dce(tt, f); }
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
            cse(&ast.tt, f);
            copy_prop(f);
            dce(&ast.tt, f);
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
    fn sroa_splits_and_commutes() {
        // A non-escaping local struct: SROA folds each field address to a per-field
        // Lea(Local), and ⟦f⟧ = ⟦sroa(f)⟧ (the Lea-address identity). Anti-vacuous: sroa
        // MUST fire, and after to_ssa the split fields promote (Lea(Local) count drops).
        let src = "struct P{int x,y,z;};\
                   int f(int n){struct P p;p.x=n;p.y=n+1;p.z=n+2;return p.x+p.y+p.z;}";
        let (ast, ir) = compile("sroa", src);
        let leas0: usize = ir
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Lea(_, Place::Local(_))))
            .count();
        let mut opt = ir.clone();
        let mut fired = 0;
        for f in opt.iter_mut() {
            fired += sroa(&ast.tt, f);
        }
        assert!(fired > 0, "sroa must fire on a non-escaping struct");
        for f in &opt {
            verify(f).unwrap_or_else(|e| panic!("verify sroa: {e}"));
        }
        equiv(&ast.tt, &ir, &opt, "f").expect("⟦f⟧ = ⟦sroa(f)⟧");
        assert_eq!(interp(&ast.tt, &opt, "f", &[10]).unwrap(), 33);
        // End-to-end: the folded fields promote and the value is unchanged.
        for f in opt.iter_mut() {
            to_ssa(&ast.tt, f);
        }
        equiv(&ast.tt, &ir, &opt, "f").expect("⟦f⟧ = ⟦to_ssa(sroa(f))⟧");
        assert_eq!(interp(&ast.tt, &opt, "f", &[10]).unwrap(), 33);
        let leas1: usize = opt
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.insts)
            .filter(|i| matches!(i, Inst::Lea(_, Place::Local(_))))
            .count();
        assert!(leas1 < leas0, "the promoted fields must drop Lea(Local) count ({leas0}→{leas1})");
    }

    #[test]
    fn sroa_respects_escape_and_overlap() {
        // The soundness gates, each proven through the full split+promote pipeline against
        // the naive semantics. (a) a pointer into the struct aliases a sibling field — SROA
        // must NOT split (else the promoted sibling misses the pointer write). (b) union
        // punning: two overlapping fields cannot both become independent scalars.
        for (nm, src, arg, want) in [
            (
                "alias",
                "struct P{int x,y;};\
                 int f(int n){struct P p;int*q=&p.x;p.y=n;q[1]=99;return p.y;}",
                5,
                99,
            ),
            (
                "union",
                "union U{int i;struct{short a,b;}s;};\
                 int f(int n){union U u;u.i=0;u.s.b=(short)n;return u.i;}",
                7,
                7 << 16,
            ),
        ] {
            let (ast, ir) = compile(nm, src);
            let mut opt = ir.clone();
            for f in opt.iter_mut() {
                sroa(&ast.tt, f);
                to_ssa(&ast.tt, f);
            }
            for f in &opt {
                verify(f).unwrap_or_else(|e| panic!("{nm} verify: {e}"));
            }
            equiv(&ast.tt, &ir, &opt, "f").unwrap_or_else(|e| panic!("{nm}: ⟦f⟧≠⟦opt(f)⟧ {e}"));
            assert_eq!(interp(&ast.tt, &opt, "f", &[arg]).unwrap(), want, "{nm}");
        }
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

    // C99 6.7.3 — VOLATILE-ACCESS PRESERVATION (the structural dual of the ⟦·⟧ commuting
    // square). ⟦·⟧-equivalence alone cannot certify volatile correctness: two reads of the
    // same location yield the same value, so a wrongly-merged pair is ⟦·⟧-invisible. What IS
    // observable is the MULTISET of volatile accesses — their number and kind must be exactly
    // preserved. This test would FAIL without the per-pass guards: CSE would merge `a=g;b=g`,
    // load_elim would forward `g=a` into `c=g`, DCE would drop an unused read, if_convert would
    // speculate one, to_ssa would promote a volatile local. The count must neither DROP (an
    // access elided) nor RISE (an access duplicated by block cloning).
    #[test]
    fn volatile_accesses_preserved() {
        let (ast, ir) = compile(
            "vol",
            "volatile int g; int f(void){ int a=g; int b=g; g=a; int c=g; return b+c; }",
        );
        let count = |fns: &[IrFunc]| -> usize {
            fns.iter()
                .flat_map(|f| &f.blocks)
                .flat_map(|b| &b.insts)
                .filter(|i| is_volatile_access(&ast.tt, i))
                .count()
        };
        let before = count(&ir);
        assert!(before >= 4, "test must exercise ≥4 volatile accesses (got {before})");
        let mut ssa = ir.clone();
        for f in ssa.iter_mut() {
            optimize_ssa(&ast.tt, f, &Passes::all(), GP_K);
        }
        for f in &ssa {
            verify(f).unwrap();
        }
        assert_eq!(
            count(&ssa),
            before,
            "every volatile access must survive optimization, none duplicated (C99 6.7.3)"
        );
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
            dce(&ast.tt, f); // strip the now-dead pre-fold arithmetic so the surviving Imm is LIVE
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

    // GATE-HAS-TEETH (audit RC1 #2): pointer_iv must DECLINE on a loop with no pointer-linear
    // address term (a pure scalar reduction) — there is nothing to strength-reduce, so it returns
    // 0 and leaves ⟦·⟧ untouched. This proves the matcher is selective (it does not fire spuriously
    // and rewrite an induction variable that has no marching memory access).
    #[test]
    fn pointer_iv_declines_scalar_loop() {
        let (ast, ir) = compile("pivneg", "int f(int n){int s=0,i;for(i=0;i<n;i=i+1)s=s+i;return s;}");
        let mut ssa = ir[0].clone();
        to_ssa(&ast.tt, &mut ssa);
        copy_prop(&mut ssa);
        let base = vec![ssa.clone()];
        let mut opt = ssa.clone();
        let n = pointer_iv(&ast.tt, &mut opt, GP_K);
        assert_eq!(n, 0, "no pointer-linear term ⟹ pointer_iv must not fire");
        verify(&opt).unwrap();
        equiv(&ast.tt, &base, &vec![opt], "f").expect("a declined pass is the identity on ⟦·⟧");
    }

    // dead_static_fns GATE-HAS-TEETH (audit RC1 #3, previously untested): an unreferenced static
    // is removed; a static that is CALLED from a root, address-taken (named in root_syms), or
    // exported (non-static) is KEPT. Removing a live function is a link error, so the gate must
    // never over-remove; leaving a provably-dead one wastes size, so it must remove it.
    #[test]
    fn dead_static_fns_gate_has_teeth() {
        let (_ast, ir) = compile(
            "dsf",
            "static int used(int x){return x+1;} \
             static int viaptr(int x){return x*2;} \
             static int deadf(int x){return x-1;} \
             int pub(int x){return used(x);}",
        );
        let idx = |n: &str| {
            ir.iter()
                .position(|f| f.name.strip_prefix('\u{1}').unwrap_or(&f.name) == n)
                .unwrap_or_else(|| panic!("no func {n}"))
        };
        let mut is_static = vec![false; ir.len()];
        for nm in ["used", "viaptr", "deadf"] {
            is_static[idx(nm)] = true;
        }
        // `pub` is a non-static root (is_static=false); `viaptr` is address-taken via a global
        // initializer, which the real driver records in root_syms under the func's own name.
        let root: std::collections::HashSet<String> =
            [ir[idx("viaptr")].name.clone()].into_iter().collect();
        let dead = dead_static_fns(&ir, &is_static, &root);
        assert!(!dead[idx("pub")], "an exported (non-static) function is a root ⟹ kept");
        assert!(!dead[idx("used")], "a static reached by a Call from a root ⟹ kept");
        assert!(!dead[idx("viaptr")], "an address-taken static (in root_syms) ⟹ kept");
        assert!(dead[idx("deadf")], "an unreferenced static ⟹ dead ⟹ removable");
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
        inline(&ast.tt, &mut inl, &ok, &ok, &InlineCfg::exercise_all());
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
        inline(&ast.tt, &mut inl, &ok, &ok, &InlineCfg::exercise_all());
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
        inline(&ast.tt, &mut inl, &ok, &ok, &InlineCfg::exercise_all());
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

