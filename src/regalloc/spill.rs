// Spilling: reduce register pressure to ≤ k so that chordal coloring cannot
// fail (REARCH.md §7.2).
//
// STATUS — R0 ships the SOUND base case, not the final algorithm. The R0/R1
// storage model keeps every C local in memory, so the only values competing for
// registers are short-lived expression temporaries and pressure stays far below
// k = 26; this spiller therefore almost never fires, and building the full
// machinery before it can be measured would be building it blind.
//
// What is here: iterative "spill the value with the furthest next use", where a
// spilled value is stored once at its definition and reloaded into a FRESH
// virtual register immediately before each use. That keeps SSA intact with no
// reconstruction (each reload's live range is one instruction) and it does split
// live ranges — the property rc3 could not express at all — but it reloads more
// often than necessary.
//
// What is NOT here, and is REQUIRED before R2.2 (mem2reg creates long-lived
// values and real pressure): Braun & Hack 2009 proper — a per-block working set
// carried across edges with Belady MIN eviction, rematerialization of values
// whose producer is pure (`iconst`, `adrp+add`, an extend), and SSA
// reconstruction (Braun 2013) so one reload can serve several uses. Recorded as
// a residual in REARCH §12 R2.2 rather than left implicit.
use super::live;
use crate::mir::*;
use std::collections::BTreeSet;

/// Bring every program point to pressure ≤ k. Returns the number of values spilled.
///
/// COMPLEXITY (the reason this is written as a sweep rather than a loop over
/// single victims): each round costs ONE liveness computation, ONE next-use
/// index and ONE rewrite, so the whole spiller is O(rounds · |f|) with `rounds`
/// the number of times a spill decision changes liveness enough to expose new
/// pressure — one or two on real code. Choosing one victim per liveness
/// recomputation, and answering "when is this used next?" by re-scanning the
/// function, made it O(spills · |f|²) — invisible on small functions and fatal
/// on a real translation unit.
pub fn spill(f: &mut MFunc) -> Result<usize, String> {
    let mut spilled = 0;
    // Termination bound, derived rather than picked: a victim is never a value
    // the current instruction reads or writes, and a spilled value is read only
    // by the `Spill` that immediately follows its definition — so it can never
    // be chosen again, and each round retires one of the ORIGINAL virtual
    // registers. Exceeding that count means the argument above is false, which
    // is a Law-2 defect and not a budget to raise.
    let bound = f.vregs.len();
    loop {
        let cfg = crate::mir::verify::cfg(f);
        let lv = live::compute(f, &cfg);
        let vs = victims(f, &lv, &cfg)?;
        if vs.is_empty() {
            return Ok(spilled);
        }
        spilled += vs.len();
        if spilled > bound {
            return Err(format!(
                "{}: spilling retired more values ({}) than the function has virtual registers ({})",
                f.name, spilled, bound
            ));
        }
        spill_values(f, &vs);
    }
}

