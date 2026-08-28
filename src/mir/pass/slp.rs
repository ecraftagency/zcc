// slp — SUPERWORD-LEVEL PARALLELISM (MECHANISM.md §G16 #13; Larsen & Amarasinghe 2000).
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
//
// Two neighbouring scalar operations that do the same thing to adjacent memory
// ARE one vector instruction. That is the whole idea, and it is a statement
// about straight-line code rather than about loops: no cross-iteration
// dependence analysis is involved, so nothing here needs a trip count, a
// direction vector, or a runtime alias check.
//
// WHY IT IS A MIR PASS AND NOT A HIR ONE, which is where the plan first put it.
// HIR would need `Ty::V128` and a vector semantics in `hir::interp`, and every
// exhaustive `match Ty` in the frontend half of the compiler would grow an arm
// for a type the frontend can never produce. The register file this needs
// ALREADY EXISTS one layer down: `Width::Q` has been carried since `long
// double`, `MemOp::Q` loads and stores sixteen bytes, the FPR class holds v
// registers, and the allocator gives a `Q` value a 16-byte slot. What was
// missing was arithmetic, which is one instruction (`MInst::VAlu`), and this.
//
// THE SHAPE, and it is deliberately ONE shape rather than a tree builder:
//
//     ldr d0, [p, #k]      ldr d1, [p, #k+8]
//     ldr d2, [q, #m]      ldr d3, [q, #m+8]
//     fop d4, d0, d2       fop d5, d1, d3
//     str d4, [r, #n]      str d5, [r, #n+8]
//
//  becomes
//
//     ldr q0, [p, #k]   ldr q1, [q, #m]   fop v2.2d, v0.2d, v1.2d   str q2, [r, #n]
//
// Eight instructions become four, and the four are the ones a NEON kernel is
// written in. Larsen-Amarasinghe's tree extension — growing the pack upward
// through operands until isomorphism fails — is the natural next row; this is
// its base case, and the base case is where the measurement should start.
//
// THE FENCES, each of which is a way the rewrite could be wrong:
//
//   * ADJACENCY, on the same base register and in the right order. Lane 0 of a
//     `q` register is the LOWEST address on a little-endian machine, so the
//     lower-addressed scalar must be the lower lane. A pair at `[p,#k]` and
//     `[p,#k+8]` is one 16-byte access of memory the program itself touches in
//     full, so the widened access reads and writes nothing new — which is what
//     makes the alignment question moot as well (A64 permits an unaligned `ldr
//     q` to normal memory, and the object is already accessed at both halves).
//   * SINGLE USE. Each scalar value the pack consumes must have exactly one
//     use, the one being replaced, or deleting its definition would lose a
//     value someone else reads.
//   * NO INTERVENING MEMORY. Between the first instruction of the pattern and
//     the last, no other instruction may write memory, be a barrier, or be a
//     call. The pack moves accesses past each other, and this is the only
//     oracle a physical MIR has — `hir::mem` did its disambiguation where the C
//     types were still visible, and a second, weaker copy of it here would be
//     the wrong place to be clever.
//   * NO VOLATILE. C99 6.7.3: a volatile access may not be duplicated, removed
//     or reordered, and merging two into one does all three.
//
// THE SQUARE. Lanes are independent (DDI 0487 C7.2): no lane sees another's
// rounding, NaN or exception state, so the meaning of the vector operation is
// exactly the two scalar meanings taken lanewise, which `mir::interp` states
// directly. Given the fences, the vector form loads the same bytes, computes
// the same two results, and stores them to the same bytes.
use crate::mir::*;

/// R5.3's A/B SEAM (`ZCC_SLP`). Off, no `VAlu` is ever built and no `Q` value
/// is ever created by this pass — the compiler is the pre-R5.3 one, which the
/// byte-identical gate checks.
pub fn wanted() -> bool {
    SLP.with(|c| c.get()).unwrap_or_else(|| {
        static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENV.get_or_init(|| std::env::var_os("ZCC_SLP").is_some())
    })
}

