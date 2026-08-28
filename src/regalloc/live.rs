// Liveness on MIR-SSA with block parameters (REARCH.md §7.1).
// THEORY A7 — liveness on SSA (Boissinot et al. 2008)
//
// The only subtlety block parameters introduce: a target's ARGUMENTS are uses on
// the EDGE, not inside the successor, and a block's PARAMETERS are definitions
// at the successor's entry. So
//
//   live_out(b) = ⋃_{s ∈ succ(b)} ( live_in(s) ∪ args(b→s) )
//   live_in(b)  = uses(b) ∪ ( live_out(b) ∖ defs(b) )        [defs include params]
//
// which is exactly the ordinary equation once edges are read that way — the
// reason block parameters were chosen over φ instructions (REARCH §14).
//
// Physical registers are tracked alongside virtual ones. They must be: the
// entry's parallel copy READS x0–x7 and a call's argument copy WRITES them, so
// a virtual register colored x1 that is live across the entry copy would be
// destroyed by it. Modeling both in one live set makes that an ordinary
// interference instead of a special case.
use crate::cfg::Cfg;
use crate::mir::*;
use std::collections::BTreeSet;

/// One index space covering virtual and physical registers.
#[derive(Clone, Copy)]
pub struct Space {
    pub nv: usize,
}

/// THEORY II-3 — the physical register file, as AAPCS64 gives it
pub const PHYS: usize = 96; // 32 GPR + 32 FPR + 32 (flags, only #0 used)

impl Space {
    pub fn new(f: &MFunc) -> Space {
        Space {
            nv: f.vregs.len(),
        }
    }
    pub fn len(&self) -> usize {
        self.nv + PHYS
    }
    pub fn idx(&self, r: Reg) -> usize {
        match r {
            Reg::V(v) => v as usize,
            Reg::P(p) => {
                self.nv
                    + match p.class {
                        Class::Gpr => 0,
                        Class::Fpr => 32,
                        Class::Flags => 64,
                    }
                    + p.num as usize
            }
        }
    }
    pub fn reg(&self, i: usize) -> Reg {
        if i < self.nv {
            Reg::V(i as VReg)
        } else {
            let j = i - self.nv;
            let (class, num) = match j / 32 {
                0 => (Class::Gpr, j),
                1 => (Class::Fpr, j - 32),
                _ => (Class::Flags, j - 64),
            };
            Reg::P(PReg {
                class,
                num: num as u8,
            })
        }
    }
}

pub struct Liveness {
    pub sp: Space,
    pub live_in: Vec<BTreeSet<usize>>,
    pub live_out: Vec<BTreeSet<usize>>,
    /// virtual registers live across at least one `Call`: they may not receive a
    /// caller-saved colour (AAPCS64 §6.1.1). This is the whole of the
    /// "value crosses a call" rule — there is no other.
    pub crosses_call: Vec<bool>,
    /// For each virtual register, the PHYSICAL registers whose live ranges
    /// overlap it. A physical register is live for real — an argument register
    /// from the parallel copy that sets it up until the call that reads it, an
    /// incoming argument until the entry copy consumes it — and a virtual
    /// register that overlaps it must not be given that colour. Without this a
    /// function pointer held in x1 is destroyed by the copy that puts the second
    /// argument into x1.
    pub phys_conflict: Vec<RegSet>,
}

