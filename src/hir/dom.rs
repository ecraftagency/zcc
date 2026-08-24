// CFG analyses shared by every layer: reverse postorder, the dominator tree
// (Cooper, Harvey & Kennedy 2001 — "A Simple, Fast Dominance Algorithm"), the
// natural-loop forest, and critical-edge splitting.
//
// These are *analyses*, not transforms: they carry no commuting-square obligation
// (they add no instruction). The one transform here, `split_critical_edges`, is
// pure CFG plumbing whose square is trivial — an inserted block whose body is
// empty and whose terminator is the original jump forwards exactly the same
// block arguments, so ⟦f⟧ is unchanged by construction.
use super::{BlockId, Func, Term};

pub struct Cfg {
    pub preds: Vec<Vec<BlockId>>,
    pub succs: Vec<Vec<BlockId>>,
    /// reverse postorder from the entry; unreachable blocks are absent
    pub rpo: Vec<BlockId>,
    /// position in `rpo`, or `u32::MAX` when unreachable
    pub rpo_num: Vec<u32>,
}

impl Cfg {
    pub fn new(f: &Func) -> Cfg {
        let n = f.blocks.len();
        let mut succs = vec![Vec::new(); n];
        let mut preds = vec![Vec::new(); n];
        for (b, blk) in f.blocks.iter().enumerate() {
            let mut s = blk.term.succs();
            s.dedup();
            for &t in &s {
                preds[t as usize].push(b as BlockId);
            }
            succs[b] = s;
        }
        // iterative postorder (the CFG can be deeper than the Rust stack on real TUs)
        let mut post = Vec::with_capacity(n);
        let mut seen = vec![false; n];
        let mut stack = vec![(f.entry, 0usize)];
        seen[f.entry as usize] = true;
        while let Some(&mut (b, ref mut i)) = stack.last_mut() {
            if *i < succs[b as usize].len() {
                let s = succs[b as usize][*i];
                *i += 1;
                if !seen[s as usize] {
                    seen[s as usize] = true;
                    stack.push((s, 0));
                }
            } else {
                post.push(b);
                stack.pop();
            }
        }
        post.reverse();
        let rpo = post;
        let mut rpo_num = vec![u32::MAX; n];
        for (i, &b) in rpo.iter().enumerate() {
            rpo_num[b as usize] = i as u32;
        }
        Cfg {
            preds,
            succs,
            rpo,
            rpo_num,
        }
    }
    pub fn reachable(&self, b: BlockId) -> bool {
        self.rpo_num[b as usize] != u32::MAX
    }
}

pub struct DomTree {
    /// immediate dominator; `idom[entry] == entry`, `u32::MAX` when unreachable
    pub idom: Vec<BlockId>,
    /// children in the dominator tree
    pub kids: Vec<Vec<BlockId>>,
    /// dominator-tree preorder — the perfect elimination order the SSA register
    /// allocator walks (Hack 2007)
    pub preorder: Vec<BlockId>,
    /// preorder index and subtree extent, so `dominates` is an O(1) range test
    pub tin: Vec<u32>,
    pub tout: Vec<u32>,
}

impl DomTree {
    pub fn new(f: &Func, cfg: &Cfg) -> DomTree {
        let n = f.blocks.len();
        let mut idom = vec![u32::MAX; n];
        idom[f.entry as usize] = f.entry;
        // Cooper-Harvey-Kennedy: iterate in RPO until the idom array stops moving.
        let mut changed = true;
        while changed {
            changed = false;
            for &b in &cfg.rpo {
                if b == f.entry {
                    continue;
                }
                let mut new = u32::MAX;
                for &p in &cfg.preds[b as usize] {
                    if idom[p as usize] == u32::MAX {
                        continue; // not yet processed this round
                    }
                    new = if new == u32::MAX {
                        p
                    } else {
                        intersect(&idom, &cfg.rpo_num, p, new)
                    };
                }
                if new != u32::MAX && idom[b as usize] != new {
                    idom[b as usize] = new;
                    changed = true;
                }
            }
        }
        let mut kids = vec![Vec::new(); n];
        for &b in &cfg.rpo {
            if b != f.entry && idom[b as usize] != u32::MAX {
                kids[idom[b as usize] as usize].push(b);
            }
        }
        let (mut tin, mut tout, mut preorder) = (vec![0u32; n], vec![0u32; n], Vec::with_capacity(n));
        let mut clock = 0u32;
        let mut stack = vec![(f.entry, 0usize)];
        tin[f.entry as usize] = clock;
        clock += 1;
        preorder.push(f.entry);
        while let Some(&mut (b, ref mut i)) = stack.last_mut() {
            if *i < kids[b as usize].len() {
                let c = kids[b as usize][*i];
                *i += 1;
                tin[c as usize] = clock;
                clock += 1;
                preorder.push(c);
                stack.push((c, 0));
            } else {
                tout[b as usize] = clock;
                stack.pop();
            }
        }
        DomTree {
            idom,
            kids,
            preorder,
            tin,
            tout,
        }
    }

