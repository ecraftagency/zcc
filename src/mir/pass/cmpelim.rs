// cmp_elim (REARCH.md §8, gcc's `-fcompare-elim`) — the compare an arithmetic
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
