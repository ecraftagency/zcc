// cmp_elim (REARCH.md §8, gcc's `-fcompare-elim`) — the compare an arithmetic
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
// instruction has already performed.
//
// A64's `adds`, `subs` and `ands` set NZCV from their own result, so
// `sub w0, w1, w2` followed by `cmp w0, #0` computes the flags twice. sqlite
// paid 10,150 compares against gcc's 6,999, and this is one of the reasons.
//
// THE CONDITION CODE IS THE WHOLE PROBLEM, and it is where a careless version of
// this pass goes wrong. `cmp d, #0` sets N and Z from `d`, and C = 1, V = 0 —
// the last two by definition, since it is a subtraction of zero. `adds` sets C
// and V from the ADDITION instead, so a consumer that reads them means something
// different afterwards. Only the codes that read N and Z alone survive
// unchanged, and two more survive REWRITTEN: with V = 0, `lt` is exactly `mi`
// and `ge` is exactly `pl`. Everything else refuses the fusion.
use crate::mir::*;

/// THEORY A6b  SQUARE an_arithmetic_result_needs_no_second_compare — the flags
/// an arithmetic instruction has already set
pub fn run(f: &mut MFunc) {
    for b in 0..f.blocks.len() {
        let mut i = 0;
        while i < f.blocks[b].insts.len() {
            match fuse(f, b, i) {
                Some((cc_fixes, fl, at)) => {
                    // The condition codes are rewritten BEFORE the compare is
                    // removed: a consumer in this same block sits at an index the
                    // removal would shift.
                    for (bb, k, cc) in cc_fixes {
                        set_cc(f, bb, k, cc);
                    }
                    if let MInst::Alu { flags, .. } = &mut f.blocks[b].insts[i] {
                        *flags = Some(fl);
                    }
                    f.blocks[b].insts.remove(at);
                }
                None => i += 1,
            }
        }
    }
}

/// Where a condition code lives: an instruction index, or the terminator.
type Site = (usize, Option<usize>, CC);

/// Does this instruction read or write NZCV? The fusion moves the flag
/// definition BACK to the arithmetic, so anything in between that touches the
/// flags would have two values live at once — which the allocator rejects
/// outright (NZCV is a register class of size one), and a `Call` clobbers
/// architecturally. This is the whole side condition of searching past an
/// instruction rather than requiring the compare to be the very next one.
fn touches_flags(i: &MInst) -> bool {
    match i {
        MInst::Alu { flags, .. } => flags.is_some(),
        MInst::Cmp { .. }
        | MInst::CSel { .. }
        | MInst::CSet { .. }
        | MInst::FpCmp { .. }
        | MInst::Call { .. } => true,
        _ => false,
    }
}

fn fuse(f: &MFunc, b: usize, i: usize) -> Option<(Vec<Site>, Reg, usize)> {
    let (op, dst) = match &f.blocks[b].insts[i] {
        MInst::Alu { op, dst, flags: None, .. }
            if matches!(op, AluOp::Add | AluOp::Sub | AluOp::And) =>
        {
            (*op, *dst)
        }
        _ => return None,
    };
    // The compare need not be the NEXT instruction — only the next one that
    // touches the flags. gcc's `-fcompare-elim` searches the same window, and
    // it is what makes a count-down loop's `sub`+`cmp`+`b.ne` collapse to
    // `subs`+`b.ne` when the scheduler has put the loop's other work between
    // them (R4.13's count-down IV shape).
    let mut at = i + 1;
    let (fl, at) = loop {
        match f.blocks[b].insts.get(at)? {
            MInst::Cmp { kind: CmpKind::Cmp, a, b: Rhs::Imm(0), flags, .. } if *a == dst => {
                break (*flags, at);
            }
            other if touches_flags(other) => return None,
            _ => at += 1,
        }
    };
    let _ = op;
    // every consumer of these flags must read only N and Z
    let mut sites: Vec<Site> = Vec::new();
    for (bb, blk) in f.blocks.iter().enumerate() {
        for (k, inst) in blk.insts.iter().enumerate() {
            if let MInst::CSel { cc, flags, .. } = inst {
                if *flags == fl {
                    sites.push((bb, Some(k), rewrite(*cc)?));
                }
            }
        }
        if let MTerm::Bcc(cc, flags, ..) = &blk.term {
            if *flags == fl {
                sites.push((bb, None, rewrite(*cc)?));
            }
        }
    }
    if sites.is_empty() {
        return None;
    }
    Some((sites, fl, at))
}

/// The same test against flags whose C and V come from the arithmetic rather
/// than from a subtraction of zero. `None` = no such test exists.
fn rewrite(cc: CC) -> Option<CC> {
    match cc {
        // N and Z only: unchanged
        CC::Eq | CC::Ne | CC::Mi | CC::Pl => Some(cc),
        // with V = 0, `lt` is `n == 1` and `ge` is `n == 0`
        CC::Lt => Some(CC::Mi),
        CC::Ge => Some(CC::Pl),
        // everything else reads C or V, which the arithmetic sets differently
        _ => None,
    }
}