thread_local! {
    // THEORY A6b — instrument half: the switch a test flips to measure that a
    // pack was actually built (the non-vacuity obligation).
    static SLP: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force the pass on or off for the CURRENT THREAD, or hand it back to the
/// environment with `None`.
#[cfg(test)]
pub fn set_slp(on: Option<bool>) {
    SLP.with(|c| c.set(on));
}

/// How many packs the last run built — the non-vacuity instrument, read by the
/// battery and by `ZCC_SLPCOUNT=1`.
pub fn take_tally() -> usize {
    TALLY.with(|c| c.replace(0))
}

thread_local! {
    // THEORY A6b — instrument half, as `SLP` above. Not a value the compiler
    // computes with: it is the count a test reads to know the pass fired, which
    // is the non-vacuity obligation and not a tuning constant.
    static TALLY: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// THEORY A6b  SQUARE slp_packs_two_scalar_lanes_into_one_vector
pub fn run(f: &mut MFunc) -> bool {
    if !wanted() {
        return false;
    }
    let uses = use_counts(f);
    let org = origins(f);
    let mut built = 0usize;
    for b in 0..f.blocks.len() {
        built += pack_block(f, b, &uses, &org);
    }
    if built > 0 {
        TALLY.with(|c| c.set(c.get() + built));
    }
    built > 0
}

/// How many times each virtual register is READ in the whole function, edges
/// included. A value the pack deletes must have exactly one reader — the
/// instruction being replaced.
fn use_counts(f: &MFunc) -> Vec<u32> {
    let mut n = vec![0u32; f.vregs.len()];
    for b in &f.blocks {
        for i in &b.insts {
            i.visit(&mut |r, c| {
                if let (Reg::V(v), Constraint::Use | Constraint::UseFixed(_)) = (r, c) {
                    n[v as usize] += 1;
                }
            });
        }
        b.term.visit(&mut |r, _| {
            if let Reg::V(v) = r {
                n[v as usize] += 1;
            }
        });
        for t in b.term.targets() {
            for a in &t.args {
                if let Reg::V(v) = a {
                    n[*v as usize] += 1;
                }
            }
        }
    }
    n
}

/// `(base, offset)` of an address this pass is willing to widen. Only the
/// simplest form: a base register plus a constant. A slot has not had its base
/// assigned yet at this layer, and an indexed or writing-back address is not a
/// pair of adjacent bytes in any sense this pass can prove.
fn base_off(m: &AddrMode) -> Option<(Reg, i32)> {
    match m {
        AddrMode::BaseImm { base, off } => Some((*base, *off)),
        _ => None,
    }
}

/// The lane arrangement two scalars of this access width pack into, if any,
/// with the byte distance the second one must sit at.
fn arr_of(op: MemOp) -> Option<(Arr, i32)> {
    match op {
        MemOp::D => Some((Arr::V2D, 8)),
        // Two singles fill only half a `q`; the pack that pays is four of them,
        // which is the tree row rather than this one. Refused rather than
        // half-built: a `4s` op on two live lanes would compute two lanes of
        // whatever the register happened to hold.
        _ => None,
    }
}

/// WHAT AN ADDRESS POINTS AT, as precisely as physical MIR can say it — the
/// same idea as `hir::mem`'s `Loc`, rebuilt here because the pack MOVES
/// accesses past each other and needs to know that they cannot be the same
/// bytes.
///
/// Merging two 8-byte stores into one 16-byte store writes the FIRST of them
/// later, past the loads of the second pair; merging two loads reads the second
/// one earlier, past the first store. Both are legal only if that store and
/// those loads are different objects. A base register at this layer is not a
/// mystery: it was defined by an `AddLo12`/`Adrp` naming a linker symbol, or by
/// a `SlotAddr` naming a stack object, and those are exactly the cases C99
/// 6.2.4 makes distinct. Anything else is `Unknown` and refuses the pack.
#[derive(Clone, PartialEq)]
enum Origin {
    Sym(Sym),
    Slot(SlotId),
    Unknown,
}

/// Where each virtual register's address came from.
fn origins(f: &MFunc) -> std::collections::HashMap<Reg, Origin> {
    let mut o: std::collections::HashMap<Reg, Origin> = Default::default();
    for b in &f.blocks {
        for i in &b.insts {
            match i {
                MInst::Adrp { dst, sym, .. } => {
                    o.insert(*dst, Origin::Sym(sym.clone()));
                }
                MInst::AddLo12 { dst, sym, .. } => {
                    o.insert(*dst, Origin::Sym(sym.clone()));
                }
                MInst::SlotAddr { dst, slot, .. } => {
                    o.insert(*dst, Origin::Slot(*slot));
                }
                _ => {}
            }
        }
    }
    o
}

/// Can a 16-byte access based at `x` and one based at `y` be the same bytes?
///
/// Only two DIFFERENT named objects answer no. Two accesses through the same
/// object are not separated here even when their offsets differ: the offsets
/// are relative to a base whose own offset within the object this layer does
/// not carry, so the honest answer is "may alias" — which costs a pack and
/// never an answer.
fn may_alias(
    o: &std::collections::HashMap<Reg, Origin>,
    x: Reg,
    y: Reg,
) -> bool {
    match (o.get(&x), o.get(&y)) {
        (Some(Origin::Sym(a)), Some(Origin::Sym(b))) => a == b,
        (Some(Origin::Slot(a)), Some(Origin::Slot(b))) => a == b,
        // no object has both automatic and static storage duration (C99 6.2.4)
        (Some(Origin::Sym(_)), Some(Origin::Slot(_)))
        | (Some(Origin::Slot(_)), Some(Origin::Sym(_))) => false,
        _ => true,
    }
}

/// Does this instruction write memory, order it, or leave the block?
fn opaque(i: &MInst) -> bool {
    match i {
        MInst::Store { vol: true, .. } | MInst::Load { vol: true, .. } => true,
        MInst::Store { .. } | MInst::Spill { .. } => true,
        MInst::Pair { load: false, .. } => true,
        MInst::Call { .. }
        | MInst::SpAdj { .. }
        | MInst::Dmb
        | MInst::LdAxr { .. }
        | MInst::StlXr { .. }
        | MInst::Stlr { .. }
        | MInst::ParallelCopy(_) => true,
        _ => false,
    }
}

/// One pass over a block, replacing every pattern it finds. Returns how many.
fn pack_block(
    f: &mut MFunc,
    b: usize,
    uses: &[u32],
    org: &std::collections::HashMap<Reg, Origin>,
) -> usize {
    let mut built = 0usize;
    // The four stores of a candidate, by the index of its SECOND store, so the
    // rewrite can be applied without invalidating the indices it is still
    // scanning: it collects first, then edits from the back.
    let mut plans: Vec<Plan> = Vec::new();
    let mut taken: Vec<bool> = vec![false; f.blocks[b].insts.len()];
    for i in 0..f.blocks[b].insts.len() {
        if taken[i] {
            continue;
        }
        if let Some(p) = plan_at(f, b, i, uses, &taken, org) {
            for &k in &p.consumed {
                taken[k] = true;
            }
            plans.push(p);
            built += 1;
        }
    }
    if built == 0 {
        return 0;
    }
    apply(f, b, plans);
    built
}

/// One recognized pattern: which instruction indices it consumes, and the four
/// it becomes.
struct Plan {
    consumed: Vec<usize>,
    /// where the vector form is placed — the position of the second store, so
    /// every value it reads is already defined
    at: usize,
    /// (low load, high load) addresses of the two operand pairs, the op, and the
    /// destination address
    a_mem: AddrMode,
    b_mem: AddrMode,
    d_mem: AddrMode,
    op: FpOp,
    arr: Arr,
}

/// Try to recognize the pattern whose SECOND store sits at `i`.
fn plan_at(
    f: &MFunc,
    b: usize,
    i: usize,
    uses: &[u32],
    taken: &[bool],
    org: &std::collections::HashMap<Reg, Origin>,
) -> Option<Plan> {
    let insts = &f.blocks[b].insts;
    // the two stores
    let (op2, src2, m2) = match &insts[i] {
        MInst::Store { op, src, mem, vol: false } => (*op, *src, mem.clone()),
        _ => return None,
    };
    let (arr, stride) = arr_of(op2)?;
    // the matching first store: the nearest earlier store to the same base at
    // `off - stride`
    let (d_base, d_off2) = base_off(&m2)?;
    let j = (0..i).rev().find(|&j| {
        !taken[j]
            && matches!(&insts[j], MInst::Store { op, mem, vol: false, .. }
                if *op == op2
                    && base_off(mem) == Some((d_base, d_off2 - stride)))
    })?;
    // the LOW address is the vector store's: lane 0 is the lowest address
    let (src1, d_mem) = match &insts[j] {
        MInst::Store { src, mem, .. } => (*src, mem.clone()),
        _ => return None,
    };
    // the two arithmetic ops that produced them
    let (i1, o1) = def_of(insts, src1, j)?;
    let (i2, o2) = def_of(insts, src2, i)?;
    if taken[i1] || taken[i2] || i1 == i2 {
        return None;
    }
    let (op_a, a1, b1) = match o1 {
        MInst::FpAlu { op, w, a, b, .. } if *w == arr.lane() => (*op, *a, *b),
        _ => return None,
    };
    let (op_b, a2, b2) = match o2 {
        MInst::FpAlu { op, w, a, b, .. } if *w == arr.lane() => (*op, *a, *b),
        _ => return None,
    };
    if op_a != op_b {
        return None;
    }
    // …and the four loads, in two adjacent pairs
    let (la1, la2, a_mem) = adjacent_loads(insts, a1, a2, i1, i2, op2, stride, taken)?;
    let (lb1, lb2, b_mem) = adjacent_loads(insts, b1, b2, i1, i2, op2, stride, taken)?;
    if [la1, la2, lb1, lb2].iter().any(|&x| x == i1 || x == i2) {
        return None;
    }
    // every intermediate value must be read exactly once — by the instruction
    // this pack is replacing
    for v in [src1, src2, a1, a2, b1, b2] {
        match v.vreg() {
            Some(x) if uses[x as usize] == 1 => {}
            _ => return None,
        }
    }
    let mut consumed = vec![la1, la2, lb1, lb2, i1, i2, j, i];
    consumed.sort_unstable();
    consumed.dedup();
    if consumed.len() != 8 {
        return None;
    }
    // NOTHING FOREIGN AND OPAQUE across the span the pack rearranges. The
    // pattern's OWN first store is inside that span and is not foreign — moving
    // it is the point — so it is skipped here and answered by the alias test
    // below instead.
    let lo = consumed[0];
    if (lo..i).any(|k| !consumed.contains(&k) && opaque(&insts[k])) {
        return None;
    }
    // THE MOVE ITSELF. The first store lands where the second one stands, which
    // carries it past the second pair's loads; the second pair's loads move back
    // to where the first pair's stand, which carries them past that same store.
    // Both are the one question: can that store and those loads name one object?
    let (a_base, _) = base_off(&a_mem)?;
    let (b_base, _) = base_off(&b_mem)?;
    if may_alias(org, d_base, a_base) || may_alias(org, d_base, b_base) {
        return None;
    }
    Some(Plan { consumed, at: i, a_mem, b_mem, d_mem, op: op_a, arr })
}

/// The instruction that defines `r`, searched backwards from `before` within
/// this block; `None` if it is defined elsewhere.
fn def_of(insts: &[MInst], r: Reg, before: usize) -> Option<(usize, &MInst)> {
    (0..before).rev().find_map(|k| {
        let mut defs = false;
        insts[k].visit(&mut |x, c| {
            if x == r && matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                defs = true;
            }
        });
        if defs {
            Some((k, &insts[k]))
        } else {
            None
        }
    })
}

/// The two loads that define `r1` and `r2` must read adjacent addresses, low
/// one first. Returns their indices and the LOW address, which is the vector
/// load's.
#[allow(clippy::too_many_arguments)]
fn adjacent_loads(
    insts: &[MInst],
    r1: Reg,
    r2: Reg,
    before1: usize,
    before2: usize,
    op: MemOp,
    stride: i32,
    taken: &[bool],
) -> Option<(usize, usize, AddrMode)> {
    let (k1, d1) = def_of(insts, r1, before1)?;
    let (k2, d2) = def_of(insts, r2, before2)?;
    if taken[k1] || taken[k2] || k1 == k2 {
        return None;
    }
    let m1 = match d1 {
        MInst::Load { op: o, mem, vol: false, .. } if *o == op => mem.clone(),
        _ => return None,
    };
    let m2 = match d2 {
        MInst::Load { op: o, mem, vol: false, .. } if *o == op => mem.clone(),
        _ => return None,
    };
    let (b1, o1) = base_off(&m1)?;
    let (b2, o2) = base_off(&m2)?;
    if b1 != b2 || o2 != o1 + stride {
        return None;
    }
    Some((k1, k2, m1))
}

/// Replace each plan's eight instructions with its four.
fn apply(f: &mut MFunc, b: usize, plans: Vec<Plan>) {
    // build the replacements first: `new_vreg` borrows the function mutably and
    // the plans borrow nothing, so the two phases do not fight
    let mut edits: Vec<(usize, Vec<usize>, Vec<MInst>)> = Vec::new();
    for p in plans {
        let qa = f.new_vreg(Width::Q);
        let qb = f.new_vreg(Width::Q);
        let qd = f.new_vreg(Width::Q);
        let seq = vec![
            MInst::Load { op: MemOp::Q, dst: qa, mem: p.a_mem, vol: false },
            MInst::Load { op: MemOp::Q, dst: qb, mem: p.b_mem, vol: false },
            MInst::VAlu { op: p.op, arr: p.arr, dst: qd, a: qa, b: qb },
            MInst::Store { op: MemOp::Q, src: qd, mem: p.d_mem, vol: false },
        ];
        edits.push((p.at, p.consumed, seq));
    }
    // one rebuild of the block: every consumed index is dropped, and each plan's
    // four instructions are inserted where its second store stood
    let mut drop_at = vec![false; f.blocks[b].insts.len()];
    let mut insert: Vec<Option<Vec<MInst>>> = (0..f.blocks[b].insts.len()).map(|_| None).collect();
    for (at, consumed, seq) in edits {
        for k in consumed {
            drop_at[k] = true;
        }
        insert[at] = Some(seq);
    }
    let old = std::mem::take(&mut f.blocks[b].insts);
    let mut new = Vec::with_capacity(old.len());
    for (k, inst) in old.into_iter().enumerate() {
        if let Some(seq) = insert[k].take() {
            new.extend(seq);
        }
        if !drop_at[k] {
            new.push(inst);
        }
    }
    f.blocks[b].insts = new;
}
