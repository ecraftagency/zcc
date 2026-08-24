// Spilling: reduce register pressure to ≤ k so that chordal colouring cannot
// fail (REARCH.md §7.2) — Braun & Hack 2009, "Register Spilling and Live-Range
// Splitting for SSA-Form Programs".
//
// WHY THIS IS THE REAL ALGORITHM NOW, AND WHAT MEASURED IT. R0/R1 shipped the
// sound base case — spill at the definition, reload before EVERY use — because
// the storage model kept every local in memory, so nothing but short-lived
// expression temporaries competed for registers and the spiller almost never
// fired. R2.2's mem2reg turns each live local into a value, and the base case
// collapsed exactly where it was predicted to: sqlite went from 12,253 frame
// memory operations to 275,665, `sqlite3VdbeExec` alone spending 83,620 of its
// 89,392 instructions reloading. The fix is not a heuristic tweak; it is the
// algorithm the milestone named as a blocking prerequisite.
//
// THE ALGORITHM. Per register class, walk each block forward carrying a WORKING
// SET `W` — the values that are in registers right now — of size at most the
// class budget:
//   * At the block head `W` is the live-in values that are not memory-resident,
//     ordered by next-use distance and truncated to the budget. What does not
//     fit becomes memory-resident (Belady's rule: evict what is needed furthest
//     in the future — provably optimal for a fixed cache size, which is what a
//     register class is).
//   * A use of a value not in `W` gets a RELOAD into a fresh virtual register,
//     which then serves every later use in the block until it is evicted. That
//     is the whole difference from the base case: one reload per block-residency
//     rather than one per use.
//   * Room is made before definitions, and — the rule that subsumes all
//     "crosses a call" reasoning — a call's clobber set counts as registers
//     already spoken for at that point, so what survives a call is at most the
//     number of callee-saved registers of its class. Two values crossing
//     DIFFERENT calls are handled by the same rule: whichever call comes first
//     has both of them live, and the ceiling binds there.
//   * A value whose producer reads no register (`MovImm`, `Adrp`, `SlotAddr`)
//     is REMATERIALIZED instead of stored and reloaded — the recomputation is
//     one instruction and the store disappears entirely.
//
// NO SSA RECONSTRUCTION IS NEEDED, and that is a deliberate structural choice
// rather than an omission. A reload's fresh register is used only inside the
// block that created it, so its live range is dominated by its definition and
// SSA holds by construction; a value that stays in `W` across an edge keeps its
// ORIGINAL name and is never renamed. The price is one reload per block for a
// memory-resident value instead of one per program region — measured, and the
// residual is recorded in REARCH §12 rather than left implicit.
//
// POST-CONDITION (what the colourer relies on): at every program point the
// number of virtual values of a class that are live, plus the allocatable
// physical registers spoken for there, is at most `isa::k(class)`.
use super::live;
use crate::mir::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn spill(f: &mut MFunc) -> Result<usize, String> {
    spill_with(f, &BTreeSet::new(), usize::MAX)
}

/// `forced` names values the caller has already decided must live in memory —
/// the colourer's answer when the chordal guarantee is not enough on its own
/// (see `regalloc::allocate`).
/// `cross_cap` lowers the ceiling on simultaneously-live CALL-CROSSING values
/// below the callee-saved count. The colourer applies "live across a call ⟹
/// callee-saved" to a VALUE over its whole range, so a value that never needed
/// the callee-saved half can still be sitting in it when a value that does needs
/// one — and no amount of spilling other values dislodges it. Tightening this cap
/// reduces the demand directly, and driving it to zero always converges: with no
/// crossing residency allowed, every value is reloaded after the call it would
/// have spanned.
pub fn spill_with(
    f: &mut MFunc,
    forced: &BTreeSet<VReg>,
    cross_cap: usize,
) -> Result<usize, String> {
    let remat = rematerializable(f);
    let web = webs(f);
    let mut spilled: BTreeSet<VReg> = forced.clone();
    // Termination: every round that does not produce a plan makes at least one
    // more value memory-resident, and a value never leaves that set, so there
    // are at most |vregs| rounds. Exceeding it means the argument is false —
    // a Law-2 defect, not a budget to raise.
    let bound = f.vregs.len() + 2;
    let mut slot_of: BTreeMap<VReg, (SlotId, Width)> = BTreeMap::new();
    let mut web_slot: BTreeMap<VReg, (SlotId, Width)> = BTreeMap::new();
    for &v in forced.iter() {
        if !remat.contains_key(&v) {
            ensure_slot(f, &web, &mut web_slot, &mut slot_of, v);
        }
    }
    evict_params(f, &slot_of);
    let plan = {
        let mut plan = None;
        for _ in 0..bound {
            let cfg = crate::mir::verify::cfg(f);
            let lv = live::compute(f, &cfg);
            match simulate(f, &lv, &cfg, &spilled, cross_cap)? {
                Sim::Plan(p) => {
                    plan = Some(p);
                    break;
                }
                Sim::More(vs) => {
                    let before = spilled.len();
                    spilled.extend(vs.iter().copied());
                    if spilled.len() == before {
                        return Err(format!("{}: spilling made no progress", f.name));
                    }
                    // A spilled PARAMETER has to leave the IR immediately, not at
                    // the end: while it is still there its incoming arguments are
                    // uses AT THE TERMINATOR, so every argument of a wide join
                    // needs a register simultaneously and no eviction the
                    // simulator can make relieves it. Removing the parameter
                    // turns those simultaneous uses into one store each.
                    // Slots are allocated for EVERY newly memory-resident value
                    // here, not only the parameters, so that `evict_params` can
                    // already see when an argument lives in the slot its
                    // parameter is being given — the store is then a no-op and is
                    // never emitted.
                    for v in vs {
                        if !remat.contains_key(&v) {
                            ensure_slot(f, &web, &mut web_slot, &mut slot_of, v);
                        }
                    }
                    evict_params(f, &slot_of);
                }
            }
        }
        match plan {
            Some(p) => p,
            None => return Err(format!("{}: spilling did not converge", f.name)),
        }
    };
    let n = spilled.len();
    apply(f, plan, &spilled, &remat, &web, web_slot, slot_of);
    drop_redundant_spills(f);
    check_pressure(f)?;
    Ok(n)
}

