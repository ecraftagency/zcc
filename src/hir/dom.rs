// HIR's instantiation of the shared control-flow analyses (`src/cfg.rs`) plus
// THEORY B — dominance and loop nesting, instantiated for HIR
// the one CFG transform HIR owns.
use super::{BlockId, Func, Term};
pub use crate::cfg::{Cfg, DomTree, Loop, LoopForest};

pub fn cfg(f: &Func) -> Cfg {
    Cfg::build(f.blocks.len(), f.entry, |b| {
        f.blocks[b as usize].term.succs()
    })
}

pub fn domtree(f: &Func, c: &Cfg) -> DomTree {
    DomTree::new(c, f.entry)
}

pub fn loops(c: &Cfg, dt: &DomTree) -> LoopForest {
    LoopForest::new(c, dt)
}

/// Split every critical edge — a source with several successors reaching a
/// target with several predecessors. Both SSA destruction and the spiller need
/// somewhere to put an edge copy, and a critical edge offers none.
///
/// The commuting square is by construction: the inserted block is empty and its
/// terminator forwards exactly the arguments the original edge carried, so no
/// value, order or effect changes — ⟦f⟧ = ⟦split f⟧.
pub fn split_critical_edges(f: &mut Func) -> bool {
    let c = cfg(f);
    let mut split = false;
    for b in 0..f.blocks.len() as BlockId {
        if !c.reachable(b) || c.succs[b as usize].len() < 2 {
            continue;
        }
        let mut term = f.blocks[b as usize].term.clone();
        for t in term.targets_mut() {
            if c.preds[t.block as usize].len() < 2 {
                continue;
            }
            let mid = f.new_block();
            let args = std::mem::take(&mut t.args);
            f.blocks[mid as usize].weight = f.blocks[b as usize].weight;
            f.blocks[mid as usize].term = Term::Jmp(super::Target {
                block: t.block,
                args,
            });
            t.block = mid;
            split = true;
        }
        f.blocks[b as usize].term = term;
    }
    split
}
