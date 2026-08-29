// THEORY B — dominance and loop nesting, OWNED once per machine function.
//
// MIR's instantiation of `hir::analysis`, and it exists for a MEASURED reason
// rather than for symmetry. `MEASURED M44` reverted the loop-constant hoist for
// 28.7% of sqlite's compile time, and named the cause exactly: the hoist rebuilds
// `cfg` + `DomTree` + `LoopForest` per function, immediately after
// `const_share::run` has just built the first two — the two passes run
// back-to-back, on the same function, and the first one's answer is still true
// when the second one throws it away. A cycle-test early-out that skipped
// loopless functions measured at ZERO, which is what proves the cost is the
// ANALYSIS and not the calls that waste it.
//
// The contract and the checker are `hir::analysis`'s, unchanged: a pass READS the
// handle and DECLARES what it invalidates, and every debug build re-derives each
// cached answer and asserts it still holds. A `DomTree` that outlives the CFG it
// describes is a miscompile, not a slow compile, and no borrow checker sees it.
use super::MFunc;
use crate::cfg::{Cfg, DomTree, LoopForest};

/// The three control-flow analyses of one machine function.
#[derive(Default)]
pub struct MAnalyses {
    cfg: Option<Cfg>,
    dt: Option<DomTree>,
    lf: Option<LoopForest>,
}

impl MAnalyses {
    pub fn new() -> MAnalyses {
        MAnalyses::default()
    }

    /// Everything computed from the CFG is now a claim about a function that no
    /// longer exists.
    pub fn invalidate(&mut self) {
        self.cfg = None;
        self.dt = None;
        self.lf = None;
    }

    pub fn cfg(&mut self, f: &MFunc) -> &Cfg {
        if self.cfg.is_none() {
            self.cfg = Some(crate::mir::verify::cfg(f));
        } else if crate::hir::analysis::checking() {
            assert!(
                same_cfg(self.cfg.as_ref().unwrap(), &crate::mir::verify::cfg(f)),
                "stale MIR Cfg: a pass rewrote the CFG without invalidating"
            );
        }
        self.cfg.as_ref().unwrap()
    }

    pub fn dom(&mut self, f: &MFunc) -> (&Cfg, &DomTree) {
        self.fill(f);
        (self.cfg.as_ref().unwrap(), self.dt.as_ref().unwrap())
    }

    pub fn all(&mut self, f: &MFunc) -> (&Cfg, &DomTree, &LoopForest) {
        self.fill(f);
        if self.lf.is_none() {
            let c = self.cfg.as_ref().unwrap();
            let dt = self.dt.as_ref().unwrap();
            self.lf = Some(LoopForest::new(c, dt));
        }
        (
            self.cfg.as_ref().unwrap(),
            self.dt.as_ref().unwrap(),
            self.lf.as_ref().unwrap(),
        )
    }

    fn fill(&mut self, f: &MFunc) {
        self.cfg(f);
        if self.dt.is_none() {
            let c = self.cfg.as_ref().unwrap();
            self.dt = Some(DomTree::new(c, f.entry));
        } else if crate::hir::analysis::checking() {
            let c = self.cfg.as_ref().unwrap();
            assert!(
                same_domtree(self.dt.as_ref().unwrap(), &DomTree::new(c, f.entry)),
                "stale MIR DomTree: a pass rewrote the CFG without invalidating"
            );
        }
    }
}

fn same_cfg(a: &Cfg, b: &Cfg) -> bool {
    a.succs == b.succs && a.preds == b.preds && a.rpo == b.rpo && a.rpo_num == b.rpo_num
}

fn same_domtree(a: &DomTree, b: &DomTree) -> bool {
    a.idom == b.idom && a.kids == b.kids && a.preorder == b.preorder
}