pub fn compute(f: &MFunc, cfg: &Cfg) -> Liveness {
    let sp = Space::new(f);
    let n = f.blocks.len();
    let (mut live_in, mut live_out) = (vec![BTreeSet::new(); n], vec![BTreeSet::new(); n]);

    // Per-block use/def summaries (uses = read before any local definition),
    // built AS the sorted runs the fixpoint below reads. The first cut built a
    // `BTreeSet` per block and then copied each one into a run, so every summary
    // was constructed twice — once in a structure whose ordering was then thrown
    // away. Membership while building is a stamp per index, cleared by the block
    // that wrote it, so the "already defined here" test stays O(1).
    let mut uses: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut defs: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut seen_def = vec![u32::MAX; sp.len()];
    for (bi, blk) in f.blocks.iter().enumerate() {
        let (u, d) = (&mut uses[bi], &mut defs[bi]);
        let era = bi as u32;
        for p in &blk.params {
            let i = sp.idx(*p);
            if seen_def[i] != era {
                seen_def[i] = era;
                d.push(i);
            }
        }
        for inst in &blk.insts {
            inst.visit(&mut |r, c| {
                let i = sp.idx(r);
                match c {
                    Constraint::Use | Constraint::UseFixed(_) => {
                        if seen_def[i] != era {
                            u.push(i);
                        }
                    }
                    Constraint::Def | Constraint::DefFixed(_) => {
                        if seen_def[i] != era {
                            seen_def[i] = era;
                            d.push(i);
                        }
                    }
                }
            });
        }
        // the terminator's own operands; edge arguments are handled by live_out
        blk.term.visit(&mut |r, _| {
            let i = sp.idx(r);
            if seen_def[i] != era {
                u.push(i);
            }
        });
        u.sort_unstable();
        u.dedup();
        d.sort_unstable();
        d.dedup();
    }

    // CP2.2 (compile-speed): a predecessor-worklist instead of a `while changed`
    // round-robin over every block each sweep. The problem is monotone with a
    // unique least fixpoint, so visit ORDER does not change the answer — only the
    // number of visits. A block is recomputed only when a successor's `live_in`
    // grew (a backward problem: `live_out[b]` reads `live_in[succ]`), so on a
    // change to `live_in[b]` its predecessors are re-queued. Seeded in reverse
    // RPO, the order that converged fastest under the old sweep. Byte-identical.
    // A SORTED RUN OF INDICES, not a tree of them. The set operations this
    // fixpoint performs are union at a join, difference against the block's
    // definitions, and equality — all of which a sorted array answers in one
    // linear pass, in cache, with the buffers reused. A `BTreeSet` pays a node
    // allocation and a pointer chase per element on every visit, and a block is
    // visited many times; measured on the sqlite amalgamation, this fixpoint was
    // 3.1 s, the largest single item in the whole allocator.
    //
    // The ORDER is the same order — ascending — so `live_in`/`live_out` are the
    // same sets, iterated the same way, and the sets handed to the caller below
    // are the same sets. Only their representation while the fixpoint runs
    // differs.
    let mut vin: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut vout: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut inq = vec![true; n];
    let mut wl: std::collections::VecDeque<usize> =
        cfg.rpo.iter().rev().map(|&b| b as usize).collect();
    let (mut out, mut inn) = (Vec::new(), Vec::new());
    while let Some(b) = wl.pop_front() {
        inq[b] = false;
        out.clear();
        for t in f.blocks[b].term.targets() {
            out.extend_from_slice(&vin[t.block as usize]);
            for a in &t.args {
                out.push(sp.idx(*a));
            }
        }
        // a computed goto has successors but carries no arguments
        for &s in &cfg.succs[b] {
            out.extend_from_slice(&vin[s as usize]);
        }
        out.sort_unstable();
        out.dedup();
        inn.clear();
        inn.extend_from_slice(&uses[b]);
        inn.extend(out.iter().copied().filter(|i| defs[b].binary_search(i).is_err()));
        inn.sort_unstable();
        inn.dedup();
        if out != vout[b] {
            vout[b].clear();
            vout[b].extend_from_slice(&out);
        }
        if inn != vin[b] {
            vin[b].clear();
            vin[b].extend_from_slice(&inn);
            for &p in &cfg.preds[b] {
                let p = p as usize;
                if !inq[p] {
                    inq[p] = true;
                    wl.push_back(p);
                }
            }
        }
    }
    for b in 0..n {
        live_in[b] = vin[b].iter().copied().collect();
        live_out[b] = vout[b].iter().copied().collect();
    }

    // A value crosses a call when it is live immediately before the call and
    // still live after it.
    let mut crosses_call = vec![false; sp.nv];
    for (bi, blk) in f.blocks.iter().enumerate() {
        if !cfg.reachable(bi as MBlockId) {
            continue;
        }
        let mut live: BTreeSet<usize> = live_out[bi].clone();
        // The TERMINATOR's own operand is live at the end of the block too, and
        // it is not in `live_out` — `live_out` is what the SUCCESSORS need, and a
        // branch condition is consumed here. Starting the backward walk without
        // it makes a value whose only use is the branch invisible to the
        // call-crossing test, and the colourer then hands it a caller-saved
        // register that the call destroys. (Latent until R2.2: while every local
        // was a memory cell, the condition was reloaded immediately before the
        // branch and never spanned a call. torture pr36343.)
        blk.term.visit(&mut |r, _| {
            live.insert(sp.idx(r));
        });
        // walk backwards, so `live` is the set live AFTER the instruction
        for inst in blk.insts.iter().rev() {
            if matches!(inst, MInst::Call { .. }) {
                for &i in &live {
                    if i < sp.nv {
                        crosses_call[i] = true;
                    }
                }
            }
            // definitions are killed BEFORE uses are added: a call both defines
            // and uses x0, and the use must survive the kill
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    live.remove(&sp.idx(*r));
                }
            }
            for (r, c) in &ops {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    live.insert(sp.idx(*r));
                }
            }
        }
    }

    // Physical/virtual overlap. The conflict is recorded at each program point
    // AFTER the point's definitions are added and its dying operands removed:
    // an entry copy's source (x0) dies exactly where its destination is born, so
    // recording before the kill would spuriously forbid the one colour that
    // makes the copy disappear.
    let mut phys_conflict = vec![RegSet::default(); sp.nv];
    let pre = Liveness {
        sp,
        live_in: live_in.clone(),
        live_out: live_out.clone(),
        crosses_call: crosses_call.clone(),
        phys_conflict: Vec::new(),
    };
    let mut lu = LastUse::new(sp);
    for bi in 0..f.blocks.len() {
        if !cfg.reachable(bi as MBlockId) {
            continue;
        }
        last_use_into(f, sp, &pre, bi, &mut lu);
        let last = &lu.at;
        let mut live: BTreeSet<usize> = live_in[bi].clone();
        for &p in &f.blocks[bi].params {
            live.insert(sp.idx(p));
        }
        // THE SET IS ORDERED, so the physical half is a range, not a filter.
        // Virtual registers index below `nv` and physical ones above it, and this
        // runs at every program point — walking the whole live set to reach the
        // tail of it is the walk this ordering exists to avoid.
        // A REGISTER SET IS A BITMASK, so what is live here is ONE mask and a
        // virtual register takes it in one OR. Adding the physical registers one
        // at a time, per live value, per program point, is a nested loop over two
        // sets where a single word operation says the same thing.
        let mut record = |live: &BTreeSet<usize>, pc: &mut Vec<RegSet>| {
            let mut here = RegSet::default();
            for &x in live.range(sp.nv..) {
                if let Some(p) = sp.reg(x).preg() {
                    here.add(p);
                }
            }
            if here.gpr == 0 && here.fpr == 0 {
                return;
            }
            for &x in live.range(..sp.nv) {
                pc[x].gpr |= here.gpr;
                pc[x].fpr |= here.fpr;
            }
        };
        record(&live, &mut phys_conflict);
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    live.insert(sp.idx(*r));
                }
            }
            // WHAT DIES HERE IS AMONG WHAT THIS INSTRUCTION READS. `last_use_into`
            // writes `at[x] = i` only for a register instruction `i` uses, so
            // `last[x] == Some(i)` implies `x` is one of its operands — and the
            // operands are already in hand. Scanning the whole live set instead
            // costs the set's size at every program point, which is the shape
            // this file's own doc-comment warns about.
            for (r, c) in &ops {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    let x = sp.idx(*r);
                    if last[x] == Some(i) {
                        live.remove(&x);
                    }
                }
            }
            record(&live, &mut phys_conflict);
        }
    }

    Liveness {
        sp,
        live_in,
        live_out,
        crosses_call,
        phys_conflict,
    }
}

