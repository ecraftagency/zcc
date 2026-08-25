// Chordal coloring of the SSA interference graph (REARCH.md §7.3).
// THEORY A7 — chordal colouring in dominance order (Hack 2007)
//
// THE theorem this whole re-architecture rests on (Hack 2007): the interference
// graph of a program in SSA form is CHORDAL, and a preorder of the dominator
// tree is a perfect elimination order for it. Greedy coloring along that order
// is therefore OPTIMAL — it uses exactly ω(G) = the maximum register pressure
// colors, and it CANNOT get stuck once pressure ≤ k. No graph is ever built, no
// node is ever merged, and there is no coalesce/simplify/spill iteration:
// rc3 ran Chaitin-Briggs AFTER SSA destruction, on a graph that is not chordal,
// where the same greedy walk has no such guarantee.
//
// Two rules cover everything AAPCS64 asks for, with no special cases:
//   * a value live across a call may not take a caller-saved colour (§6.1.1) —
//     `Liveness::crosses_call`;
//   * argument and result registers are ordinary physical registers made live
//     by the parallel copies isel places around a call, so they interfere the
//     normal way.
//
// Coalescing is BIASED COLORING (§7.4): at a definition that is a copy, prefer
// the partner's colour when it is free. It never merges nodes and so can never
// break the pressure guarantee — the property Chaitin-Briggs coalescing has to
// be careful about.
use super::live::{Liveness, Space};
use super::live;
use crate::cfg::DomTree;
use crate::mir::*;
use std::collections::BTreeSet;

/// A colouring failure names the VALUE that ran out of registers, because the
/// caller can act on that: force it into memory and try again.
pub enum ColorErr {
    /// the value that ran out of registers, every VIRTUAL value holding one of
    /// its class at that moment (the caller may force any of them into memory
    /// instead), and the message
    NoColour(VReg, Vec<VReg>, String),
    Other(String),
}

impl From<ColorErr> for String {
    fn from(e: ColorErr) -> String {
        match e {
            ColorErr::NoColour(_, _, m) | ColorErr::Other(m) => m,
        }
    }
}

pub struct Coloring {
    /// colour of each virtual register, once assigned
    pub color: Vec<Option<PReg>>,
    /// physical registers this function actually defines (the callee-saved
    /// subset of which the prologue must preserve)
    pub used: RegSet,
}

fn class_of(f: &MFunc, r: Reg) -> Class {
    match r {
        Reg::V(v) => f.vregs[v as usize].class,
        Reg::P(p) => p.class,
    }
}

