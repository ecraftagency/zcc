// SSA destruction and the parallel-copy sequentializer (REARCH.md §7.5).
//
// With block parameters the whole of SSA destruction is: each edge's arguments
// become ONE parallel copy into the successor's parameters. There is no
// lost-copy problem and no swap problem to reason about — those are artifacts of
// φ instructions, whose reads happen "on the edge" only by convention. Here the
// edge is a real place, and the copy goes in it.
//
// The copy is parallel (simultaneous), so it must then be sequentialized. That
// is the windmill algorithm: emit any copy whose destination is nobody's source;
// when only cycles remain, break one with the reserved scratch register (x16 =
// AAPCS64 IP0, v31), which is exactly what those registers are reserved for.
use crate::mir::*;

/// A critical edge — several successors on one side, several predecessors on the
/// other — has no block to put an edge copy in, so one is created. (Same
/// theorem as `hir::dom::split_critical_edges`; MIR needs its own because isel
/// creates blocks of its own, e.g. a switch compare chain.)
pub fn split_critical_edges(f: &mut MFunc) -> bool {
    let cfg = crate::mir::verify::cfg(f);
    let mut split = false;
    for b in 0..f.blocks.len() as MBlockId {
        if !cfg.reachable(b) || cfg.succs[b as usize].len() < 2 {
            continue;
        }
        let mut term = f.blocks[b as usize].term.clone();
        for t in term.targets_mut() {
            // A critical edge proper, AND any argument-carrying edge out of a
            // block whose terminator reads registers: the edge copy is emitted
            // before that terminator, so it must not be able to overwrite what
            // the terminator is about to read.
            if cfg.preds[t.block as usize].len() < 2 && t.args.is_empty() {
                continue;
            }
            let mid = f.new_block();
            let args = std::mem::take(&mut t.args);
            f.blocks[mid as usize].weight = f.blocks[b as usize].weight;
            f.blocks[mid as usize].term = MTerm::B(MTarget {
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

/// Replace every register in the function by its assigned colour.
pub fn apply_colors(f: &mut MFunc, color: &[Option<PReg>]) -> Result<(), String> {
    let mut bad = None;
    let mut map = |r: &mut Reg| {
        if let Reg::V(v) = *r {
            match color[v as usize] {
                Some(p) => *r = Reg::P(p),
                None => bad = Some(v),
            }
        }
    };
    for b in f.blocks.iter_mut() {
        for p in b.params.iter_mut() {
            map(p);
        }
        for i in b.insts.iter_mut() {
            i.visit_mut(&mut |r, _| map(r));
        }
        b.term.visit_mut(&mut |r, _| map(r));
    }
    match bad {
        // Only an unreachable block can hold an uncoloured value (coloring walks
        // the dominator tree, which by definition covers exactly the reachable
        // blocks), and unreachable code is deleted by `layout`.
        Some(v) => Err(format!("{}: v{} was never coloured", f.name, v)),
        None => Ok(()),
    }
}

/// Turn each edge's arguments into a parallel copy in the predecessor, then
/// drop the block parameters.
pub fn destruct(f: &mut MFunc) {
    for b in 0..f.blocks.len() {
        let mut term = f.blocks[b].term.clone();
        let mut copies: Vec<(Reg, Reg, Width)> = Vec::new();
        for t in term.targets_mut() {
            if t.args.is_empty() {
                continue;
            }
            let params = f.blocks[t.block as usize].params.clone();
            for (p, a) in params.iter().zip(std::mem::take(&mut t.args)) {
                let w = width_of(f, *p);
                copies.push((*p, a, w));
            }
        }
        f.blocks[b].term = term;
        if !copies.is_empty() {
            f.blocks[b].insts.push(MInst::ParallelCopy(copies));
        }
    }
    for b in f.blocks.iter_mut() {
        b.params.clear();
    }
}

fn width_of(f: &MFunc, r: Reg) -> Width {
    match r {
        Reg::V(v) => f.vregs[v as usize].width,
        Reg::P(p) => match p.class {
            Class::Fpr => Width::D,
            _ => Width::W64,
        },
    }
}

/// Expand every `ParallelCopy` into a legal sequence of moves and drop the
/// self-moves that biased coloring made redundant.
pub fn sequentialize(f: &mut MFunc) {
    for b in f.blocks.iter_mut() {
        let insts = std::mem::take(&mut b.insts);
        let mut out = Vec::with_capacity(insts.len());
        for i in insts {
            match i {
                MInst::ParallelCopy(pairs) => out.extend(seq_copy(pairs)),
                MInst::Copy { dst, src, .. } if dst == src => {}
                MInst::FMov { dst, src, dw, sw } if dst == src && dw == sw => {}
                other => out.push(other),
            }
        }
        b.insts = out;
    }
}

/// The windmill: emit a copy whose destination is not still needed as a source;
/// when every remaining copy is in a cycle, route one through the scratch
/// register and continue. Terminates because each step removes one pair.
fn seq_copy(pairs: Vec<(Reg, Reg, Width)>) -> Vec<MInst> {
    let mut todo: Vec<(Reg, Reg, Width)> = pairs.into_iter().filter(|(d, s, _)| d != s).collect();
    let mut out = Vec::with_capacity(todo.len() + 2);
    while !todo.is_empty() {
        match todo
            .iter()
            .position(|(d, _, _)| !todo.iter().any(|(_, s, _)| s == d))
        {
            Some(k) => {
                let (d, s, w) = todo.remove(k);
                out.push(mov(d, s, w));
            }
            None => {
                // a pure cycle: park one source in the scratch register and
                // rewrite the copy that reads it
                let (_, s, w) = todo[0];
                // park at FULL width: the cycle may mix w- and x-forms, and a
                // 32-bit park would drop the upper half of an x value
                let (scratch, pw) = match w.class() {
                    Class::Fpr => (Reg::P(isa::SCRATCH_FPR), Width::D),
                    _ => (Reg::P(isa::SCRATCH_GPR), Width::W64),
                };
                out.push(mov(scratch, s, pw));
                for (_, q, _) in todo.iter_mut() {
                    if *q == s {
                        *q = scratch;
                    }
                }
            }
        }
    }
    out
}

fn mov(dst: Reg, src: Reg, w: Width) -> MInst {
    match w.class() {
        Class::Fpr => MInst::FMov {
            dw: w,
            sw: w,
            dst,
            src,
        },
        _ => MInst::Copy { w, dst, src },
    }
}