/// The spiller's POST-CONDITION, checked rather than trusted (REARCH §7.6a):
/// at every program point the virtual values of a class that are live, plus the
/// allocatable physical registers spoken for there, are at most `isa::k(class)`;
/// and the call-crossing ones are at most the callee-saved count. The colourer's
/// theorem is "this cannot fail once pressure ≤ k", so a colouring failure means
/// the PRECONDITION was false — and without this check that shows up as an
/// unlocalized "no colour for v161" instead of naming the point and the count.
pub fn check_pressure(f: &MFunc) -> Result<(), String> {
    let cfg = crate::mir::verify::cfg(f);
    let lv = live::compute(f, &cfg);
    let sp = lv.sp;
    let mut lu = live::LastUse::new(sp);
    let masks = [isa::alloc_mask(Class::Gpr), isa::alloc_mask(Class::Fpr)];
    let cs = [
        (isa::callee_saved_mask(Class::Gpr) & masks[0]).count_ones() as usize,
        (isa::callee_saved_mask(Class::Fpr) & masks[1]).count_ones() as usize,
    ];
    for &b in &cfg.rpo {
        let bi = b as usize;
        live::last_use_into(f, sp, &lv, bi, &mut lu);
        let last = &lu.at;
        let mut live: BTreeSet<usize> = lv.live_in[bi].clone();
        for &p in &f.blocks[bi].params {
            live.insert(sp.idx(p));
        }
        let mut probe = |live: &BTreeSet<usize>, at: &str, extra: Option<RegSet>| -> Result<(), String> {
            for (ci, c) in [Class::Gpr, Class::Fpr].into_iter().enumerate() {
                let mut phys = 0u32;
                let (mut n, mut ncross) = (0usize, 0usize);
                for &x in live.iter() {
                    match sp.reg(x) {
                        Reg::P(p) if p.class == c => phys |= 1 << p.num,
                        Reg::V(v) if f.vregs[v as usize].class == c => {
                            n += 1;
                            if lv.crosses_call[v as usize] {
                                ncross += 1;
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(e) = extra {
                    phys |= if c == Class::Gpr { e.gpr } else { e.fpr };
                }
                let held = (phys & masks[ci]).count_ones() as usize;
                if n + held > isa::k(c) {
                    let mut who: Vec<String> = live
                        .iter()
                        .filter_map(|&x| match sp.reg(x) {
                            Reg::V(v) if f.vregs[v as usize].class == c => Some(format!("v{}", v)),
                            _ => None,
                        })
                        .collect();
                    who.sort();
                    return Err(format!(
                        "{}: {:?} pressure {} + {} held > k={} at {} [{}]",
                        f.name, c, n, held, isa::k(c), at, who.join(" ")
                    ));
                }
                if ncross > cs[ci] {
                    return Err(format!(
                        "{}: {} call-crossing {:?} values live at {} but only {} callee-saved",
                        f.name, ncross, c, at, cs[ci]
                    ));
                }
            }
            Ok(())
        };
        probe(&live, &format!("bb{} head", bi), None)?;
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    live.insert(sp.idx(*r));
                }
            }
            let clob = match inst {
                MInst::Call { clobbers, .. } => Some(*clobbers),
                _ => None,
            };
            probe(&live, &format!("bb{}[{}] {:?}", bi, i, mnemonic(inst)), clob)?;
            let mut dead: Vec<usize> = live.iter().copied().filter(|&x| last[x] == Some(i)).collect();
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) && last[sp.idx(*r)].is_none() {
                    dead.push(sp.idx(*r));
                }
            }
            for x in dead {
                live.remove(&x);
            }
        }
    }
    Ok(())
}

fn mnemonic(i: &MInst) -> &'static str {
    match i {
        MInst::Alu { .. } => "alu",
        MInst::Cmp { .. } => "cmp",
        MInst::MovImm { .. } => "movimm",
        MInst::Ext { .. } => "ext",
        MInst::Load { .. } => "load",
        MInst::Store { .. } => "store",
        MInst::Adrp { .. } => "adrp",
        MInst::AddLo12 { .. } => "addlo12",
        MInst::CSel { .. } => "csel",
        MInst::Call { .. } => "call",
        MInst::Spill { .. } => "spill",
        MInst::Reload { .. } => "reload",
        MInst::ParallelCopy(..) => "pcopy",
        MInst::Copy { .. } => "copy",
        _ => "other",
    }
}

/// Values whose producer reads no register and has no fixed destination: the
/// producer can simply be re-executed wherever the value is wanted, so the value
/// needs no stack slot and no store. (`MovImm` is an immediate, `Adrp` a page
/// address, `SlotAddr` a frame offset — all three are constants of the frame.)
fn rematerializable(f: &MFunc) -> BTreeMap<VReg, MInst> {
    let mut out = BTreeMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            let ok = matches!(
                inst,
                MInst::MovImm { .. } | MInst::Adrp { .. } | MInst::SlotAddr { .. }
            );
            if !ok {
                continue;
            }
            let mut dst = None;
            let mut bad = false;
            inst.visit(&mut |r, c| match c {
                Constraint::Def => dst = r.vreg(),
                Constraint::DefFixed(_) | Constraint::Use | Constraint::UseFixed(_) => bad = true,
            });
            if let (Some(v), false) = (dst, bad) {
                out.insert(v, inst.clone());
            }
        }
    }
    out
}

// ── the plan a simulation produces ─────────────────────────────────────────

/// A reload copy, named by an index while the function is still borrowed
/// immutably; `apply` turns each one into a real virtual register.
type CopyId = u32;

