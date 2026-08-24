// sink (REARCH §16 ★, pulled forward because §13b ranked it) — move a pure
// instruction down to the block that uses it.
//
// NOT an instruction-count optimization: it removes nothing. It is a REGISTER
// PRESSURE optimization, and pressure is what §13b measured as the largest
// single item in the remaining gap — 44,394 frame memory operations, every one
// of them a value the allocator could not keep. A value computed early and used
// late is live over everything in between; computing it where it is used makes
// its range as short as the program allows.
//
// COMMUTING SQUARE. Moving `i` from block B to block C preserves ⟦f⟧ when
//   (1) `i` is `Effect::Pure` — it observes nothing and nothing observes it but
//       its result,
//   (2) every use of its result is in C or dominated by C, so the definition
//       still dominates every use,
//   (3) C is dominated by B, so `i`'s own operands still dominate it,
//   (4) `i` cannot fault (division by a possibly-zero divisor stays put): C may
//       be executed strictly less often than B, and moving a fault ONTO a path
//       is illegal, but moving one OFF a path is only a refinement of ⊥.
// (4) is the reverse of licm's condition and for the same reason.
use super::*;

pub fn run(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    // where each value is used
    let mut users: Vec<Vec<BlockId>> = vec![Vec::new(); f.values.len()];
    for b in 0..f.blocks.len() {
        let bi = b as BlockId;
        let mut note = |o: Operand| {
            if let Operand::Val(v) = o {
                let u = &mut users[v as usize];
                if u.last() != Some(&bi) {
                    u.push(bi);
                }
            }
        };
        for inst in &f.blocks[b].insts {
            inst.uses(&mut note);
        }
        f.blocks[b].term.uses(&mut note);
        // a block argument is consumed by the SUCCESSOR's parameter, so the
        // value is needed on the EDGE — that is this block, not the successor
        for t in f.blocks[b].term.targets() {
            for a in &t.args {
                if let Operand::Val(v) = a {
                    let u = &mut users[*v as usize];
                    if u.last() != Some(&bi) {
                        u.push(bi);
                    }
                }
            }
        }
    }

    let mut moves: Vec<(usize, usize, BlockId)> = Vec::new();
    for b in 0..f.blocks.len() {
        if !c.reachable(b as BlockId) {
            continue;
        }
        for (i, inst) in f.blocks[b].insts.iter().enumerate() {
            if inst.effect() != Effect::Pure || faults(inst) {
                continue;
            }
            let d = match inst.dst() {
                Some(d) => d,
                None => continue,
            };
            let us = &users[d as usize];
            // one destination block, and it is not this one
            let target = match us.as_slice() {
                [x] if *x != b as BlockId => *x,
                _ => continue,
            };
            // it must be dominated by here, and no deeper in a loop than here —
            // sinking INTO a loop would execute the instruction every iteration
            if !dt.dominates(b as BlockId, target)
                || lf.depth[target as usize] > lf.depth[b]
            {
                continue;
            }
            moves.push((b, i, target));
        }
    }
    // Two sunk instructions where one feeds the other would have to land in the
    // right order, and the order they are visited in is the block numbering, not
    // the dependence. Rather than sort them, this round drops any candidate one
    // of whose OPERANDS is also a candidate: the ladder re-runs, and the next
    // round sinks it behind the one it depends on.
    let moving: std::collections::HashSet<ValueId> = moves
        .iter()
        .filter_map(|&(b, i, _)| f.blocks[b].insts[i].dst())
        .collect();
    moves.retain(|&(b, i, _)| {
        let mut ok = true;
        f.blocks[b].insts[i].uses(|o| {
            if let Operand::Val(v) = o {
                if moving.contains(&v) {
                    ok = false;
                }
            }
        });
        ok
    });
    if moves.is_empty() {
        return false;
    }
    // EVERY removal happens before ANY insertion. A target block can also be a
    // source block, and inserting into it first would shift the indices the
    // later removals were computed from — taking the wrong instruction out.
    moves.sort_by_key(|&(b, i, _)| (b, std::cmp::Reverse(i)));
    let taken: Vec<(BlockId, Inst)> = moves
        .into_iter()
        .map(|(b, i, target)| (target, f.blocks[b].insts.remove(i)))
        .collect();
    for (target, inst) in taken {
        // As late as it can go — immediately before the first use in the target —
        // but no earlier than its own OPERANDS. An operand is normally defined
        // outside the target (its definition dominates the block it came from,
        // which dominates the target), but a previous ROUND may already have sunk
        // one INTO the target, and then the order inside the block decides.
        let blk = &f.blocks[target as usize];
        let lo = last_def(blk, &inst);
        let at = first_use(blk, inst.dst());
        f.blocks[target as usize].insts.insert(at.max(lo), inst);
    }
    refresh_defs(f);
    true
}

/// One past the last instruction in `blk` that DEFINES an operand of `i`, or 0
/// when none does.
fn last_def(blk: &Block, i: &Inst) -> usize {
    let mut ops: Vec<ValueId> = Vec::new();
    i.uses(|o| {
        if let Operand::Val(v) = o {
            ops.push(v);
        }
    });
    let mut at = 0;
    for (k, inst) in blk.insts.iter().enumerate() {
        if inst.dst().is_some_and(|d| ops.contains(&d)) {
            at = k + 1;
        }
    }
    at
}

/// The index of the first instruction in `blk` that reads `d`; the end of the
/// list when only the terminator does.
fn first_use(blk: &Block, d: Option<ValueId>) -> usize {
    let d = match d {
        Some(d) => d,
        None => return 0,
    };
    for (i, inst) in blk.insts.iter().enumerate() {
        let mut hit = false;
        inst.uses(|o| {
            if o == Operand::Val(d) {
                hit = true;
            }
        });
        if hit {
            return i;
        }
    }
    blk.insts.len()
}

/// The pure instructions that can fault: division and remainder by a divisor
/// that is not a known non-zero literal (C99 6.5.5p5).
fn faults(i: &Inst) -> bool {
    match i {
        Inst::Bin { op, b, .. } => {
            matches!(op, BinOp::SDiv | BinOp::UDiv | BinOp::SRem | BinOp::URem)
                && !matches!(b, Operand::Imm(k) if *k != 0)
        }
        _ => false,
    }
}