    /// Reflexive dominance: `a` dominates `b` (every path entry→b goes through a).
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        let (a, b) = (a as usize, b as usize);
        self.tin[a] <= self.tin[b] && self.tout[b] <= self.tout[a]
    }
}

fn intersect(idom: &[BlockId], rpo_num: &[u32], mut a: BlockId, mut b: BlockId) -> BlockId {
    while a != b {
        while rpo_num[a as usize] > rpo_num[b as usize] {
            a = idom[a as usize];
        }
        while rpo_num[b as usize] > rpo_num[a as usize] {
            b = idom[b as usize];
        }
    }
    a
}

pub struct Loop {
    pub header: BlockId,
    pub body: Vec<BlockId>,
    pub latches: Vec<BlockId>,
    pub parent: Option<u32>,
    pub depth: u32,
}

pub struct LoopForest {
    pub loops: Vec<Loop>,
    /// innermost loop containing each block, if any
    pub of: Vec<Option<u32>>,
    pub depth: Vec<u32>,
}

impl LoopForest {
    /// Natural loops: a back edge b→h with h dom b defines the loop of h; its body
    /// is the set of blocks reaching b without leaving h.
    pub fn new(f: &Func, cfg: &Cfg, dt: &DomTree) -> LoopForest {
        let n = f.blocks.len();
        let mut headers: Vec<(BlockId, Vec<BlockId>)> = Vec::new();
        for &b in &cfg.rpo {
            for &s in &cfg.succs[b as usize] {
                if dt.dominates(s, b) {
                    match headers.iter_mut().find(|(h, _)| *h == s) {
                        Some((_, l)) => l.push(b),
                        None => headers.push((s, vec![b])),
                    }
                }
            }
        }
        let mut loops: Vec<Loop> = Vec::new();
        for (h, latches) in headers {
            let mut body = vec![h];
            let mut mark = vec![false; n];
            mark[h as usize] = true;
            let mut work: Vec<BlockId> = latches.clone();
            for &l in &latches {
                if !mark[l as usize] {
                    mark[l as usize] = true;
                    body.push(l);
                }
            }
            while let Some(b) = work.pop() {
                for &p in &cfg.preds[b as usize] {
                    if !mark[p as usize] {
                        mark[p as usize] = true;
                        body.push(p);
                        work.push(p);
                    }
                }
            }
            loops.push(Loop {
                header: h,
                body,
                latches,
                parent: None,
                depth: 0,
            });
        }
        // Nesting: the smaller body is the child. Sorting by size makes the first
        // enclosing loop found the immediate parent.
        loops.sort_by_key(|l| l.body.len());
        let mut of = vec![None; n];
        for i in 0..loops.len() {
            for j in i + 1..loops.len() {
                if loops[j].body.contains(&loops[i].header) {
                    loops[i].parent = Some(j as u32);
                    break;
                }
            }
        }
        for i in 0..loops.len() {
            let mut d = 0;
            let mut p = loops[i].parent;
            while let Some(k) = p {
                d += 1;
                p = loops[k as usize].parent;
            }
            loops[i].depth = d;
        }
        // innermost wins: assign from the outermost inward
        let order: Vec<usize> = {
            let mut v: Vec<usize> = (0..loops.len()).collect();
            v.sort_by_key(|&i| std::cmp::Reverse(loops[i].body.len()));
            v
        };
        let mut depth = vec![0u32; n];
        for i in order {
            for &b in &loops[i].body {
                of[b as usize] = Some(i as u32);
                depth[b as usize] = loops[i].depth + 1;
            }
        }
        LoopForest { loops, of, depth }
    }
}

/// Split every critical edge (a source with several successors reaching a target
/// with several predecessors). Both SSA destruction and the spiller need a place
/// to put an edge copy; a critical edge has none.
pub fn split_critical_edges(f: &mut Func) -> bool {
    let cfg = Cfg::new(f);
    let mut split = false;
    for b in 0..f.blocks.len() as BlockId {
        if !cfg.reachable(b) || cfg.succs[b as usize].len() < 2 {
            continue;
        }
        let mut term = f.blocks[b as usize].term.clone();
        for t in term.targets_mut() {
            if cfg.preds[t.block as usize].len() < 2 {
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