#[derive(Default)]
struct Plan {
    /// per block: (before instruction i, value, copy) — `i == insts.len()` means
    /// "before the terminator"
    reloads: Vec<Vec<(usize, VReg, CopyId)>>,
    /// per block: at instruction i, read `value` from `copy` instead
    subs: Vec<Vec<(usize, VReg, CopyId)>>,
    ncopies: u32,
}

enum Sim {
    Plan(Plan),
    More(Vec<VReg>),
}

/// One value's residency in a register right now.
#[derive(Clone, Copy)]
struct Res {
    v: VReg,
    /// `None` = still under its original name; `Some(id)` = a reload copy
    copy: Option<CopyId>,
    class: Class,
    /// this residency is live across some call, so it needs a callee-saved
    /// colour for its whole range
    cross: bool,
}

fn simulate(
    f: &MFunc,
    lv: &live::Liveness,
    cfg: &crate::cfg::Cfg,
    spilled: &BTreeSet<VReg>,
    cross_cap: usize,
) -> Result<Sim, String> {
    let base = linear_positions(f, cfg);
    let uses = use_positions(f, lv, cfg, &base);
    // Once the function contains a call the register file is PARTITIONED: a value
    // live across a call may use only the callee-saved half (AAPCS64 §6.1.1, and
    // `color.rs` applies it per VALUE over its whole range), and a value that is
    // not must therefore stay out of that half — otherwise it starves the values
    // that have nowhere else to go, and a greedy colouring in dominance order
    // (the order chordality requires) cannot go back and undo it. So there are
    // two ceilings, not one.
    let has_calls = f
        .blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| matches!(i, MInst::Call { .. })));
    let nb = f.blocks.len();
    let mut plan = Plan {
        reloads: vec![Vec::new(); nb],
        subs: vec![Vec::new(); nb],
        ncopies: 0,
    };
    let mut newsp: Vec<VReg> = Vec::new();
    let mut lu = live::LastUse::new(lv.sp);
    let masks = [
        isa::alloc_mask(Class::Gpr),
        isa::alloc_mask(Class::Fpr),
        0u32,
    ];
    let ki = |c: Class| match c {
        Class::Gpr => 0usize,
        Class::Fpr => 1,
        Class::Flags => 2,
    };
    // AAPCS64 §6.1.1: a value live across a call may only take a callee-saved
    // colour, and `color.rs` applies that as a property of the VALUE — over its
    // whole range, not only at the call. So the ceiling is not only "how many
    // values are live here" but "how many of them are call-crossing", and it
    // binds at every point, including points with no call in sight: two values
    // crossing DIFFERENT calls can be live together in between and still need
    // two distinct callee-saved registers.
    let cs = [
        ((isa::callee_saved_mask(Class::Gpr) & masks[0]).count_ones() as usize).min(cross_cap),
        ((isa::callee_saved_mask(Class::Fpr) & masks[1]).count_ones() as usize).min(cross_cap),
        usize::MAX,
    ];
    // the other half of the partition
    let cr = [
        isa::caller_saved_mask(Class::Gpr).count_ones() as usize,
        isa::caller_saved_mask(Class::Fpr).count_ones() as usize,
        usize::MAX,
    ];

    for &b in &cfg.rpo {
        let bi = b as usize;
        let blk = &f.blocks[bi];
        live::last_use_into(f, lv.sp, lv, bi, &mut lu);
        let last = &lu.at;
        let head = base[bi];

        // physical registers spoken for, tracked exactly like the values
        let mut physlive: BTreeSet<usize> = lv.live_in[bi]
            .iter()
            .copied()
            .filter(|&x| x >= lv.sp.nv)
            .collect();
        let phys_mask = |set: &BTreeSet<usize>, c: Class| -> u32 {
            let mut m = 0u32;
            for &x in set {
                if let Some(p) = lv.sp.reg(x).preg() {
                    if p.class == c {
                        m |= 1 << p.num;
                    }
                }
            }
            m
        };

        // (1) the working set at the head: everything live in and not already
        //     memory-resident, nearest next use first, truncated to the budget
        let mut w: Vec<Res> = Vec::new();
        for c in [Class::Gpr, Class::Fpr] {
            let mut cand: Vec<VReg> = lv.live_in[bi]
                .iter()
                .copied()
                .filter(|&x| x < lv.sp.nv)
                .map(|x| x as VReg)
                .chain(blk.params.iter().filter_map(|p| p.vreg()))
                .filter(|v| !spilled.contains(v) && class_of(f, Reg::V(*v)) == c)
                .collect();
            cand.sort_unstable();
            cand.dedup();
            cand.sort_by_key(|v| next_use(&uses, *v as usize, head));
            let hm = phys_mask(&physlive, c) & masks[ki(c)];
            let budget = isa::k(c).saturating_sub(hm.count_ones() as usize);
            let mut bcross = cs[ki(c)]
                .saturating_sub((hm & isa::callee_saved_mask(c)).count_ones() as usize);
            let mut bplain = usize::MAX;
            let mut taken = 0usize;
            for v in cand.into_iter() {
                let cross = lv.crosses_call[v as usize];
                let room = if cross { &mut bcross } else { &mut bplain };
                if taken < budget && *room > 0 {
                    *room -= 1;
                    taken += 1;
                    w.push(Res { v, copy: None, class: c, cross });
                } else {
                    newsp.push(v);
                }
            }
        }

        // How many calls precede each instruction: a reload copy that will be
        // live across one is restricted to a callee-saved colour for its WHOLE
        // range (`color.rs` applies the rule per VALUE), so it has to be counted
        // as call-crossing from the moment it is created — not from the call.
        let calls_before: Vec<usize> = {
            let mut v = Vec::with_capacity(blk.insts.len() + 1);
            let mut n = 0;
            v.push(0);
            for inst in &blk.insts {
                if matches!(inst, MInst::Call { .. }) {
                    n += 1;
                }
                v.push(n);
            }
            v
        };

        // the head is a program point like any other: the ceilings bind there
        for c in [Class::Gpr, Class::Fpr] {
            loop {
                let ncross = w.iter().filter(|r| r.class == c && r.cross).count();
                if ncross <= cs[ki(c)] {
                    break;
                }
                let pick = w
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.class == c && r.cross)
                    .max_by_key(|(_, r)| next_use(&uses, r.v as usize, head))
                    .map(|(j, r)| (j, r.v));
                match pick {
                    Some((j, v)) => {
                        w.remove(j);
                        newsp.push(v);
                    }
                    None => break,
                }
            }
        }

        // (2) walk the block
        let n = blk.insts.len();
        for i in 0..=n {
            let mut ops: Vec<(Reg, Constraint)> = Vec::new();
            let clob = if i < n {
                blk.insts[i].visit(&mut |r, c| ops.push((r, c)));
                match &blk.insts[i] {
                    MInst::Call { clobbers, .. } => Some(*clobbers),
                    _ => None,
                }
            } else {
                match &blk.term {
                    MTerm::Bcc(_, r, ..) => ops.push((*r, Constraint::Use)),
                    MTerm::Cbz { reg, .. } | MTerm::Tb { reg, .. } => {
                        ops.push((*reg, Constraint::Use))
                    }
                    MTerm::Switch { idx, .. } => ops.push((*idx, Constraint::Use)),
                    MTerm::BrReg(r, _) => ops.push((*r, Constraint::Use)),
                    _ => {}
                }
                // A JOIN can be wider than the register file: mem2reg gives a
                // block one parameter per live local, and every argument of the
                // edge is a use at this single point. No eviction relieves that —
                // the argument must be in a register HERE — so the ceiling is met
                // by removing the PARAMETER instead: the value then travels
                // through its slot, one store per argument, and the edge stops
                // demanding a register at all.
                let mut kept: Vec<(usize, usize)> = Vec::new();
                for (ti, t) in blk.term.targets().iter().enumerate() {
                    for k in 0..t.args.len() {
                        kept.push((ti, k));
                    }
                }
                for c in [Class::Gpr, Class::Fpr] {
                    loop {
                        // Everything that must hold a register at this point:
                        // the values still resident, plus the terminator's own
                        // operand and every edge argument. The arguments are the
                        // only ones no eviction can relieve, so they are what the
                        // loop below gives up.
                        let mut need: Vec<VReg> = ops
                            .iter()
                            .filter_map(|(r, _)| r.vreg())
                            .chain(kept.iter().filter_map(|&(ti, k)| {
                                blk.term.targets()[ti].args[k].vreg()
                            }))
                            .chain(w.iter().map(|r| r.v))
                            .filter(|v| class_of(f, Reg::V(*v)) == c)
                            .collect();
                        need.sort_unstable();
                        need.dedup();
                        // no call clobbers at a terminator, so only live physical
                        // registers are spoken for here
                        let ph = phys_mask(&physlive, c) & masks[ki(c)];
                        let cap = isa::k(c).saturating_sub(ph.count_ones() as usize);
                        let ncross = need.iter().filter(|&&v| lv.crosses_call[v as usize]).count();
                        let capx = cs[ki(c)]
                            .saturating_sub((ph & isa::callee_saved_mask(c)).count_ones() as usize);
                        let capp = usize::MAX;
                        if need.len() <= cap && ncross <= capx && need.len() - ncross <= capp {
                            break;
                        }
                        let pick = kept
                            .iter()
                            .enumerate()
                            .filter(|(_, tk)| {
                                blk.term.targets()[tk.0].args[tk.1]
                                    .vreg()
                                    .is_some_and(|v| class_of(f, Reg::V(v)) == c)
                            })
                            .filter_map(|(j, tk)| {
                                let tb = blk.term.targets()[tk.0].block as usize;
                                f.blocks[tb]
                                    .params
                                    .get(tk.1)
                                    .and_then(|p| p.vreg())
                                    .map(|p| (j, p))
                            })
                            .max_by_key(|&(_, p)| next_use(&uses, p as usize, head + i));
                        match pick {
                            Some((j, p)) => {
                                kept.remove(j);
                                newsp.push(p);
                            }
                            None => break,
                        }
                    }
                }
                for &(ti, k) in &kept {
                    ops.push((blk.term.targets()[ti].args[k], Constraint::Use));
                }
                None
            };
            let at = head + i;
            // registers this instruction pins: never a victim
            let pinned: Vec<VReg> = ops.iter().filter_map(|(r, _)| r.vreg()).collect();
            let held_mask = |set: &BTreeSet<usize>, c: Class| -> u32 {
                let mut m = phys_mask(set, c);
                if let Some(cl) = clob {
                    m |= match c {
                        Class::Gpr => cl.gpr,
                        Class::Fpr => cl.fpr,
                        Class::Flags => 0,
                    };
                }
                m & masks[ki(c)]
            };
            let held = |set: &BTreeSet<usize>, c: Class| -> usize {
                held_mask(set, c).count_ones() as usize
            };
            // `need` = registers this instruction is about to occupy;
            // `need_cross` = how many of them will carry the callee-saved
            // restriction. A RELOAD copy never does (its range ends at the
            // instruction that reads it), which is why the two counts are
            // separate — conflating them makes the ceiling reject a split that
            // is precisely what relieves it.
            let mut room = |w: &mut Vec<Res>,
                            newsp: &mut Vec<VReg>,
                            c: Class,
                            need: usize,
                            need_cross: usize,
                            physlive: &BTreeSet<usize>|
             -> Result<(), String> {
                loop {
                    let cnt = w.iter().filter(|r| r.class == c).count();
                    let ncross = w.iter().filter(|r| r.class == c && r.cross).count();
                    let hm = held_mask(physlive, c);
                    let over_k = cnt + hm.count_ones() as usize + need > isa::k(c);
                    // The physical registers actually LIVE here, without a call's
                    // clobber set: a clobber constrains what may SURVIVE the call,
                    // which is the crossing ceiling's business — counting it
                    // against the caller-saved half would declare every call
                    // over-pressured, since its clobbers ARE that half.
                    let ph = phys_mask(physlive, c) & masks[ki(c)];
                    let over_cross = ncross
                        + need_cross
                        + (ph & isa::callee_saved_mask(c)).count_ones() as usize
                        > cs[ki(c)];
                    let over_plain = false;
                    if !over_k && !over_cross && !over_plain {
                        return Ok(());
                    }
                    if c == Class::Flags {
                        // Flags are never stored: their producer is pure, so the
                        // answer is to rematerialize the compare. Reaching here
                        // means isel let two flag values overlap — a Law-2
                        // Side-I defect, not a spill problem.
                        return Err(format!(
                            "{}: two NZCV values live at once; the compare must be rematerialized",
                            f.name
                        ));
                    }
                    let pick = w
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.class == c && !pinned.contains(&r.v))
                        .filter(|(_, r)| !over_cross || r.cross)
                        .filter(|(_, r)| !(over_plain && !over_cross && !over_k) || !r.cross)
                        .max_by_key(|(_, r)| next_use(&uses, r.v as usize, at))
                        .map(|(j, r)| (j, *r));
                    match pick {
                        Some((j, r)) => {
                            w.remove(j);
                            // A reload copy is a clean duplicate of what the slot
                            // already holds, so dropping it costs nothing. An
                            // original value has to become memory-resident.
                            if r.copy.is_none() && !spilled.contains(&r.v) {
                                newsp.push(r.v);
                            }
                        }
                        // Every value live here is pinned by the very instruction
                        // that overflows: it reads more registers than the class
                        // has. No A64 instruction reads more than four, so this
                        // is a Law-2 defect in isel.
                        None => {
                            return Err(format!(
                                "{}: {:?} pressure exceeds k at bb{}[{}] with nothing evictable                                  (resident {}, held {}, need {}, pinned {}, k {}, inst {})",
                                f.name,
                                c,
                                bi,
                                i,
                                if i < n { mnemonic(&blk.insts[i]) } else { "term" },
                                cnt,
                                held(physlive, c),
                                need,
                                pinned.len(),
                                isa::k(c)
                            ));
                        }
                    }
                }
            };

            // (2a0) The call-crossing ceiling, relaxed for values this
            // instruction READS. `color.rs` applies the callee-saved restriction
            // to a VALUE over its whole range, so ten is the hard limit on
            // simultaneously-live call-crossing GPR values — and a call with
            // many arguments reads more than ten of them at once, every one
            // pinned by the very instruction that needs them. Evicting a pinned
            // READ is nonetheless legal and is exactly the live-range split the
            // situation calls for: the use below reloads it into a fresh
            // register whose range ends at this instruction, so that register
            // crosses no call and needs no callee-saved home. Only a value this
            // instruction DEFINES is un-evictable, since no reload can supply it.
            let pinned_defs: Vec<VReg> = ops
                .iter()
                .filter(|(_, k)| matches!(k, Constraint::Def | Constraint::DefFixed(_)))
                .filter_map(|(r, _)| r.vreg())
                .collect();
            for c in [Class::Gpr, Class::Fpr] {
                loop {
                    let ncross = w.iter().filter(|r| r.class == c && r.cross).count();
                    if ncross <= cs[ki(c)] {
                        break;
                    }
                    let pick = w
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.class == c && r.cross)
                        .filter(|(_, r)| !pinned_defs.contains(&r.v))
                        .max_by_key(|(_, r)| next_use(&uses, r.v as usize, at))
                        .map(|(j, r)| (j, *r));
                    match pick {
                        Some((j, r)) => {
                            w.remove(j);
                            if r.copy.is_none() && !spilled.contains(&r.v) {
                                newsp.push(r.v);
                            }
                        }
                        None => break,
                    }
                }
            }

            // (2a) every virtual use must be resident
            for (r, c) in ops.iter() {
                if !matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    continue;
                }
                let v = match r.vreg() {
                    Some(v) => v,
                    None => continue,
                };
                match w.iter().find(|x| x.v == v).map(|x| x.copy) {
                    Some(Some(id)) => plan.subs[bi].push((i, v, id)),
                    Some(None) => {}
                    None => {
                        let cl = class_of(f, Reg::V(v));
                        room(&mut w, &mut newsp, cl, 1, 0, &physlive)?;
                        let id = plan.ncopies;
                        plan.ncopies += 1;
                        plan.reloads[bi].push((i, v, id));
                        plan.subs[bi].push((i, v, id));
                        let end = match last[lv.sp.idx(Reg::V(v))] {
                            Some(j) if j < blk.insts.len() => j,
                            _ => blk.insts.len(),
                        };
                        let cross = calls_before[end] > calls_before[i.min(end)];
                        w.push(Res { v, copy: Some(id), class: cl, cross });
                        if !spilled.contains(&v) {
                            newsp.push(v);
                        }
                    }
                }
            }
            if i == n {
                break;
            }
            // Everything resident BEFORE the call is live across it (whatever
            // dies at the call leaves below), so its residency inherits the
            // callee-saved restriction — including a reload copy, whose flag
            // `Liveness` cannot know until the copy exists.
            let pre_call: Vec<VReg> = if clob.is_some() {
                w.iter().map(|r| r.v).collect()
            } else {
                Vec::new()
            };
            // (2b) room for what this instruction defines, counting a call's
            //      clobber set as registers already taken
            for c in [Class::Gpr, Class::Fpr, Class::Flags] {
                let need = ops
                    .iter()
                    .filter(|(r, k)| {
                        matches!(k, Constraint::Def | Constraint::DefFixed(_))
                            && r.vreg().is_some()
                            && class_of(f, *r) == c
                    })
                    .count();
                let need_cross = ops
                    .iter()
                    .filter(|(r, k)| {
                        matches!(k, Constraint::Def | Constraint::DefFixed(_))
                            && r.vreg().is_some_and(|v| lv.crosses_call[v as usize])
                            && class_of(f, *r) == c
                    })
                    .count();
                room(&mut w, &mut newsp, c, need, need_cross, &physlive)?;
            }
            // (2c) the definitions themselves
            for (r, c) in ops.iter() {
                if !matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    continue;
                }
                match r {
                    Reg::V(v) => {
                        if !w.iter().any(|x| x.v == *v) {
                            w.push(Res {
                                v: *v,
                                copy: None,
                                class: class_of(f, *r),
                                cross: lv.crosses_call[*v as usize],
                            });
                        }
                    }
                    Reg::P(_) => {
                        physlive.insert(lv.sp.idx(*r));
                    }
                }
            }
            // (2d) whatever dies here leaves — INCLUDING a definition this block
            //      never reads and that does not escape (`last` is `None` for it)
            w.retain(|r| {
                let x = lv.sp.idx(Reg::V(r.v));
                match last[x] {
                    Some(j) => j != i,
                    None => !ops.iter().any(|(q, k)| {
                        matches!(k, Constraint::Def | Constraint::DefFixed(_)) && q.vreg() == Some(r.v)
                    }),
                }
            });
            let dead: Vec<usize> = physlive
                .iter()
                .copied()
                .filter(|&x| last[x] == Some(i))
                .collect();
            for x in dead {
                physlive.remove(&x);
            }
            if clob.is_some() {
                for r in w.iter_mut() {
                    if pre_call.contains(&r.v) {
                        r.cross = true;
                    }
                }
                for c in [Class::Gpr, Class::Fpr] {
                    room(&mut w, &mut newsp, c, 0, 0, &physlive)?;
                }
            }
        }
    }
    if newsp.is_empty() {
        Ok(Sim::Plan(plan))
    } else {
        newsp.sort_unstable();
        newsp.dedup();
        Ok(Sim::More(newsp))
    }
}