fn set_cc(f: &mut MFunc, b: usize, at: Option<usize>, cc: CC) {
    match at {
        Some(k) => {
            if let MInst::CSel { cc: c, .. } = &mut f.blocks[b].insts[k] {
                *c = cc;
            }
        }
        None => {
            if let MTerm::Bcc(c, ..) = &mut f.blocks[b].term {
                *c = cc;
            }
        }
    }
}

/// Branch on the FLAGS, not on a boolean made from them.
///
/// `cmp` / `cset w, cc` / `cbnz w` is three instructions for what `cmp` /
/// `b.cc` does in two. The middle one exists only because the value was
/// materialized before anyone noticed its single use was a branch.
///
/// MEASURED on sqlite: **346 csets are consumed by a `cbz`/`cbnz`/`tbz`** and
/// nothing else — 0.20% of the program. (Another 180 are stored to memory and
/// 181 have other uses; those genuinely need the register.) zcc emits 796 csets
/// against gcc's 411, and this is where the difference goes.
///
/// SQUARE. `cset dst, cc` writes 1 exactly when `cc` holds. So branching on
/// `dst != 0` is branching on `cc`, and on `dst == 0` is branching on its
/// inverse — the same edge is taken in both forms, from the same flags. The
/// fences: the `cset` must be the LAST instruction of its block, so nothing
/// between it and the terminator can disturb the flags it read, and its result
/// must have no other reader, or the register still has to be produced.
pub fn branch_on_flags(f: &mut MFunc) -> usize {
    let mut n = 0usize;
    for bi in 0..f.blocks.len() {
        let Some(last) = f.blocks[bi].insts.last().cloned() else { continue };
        let MInst::CSet { dst, cc, flags, .. } = last else { continue };
        let Reg::V(dv) = dst else { continue };
        // the terminator must be a zero-test on exactly that value
        let (taken_when_true, t, e) = match &f.blocks[bi].term {
            MTerm::Cbz { reg: Reg::V(v), zero, t, f: e, .. } if *v == dv => {
                (!*zero, t.clone(), e.clone())
            }
            _ => continue,
        };
        // no other reader anywhere
        let mut uses = 0usize;
        for b in &f.blocks {
            for i in &b.insts {
                i.visit(&mut |r, c| {
                    if r == dst && matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                        uses += 1;
                    }
                });
            }
            b.term.visit(&mut |r, _| {
                if r == dst {
                    uses += 1;
                }
            });
        }
        if uses != 1 {
            continue; // the terminator's own use is the only one allowed
        }
        let cc = if taken_when_true { cc } else { cc.invert() };
        f.blocks[bi].insts.pop();
        f.blocks[bi].term = MTerm::Bcc(cc, flags, t, e);
        n += 1;
    }
    n
}

/// THEORY A6b  SQUARE the_same_compare_twice_sets_the_same_flags — a compare
/// whose flags are already live
///
/// THE SHAPE, measured before it was written (`m2_http_parse`, 2026-08-28). One
/// C condition consumed by several selects lowers to a compare, a `cset` that
/// turns the flags into a boolean, and then — before EVERY consumer — a fresh
/// `cmp w3, #0` that turns that boolean back into flags:
///
///     cmp w3, #0 ; csel w12, w4, w12, ne
///     cmp w3, #0 ; csel x13, x13, x2, ne      <- both of these
///     cmp w3, #0 ; csinc x15, x15, x15, eq    <- are already true
///
/// `csel`, `csinc`, `movz` and a non-`S` `add` do not write NZCV, so the second
/// and third compares are dead BY CONSTRUCTION rather than by analysis. Deleting
/// the two of them in the two hot states of that parser: 68.4 ms to 65.4 ms,
/// **4.4%**, on three instructions.
///
/// THE FENCES, and each is a way the deletion could be wrong:
///   * IDENTICAL — same kind, same width, same operands. A `Rhs::Reg` compare is
///     only the same compare while neither register has been redefined.
///   * NZCV UNTOUCHED between the two, which is what makes the flags still the
///     ones the first compare set.
///   * NO SECOND FLAGS VALUE between the survivor and the last reader of the
///     deleted one. MIR before allocation lets two flag values coexist as
///     virtual registers, but the machine has one NZCV: extending a range across
///     another flags definition is exactly the "two NZCV values live at once"
///     the spiller refuses, and a pass must not hand it that.
pub fn drop_redundant_cmps(f: &mut MFunc) -> usize {
    let mut n = 0;
    for b in 0..f.blocks.len() {
        // (the compare, the flags register it defined, its index)
        let mut live: Option<(MInst, Reg, usize)> = None;
        let mut rewrite: Vec<(Reg, Reg)> = Vec::new();
        let mut drop_at: Vec<usize> = Vec::new();
        for i in 0..f.blocks[b].insts.len() {
            let inst = f.blocks[b].insts[i].clone();
            // does this instruction redefine anything the live compare reads?
            if let Some((c, _, _)) = &live {
                let mut kills = false;
                let (mut ra, mut rb) = (None, None);
                if let MInst::Cmp { a, b: rhs, .. } = c {
                    ra = Some(*a);
                    if let Rhs::Reg(r) | Rhs::Shifted(r, ..) | Rhs::Extended(r, ..) = rhs {
                        rb = Some(*r);
                    }
                }
                inst.visit(&mut |r, cons| {
                    if matches!(cons, Constraint::Def | Constraint::DefFixed(_))
                        && (Some(r) == ra || Some(r) == rb)
                    {
                        kills = true;
                    }
                });
                if kills {
                    live = None;
                }
            }
            match &inst {
                MInst::Cmp { flags, .. } => {
                    let same = match &live {
                        Some((c, _, _)) => same_compare(c, &inst),
                        None => false,
                    };
                    if same {
                        let keep = live.as_ref().map(|(_, r, _)| *r).expect("live is Some");
                        rewrite.push((*flags, keep));
                        drop_at.push(i);
                        n += 1;
                    } else {
                        live = Some((inst.clone(), *flags, i));
                    }
                }
                // anything not KNOWN to preserve NZCV ends the range
                _ if !preserves_flags(&inst) => live = None,
                _ => {}
            }
        }
        if drop_at.is_empty() {
            continue;
        }
        // every reader of a deleted flags value now reads the survivor
        for (from, to) in &rewrite {
            for i in 0..f.blocks[b].insts.len() {
                f.blocks[b].insts[i].visit_mut(&mut |r, cons| {
                    if *r == *from && matches!(cons, Constraint::Use | Constraint::UseFixed(_)) {
                        *r = *to;
                    }
                });
            }
            f.blocks[b].term.visit_mut(&mut |r, _| {
                if *r == *from {
                    *r = *to;
                }
            });
        }
        for &i in drop_at.iter().rev() {
            f.blocks[b].insts.remove(i);
        }
    }
    n
}

