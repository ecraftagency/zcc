// Control-flow analyses, written once over an abstract successor relation and
// THEORY B — graph theory: dominance (Cooper/Harvey/Kennedy), loop nesting
// instantiated for both HIR and MIR. These are analyses, not transforms: they
// add no instruction and so carry no commuting-square obligation.
//
// Dominators: Cooper, Harvey & Kennedy 2001, "A Simple, Fast Dominance
// Algorithm" — iterate the meet over predecessors in reverse postorder until the
// idom array is stable. Adequate at our scale and far shorter than
// Lengauer-Tarjan (MECHANISM.md §G3.3).
pub type Node = u32;

pub struct Cfg {
    pub preds: Vec<Vec<Node>>,
    pub succs: Vec<Vec<Node>>,
    /// reverse postorder from the entry; unreachable nodes are absent
    pub rpo: Vec<Node>,
    /// index into `rpo`, or `u32::MAX` when unreachable
    pub rpo_num: Vec<u32>,
}

impl Cfg {
    pub fn build(n: usize, entry: Node, succ_of: impl Fn(Node) -> Vec<Node>) -> Cfg {
        let mut succs: Vec<Vec<Node>> = (0..n as Node).map(&succ_of).collect();
        for s in succs.iter_mut() {
            s.dedup();
        }
        let mut preds = vec![Vec::new(); n];
        for (b, ss) in succs.iter().enumerate() {
            for &t in ss {
                preds[t as usize].push(b as Node);
            }
        }
        // iterative postorder: a real translation unit nests deeper than the
        // Rust stack tolerates
        let mut post = Vec::with_capacity(n);
        let mut seen = vec![false; n];
        let mut stack = vec![(entry, 0usize)];
        seen[entry as usize] = true;
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
        let mut rpo_num = vec![u32::MAX; n];
        for (i, &b) in post.iter().enumerate() {
            rpo_num[b as usize] = i as u32;
        }
        Cfg {
            preds,
            succs,
            rpo: post,
            rpo_num,
        }
    }
    pub fn reachable(&self, b: Node) -> bool {
        self.rpo_num[b as usize] != u32::MAX
    }
}

pub struct DomTree {
    /// immediate dominator; `idom[entry] == entry`, `u32::MAX` when unreachable
    pub idom: Vec<Node>,
    pub kids: Vec<Vec<Node>>,
    /// dominator-tree preorder — the perfect elimination order of the SSA
    /// interference graph (Hack 2007), which the register allocator walks
    pub preorder: Vec<Node>,
    tin: Vec<u32>,
    tout: Vec<u32>,
}

impl DomTree {
    pub fn new(cfg: &Cfg, entry: Node) -> DomTree {
        let n = cfg.succs.len();
        let mut idom = vec![u32::MAX; n];
        idom[entry as usize] = entry;
        let mut changed = true;
        while changed {
            changed = false;
            for &b in &cfg.rpo {
                if b == entry {
                    continue;
                }
                let mut new = u32::MAX;
                for &p in &cfg.preds[b as usize] {
                    if idom[p as usize] == u32::MAX {
                        continue;
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
            if b != entry && idom[b as usize] != u32::MAX {
                kids[idom[b as usize] as usize].push(b);
            }
        }
        let (mut tin, mut tout) = (vec![0u32; n], vec![0u32; n]);
        let mut preorder = Vec::with_capacity(cfg.rpo.len());
        let mut clock = 0u32;
        let mut stack = vec![(entry, 0usize)];
        tin[entry as usize] = clock;
        clock += 1;
        preorder.push(entry);
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

    /// Reflexive dominance, as an O(1) preorder-interval test.
    pub fn dominates(&self, a: Node, b: Node) -> bool {
        let (a, b) = (a as usize, b as usize);
        self.tin[a] <= self.tin[b] && self.tout[b] <= self.tout[a]
    }
}

fn intersect(idom: &[Node], rpo_num: &[u32], mut a: Node, mut b: Node) -> Node {
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
    pub header: Node,
    pub body: Vec<Node>,
    pub latches: Vec<Node>,
    pub parent: Option<u32>,
    pub depth: u32,
}

pub struct LoopForest {
    pub loops: Vec<Loop>,
    /// the innermost loop containing each node
    pub of: Vec<Option<u32>>,
    /// nesting depth, 0 outside every loop — the weight the spiller's next-use
    /// distance and the block layout both read
    pub depth: Vec<u32>,
}

impl LoopForest {
    /// Natural loops: a back edge b→h (h dominates b) defines the loop headed by
    /// h; its body is every node that reaches b without leaving h.
    pub fn new(cfg: &Cfg, dt: &DomTree) -> LoopForest {
        let n = cfg.succs.len();
        let mut headers: Vec<(Node, Vec<Node>)> = Vec::new();
        // Where each header already sits in `headers`, so a second back edge to
        // the same header is an index instead of a scan of every header found so
        // far. Same list, same order, same latch order — the scan was answering
        // a question the block number can answer directly.
        let mut hpos: Vec<usize> = vec![usize::MAX; n];
        for &b in &cfg.rpo {
            for &s in &cfg.succs[b as usize] {
                if dt.dominates(s, b) {
                    match hpos[s as usize] {
                        usize::MAX => {
                            hpos[s as usize] = headers.len();
                            headers.push((s, vec![b]));
                        }
                        k => headers[k].1.push(b),
                    }
                }
            }
        }
        let mut loops: Vec<Loop> = Vec::new();
        // CP2.6 (compile-speed): one `mark` scratch reused across headers instead
        // of a fresh `vec![false; n]` per loop — the backward walk touches only a
        // loop's own body, so clearing exactly `body` afterward restores it to
        // all-false in O(body), turning the O(loops × n) allocation into O(Σbody).
        // Same body sets, byte-identical.
        let mut mark = vec![false; n];
        for (h, latches) in headers {
            let mut body = vec![h];
            mark[h as usize] = true;
            let mut work = Vec::new();
            for &l in &latches {
                if !mark[l as usize] {
                    mark[l as usize] = true;
                    body.push(l);
                    work.push(l);
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
            for &b in &body {
                mark[b as usize] = false;
            }
            loops.push(Loop {
                header: h,
                body,
                latches,
                parent: None,
                depth: 0,
            });
        }
        // the smaller body nests inside the larger: sorting by size makes the
        // first enclosing loop the immediate parent
        loops.sort_by_key(|l| l.body.len());
        // The parent is the FIRST loop after `i` in this size order whose body
        // holds `i`'s header. Asked as a scan it is O(loops² × body); asked of an
        // index from block to the loops containing it — built once in ascending
        // loop order, so the first entry past `i` IS that first loop — it is one
        // walk of Σbody. Same parent for every loop.
        let mut holding: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (j, l) in loops.iter().enumerate() {
            for &b in &l.body {
                holding[b as usize].push(j as u32);
            }
        }
        for i in 0..loops.len() {
            let h = loops[i].header as usize;
            loops[i].parent = holding[h].iter().copied().find(|&j| j as usize > i);
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
        let mut of = vec![None; n];
        let mut depth = vec![0u32; n];
        let mut order: Vec<usize> = (0..loops.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(loops[i].body.len()));
        for i in order {
            for &b in &loops[i].body {
                of[b as usize] = Some(i as u32);
                depth[b as usize] = loops[i].depth + 1;
            }
        }
        LoopForest { loops, of, depth }
    }
}