/// Every value this sweep decides to spill: walk each program point once and,
/// while its pressure exceeds k, evict the live value whose next use is
/// furthest away (Belady's rule). A value chosen here is treated as gone for the
/// rest of the sweep, which is what lets one liveness computation serve the
/// whole round.
fn victims(
    f: &MFunc,
    lv: &live::Liveness,
    cfg: &crate::cfg::Cfg,
) -> Result<Vec<VReg>, String> {
    // Linearize in reverse postorder so "next use" has a meaning. `base[b]` is
    // the absolute position of block b's first instruction, so a position is a
    // real instruction index — not a block index scaled by an assumed maximum
    // block length, which would mis-rank the moment a block grew past it.
    let base = linear_positions(f, cfg);
    let uses = use_positions(f, lv, cfg, &base);
    let mut chosen: Vec<VReg> = Vec::new();
    let mut lu = live::LastUse::new(lv.sp);
    let mut gone: BTreeSet<usize> = BTreeSet::new();
    let masks = [
        isa::alloc_mask(Class::Gpr),
        isa::alloc_mask(Class::Fpr),
        0u32,
    ];
    // AAPCS64 §6.1.1: a value that crosses a call can ONLY live in a
    // callee-saved register, so at EVERY point — not only at the calls — the
    // number of live call-crossing values of a class is bounded by how many
    // such registers it has. Measuring this only at calls is not enough: two
    // values may cross DIFFERENT calls and still be live together somewhere in
    // between, and the colourer would then need more callee-saved colours than
    // exist. That is precisely the shape a long-double-heavy prologue produces.
    let cs = [
        (isa::callee_saved_mask(Class::Gpr) & masks[0]).count_ones() as usize,
        (isa::callee_saved_mask(Class::Fpr) & masks[1]).count_ones() as usize,
        usize::MAX,
    ];
    for &b in &cfg.rpo {
        let bi = b as usize;
        let blk = &f.blocks[bi];
        // The live set, plus the per-class COUNTS that go with it. Counting
        // incrementally is what keeps the sweep linear: rebuilding the member
        // list of each class at each instruction is O(live) per instruction,
        // which is the whole function squared on a large one.
        let mut st = LiveSet::new(f, lv);
        for &x in lv.live_in[bi].iter().filter(|x| !gone.contains(x)) {
            st.insert(f, lv, x);
        }
        live::last_use_into(f, lv.sp, lv, bi, &mut lu);
        let last = &lu.at;
        // The colourer assigns at the BLOCK HEAD too (block parameters are
        // defined there), so the ceilings bind there as well — checking only
        // instructions leaves a point where it can run out of colours.
        for &p in &blk.params {
            let x = lv.sp.idx(p);
            if !gone.contains(&x) {
                st.insert(f, lv, x);
            }
        }
        let head = base[bi];
        for ci in 0..2 {
            let class = if ci == 0 { Class::Gpr } else { Class::Fpr };
            let extra = (st.phys_mask(ci) & masks[ci]).count_ones() as usize;
            while st.cross[ci] > cs[ci] || st.count[ci] + extra > isa::k(class) {
                let over_cross = st.cross[ci] > cs[ci];
                let cand = st
                    .set
                    .iter()
                    .copied()
                    .filter(|&x| x < lv.sp.nv && class_of(f, lv.sp.reg(x)) == class)
                    .filter(|&x| !over_cross || lv.crosses_call[x])
                    .filter(|&x| !blk.params.iter().any(|p| lv.sp.idx(*p) == x))
                    .max_by_key(|&x| next_use(&uses, x, head));
                match cand {
                    Some(x) => {
                        chosen.push(x as VReg);
                        gone.insert(x);
                        st.remove(f, lv, x);
                    }
                    None => {
                        return Err(format!(
                            "{}: {:?} pressure exceeds k at the head of bb{} with nothing evictable",
                            f.name, class, bi
                        ));
                    }
                }
            }
        }
        for (i, inst) in blk.insts.iter().enumerate() {
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    let x = lv.sp.idx(*r);
                    if !gone.contains(&x) {
                        st.insert(f, lv, x);
                    }
                }
            }
            // REARCH §7.3: a call's clobber set counts as FIXED DEFINITIONS
            // live across the instruction. That single rule is the whole of the
            // "value crosses a call" theory — with it, pressure at a call is
            // (values live across) + (clobbered colours of the class), so the
            // spiller fires exactly when more values live across the call than
            // the class has callee-saved registers, and the colourer's
            // caller-saved exclusion can never fail afterwards. Only ALLOCATABLE
            // clobbers count, and only those not already live, so the number is
            // the spec's and not an over-estimate.
            // Pressure of a class = the VIRTUAL values live here, plus every
            // allocatable register already spoken for: the physical registers
            // live at this point and, at a call, its whole clobber set. The two
            // are UNIONED, never summed — a live argument register is itself
            // clobbered, and subtracting it would pretend it frees a
            // callee-saved colour, which is exactly what it does not do. With
            // the union, `k − |clobbered ∩ allocatable|` is precisely the number
            // of callee-saved registers, so the colourer's caller-saved
            // exclusion can never fail afterwards.
            let held = match inst {
                MInst::Call { clobbers, .. } => RegSet {
                    gpr: st.phys.gpr | clobbers.gpr,
                    fpr: st.phys.fpr | clobbers.fpr,
                },
                _ => st.phys,
            };
            let extra = [
                (held.gpr & masks[0]).count_ones() as usize,
                (held.fpr & masks[1]).count_ones() as usize,
                0,
            ];
            let at = base[bi] + i;
            for (ci, class) in [Class::Gpr, Class::Fpr, Class::Flags].into_iter().enumerate() {
                // the call-crossing ceiling first: only a CROSSING value can
                // relieve it, so choosing a victim from the whole live set
                // would not converge
                while st.cross[ci] > cs[ci] {
                    let cand = st
                        .set
                        .iter()
                        .copied()
                        .filter(|&x| x < lv.sp.nv && lv.crosses_call[x])
                        .filter(|&x| class_of(f, lv.sp.reg(x)) == class)
                        .filter(|&x| !ops.iter().any(|(r, _)| lv.sp.idx(*r) == x))
                        .max_by_key(|&x| next_use(&uses, x, at));
                    match cand {
                        Some(x) => {
                            chosen.push(x as VReg);
                            gone.insert(x);
                            st.remove(f, lv, x);
                        }
                        None => {
                            return Err(format!(
                                "{}: {:?} has more call-crossing values live at bb{}[{}] than it \
                                 has callee-saved registers, and none is evictable",
                                f.name, class, bi, i
                            ));
                        }
                    }
                }
                while st.count[ci] + extra[ci] > isa::k(class) {
                    if class == Class::Flags {
                        // Flags are never spilled: their producer is pure, so the
                        // answer is to rematerialize the compare. Reaching here
                        // means isel let two flag values overlap — a Law-2
                        // Side-I defect, not a spill problem.
                        return Err(format!(
                            "{}: two NZCV values live at once; the compare must be rematerialized",
                            f.name
                        ));
                    }
                    // never evict something this instruction is about to read or
                    // has just defined, and never a physical register
                    let cand = st
                        .set
                        .iter()
                        .copied()
                        .filter(|&x| x < lv.sp.nv && class_of(f, lv.sp.reg(x)) == class)
                        .filter(|&x| !ops.iter().any(|(r, _)| lv.sp.idx(*r) == x))
                        .max_by_key(|&x| next_use(&uses, x, at));
                    match cand {
                        Some(x) => {
                            chosen.push(x as VReg);
                            gone.insert(x);
                            st.remove(f, lv, x);
                        }
                        // Every live value at this point is pinned by the very
                        // instruction that overflows: the instruction itself needs
                        // more registers than the class has. On A64 no instruction
                        // reads more than four, so this is a Law-2 defect in isel.
                        None => {
                            return Err(format!(
                                "{}: {:?} pressure exceeds k with nothing evictable at bb{}[{}]",
                                f.name, class, bi, i
                            ));
                        }
                    }
                }
            }
            // values whose last use is this instruction die here
            let dead: Vec<usize> = st
                .set
                .iter()
                .copied()
                .filter(|&x| last[x] == Some(i))
                .collect();
            for x in dead {
                st.remove(f, lv, x);
            }
        }
    }
    Ok(chosen)
}

