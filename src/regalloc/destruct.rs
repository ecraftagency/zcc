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
    drop_self_moves(f, color);
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

/// One self-move candidate: a copy — standalone, or one pair of a
/// `ParallelCopy` — whose destination and source have landed on the SAME
/// physical register.
#[derive(Clone, Copy)]
struct Cand {
    b: usize,
    i: usize,
    /// `Some(k)`: pair `k` of the `ParallelCopy` at `(b, i)`; `None`: the whole
    /// instruction at `(b, i)`.
    pair: Option<usize>,
    dst: Reg,
    src: Reg,
    w: Width,
}

/// The physical register a name has landed on: its colour if it is virtual,
/// itself if it is already physical.
fn preg_of(r: Reg, color: &[Option<PReg>]) -> Option<PReg> {
    match r {
        Reg::V(v) => color.get(v as usize).copied().flatten(),
        Reg::P(p) => Some(p),
    }
}

/// Delete the copies that biased colouring turned into self-moves.
///
/// A self-move is usually nothing — but `mov w0, w0` TRUNCATES (DDI 0487
/// B1.2.1 zeroes bits 63:32 on every 32-bit write), so deleting it is only
/// right when nobody looks at the upper half. This is the only place both facts
/// are available: the virtual identity, which says how wide the value is ever
/// READ, and the colour, which says whether the move is a self-move at all.
/// `(int)someLong` is exactly a `mov w, w` and nothing more, and sqlite has
/// 20,000 of them.
///
/// The decision is a FIXPOINT, not a single pass, because deleting one copy
/// changes what the next one is asked about: with `t1 = trunc x; t2 = copy t1;
/// use64 t2`, `t1` looks 32-bit-only — until `t2`'s copy is deleted and `t1`
/// inherits its 64-bit reader. Start optimistic and give up one candidate at a
/// time (yarpgen s0131).
///
/// **R4.2** — the candidate set is not only the virtual/virtual copies. A name
/// that has landed on a physical register is a self-move partner just as much
/// as a virtual one, and the three shapes this adds are, on sqlite, 13,322
/// instructions of pure no-op:
///
///   * `V ← P` — a call result read back, or an incoming parameter copied out
///     of its argument register. Answered by the machinery already here: the
///     copy goes when no reader of the destination looks past `w`.
///   * `P ← V` and `P ← P` — a value placed in a fixed ARGUMENT or RESULT
///     register. `max_read` cannot answer this one (a physical destination has
///     no virtual identity, so it has no readers to catalogue); `abi_reader`
///     answers it from AAPCS64 instead.
///
/// Doing it HERE, before `destruct`, matters twice over. Every `ParallelCopy`
/// in the function is still isel's ABI marshalling — SSA destruction has not
/// yet created any edge copy — so a physical destination is an ABI-fixed
/// operand by construction, which is what makes `abi_reader`'s question
/// answerable. And a narrow identity pair left in a `ParallelCopy` reads to the
/// windmill as a one-element CYCLE: it breaks it through the scratch register
/// and emits `mov x16, xN ; mov wN, w16`, two instructions where the right
/// answer is none (4,208 of these on sqlite).
fn drop_self_moves(f: &mut MFunc, color: &[Option<PReg>]) {
    let widths: Vec<Width> = f.vregs.iter().map(|v| v.width).collect();
    let mut cand: Vec<Cand> = Vec::new();
    let mut add = |b: usize, i: usize, pair: Option<usize>, dst: Reg, src: Reg, w: Width| {
        let (pd, ps) = (preg_of(dst, color), preg_of(src, color));
        if pd.is_some() && pd == ps {
            cand.push(Cand { b, i, pair, dst, src, w });
        }
    };
    for (b, blk) in f.blocks.iter().enumerate() {
        for (i, inst) in blk.insts.iter().enumerate() {
            match inst {
                MInst::Copy { w, dst, src } => add(b, i, None, *dst, *src, *w),
                MInst::FMov { dw, sw, dst, src } if dw == sw => add(b, i, None, *dst, *src, *dw),
                MInst::ParallelCopy(pairs) => {
                    for (k, &(dst, src, w)) in pairs.iter().enumerate() {
                        add(b, i, Some(k), dst, src, w);
                    }
                }
                _ => {}
            }
        }
    }
    let mut drop = vec![true; cand.len()];
    for _ in 0..cand.len() + 1 {
        let reads = max_read(f, &cand, &drop);
        let mut changed = false;
        for (k, c) in cand.iter().enumerate() {
            if !drop[k] {
                continue;
            }
            let ok = match c.dst {
                // A virtual destination knows how wide it is ever read.
                Reg::V(d) => {
                    matches!(c.w, Width::W64 | Width::Q)
                        || matches!(c.src, Reg::V(s) if widths[s as usize] == c.w)
                        || reads[d as usize] <= c.w.bytes()
                }
                // A physical one does not; AAPCS64 answers instead.
                Reg::P(p) => abi_reader(f, c.b, c.i, p),
            };
            if !ok {
                drop[k] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    residual_report(f, &cand, &drop, &widths);
    // Remove back-to-front so earlier indices stay valid: pairs within one
    // instruction first, then whole instructions.
    let mut kill: Vec<(usize, usize, Option<usize>)> = cand
        .iter()
        .enumerate()
        .filter(|(k, _)| drop[*k])
        .map(|(_, c)| (c.b, c.i, c.pair))
        .collect();
    kill.sort_unstable();
    for &(b, i, pair) in kill.iter().rev() {
        match pair {
            // Dropping an IDENTITY pair leaves the simultaneous assignment the
            // `ParallelCopy` denotes unchanged: it assigned a register to
            // itself, so no other pair's source or destination is affected.
            Some(k) => {
                if let MInst::ParallelCopy(pairs) = &mut f.blocks[b].insts[i] {
                    pairs.remove(k);
                }
            }
            None => {
                f.blocks[b].insts.remove(i);
            }
        }
    }
    f.blocks.iter_mut().for_each(|b| {
        b.insts
            .retain(|i| !matches!(i, MInst::ParallelCopy(p) if p.is_empty()))
    });
}

/// LAW-4 RESIDUAL (`ZCC_R42RES=1`) — read-only, changes nothing.
///
/// A self-move that survives is refused for exactly one of three reasons, and
/// the row is not exhausted until every survivor is classified:
///
///   * `wide-read` — a reader genuinely looks past `w`, so the truncation has an
///     observer. FUNDAMENTAL (category (a)): the instruction is doing work.
///   * `unknown-form` — `max_read` met an `MInst` form it does not list and
///     charged the operand a full 8-byte read. A convenience truncation
///     (category (b)) in the ANALYSIS, not in the program: listing the form
///     lifts it.
///   * `no-abi-reader` — a physical destination whose reader is neither the next
///     `Call`'s argument list nor the block's `Ret`. Category (b) as well: a
///     wider reader analysis would decide it.
fn residual_report(f: &MFunc, cand: &[Cand], drop: &[bool], widths: &[Width]) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("ZCC_R42RES").is_some()) {
        return;
    }
    // what `max_read` would say with NOTHING dropped, i.e. the pessimistic bound
    let none = vec![false; cand.len()];
    let reads = max_read(f, cand, &none);
    // the vregs an unlisted instruction form charged a full 8-byte read
    let mut unknown = vec![false; f.vregs.len()];
    for b in f.blocks.iter() {
        for i in b.insts.iter() {
            if matches!(
                i,
                MInst::Alu { .. } | MInst::Alu3 { .. } | MInst::Cmp { .. } | MInst::CSel { .. }
                    | MInst::Copy { .. } | MInst::FMov { .. } | MInst::Bfx { .. }
                    | MInst::Ext { .. } | MInst::Store { .. } | MInst::Spill { .. }
                    | MInst::ParallelCopy(_) | MInst::Pair { .. }
            ) {
                continue;
            }
            i.visit(&mut |r, c| {
                if let (Reg::V(v), Constraint::Use | Constraint::UseFixed(_)) = (r, c) {
                    unknown[v as usize] = true;
                }
            });
        }
    }
    let (mut wide, mut unk, mut noabi) = (0usize, 0usize, 0usize);
    for (k, c) in cand.iter().enumerate() {
        if drop[k] {
            continue;
        }
        match c.dst {
            Reg::V(d) => {
                let _ = widths;
                if unknown[d as usize] && reads[d as usize] > c.w.bytes() {
                    unk += 1;
                } else {
                    wide += 1;
                }
            }
            Reg::P(_) => noabi += 1,
        }
    }
    if wide + unk + noabi > 0 {
        eprintln!("R42RES {} {} {} {}", f.name, wide, unk, noabi);
    }
}

/// Is the only reader of physical register `p`, after the copy at `(b, i)`, an
/// ABI boundary that reads it at its declared width?
///
/// **Side II — AAPCS64 §6.4.2 (arguments) and §6.8.2 (return values).** The
/// bits of a parameter or result register ABOVE the declared type's width are
/// UNSPECIFIED. A callee handed an `int` in x0 reads `w0` and may not look
/// higher; a caller taking an `int` result reads `w0` and may not look higher.
/// So a narrow write into such a register truncates something no conforming
/// program observes, and a narrow SELF-move into it is a no-op.
///
/// Only two shapes are accepted, and everything else is refused:
///
///   * the register is an argument of the very next `Call` — `uses` is exactly
///     the AAPCS64 assignment isel computed, at the width the pair carries;
///   * nothing in the rest of the block mentions the register and the block
///     RETURNS — so the only reader is the caller, at the declared result width.
///
/// Everything else is refused because the reader is not known to be an ABI one.
/// `Asm` in particular must never be accepted: an inline-asm template chooses
/// its own operand width and may name `%x0` for an `int` operand, so its upper
/// half is a real reader. It is refused by falling into the general
/// "mentions `p`" case below.
fn abi_reader(f: &MFunc, b: usize, i: usize, p: PReg) -> bool {
    for inst in f.blocks[b].insts[i + 1..].iter() {
        if let MInst::Call { uses, clobbers, .. } = inst {
            if uses.iter().any(|&(_, u)| u == p) {
                return true; // §6.4.2 — an argument, read at its declared width
            }
            if clobbers.has(p) {
                return false; // the value dies here; not our business
            }
        }
        let mut touched = false;
        inst.visit(&mut |r, _| touched |= r == Reg::P(p));
        if touched {
            return false;
        }
    }
    let mut touched = false;
    f.blocks[b].term.visit(&mut |r, _| touched |= r == Reg::P(p));
    // §6.8.2 — the result register, read by the caller at the declared width
    !touched && matches!(f.blocks[b].term, MTerm::Ret)
}

/// Turn each edge's arguments into a parallel copy in the predecessor, then
/// drop the block parameters.
pub fn destruct(f: &mut MFunc) -> usize {
    // how many of the parallel copies below are EDGE copies — the count the R4.2
    // prediction needs kept apart from isel's ABI marshalling, which uses the
    // very same instruction (see `movkind_report`).
    let mut edge_pairs = 0usize;
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
                if *p != a {
                    edge_pairs += 1;
                }
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
    edge_pairs
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
/// R4.2 PREDICTION (`ZCC_MOVKIND=1`) — read-only, changes nothing.
///
/// `mov` is 24% of sqlite's excess over gcc -O1, and §13n asks which KIND of
/// copy it is before any coalescer is written, because Boissinot's merge only
/// answers one of them. A copy reaching the emitter is one of three things and
/// they are distinguishable HERE, where each is still structurally what it is,
/// but not in the `.s`, where all three are the letters `mov`:
///
///   * EDGE    — a block argument becoming a `ParallelCopy` at SSA destruction.
///              Boissinot's merge is about exactly these, and about nothing else.
///   * ABI     — a `ParallelCopy` isel emitted to place call arguments, a return
///              value or a division's operands in the registers the AAPCS64
///              names. It is the SAME instruction as an edge copy and the first
///              draft of this instrument counted the two together, which
///              overstated the coalescer's ceiling by 2.8x. They are separated
///              because only one of them is a coalescing question at all: the
///              other is an argument-TARGETING question.
///   * WIDE    — a standalone 64-bit `Copy` surviving biased colouring: isel or
///              a pass wanted a value in a second name, or an ABI-pinned
///              register (a call argument, a return value, a division operand)
///              made the two names un-mergeable. The ABI ones are not a
///              coalescer's to remove — better argument TARGETING is what
///              removes those — so this column is mixed and is reported apart.
///   * NARROW  — a `w`-form standalone copy: the shape a narrowing takes when a
///              vreg's width is part of its identity. R4.3's subject, not R4.2's,
///              and counted here so R4.3 inherits a measured starting point.
///   * FP      — the same-width `FMov` form of the above.
///
/// The point of the split is that the `.s` cannot make it: there, all four are
/// the letters `mov`, and treating the total as one coalescing opportunity is
/// how a step gets aimed at the wrong layer.
fn movkind_report(f: &MFunc, edge: usize, abi: usize, wide: usize, narrow: usize, fp: usize) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("ZCC_MOVKIND").is_some()) {
        return;
    }
    if edge + abi + wide + narrow + fp > 0 {
        eprintln!("MOVKIND {} {} {} {} {} {}", f.name, edge, abi, wide, narrow, fp);
    }
}