pub fn color(f: &MFunc, lv: &Liveness, dt: &DomTree) -> Result<Coloring, ColorErr> {
    let sp = lv.sp;
    let partners = copy_partners(f, sp);
    // Once the function contains a call the file is partitioned (see
    // `isa::caller_saved_mask`): values live across a call take the callee-saved
    // half, everything else the caller-saved half. In a call-free function no
    // value is constrained and the whole file is one pool.
    let has_calls = f
        .blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| matches!(i, MInst::Call { .. })));
    let mut color: Vec<Option<PReg>> = vec![None; sp.nv];
    let mut used = RegSet::default();
    let mut lu = super::live::LastUse::new(sp);

    for &b in &dt.preorder {
        let bi = b as usize;
        let blk = &f.blocks[bi];
        // How many LIVE values hold each register right now. A count, not a set:
        // a physical register is not SSA — the same one is defined again and
        // again — so "is it taken" cannot be answered by remembering that
        // someone once took it, and a set that only ever gains entries runs the
        // block out of colours it actually has.
        let mut occ = Occupancy::new();
        let mut live_here: BTreeSet<usize> = lv.live_in[bi].clone();
        for &i in &live_here {
            if let Some(p) = color_of(&color, sp, i) {
                occ.add(p);
            }
        }

        super::live::last_use_into(f, sp, lv, bi, &mut lu);
        let last = &lu.at;

        // Block parameters are defined at the block's entry. A parameter always
        // OCCUPIES its register for the length of the block, even when nothing
        // reads it: SSA destruction materializes an edge copy into it either way,
        // so letting a second parameter share the register would let one copy
        // destroy the other. (Dead parameters are removed before colouring —
        // `regalloc::prune_dead_params` — so this is not a lost opportunity.)
        for &p in &blk.params {
            assign(f, lv, &mut color, &mut used, &mut occ, p, &partners, has_calls)
                .map_err(|e| with_holders(e, &live_here, &color, sp, f, lv))?;
            if live_here.insert(sp.idx(p)) {
                if let Some(c) = color_of(&color, sp, sp.idx(p)) {
                    occ.add(c);
                }
            }
        }

        for (i, inst) in blk.insts.iter().enumerate() {
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            // An operand that DIES here hands its register to this
            // instruction's destination, because on A64 an instruction reads
            // every source before it writes any destination — so the register is
            // free the instant the reads are done, and the definition taking it
            // is what turns a loop-carried edge copy into a self-move that
            // `sequentialize` deletes.
            //
            // This used to apply to a plain `Copy` alone, with the conservative
            // order kept "for everything else" to avoid the case analysis. The
            // case analysis is `reads_before_writes`, it is six lines, and the
            // convenience cost exactly the copy this branch was measured on:
            // `add w1, w0, #1 ; mov x0, w1` at the bottom of every counted loop,
            // because w1 was placed while the dying w0 still held its register.
            if reads_before_writes(inst) {
                let mut dying: Vec<usize> = Vec::new();
                inst.visit(&mut |r, c| {
                    if matches!(c, Constraint::Use) {
                        dying.push(sp.idx(r));
                    }
                });
                for x in dying {
                    if last[x] == Some(i) && live_here.remove(&x) {
                        if let Some(p) = color_of(&color, sp, x) {
                            occ.sub(p);
                        }
                    }
                }
            }
            // A pre/post-index writeback updates the BASE register in place —
            // `emit` prints only the base — so `wb` MUST take the base's physical
            // register. `auto_inc` folds only when the base dies at the access, so
            // hand its colour to `wb` here, BEFORE the transfer register is
            // placed: the transfer register then cannot take it and Xt != Xn holds
            // by construction (DDI 0487 C6.2). base leaves the live set and wb
            // enters the SAME register, so occupancy is unchanged. `check` asserts
            // the tie; a base that did NOT die here leaves both live in one
            // register and is caught there rather than miscompiled.
            if let MInst::Load { mem, .. } | MInst::Store { mem, .. } = inst {
                if let AddrMode::PreIdx { base, wb, .. } | AddrMode::PostIdx { base, wb, .. } =
                    mem
                {
                    let bp = phys_of(&color, sp, *base);
                    color[sp.idx(*wb)] = bp;
                    if let Some(p) = bp {
                        used.add(p);
                    }
                    if last[sp.idx(*base)] == Some(i) {
                        live_here.remove(&sp.idx(*base));
                    }
                    live_here.insert(sp.idx(*wb));
                }
            }
            // R4.3 — A PARALLEL-COPY DESTINATION MAY TAKE ITS OWN DYING
            // SOURCE'S REGISTER. The simultaneity argument below forbids a
            // destination from taking ANOTHER pair's dying source: that pair
            // still has to read it. It says nothing about a destination taking
            // the source of ITS OWN pair — that assignment makes the pair a
            // self-move, which writes nothing, so every other pair reads exactly
            // what it read before. §13n measured the cost of not distinguishing
            // them: 2.22 register movs per call against gcc's 1.00, and roughly
            // three copies at the entry of every function moving arguments out
            // of x0–x7.
            //
            // The colour is freed for THIS destination only and re-occupied the
            // moment the bias declines it, so no other value can slip into it.
            // `check` still asserts that no two live values share a colour.
            if let MInst::ParallelCopy(pairs) = inst {
                for (d, s, _) in pairs.clone() {
                    let Reg::V(_) = d else { continue };
                    let (di, si) = (sp.idx(d), sp.idx(s));
                    if color[di].is_some() || last[si] != Some(i) || !live_here.contains(&si) {
                        continue;
                    }
                    let Some(sc) = color_of(&color, sp, si) else { continue };
                    occ.sub(sc);
                    let r = assign(
                        f, lv, &mut color, &mut used, &mut occ, d, &partners, has_calls,
                    );
                    let got = color_of(&color, sp, di);
                    if got == Some(sc) {
                        // the source handed its register over; it is no longer
                        // live and must not be freed a second time below
                        live_here.remove(&si);
                    } else {
                        occ.add(sc);
                    }
                    r.map_err(|e| with_holders(e, &live_here, &color, sp, f, lv))?;
                    if live_here.insert(di) {
                        if let Some(p) = got {
                            occ.add(p);
                        }
                    }
                }
            }
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    assign(f, lv, &mut color, &mut used, &mut occ, *r, &partners, has_calls)
                        .map_err(|e| with_holders(e, &live_here, &color, sp, f, lv))?;
                    if live_here.insert(sp.idx(*r)) {
                        if let Some(p) = color_of(&color, sp, sp.idx(*r)) {
                            occ.add(p);
                        }
                    }
                }
            }
            // Free colours only AFTER the definitions of this instruction are
            // placed. Reusing a dying operand's colour for the result is legal
            // on A64 but NOT for a parallel copy, whose assignments are
            // simultaneous; taking the conservative order costs at most one
            // register and removes the case analysis entirely.
            let mut dead: Vec<usize> = live_here
                .iter()
                .copied()
                .filter(|&x| last[x] == Some(i))
                .collect();
            // A definition this block never reads and that does not escape
            // through `live_out` is dead the moment it is made: `last_use_into`
            // reports `None` for it, and treating that as "live forever" leaks a
            // colour for the rest of the block. Rematerialization is what made
            // this reachable — a remat'd value keeps no store, so its original
            // definition can end up with no uses at all.
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_))
                    && last[sp.idx(*r)].is_none()
                {
                    dead.push(sp.idx(*r));
                }
            }
            for x in dead {
                if live_here.remove(&x) {
                    if let Some(p) = color_of(&color, sp, x) {
                        occ.sub(p);
                    }
                }
            }
        }
    }
    Ok(Coloring { color, used })
}