// ── applying a plan ────────────────────────────────────────────────────────

fn apply(
    f: &mut MFunc,
    plan: Plan,
    spilled: &BTreeSet<VReg>,
    remat: &BTreeMap<VReg, MInst>,
    web: &[VReg],
    mut web_slot: BTreeMap<VReg, (SlotId, Width)>,
    mut slot_of: BTreeMap<VReg, (SlotId, Width)>,
) {
    for &v in spilled {
        if remat.contains_key(&v) {
            continue;
        }
        ensure_slot(f, web, &mut web_slot, &mut slot_of, v);
    }
    evict_params(f, &slot_of);

    let mut copy_reg: Vec<Reg> = vec![Reg::P(isa::ZR); plan.ncopies as usize];
    for b in 0..f.blocks.len() {
        let reloads = &plan.reloads[b];
        let subs = &plan.subs[b];
        if reloads.is_empty() && subs.is_empty() && !has_spilled_def(f, b, spilled, remat) {
            continue;
        }
        let insts = std::mem::take(&mut f.blocks[b].insts);
        let n = insts.len();
        let mut out: Vec<MInst> = Vec::with_capacity(n + reloads.len() * 2);
        let mut term = f.blocks[b].term.clone();
        for (i, inst) in insts.into_iter().enumerate().map(|(i, x)| (i, Some(x))).chain(
            std::iter::once((n, None)),
        ) {
            for &(at, v, id) in reloads.iter().filter(|(at, ..)| *at == i) {
                let w = f.vregs[v as usize].width;
                let d = f.new_vreg(w);
                copy_reg[id as usize] = d;
                let _ = at;
                match remat.get(&v) {
                    Some(src) => {
                        let mut c = src.clone();
                        c.visit_mut(&mut |r, k| {
                            if matches!(k, Constraint::Def) {
                                *r = d;
                            }
                        });
                        out.push(c);
                    }
                    None => {
                        let (slot, sw) = slot_of[&v];
                        out.push(MInst::Reload { slot, dst: d, w: sw });
                    }
                }
            }
            let rewrite = |r: &mut Reg, k: Constraint| {
                if !matches!(k, Constraint::Use | Constraint::UseFixed(_)) {
                    return;
                }
                if let Some(v) = r.vreg() {
                    if let Some(&(_, _, id)) = subs.iter().find(|(at, x, _)| *at == i && *x == v) {
                        *r = copy_reg[id as usize];
                    }
                }
            };
            match inst {
                Some(mut inst) => {
                    inst.visit_mut(&mut |r, k| rewrite(r, k));
                    let mut defs: Vec<Reg> = Vec::new();
                    inst.visit(&mut |r, k| {
                        if matches!(k, Constraint::Def | Constraint::DefFixed(_)) {
                            if let Some(v) = r.vreg() {
                                if slot_of.contains_key(&v) {
                                    defs.push(r);
                                }
                            }
                        }
                    });
                    out.push(inst);
                    for d in defs {
                        let (slot, w) = slot_of[&d.vreg().unwrap()];
                        out.push(MInst::Spill { slot, src: d, w });
                    }
                }
                None => {
                    term.visit_mut(&mut |r, _| rewrite(r, Constraint::Use));
                }
            }
        }
        f.blocks[b].insts = out;
        f.blocks[b].term = term;
    }
}

