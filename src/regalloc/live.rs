// Liveness on MIR-SSA with block parameters (REARCH.md §7.1).
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

    // per-block use/def summaries (uses = read before any local definition)
    let mut uses = vec![BTreeSet::new(); n];
    let mut defs = vec![BTreeSet::new(); n];
    for (bi, blk) in f.blocks.iter().enumerate() {
        let (u, d) = (&mut uses[bi], &mut defs[bi]);
        for p in &blk.params {
            d.insert(sp.idx(*p));
        }
        for inst in &blk.insts {
            inst.visit(&mut |r, c| match c {
                Constraint::Use | Constraint::UseFixed(_) => {
                    let i = sp.idx(r);
                    if !d.contains(&i) {
                        u.insert(i);
                    }
                }
                Constraint::Def | Constraint::DefFixed(_) => {
                    d.insert(sp.idx(r));
                }
            });
        }
        // the terminator's own operands; edge arguments are handled by live_out
        blk.term.visit(&mut |r, _| {
            let i = sp.idx(r);
            if !d.contains(&i) {
                u.insert(i);
            }
        });
    }

    let mut changed = true;
    while changed {
        changed = false;
        // reverse RPO converges fastest for a backward problem
        for &b in cfg.rpo.iter().rev() {
            let b = b as usize;
            let mut out = BTreeSet::new();
            for t in f.blocks[b].term.targets() {
                out.extend(live_in[t.block as usize].iter().copied());
                for a in &t.args {
                    out.insert(sp.idx(*a));
                }
            }
            // a computed goto has successors but carries no arguments
            for &s in &cfg.succs[b] {
                out.extend(live_in[s as usize].iter().copied());
            }
            let mut inn = uses[b].clone();
            inn.extend(out.iter().filter(|i| !defs[b].contains(i)).copied());
            if out != live_out[b] || inn != live_in[b] {
                live_out[b] = out;
                live_in[b] = inn;
                changed = true;
            }
        }
    }

    // A value crosses a call when it is live immediately before the call and
    // still live after it.
    let mut crosses_call = vec![false; sp.nv];
    for (bi, blk) in f.blocks.iter().enumerate() {
        if !cfg.reachable(bi as MBlockId) {
            continue;
        }
        let mut live: BTreeSet<usize> = live_out[bi].clone();
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
    for bi in 0..f.blocks.len() {
        if !cfg.reachable(bi as MBlockId) {
            continue;
        }
        let last = last_use(f, sp, &pre, bi);
        let mut live: BTreeSet<usize> = live_in[bi].clone();
        for &p in &f.blocks[bi].params {
            live.insert(sp.idx(p));
        }
        let mut record = |live: &BTreeSet<usize>, pc: &mut Vec<RegSet>| {
            let phys: Vec<PReg> = live
                .iter()
                .filter(|&&x| x >= sp.nv)
                .filter_map(|&x| sp.reg(x).preg())
                .collect();
            if phys.is_empty() {
                return;
            }
            for &x in live.iter().filter(|&&x| x < sp.nv) {
                for p in &phys {
                    pc[x].add(*p);
                }
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
            let dead: Vec<usize> = live
                .iter()
                .copied()
                .filter(|&x| last[x] == Some(i))
                .collect();
            for x in dead {
                live.remove(&x);
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
pub fn last_use(f: &MFunc, sp: Space, lv: &Liveness, b: usize) -> Vec<Option<usize>> {
    let blk = &f.blocks[b];
    let mut last: Vec<Option<usize>> = vec![None; sp.len()];
    for (i, inst) in blk.insts.iter().enumerate() {
        inst.visit(&mut |r, c| {
            if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                last[sp.idx(r)] = Some(i);
            }
        });
    }
    let n = blk.insts.len();
    blk.term.visit(&mut |r, _| last[sp.idx(r)] = Some(n));
    for &i in &lv.live_out[b] {
        last[i] = Some(usize::MAX);
    }
    last
}