/// The live set carried through a block, with its per-class population and the
/// physical part kept as a bit mask — everything the pressure test needs in O(1).
struct LiveSet {
    set: BTreeSet<usize>,
    count: [usize; 3],
    /// of those, the ones that cross a call — bounded by the callee-saved count
    cross: [usize; 3],
    phys: RegSet,
}

impl LiveSet {
    fn new(_f: &MFunc, _lv: &live::Liveness) -> LiveSet {
        LiveSet {
            set: BTreeSet::new(),
            count: [0; 3],
            cross: [0; 3],
            phys: RegSet::default(),
        }
    }
    fn phys_mask(&self, ci: usize) -> u32 {
        if ci == 0 { self.phys.gpr } else { self.phys.fpr }
    }
    fn slot(f: &MFunc, lv: &live::Liveness, x: usize) -> usize {
        match class_of(f, lv.sp.reg(x)) {
            Class::Gpr => 0,
            Class::Fpr => 1,
            Class::Flags => 2,
        }
    }
    fn insert(&mut self, f: &MFunc, lv: &live::Liveness, x: usize) {
        if self.set.insert(x) {
            match lv.sp.reg(x) {
                Reg::P(p) => self.phys.add(p),
                // only VIRTUAL values are counted here; the physical ones are
                // counted through `phys`, unioned with the clobber set
                Reg::V(_) => {
                    self.count[Self::slot(f, lv, x)] += 1;
                    if lv.crosses_call[x] {
                        self.cross[Self::slot(f, lv, x)] += 1;
                    }
                }
            }
        }
    }
    fn remove(&mut self, f: &MFunc, lv: &live::Liveness, x: usize) {
        if self.set.remove(&x) {
            if x < lv.sp.nv {
                self.count[Self::slot(f, lv, x)] -= 1;
                if lv.crosses_call[x] {
                    self.cross[Self::slot(f, lv, x)] -= 1;
                }
            }
            if let Reg::P(p) = lv.sp.reg(x) {
                match p.class {
                    Class::Gpr => self.phys.gpr &= !(1 << p.num),
                    Class::Fpr => self.phys.fpr &= !(1 << p.num),
                    Class::Flags => {}
                }
            }
        }
    }
}

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
/// sweep. Belady's rule needs "the next use after this point", and with this
/// index that is a binary search instead of a scan of the whole function.
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