fn has_spilled_def(
    f: &MFunc,
    b: usize,
    spilled: &BTreeSet<VReg>,
    remat: &BTreeMap<VReg, MInst>,
) -> bool {
    f.blocks[b].insts.iter().any(|inst| {
        let mut hit = false;
        inst.visit(&mut |r, k| {
            if matches!(k, Constraint::Def | Constraint::DefFixed(_)) {
                if let Some(v) = r.vreg() {
                    if spilled.contains(&v) && !remat.contains_key(&v) {
                        hit = true;
                    }
                }
            }
        });
        hit
    })
}

/// A spilled BLOCK PARAMETER cannot be stored "at its definition": its
/// definition IS the block head, and a value occupying a register there is
/// exactly the pressure the spill was meant to relieve. Braun & Hack handle a
/// spilled phi the only way it can be handled — the parameter stops existing,
/// and each predecessor writes the value it would have passed straight into the
/// slot.
///
/// The square is the ordinary spill one: on every edge into the block the slot
/// afterwards holds exactly the operand that edge carried, and every use reloads
/// from it. Writing the slot in a predecessor that ALSO branches elsewhere is
/// harmless — the slot belongs to this value alone, so a path that never reads
/// it cannot observe the write.
fn evict_params(f: &mut MFunc, slot_of: &BTreeMap<VReg, (SlotId, Width)>) {
    for b in 0..f.blocks.len() {
        let keep: Vec<bool> = f.blocks[b]
            .params
            .iter()
            .map(|p| p.vreg().is_none_or(|v| !slot_of.contains_key(&v)))
            .collect();
        if keep.iter().all(|k| *k) {
            continue;
        }
        let dropped: Vec<(usize, SlotId, Width)> = f.blocks[b]
            .params
            .iter()
            .enumerate()
            .filter(|(i, _)| !keep[*i])
            .map(|(i, p)| {
                let (slot, w) = slot_of[&p.vreg().unwrap()];
                (i, slot, w)
            })
            .collect();
        let mut i = 0;
        f.blocks[b].params.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        for p in 0..f.blocks.len() {
            let mut term = f.blocks[p].term.clone();
            let mut stores: Vec<MInst> = Vec::new();
            let mut edited = false;
            for t in term.targets_mut() {
                if t.block as usize != b {
                    continue;
                }
                edited = true;
                for &(k, slot, w) in &dropped {
                    // If the argument is itself memory-resident IN THIS SLOT, the
                    // value is already where the parameter's uses will look for
                    // it, and the store is a no-op. That is the ordinary case
                    // after slot coalescing — a variable flowing through a join
                    // does not move — and skipping it is what stops a
                    // pass-through block from emitting `ldr d,[S]; str d,[S]`.
                    // Nothing can have overwritten the slot in between: the only
                    // other values that use it are web members, and a member
                    // defined while the argument is live would INTERFERE with it,
                    // which is exactly what `webs` refused to coalesce.
                    if t.args[k]
                        .vreg()
                        .and_then(|v| slot_of.get(&v))
                        .is_some_and(|&(s2, _)| s2 == slot)
                    {
                        continue;
                    }
                    stores.push(MInst::Spill { slot, src: t.args[k], w });
                }
                let mut i = 0;
                t.args.retain(|_| {
                    i += 1;
                    keep[i - 1]
                });
            }
            // The rewritten terminator goes back even when NO store was needed:
            // dropping the arguments is the point, and the store is only the
            // occasional extra. Writing it back conditionally left the edge
            // carrying arguments for parameters that no longer existed.
            if edited {
                f.blocks[p].insts.extend(stores);
                f.blocks[p].term = term;
            }
        }
    }
}