pub fn sequentialize(f: &mut MFunc, edge_pairs: usize) {
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
    let (mut edge, mut wide, mut narrow, mut fp) = (0usize, 0usize, 0usize, 0usize);
    for b in f.blocks.iter_mut() {
        let insts = std::mem::take(&mut b.insts);
        let mut out = Vec::with_capacity(insts.len());
        for i in insts {
            match i {
                MInst::ParallelCopy(pairs) => {
                    let seq = seq_copy(pairs, &nop);
                    edge += seq.len();
                    out.extend(seq);
                }
                MInst::Copy { dst, src, w } if nop(dst, src, w) => {}
                MInst::FMov { dst, src, dw, sw } if dw == sw && nop(dst, src, dw) => {}
                other => {
                    match &other {
                        MInst::Copy { w: Width::W64, .. } => wide += 1,
                        MInst::Copy { .. } => narrow += 1,
                        MInst::FMov { dw, sw, .. } if dw == sw => fp += 1,
                        _ => {}
                    }
                    out.push(other);
                }
            }
        }
        b.insts = out;
    }
    movkind_report(f, edge_pairs, edge.saturating_sub(edge_pairs), wide, narrow, fp);
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
fn max_read(f: &MFunc, cand: &[Cand], drop: &[bool]) -> Vec<u32> {
    let mut out = vec![0u32; f.vregs.len()];
    // a copy that is going to DISAPPEAR does not narrow anything: its source
    // inherits whatever its destination is read at
    let dropped: std::collections::HashSet<(usize, usize, Option<usize>)> = cand
        .iter()
        .enumerate()
        .filter(|(k, _)| drop[*k])
        .map(|(_, c)| (c.b, c.i, c.pair))
        .collect();
    // …and only where BOTH ends are virtual is there anything to propagate to.
    let mut prop: Vec<(VReg, VReg)> = cand
        .iter()
        .enumerate()
        .filter(|(k, _)| drop[*k])
        .filter_map(|(_, c)| match (c.dst, c.src) {
            (Reg::V(d), Reg::V(s)) => Some((d, s)),
            _ => None,
        })
        .collect();
    // Every EDGE is a copy that may disappear the same way. SSA destruction
    // turns an argument into a copy into its parameter, and when the two share
    // a colour that copy is deleted — after which the argument IS the
    // parameter, and inherits every reader the parameter has. Counting the
    // argument at the parameter's WIDTH alone is therefore not enough: a `w`
    // parameter read as an `x` (`str x1` of a value the edge copy narrowed) is
    // exactly the shape that made this rule miscompile yarpgen s0188. The
    // parameter's width still goes in as the floor above, for the case where
    // the copy survives; this adds the case where it does not.
    for b in f.blocks.iter() {
        for t in b.term.targets() {
            let params = &f.blocks[t.block as usize].params;
            for (k, a) in t.args.iter().enumerate() {
                if let (Some(Reg::V(p)), Reg::V(a)) = (params.get(k).copied(), a) {
                    prop.push((p, *a));
                }
            }
        }
    }
    let mut note = |r: Reg, bytes: u32, out: &mut Vec<u32>| {
        if let Reg::V(v) = r {
            out[v as usize] = out[v as usize].max(bytes);
        }
    };
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            if dropped.contains(&(bi, ii, None)) {
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
                // Each pair reads its own source at its OWN width, so the
                // instruction has no single one — note them here and contribute
                // nothing to the common path. Without this arm a `ParallelCopy`
                // falls into the catch-all below and every argument counts as a
                // full 8-byte read, which is what used to hide every narrow
                // call-argument copy from this analysis.
                MInst::ParallelCopy(pairs) => {
                    for (k, (_, s, w)) in pairs.iter().enumerate() {
                        if !dropped.contains(&(bi, ii, Some(k))) {
                            note(*s, w.bytes(), &mut out);
                        }
                    }
                    (Vec::new(), 0)
                }
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
        // A terminator's own operand is read at the terminator's own width, and
        // an edge ARGUMENT at the width of the parameter it will be copied into
        // — which is exactly the width `destruct` gives the edge copy. Counting
        // either as a full 8 bytes is the conservatism that used to keep every
        // narrow call result (`w0` copied out after a `bl` and passed along an
        // edge) out of reach of this analysis.
        {
            match &b.term {
                MTerm::Cbz { w, reg, .. } | MTerm::Tb { w, reg, .. } => note(*reg, w.bytes(), &mut out),
                MTerm::Bcc(_, r, ..) | MTerm::Switch { idx: r, .. } | MTerm::BrReg(r, _) => {
                    note(*r, 8, &mut out)
                }
                MTerm::B(_) | MTerm::Ret | MTerm::Unreachable => {}
            }
            for t in b.term.targets() {
                let params = &f.blocks[t.block as usize].params;
                for (k, a) in t.args.iter().enumerate() {
                    let w = params.get(k).map_or(8, |p| width_of(f, *p).bytes());
                    note(*a, w, &mut out);
                }
            }
        }
    }
    // propagate through the copies that are going away, to a fixpoint: a chain
    // of them passes the widest reader all the way back to the real producer
    for _ in 0..prop.len() + 1 {
        let mut changed = false;
        for (d, s) in prop.iter() {
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
