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
    // A copy that biased colouring turned into a self-move is usually nothing —
    // but `mov w0, w0` TRUNCATES (DDI 0487 B1.2.1 zeroes bits 63:32 on every
    // 32-bit write), so deleting it is only right when nobody looks at the upper
    // half. This is the only place both facts are available: the virtual
    // identity, which says how wide the value is ever READ, and the colour, which
    // says whether the move is a self-move at all. `(int)someLong` is exactly a
    // `mov w, w` and nothing more, and sqlite has 20,000 of them.
    //
    // The decision is a FIXPOINT, not a single pass, because deleting one copy
    // changes what the next one is asked about: with `t1 = trunc x; t2 = copy
    // t1; use64 t2`, `t1` looks 32-bit-only — until `t2`'s copy is deleted and
    // `t1` inherits its 64-bit reader. Start optimistic and give up one candidate
    // at a time (yarpgen s0131).
    let widths: Vec<Width> = f.vregs.iter().map(|v| v.width).collect();
    let mut cand: Vec<(usize, usize, VReg, VReg, Width)> = Vec::new();
    for (b, blk) in f.blocks.iter().enumerate() {
        for (i, inst) in blk.insts.iter().enumerate() {
            let (w, dst, src) = match inst {
                MInst::Copy { w, dst, src } => (*w, *dst, *src),
                MInst::FMov { dw, sw, dst, src } if dw == sw => (*dw, *dst, *src),
                _ => continue,
            };
            if let (Reg::V(d), Reg::V(s)) = (dst, src) {
                if color[d as usize] == color[s as usize] {
                    cand.push((b, i, d, s, w));
                }
            }
        }
    }
    let mut drop = vec![true; cand.len()];
    for _ in 0..cand.len() + 1 {
        let reads = max_read(f, &cand, &drop);
        let mut changed = false;
        for (k, &(_, _, d, s, w)) in cand.iter().enumerate() {
            if !drop[k] {
                continue;
            }
            let ok = matches!(w, Width::W64 | Width::Q)
                || widths[s as usize] == w
                || reads[d as usize] <= w.bytes();
            if !ok {
                drop[k] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut kill: Vec<(usize, usize)> = cand
        .iter()
        .enumerate()
        .filter(|(k, _)| drop[*k])
        .map(|(_, &(b, i, ..))| (b, i))
        .collect();
    kill.sort_unstable();
    for &(b, i) in kill.iter().rev() {
        f.blocks[b].insts.remove(i);
    }
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
    // A self-move is only a NO-OP at the register's full width. `mov w0, w0` is
    // not: DDI 0487 B1.2.1 makes every 32-bit write ZERO bits 63:32, so the
    // instruction truncates, and deleting it leaves whatever the upper half held.
    // It is redundant only when the source was itself produced at 32 bits, which
    // is exactly what the source's own width records. Latent until biased
    // colouring started handing a copy its source's register on purpose
    // (yarpgen s0131 and nine others).
    let widths: Vec<Width> = f.vregs.iter().map(|v| v.width).collect();
    let nop = |dst: Reg, src: Reg, w: Width| -> bool {
        if dst != src {
            return false;
        }
        match w {
            Width::W64 | Width::Q => true,
            // The upper half only matters to whoever READS the register, and a
            // narrow virtual register is only ever read narrow — so the move is
            // redundant when the DESTINATION is one. A physical destination
            // carries no width record (an ABI register may be read whole), and a
            // narrow write into a wider name really does truncate.
            _ => match src {
                Reg::V(v) => widths[v as usize] == w,
                // a physical source carries no width record — an ABI register's
                // upper half is whatever the caller left there
                Reg::P(_) => false,
            },
        }
    };
    for b in f.blocks.iter_mut() {
        let insts = std::mem::take(&mut b.insts);
        let mut out = Vec::with_capacity(insts.len());
        for i in insts {
            match i {
                MInst::ParallelCopy(pairs) => out.extend(seq_copy(pairs, &nop)),
                MInst::Copy { dst, src, w } if nop(dst, src, w) => {}
                MInst::FMov { dst, src, dw, sw } if dw == sw && nop(dst, src, dw) => {}
                other => out.push(other),
            }
        }
        b.insts = out;
    }
}

/// The windmill: emit a copy whose destination is not still needed as a source;
/// when every remaining copy is in a cycle, route one through the scratch
/// register and continue. Terminates because each step removes one pair.
fn seq_copy(pairs: Vec<(Reg, Reg, Width)>, nop: &dyn Fn(Reg, Reg, Width) -> bool) -> Vec<MInst> {
    // the same rule as above: a same-register pair is only droppable when the
    // move would not have truncated
    let mut todo: Vec<(Reg, Reg, Width)> =
        pairs.into_iter().filter(|&(d, s, w)| !nop(d, s, w)).collect();
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

/// For each virtual register, the WIDEST access any reader makes of it, in
/// bytes. Anything this does not recognize counts as a full 8-byte read, so an
/// instruction form added later is conservative until it is listed here.
fn max_read(f: &MFunc, cand: &[(usize, usize, VReg, VReg, Width)], drop: &[bool]) -> Vec<u32> {
    let mut out = vec![0u32; f.vregs.len()];
    // a copy that is going to DISAPPEAR does not narrow anything: its source
    // inherits whatever its destination is read at
    let dropped: std::collections::HashMap<(usize, usize), (VReg, VReg)> = cand
        .iter()
        .enumerate()
        .filter(|(k, _)| drop[*k])
        .map(|(_, &(b, i, d, s, _))| ((b, i), (d, s)))
        .collect();
    let mut note = |r: Reg, bytes: u32, out: &mut Vec<u32>| {
        if let Reg::V(v) = r {
            out[v as usize] = out[v as usize].max(bytes);
        }
    };
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            if dropped.contains_key(&(bi, ii)) {
                continue; // handled by the propagation below
            }
            let (regs, bytes): (Vec<Reg>, u32) = match i {
                MInst::Alu { w, a, b, .. } => {
                    let mut v = vec![*a];
                    if let Rhs::Reg(r) | Rhs::Shifted(r, ..) | Rhs::Extended(r, ..) = b {
                        v.push(*r);
                    }
                    (v, w.bytes())
                }
                MInst::Alu3 { w, a, b, c, .. } => (vec![*a, *b, *c], w.bytes()),
                MInst::Cmp { w, a, b, .. } => {
                    let mut v = vec![*a];
                    if let Rhs::Reg(r) | Rhs::Shifted(r, ..) | Rhs::Extended(r, ..) = b {
                        v.push(*r);
                    }
                    (v, w.bytes())
                }
                MInst::CSel { w, a, b, .. } => (vec![*a, *b], w.bytes()),
                MInst::Copy { w, src, .. } => (vec![*src], w.bytes()),
                MInst::FMov { sw, src, .. } => (vec![*src], sw.bytes()),
                MInst::Bfx { w, src, .. } => (vec![*src], w.bytes()),
                // an extension reads only the bits it names
                MInst::Ext { op, src, .. } => (
                    vec![*src],
                    match op {
                        ExtOp::Sxtb | ExtOp::Uxtb => 1,
                        ExtOp::Sxth | ExtOp::Uxth => 2,
                        ExtOp::Sxtw => 4,
                    },
                ),
                MInst::Store { op, src, .. } => (vec![*src], op.bytes()),
                MInst::Spill { w, src, .. } => (vec![*src], w.bytes()),
                MInst::Pair { w, a, b, load: false, .. } => (vec![*a, *b], w.bytes()),
                _ => {
                    // unknown form: every operand is read whole
                    let mut v = Vec::new();
                    i.visit(&mut |r, c| {
                        if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                            v.push(r);
                        }
                    });
                    (v, 8)
                }
            };
            for r in regs {
                note(r, bytes, &mut out);
            }
            // an ADDRESS is always a 64-bit read, whatever the access width
            i.visit(&mut |r, c| {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    if let MInst::Load { mem, .. } | MInst::Store { mem, .. } | MInst::Pair { mem, .. } = i {
                        let mut addr = Vec::new();
                        match mem {
                            AddrMode::BaseImm { base, .. } | AddrMode::SymLo12 { base, .. } => {
                                addr.push(*base)
                            }
                            AddrMode::BaseReg { base, idx, .. } => {
                                addr.push(*base);
                                addr.push(*idx);
                            }
                            AddrMode::PreIdx { base, .. } | AddrMode::PostIdx { base, .. } => {
                                addr.push(*base)
                            }
                            _ => {}
                        }
                        if addr.contains(&r) {
                            note(r, 8, &mut out);
                        }
                    }
                }
            });
        }
        // a terminator's own operand, and every edge argument
        b.term.visit(&mut |r, _| note(r, 8, &mut out));
    }
    // propagate through the copies that are going away, to a fixpoint: a chain
    // of them passes the widest reader all the way back to the real producer
    for _ in 0..dropped.len() + 1 {
        let mut changed = false;
        for (d, s) in dropped.values() {
            let want = out[*d as usize];
            if out[*s as usize] < want {
                out[*s as usize] = want;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}