// ── shared indices ─────────────────────────────────────────────────────────

/// Absolute position of each block's first instruction, in reverse postorder.
/// `usize::MAX` marks an unreachable block. A block occupies
/// `base[b] ..= base[b] + insts.len()`, the last slot being its terminator.
fn linear_positions(f: &MFunc, cfg: &crate::cfg::Cfg) -> Vec<usize> {
    let mut base = vec![usize::MAX; f.blocks.len()];
    let mut at = 0usize;
    for &b in &cfg.rpo {
        base[b as usize] = at;
        at += f.blocks[b as usize].insts.len() + 1;
    }
    base
}

/// Every position at which each value is READ, ascending — built once per
/// simulation. Belady's rule needs "the next use after this point", and with
/// this index that is a binary search instead of a scan of the whole function.
fn use_positions(
    f: &MFunc,
    lv: &live::Liveness,
    cfg: &crate::cfg::Cfg,
    base: &[usize],
) -> Vec<Vec<usize>> {
    let mut uses = vec![Vec::new(); lv.sp.len()];
    for &b in &cfg.rpo {
        let bi = b as usize;
        if base[bi] == usize::MAX {
            continue;
        }
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            let at = base[bi] + i;
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    uses[lv.sp.idx(r)].push(at);
                }
            });
        }
        let at = base[bi] + f.blocks[bi].insts.len();
        f.blocks[bi].term.visit(&mut |r, _| uses[lv.sp.idx(r)].push(at));
    }
    for u in uses.iter_mut() {
        u.sort_unstable();
        u.dedup();
    }
    uses
}