/// Store every victim once at its definition and reload it into a FRESH
/// register before each use. The fresh registers are what makes this a live-range
/// SPLIT rather than rc3's one-home-per-value. All victims of a round are
/// rewritten in ONE pass over the function — a pass per victim would make the
/// spiller quadratic in the number of spills.
fn spill_values(f: &mut MFunc, vs: &[VReg]) {
    let mut slot_of: std::collections::HashMap<VReg, (SlotId, Width)> =
        std::collections::HashMap::new();
    for &v in vs {
        let w = f.vregs[v as usize].width;
        // a `q` spill needs 16 bytes AND 16-byte alignment (DDI 0487 C3.2: the
        // unsigned offset form scales by the access size)
        let slot = f.new_slot(w.bytes().max(8), w.bytes().max(8), SlotKind::Spill);
        slot_of.insert(v, (slot, w));
    }
    let hit = |r: Reg| -> Option<(SlotId, Width)> { r.vreg().and_then(|v| slot_of.get(&v).copied()) };
    for b in 0..f.blocks.len() {
        let mut out: Vec<MInst> = Vec::with_capacity(f.blocks[b].insts.len() + 2);
        // a block parameter is defined at the block's entry
        let params = f.blocks[b].params.clone();
        for p in params {
            if let Some((slot, w)) = hit(p) {
                out.push(MInst::Spill { slot, src: p, w });
            }
        }
        let insts = std::mem::take(&mut f.blocks[b].insts);
        for mut inst in insts {
            let mut reloads: Vec<(Reg, Reg)> = Vec::new();
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_))
                    && hit(r).is_some()
                    && !reloads.iter().any(|(o, _)| *o == r)
                {
                    reloads.push((r, r));
                }
            });
            for (orig, fresh) in reloads.iter_mut() {
                let (slot, w) = hit(*orig).unwrap();
                let d = f.new_vreg(w);
                out.push(MInst::Reload { slot, dst: d, w });
                *fresh = d;
            }
            if !reloads.is_empty() {
                inst.visit_mut(&mut |r, c| {
                    if matches!(c, Constraint::Use | Constraint::UseFixed(_))
                        && let Some((_, d)) = reloads.iter().find(|(o, _)| o == r)
                    {
                        *r = *d;
                    }
                });
            }
            let mut defs: Vec<Reg> = Vec::new();
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) && hit(r).is_some() {
                    defs.push(r);
                }
            });
            out.push(inst);
            for d in defs {
                let (slot, w) = hit(d).unwrap();
                out.push(MInst::Spill { slot, src: d, w });
            }
        }
        // the terminator's operands (including edge arguments)
        let mut term = f.blocks[b].term.clone();
        let mut reloads: Vec<(Reg, Reg)> = Vec::new();
        term.visit(&mut |r, _| {
            if hit(r).is_some() && !reloads.iter().any(|(o, _)| *o == r) {
                reloads.push((r, r));
            }
        });
        for (orig, fresh) in reloads.iter_mut() {
            let (slot, w) = hit(*orig).unwrap();
            let d = f.new_vreg(w);
            out.push(MInst::Reload { slot, dst: d, w });
            *fresh = d;
        }
        if !reloads.is_empty() {
            term.visit_mut(&mut |r, _| {
                if let Some((_, d)) = reloads.iter().find(|(o, _)| o == r) {
                    *r = *d;
                }
            });
            f.blocks[b].term = term;
        }
        f.blocks[b].insts = out;
    }
}