/// May a DESTINATION of this instruction take the register of a source that dies
/// at it?
///
/// On A64 an instruction reads all of its source registers before writing any
/// destination, so for an ordinary one the answer is yes. Every exception is a
/// rule rather than a caution:
///   * `ParallelCopy` is not one instruction — its assignments are SIMULTANEOUS,
///     and a destination taking a dying source's register would destroy a value
///     another pair of the same copy still has to read.
///   * `StlXr`: DDI 0487 makes `stlxr Ws, Xt, [Xn]` CONSTRAINED UNPREDICTABLE
///     when Ws is Xt or Xn, so the status register may never inherit either.
///   * `Pair` transfers TWO registers and `Asm` is an opaque template whose
///     operands may be tied; neither is a single read-then-write.
///   * `Call` defines fixed registers and clobbers the caller-saved half.
///   * `StackAlloc` moves sp and then materializes a base — two instructions.
///   * a pre/post-index access is settled above, where the writeback is TIED to
///     the base deliberately (R3.2).
fn reads_before_writes(inst: &MInst) -> bool {
    match inst {
        MInst::ParallelCopy(_)
        | MInst::StlXr { .. }
        | MInst::Pair { .. }
        | MInst::Asm { .. }
        | MInst::Call { .. }
        | MInst::StackAlloc { .. } => false,
        MInst::Load { mem, .. } | MInst::Store { mem, .. } => {
            !matches!(mem, AddrMode::PreIdx { .. } | AddrMode::PostIdx { .. })
        }
        _ => true,
    }
}

/// Registers that hold the SAME value at some program point and would therefore
/// become a `mov` if they were given different colours.
fn copy_partners(f: &MFunc, sp: Space) -> Vec<Vec<Reg>> {
    let mut out: Vec<Vec<Reg>> = vec![Vec::new(); sp.nv];
    let mut link = |a: Reg, b: Reg, out: &mut Vec<Vec<Reg>>| {
        if let Reg::V(v) = a {
            if !out[v as usize].contains(&b) {
                out[v as usize].push(b);
            }
        }
    };
    for b in &f.blocks {
        for inst in &b.insts {
            match inst {
                MInst::Copy { dst, src, .. } | MInst::FMov { dst, src, .. } => {
                    link(*dst, *src, &mut out);
                    link(*src, *dst, &mut out);
                }
                MInst::ParallelCopy(pairs) => {
                    for (d, s, _) in pairs {
                        link(*d, *s, &mut out);
                        link(*s, *d, &mut out);
                    }
                }
                _ => {}
            }
        }
        for t in b.term.targets() {
            let params = &f.blocks[t.block as usize].params;
            for (p, a) in params.iter().zip(&t.args) {
                link(*p, *a, &mut out);
                link(*a, *p, &mut out);
            }
        }
    }
    out
}