/// The next position at which value `x` is read, strictly after `from`;
/// `usize::MAX` when it is never used again. Belady's rule evicts the value
/// whose next use is furthest away, so this number IS the policy.
fn next_use(uses: &[Vec<usize>], x: usize, from: usize) -> usize {
    let u = &uses[x];
    match u.partition_point(|&p| p <= from) {
        i if i < u.len() => u[i],
        _ => usize::MAX,
    }
}

fn class_of(f: &MFunc, r: Reg) -> Class {
    match r {
        Reg::V(v) => f.vregs[v as usize].class,
        Reg::P(p) => p.class,
    }
}

// ── one slot per SSA web ───────────────────────────────────────────────────

/// Union-find over "values that are the same C variable and never hold two
/// different things at once": a block parameter and the arguments its edges pass
/// it, merged only when the two classes do not INTERFERE. mem2reg splits one
/// local into a web of SSA values joined exactly by those edges, and in the
/// ordinary case each value dies exactly where the next is defined — so the web
/// collapses to one slot. Where it does not (a value still live after the next
/// definition), the merge is refused and the two keep separate slots: sharing
/// there would overwrite a live value, which is a miscompile, not a size cost.
///
/// WHY IT MATTERS, measured. Giving each spilled VALUE its own slot makes a
/// spilled parameter copy between slots on every incoming edge — sqlite's
/// `sqlite3VdbeExec` grew 110,000 stores that way, moving one variable from one
/// slot to another. With one slot per web, the incoming value has usually just
/// been reloaded FROM that slot, and `drop_redundant_spills` deletes the store
/// outright: the variable simply stays where it was.
fn webs(f: &MFunc) -> Vec<VReg> {
    let cfg = crate::mir::verify::cfg(f);
    let lv = live::compute(f, &cfg);
    let sp = lv.sp;
    let nv = f.vregs.len();
    // definition point of every value, on a 1-based scale where 0 is the block
    // head (a parameter) and instruction i sits at i + 1
    let mut def_blk = vec![u32::MAX; nv];
    let mut def_idx = vec![0u32; nv];
    let mut last: Vec<BTreeMap<VReg, usize>> = vec![BTreeMap::new(); f.blocks.len()];
    let mut lu = live::LastUse::new(sp);
    for b in 0..f.blocks.len() {
        if !cfg.reachable(b as MBlockId) {
            continue;
        }
        for &p in &f.blocks[b].params {
            if let Some(v) = p.vreg() {
                def_blk[v as usize] = b as u32;
                def_idx[v as usize] = 0;
            }
        }
        for (i, inst) in f.blocks[b].insts.iter().enumerate() {
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    if let Some(v) = r.vreg() {
                        def_blk[v as usize] = b as u32;
                        def_idx[v as usize] = i as u32 + 1;
                    }
                }
            });
        }
        live::last_use_into(f, sp, &lv, b, &mut lu);
        for v in 0..nv as VReg {
            if let Some(j) = lu.at[sp.idx(Reg::V(v))] {
                last[b].insert(v, j);
            }
        }
    }
    // `v` holds a value at the point just after position `idx` in block `b`
    let live_at = |v: VReg, b: u32, idx: u32| -> bool {
        if b == u32::MAX {
            return false;
        }
        let bi = b as usize;
        let mine = def_blk[v as usize] == b;
        if mine {
            if def_idx[v as usize] > idx {
                return false; // not defined yet
            }
            if def_idx[v as usize] == idx {
                return true; // defined right here
            }
        } else if !lv.live_in[bi].contains(&sp.idx(Reg::V(v))) {
            return false;
        }
        if lv.live_out[bi].contains(&sp.idx(Reg::V(v))) {
            return true;
        }
        match last[bi].get(&v) {
            // a use at instruction j is at position j + 1
            Some(&j) => j == usize::MAX || j as u32 + 1 > idx,
            None => false,
        }
    };
    let interfere = |x: VReg, y: VReg| -> bool {
        x == y
            || live_at(y, def_blk[x as usize], def_idx[x as usize])
            || live_at(x, def_blk[y as usize], def_idx[y as usize])
    };

    let mut parent: Vec<VReg> = (0..nv as VReg).collect();
    fn find(p: &mut Vec<VReg>, mut x: VReg) -> VReg {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let mut members: BTreeMap<VReg, Vec<VReg>> = BTreeMap::new();
    for b in &f.blocks {
        for t in b.term.targets() {
            let want = &f.blocks[t.block as usize].params;
            for (a, q) in t.args.iter().zip(want) {
                let (a, q) = match (a.vreg(), q.vreg()) {
                    (Some(a), Some(q)) => (a, q),
                    _ => continue,
                };
                if f.vregs[a as usize].width != f.vregs[q as usize].width {
                    continue;
                }
                let (ra, rq) = (find(&mut parent, a), find(&mut parent, q));
                if ra == rq {
                    continue;
                }
                let ma: Vec<VReg> = members.get(&ra).cloned().unwrap_or_else(|| vec![ra]);
                let mq: Vec<VReg> = members.get(&rq).cloned().unwrap_or_else(|| vec![rq]);
                // Transitivity is not free: merging two classes makes EVERY
                // member share one slot, so the check is class against class,
                // not just the pair that proposed the merge.
                if ma.iter().any(|&x| mq.iter().any(|&y| interfere(x, y))) {
                    continue;
                }
                parent[ra as usize] = rq;
                let mut all = ma;
                all.extend(mq);
                members.remove(&ra);
                members.insert(rq, all);
            }
        }
    }
    (0..nv as VReg).map(|v| find(&mut parent, v)).collect()
}