/// Last use of each register INSIDE block `b`, as an instruction index; `n` for
/// the terminator and `usize::MAX` when the value is live out (never freed
/// inside this block). Both the spiller and the colourer walk a block forward
/// maintaining a live set, and this is what tells them when to drop a value —
/// without it a "live set" only ever grows and every pressure reading is wrong.
/// For each value, the index of its LAST use inside block `b` — `usize::MAX`
/// when it escapes through `live_out`, `None` when it is not used here at all.
///
/// COMPLEXITY: the result is indexed by value, but it is written into a buffer
/// the CALLER owns and only the entries this block touches are reset. Allocating
/// and zeroing a fresh `Vec<Option<usize>>` of every value in the function, once
/// per block, is quadratic — invisible on a small function and the dominant cost
/// on a real one.
pub fn last_use_into(f: &MFunc, sp: Space, lv: &Liveness, b: usize, buf: &mut LastUse) {
    buf.reset();
    let blk = &f.blocks[b];
    let mut set = |buf: &mut LastUse, i: usize, at: usize| {
        if buf.at[i].is_none() {
            buf.touched.push(i);
        }
        buf.at[i] = Some(at);
    };
    for (i, inst) in blk.insts.iter().enumerate() {
        let mut hits: Vec<usize> = Vec::new();
        inst.visit(&mut |r, c| {
            if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                hits.push(sp.idx(r));
            }
        });
        for h in hits {
            set(buf, h, i);
        }
    }
    let n = blk.insts.len();
    let mut hits: Vec<usize> = Vec::new();
    blk.term.visit(&mut |r, _| hits.push(sp.idx(r)));
    for h in hits {
        set(buf, h, n);
    }
    for &i in &lv.live_out[b] {
        set(buf, i, usize::MAX);
    }
}

/// A reusable last-use buffer (see `last_use_into`).
pub struct LastUse {
    pub at: Vec<Option<usize>>,
    touched: Vec<usize>,
}

impl LastUse {
    pub fn new(sp: Space) -> LastUse {
        LastUse {
            at: vec![None; sp.len()],
            touched: Vec::new(),
        }
    }
    fn reset(&mut self) {
        for &i in &self.touched {
            self.at[i] = None;
        }
        self.touched.clear();
    }
    /// The entries this block actually wrote. A reader that walks the whole
    /// value space instead pays for every value in the function at every block,
    /// which is the quadratic this list exists to avoid.
    pub fn touched(&self) -> &[usize] {
        &self.touched
    }
}