/// Fill in the values that were holding a register of the failing class, so the
/// caller can free one of them instead of giving up.
fn with_holders(
    e: ColorErr,
    live_here: &BTreeSet<usize>,
    color: &[Option<PReg>],
    sp: Space,
    f: &MFunc,
    lv: &Liveness,
) -> ColorErr {
    match e {
        ColorErr::NoColour(v, _, m) => {
            let class = f.vregs[v as usize].class;
            // Ordered by how much freeing them would HELP: a value that does not
            // cross a call yet is sitting in a callee-saved register is the
            // squatter that starved this one, so it comes first.
            let mut hold: Vec<VReg> = live_here
                .iter()
                .filter(|&&x| x < sp.nv)
                .map(|&x| x as VReg)
                .filter(|&x| x != v && color[x as usize].is_some_and(|p| p.class == class))
                .collect();
            hold.sort_by_key(|&x| {
                let squatter = color[x as usize].is_some_and(isa::is_callee_saved)
                    && !lv.crosses_call[x as usize];
                (!squatter, x)
            });
            ColorErr::NoColour(v, hold, m)
        }
        other => other,
    }
}

/// Live holders per physical register.
struct Occupancy([u16; 96]);

impl Occupancy {
    fn new() -> Occupancy {
        Occupancy([0; 96])
    }
    fn slot(p: PReg) -> usize {
        let base = match p.class {
            Class::Gpr => 0,
            Class::Fpr => 32,
            Class::Flags => 64,
        };
        base + p.num as usize
    }
    fn add(&mut self, p: PReg) {
        self.0[Self::slot(p)] += 1;
    }
    fn sub(&mut self, p: PReg) {
        let s = Self::slot(p);
        self.0[s] = self.0[s].saturating_sub(1);
    }
    fn taken(&self, p: PReg) -> bool {
        self.0[Self::slot(p)] > 0
    }
    fn len(&self) -> usize {
        self.0.iter().filter(|c| **c > 0).count()
    }
    fn callee_saved_taken(&self, class: Class) -> usize {
        isa::alloc_order(class)
            .iter()
            .map(|&n| PReg { class, num: n })
            .filter(|p| isa::is_callee_saved(*p) && self.taken(*p))
            .count()
    }
}

/// The colouring's own obligation (REARCH §7.6a), checked INDEPENDENTLY of the
/// walk that produced it: at every program point, two values that are live
/// together hold different registers, and no value holds a physical register
/// that is live there. The colourer maintains an `occupied` set incrementally,
/// and an incremental set is exactly the kind of thing that can be subtly wrong;
/// this recomputes the live set from `Liveness` and compares.
pub fn check(f: &MFunc, lv: &Liveness, col: &Coloring) -> Result<(), String> {
    let sp = lv.sp;
    let cfg = super::super::mir::verify::cfg(f);
    let mut lu = live::LastUse::new(sp);
    for &b in &cfg.rpo {
        let bi = b as usize;
        live::last_use_into(f, sp, lv, bi, &mut lu);
        let last = &lu.at;
        let mut live: BTreeSet<usize> = lv.live_in[bi].clone();
        for &p in &f.blocks[bi].params {
            live.insert(sp.idx(p));
        }
        let mut probe = |live: &BTreeSet<usize>, at: String, note: &str| -> Result<(), String> {
            let mut seen: Vec<(PReg, usize)> = Vec::new();
            for &x in live.iter() {
                let p = match color_of(&col.color, sp, x) {
                    Some(p) => p,
                    None => continue,
                };
                if let Some(&(_, y)) = seen.iter().find(|(q, _)| *q == p) {
                    return Err(format!(
                        "{}: {:?} and {:?} are both live at {} and both hold {:?}{} ({})",
                        f.name,
                        sp.reg(y),
                        sp.reg(x),
                        at,
                        p.class,
                        p.num,
                        note
                    ));
                }
                seen.push((p, x));
            }
            Ok(())
        };
        // every colour must be one the allocator was allowed to hand out
        for &x in live.iter() {
            if let (Reg::V(v), Some(p)) = (sp.reg(x), color_of(&col.color, sp, x)) {
                if isa::alloc_mask(p.class) & (1 << p.num) == 0 {
                    return Err(format!(
                        "{}: v{} was given {:?}{}, which is not allocatable",
                        f.name, v, p.class, p.num
                    ));
                }
            }
        }
        probe(&live, format!("bb{} head", bi), "block entry")?;
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            // A pre/post-index writeback lands in the base register (emit prints
            // only the base), so the two must be tied. With base then dead, the
            // `probe` below also holds; were base live-out, probe would flag the
            // shared register. Together: base tied to wb AND base dead here.
            if let MInst::Load { mem, .. } | MInst::Store { mem, .. } = inst {
                if let AddrMode::PreIdx { base, wb, .. } | AddrMode::PostIdx { base, wb, .. } =
                    mem
                {
                    let bp = color_of(&col.color, sp, sp.idx(*base));
                    let wp = color_of(&col.color, sp, sp.idx(*wb));
                    if bp != wp {
                        return Err(format!(
                            "{}: bb{}[{}] pre/post-index base and writeback got \
                             different registers ({:?} vs {:?}); the writeback updates \
                             the base in place, so they must be tied",
                            f.name, bi, i, bp, wp
                        ));
                    }
                }
            }
            probe(&live, format!("bb{}[{}]", bi, i), "before the definitions")?;
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    live.insert(sp.idx(*r));
                }
            }
            let mut dead: Vec<usize> = live.iter().copied().filter(|&x| last[x] == Some(i)).collect();
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) && last[sp.idx(*r)].is_none() {
                    dead.push(sp.idx(*r));
                }
            }
            for x in dead {
                live.remove(&x);
            }
            // Probed AFTER the dying operands leave: a definition may reuse the
            // register of an operand this very instruction consumes — `blr x0`
            // reads the target before the call writes the result into it — and
            // that reuse is the whole point of the coalescing hint.
            probe(&live, format!("bb{}[{}]", bi, i), &format!("{:?}", inst))?;
        }
    }
    Ok(())
}