/// The slot a spilled value uses: one per WEB, allocated on first demand.
///
/// `slot_of` is keyed by VALUE and holds an entry only for values that are
/// actually spilled — `web_slot` is the per-class index. Keeping the two apart
/// matters: `evict_params` decides which parameters to remove by asking whether
/// `slot_of` names them, and a web root that merely gave its name to a slot must
/// never answer yes.
fn ensure_slot(
    f: &mut MFunc,
    web: &[VReg],
    web_slot: &mut BTreeMap<VReg, (SlotId, Width)>,
    slot_of: &mut BTreeMap<VReg, (SlotId, Width)>,
    v: VReg,
) -> (SlotId, Width) {
    let root = web[v as usize];
    if let Some(&e) = web_slot.get(&root) {
        slot_of.insert(v, e);
        return e;
    }
    let w = f.vregs[v as usize].width;
    // a `q` spill needs 16 bytes AND 16-byte alignment (DDI 0487 C3.2: the
    // unsigned offset form scales the immediate by the access size)
    let slot = f.new_slot(w.bytes().max(8), w.bytes().max(8), SlotKind::Spill);
    web_slot.insert(root, (slot, w));
    slot_of.insert(v, (slot, w));
    (slot, w)
}

/// A store of exactly what the slot already holds. Spill slots are compiler
/// private — their address is never taken and no C-level access can reach them —
/// so within a block the last `Spill`/`Reload` of a slot names the register that
/// equals its contents, and storing that same register again is a no-op. This is
/// what turns a spilled parameter from "copy the variable into its slot on every
/// edge" into "leave the variable where it already is".
fn drop_redundant_spills(f: &mut MFunc) {
    for b in f.blocks.iter_mut() {
        let mut holds: BTreeMap<SlotId, Reg> = BTreeMap::new();
        let mut keep: Vec<bool> = Vec::with_capacity(b.insts.len());
        for inst in &b.insts {
            let k = match inst {
                MInst::Reload { slot, dst, .. } => {
                    holds.insert(*slot, *dst);
                    true
                }
                MInst::Spill { slot, src, .. } => {
                    if holds.get(slot) == Some(src) {
                        false
                    } else {
                        holds.insert(*slot, *src);
                        true
                    }
                }
                _ => true,
            };
            keep.push(k);
        }
        let mut i = 0;
        b.insts.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    }
}