/// Do these two compares set NZCV to the same thing?
fn same_compare(a: &MInst, b: &MInst) -> bool {
    match (a, b) {
        (
            MInst::Cmp { kind: k1, w: w1, a: a1, b: b1, .. },
            MInst::Cmp { kind: k2, w: w2, a: a2, b: b2, .. },
        ) => k1 == k2 && w1 == w2 && a1 == a2 && same_rhs(b1, b2),
        _ => false,
    }
}

fn same_rhs(a: &Rhs, b: &Rhs) -> bool {
    match (a, b) {
        (Rhs::Imm(x), Rhs::Imm(y)) => x == y,
        (Rhs::Reg(x), Rhs::Reg(y)) => x == y,
        (Rhs::Shifted(x, k1, n1), Rhs::Shifted(y, k2, n2)) => x == y && k1 == k2 && n1 == n2,
        (Rhs::Extended(x, k1, n1), Rhs::Extended(y, k2, n2)) => x == y && k1 == k2 && n1 == n2,
        _ => false,
    }
}

/// Is this instruction KNOWN to leave NZCV alone?
///
/// A WHITELIST, and the direction matters: answering "preserves" for something
/// that does not is a miscompile, while answering "does not" for something that
/// does costs one deleted compare. The first cut asked the opposite question —
/// "does it DEFINE a flags register?" — and a CALL does not. Flags are a virtual
/// register before allocation and a call's clobber set is physical, so
/// `bl printf` sat between two identical compares and the second was deleted
/// while NZCV had, in fact, been destroyed. `tests/decay.sh` failed on a ternary
/// whose select then read the callee's flags: `c ? "T" : ""` returned the empty
/// string with `c == 1`. AAPCS64 does not preserve the condition flags across a
/// call, and neither does this pass now.
fn preserves_flags(i: &MInst) -> bool {
    match i {
        MInst::Alu { flags, .. } => flags.is_none(),
        MInst::Copy { .. }
        | MInst::MovImm { .. }
        | MInst::Load { .. }
        | MInst::Store { .. }
        | MInst::Pair { .. }
        | MInst::Spill { .. }
        | MInst::Reload { .. }
        | MInst::Adrp { .. }
        | MInst::AddLo12 { .. }
        | MInst::SlotAddr { .. }
        | MInst::SpAddr { .. }
        | MInst::CSel { .. }
        | MInst::CSet { .. }
        | MInst::Ext { .. }
        | MInst::Bfx { .. }
        | MInst::Alu3 { .. }
        | MInst::FpAlu { .. }
        | MInst::FpUn { .. }
        | MInst::FpCvt { .. }
        | MInst::FMov { .. }
        | MInst::VAlu { .. }
        | MInst::ParallelCopy(_) => true,
        // a call, an `asm`, an atomic, a barrier, a frame adjust, an FP compare
        // and every compare-like form: not known to preserve, so they end it
        _ => false,
    }
}