fn color_of(color: &[Option<PReg>], sp: Space, i: usize) -> Option<PReg> {
    if i < sp.nv {
        color[i]
    } else {
        match sp.reg(i) {
            Reg::P(p) => Some(p),
            Reg::V(_) => None,
        }
    }
}

fn phys_of(color: &[Option<PReg>], sp: Space, r: Reg) -> Option<PReg> {
    match r {
        Reg::P(p) => Some(p),
        Reg::V(v) => {
            let _ = sp;
            color[v as usize]
        }
    }
}

fn assign(
    f: &MFunc,
    lv: &Liveness,
    color: &mut [Option<PReg>],
    used: &mut RegSet,
    occ: &mut Occupancy,
    r: Reg,
    partners: &[Vec<Reg>],
    has_calls: bool,
) -> Result<(), ColorErr> {
    let v = match r {
        // A physical definition needs no choice; the CALLER records it in the
        // live set, and the occupancy count follows from that. Counting holders
        // rather than remembering a set is what keeps an indirect call correct:
        // the callee pointer may be coloured x0 and die at the `blr` whose result
        // also lands in x0, and one register then has two holders for an instant
        // (torture pr34768-2, found by `color::check`).
        Reg::P(p) => {
            used.add(p);
            return Ok(());
        }
        Reg::V(v) => v,
    };
    if color[v as usize].is_some() {
        return Ok(()); // already coloured (a value defined once, visited once)
    }
    let class = f.vregs[v as usize].class;
    if class == Class::Flags {
        // k = 1: NZCV is the only member, and `spill` has already guaranteed no
        // two flag values are live at once.
        color[v as usize] = Some(PReg::NZCV);
        return Ok(());
    }
    let avoid_caller_saved = lv.crosses_call[v as usize];
    // AAPCS64 §6.1.2: only the LOW 64 bits of v8–v15 are preserved across a
    // call, so a 128-bit value live across one has NO legal colour. isel
    // guarantees this never happens (a quad is parked in memory before any
    // call), and saying so here turns a future violation into a loud failure
    // instead of a silently truncated long double.
    if avoid_caller_saved && class == Class::Fpr && f.vregs[v as usize].width == Width::Q {
        return Err(ColorErr::NoColour(
            v,
            Vec::new(),
            format!(
                "{}: v{} is a 128-bit value live across a call — v8–v15 preserve only \
                 their low half (AAPCS64 §6.1.2), so it must be parked in memory",
                f.name, v
            ),
        ));
    }
    let sp = lv.sp;
    let conflict = lv.phys_conflict[v as usize];
    // The partition, as one predicate: a value live across a call may take only a
    // callee-saved register, and — once the function has a call at all — a value
    // that is NOT may take only a caller-saved one. Without the second half a
    // value with no need of the callee-saved registers can occupy them and starve
    // the values that have nowhere else to go, which greedy in dominance order
    // cannot undo.
    let free = |p: PReg, occ: &Occupancy| -> bool {
        // The hint path reaches registers `alloc_order` would never offer — a
        // constant-zero operand is `Reg::P(ZR)`, and an edge argument holding one
        // would otherwise hand the zero register to a real value.
        isa::alloc_mask(p.class) & (1 << p.num) != 0
            && !occ.taken(p)
            && !conflict.has(p)
            && !(avoid_caller_saved && !isa::is_callee_saved(p))
    };
    let _ = has_calls;
    // COALESCING is biased colouring (§7.4): take a copy partner's colour when
    // it is free. The partner set is not just `Copy` — it is every place two
    // registers hold the same value at some point: a parallel copy's pairs (call
    // argument setup) and, above all, a block PARAMETER and the arguments its
    // edges pass it. SSA destruction turns each of those pairs into a `mov`
    // AFTER colouring, so a colourer that only hints on instructions it can see
    // is blind to the largest source of copies there is — mem2reg gives every
    // join one parameter per live local.
    // R4.10 — THE PARTNER GRAPH IS FOLLOWED, NOT JUST READ ONE HOP.
    //
    // A direct partner is often not coloured yet: colouring walks the DOMINATOR
    // tree, and a block parameter's argument comes from a PREDECESSOR, which
    // need not be a dominator at all (a back edge is coloured strictly later).
    // The hint then finds nothing and the parameter takes an arbitrary register,
    // which is one `mov` on that edge for ever after. `ZCC_COALESCE` measured
    // the residual: **5,070 pairs where the two do not interfere and biased
    // colouring simply did not find the merge** — Boissinot's ceiling.
    //
    // Following the partner graph transitively reaches a coloured member through
    // the chain the copies form (`a → p → b`: `p` is uncoloured, `b` is not).
    // NOTHING ABOUT CORRECTNESS CHANGES: this only proposes a colour, and `free`
    // still refuses one that is occupied, conflicting or in the wrong half of
    // the partition — a hint that is wrong costs a `mov`, never a value. The
    // depth is bounded because a chain longer than a few links is a different
    // value's neighbourhood, not this one's.
    let mut seen: Vec<VReg> = vec![v];
    let mut wave: Vec<Reg> = partners[v as usize].clone();
    let mut hint: Option<PReg> = None;
    let depth: usize = std::env::var("ZCC_CODEPTH").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    for _ in 0..depth {
        if hint.is_some() || wave.is_empty() {
            break;
        }
        let mut next: Vec<Reg> = Vec::new();
        for q in &wave {
            if let Some(h) = phys_of(color, sp, *q) {
                if h.class == class && free(h, occ) {
                    hint = Some(h);
                    break;
                }
            }
            if let Reg::V(qv) = q {
                if !seen.contains(qv) {
                    seen.push(*qv);
                    next.extend_from_slice(&partners[*qv as usize]);
                }
            }
        }
        wave = next;
    }
    let pick = hint
        .or_else(|| {
            isa::alloc_order(class)
                .iter()
                .map(|&n| PReg { class, num: n })
                .find(|p| free(*p, occ))
        });
    match pick {
        Some(p) => {
            color[v as usize] = Some(p);
            used.add(p);
            Ok(())
        }
        // Unreachable once `spill` holds pressure ≤ k — and that is the theorem,
        // so reaching here is a Law-2 defect in the spiller, not a condition to
        // paper over with a fallback.
        None => Err(ColorErr::NoColour(v, Vec::new(), format!(
            "{}: no colour for v{} in class {:?} — {} registers occupied ({} of them \
             callee-saved), callee-saved-only {}, physical conflicts {:#x}/{:#x}, k {}",
            f.name,
            v,
            class,
            occ.len(),
            occ.callee_saved_taken(class),
            avoid_caller_saved,
            conflict.gpr,
            conflict.fpr,
            isa::k(class)
        ))),
    }
}
