// THEORY B — dominance and loop nesting, OWNED once per function instead of
// rebuilt once per pass.
//
// WHAT THIS IS AND WHY IT IS NOT AN ABSTRACTION FOR ITS OWN SAKE. Every loop row
// in `hir/pass/` opens with the same three lines — `dom::cfg`, `dom::domtree`,
// `dom::loops` — and there are twenty-four such sites, run `ROUNDS` times per
// function. `MEASURED M44` priced exactly one of them: the MIR loop-constant
// hoist cost 28.7% of sqlite's compile time and was reverted, and a cycle-test
// early-out that skipped loopless functions measured at ZERO. The cost is the
// analysis, not the calls that waste it. This type is the owner those rows never
// had.
//
// THE CONTRACT, and it is the seam Article B asks for: a pass READS the handle
// and DECLARES what it invalidates. `invalidate` is called by the driver on any
// pass that reports a change, and by a pass that rewrites the CFG mid-run before
// it reads again.
//
// HOW THE DECLARATION IS PROVEN RATHER THAN TRUSTED. A cached `DomTree` that
// outlives the CFG it describes is a MISCOMPILE, not a slow compile, and no
// borrow checker can see it: `Cfg` is owned data, not a borrow of `Func`. So the
// non-vacuity instrument here is a CHECKER (`ZCC_ACHECK`, and every debug build):
// each read rebuilds the analysis from `f` and asserts the cached answer is
// bit-identical. The battery and the gate corpus run with it on; a release build
// pays nothing for it. A stale-cache defect is then a loud assertion at the pass
// that lied, which is Law 2's "locate the line mechanically" made automatic.
use super::{Func, dom};
pub use crate::cfg::{Cfg, DomTree, LoopForest};

/// The three control-flow analyses of one function, computed at most once each
/// between the CFG rewrites that invalidate them.
#[derive(Default)]
pub struct Analyses {
    cfg: Option<Cfg>,
    dt: Option<DomTree>,
    lf: Option<LoopForest>,
}

/// Is the stale-cache checker wanted? ON in every debug build — the battery is a
/// debug build, so `cargo test` proves the invalidation discipline on every case
/// it has — and off in release unless `ZCC_ACHECK` asks, which is how the gate
/// corpus can be run under it without paying for it in a shipped compiler.
pub fn checking() -> bool {
    cfg!(debug_assertions) || {
        static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENV.get_or_init(|| std::env::var_os("ZCC_ACHECK").is_some())
    }
}

impl Analyses {
    pub fn new() -> Analyses {
        Analyses::default()
    }

    /// Everything computed from the CFG is now a claim about a function that no
    /// longer exists. Called by the driver after any pass that reports a change,
    /// and by a pass that rewrites blocks between its own reads.
    pub fn invalidate(&mut self) {
        self.cfg = None;
        self.dt = None;
        self.lf = None;
    }

    pub fn cfg(&mut self, f: &Func) -> &Cfg {
        if self.cfg.is_none() {
            self.cfg = Some(dom::cfg(f));
        } else if checking() {
            assert!(
                same_cfg(self.cfg.as_ref().unwrap(), &dom::cfg(f)),
                "stale Cfg: a pass rewrote the CFG without invalidating"
            );
        }
        self.cfg.as_ref().unwrap()
    }

    pub fn domtree(&mut self, f: &Func) -> &DomTree {
        self.fill(f);
        self.dt.as_ref().unwrap()
    }

    pub fn loops(&mut self, f: &Func) -> &LoopForest {
        self.fill_loops(f);
        self.lf.as_ref().unwrap()
    }

    /// The three at once, which is the shape every loop row opens with.
    pub fn all(&mut self, f: &Func) -> (&Cfg, &DomTree, &LoopForest) {
        self.fill_loops(f);
        (
            self.cfg.as_ref().unwrap(),
            self.dt.as_ref().unwrap(),
            self.lf.as_ref().unwrap(),
        )
    }

    /// `cfg` and `domtree`, for the rows that do not name a loop.
    pub fn dom(&mut self, f: &Func) -> (&Cfg, &DomTree) {
        self.fill(f);
        (self.cfg.as_ref().unwrap(), self.dt.as_ref().unwrap())
    }

    fn fill(&mut self, f: &Func) {
        self.cfg(f);
        if self.dt.is_none() {
            let c = self.cfg.as_ref().unwrap();
            self.dt = Some(dom::domtree(f, c));
        } else if checking() {
            let c = self.cfg.as_ref().unwrap();
            assert!(
                same_domtree(self.dt.as_ref().unwrap(), &dom::domtree(f, c)),
                "stale DomTree: a pass rewrote the CFG without invalidating"
            );
        }
    }

    fn fill_loops(&mut self, f: &Func) {
        self.fill(f);
        if self.lf.is_none() {
            let c = self.cfg.as_ref().unwrap();
            let dt = self.dt.as_ref().unwrap();
            self.lf = Some(dom::loops(c, dt));
        }
    }
}

// ── the checker's equalities ──────────────────────────────────────────────
//
// Structural, not hashed: a hash collision here would be a silently wrong
// compile, and Law 0 does not trade a proof for a constant factor in an
// assertion that only debug builds run.

fn same_cfg(a: &Cfg, b: &Cfg) -> bool {
    a.succs == b.succs && a.preds == b.preds && a.rpo == b.rpo && a.rpo_num == b.rpo_num
}

/// `tin`/`tout` are private and are a deterministic function of `kids` and the
/// preorder walk over them, so these three settle the question.
fn same_domtree(a: &DomTree, b: &DomTree) -> bool {
    a.idom == b.idom && a.kids == b.kids && a.preorder == b.preorder
}
