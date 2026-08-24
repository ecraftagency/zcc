// licm — loop-invariant code motion (REARCH §4 row 8).
//
// UNCONDITIONAL, per §4: no register-pressure guard. The allocator owns pressure
// and now actually can — R2.2's Belady spiller splits a live range instead of
// failing on it — so a pass that refuses to hoist "in case the allocator cannot
// cope" would be pricing in a weakness that no longer exists.
//
// COMMUTING SQUARE. Moving an instruction from a loop body to its preheader
// preserves ⟦f⟧ when three things hold, and each is checked rather than assumed:
//   (1) INVARIANCE — every operand is defined outside the loop (or was itself
//       hoisted). Operands are SSA values, so "defined outside" is exactly "the
//       same value on every iteration".
//   (2) PURITY — `Effect::Pure`. A read could be hoisted too when nothing in the
//       loop writes, but a hoisted read may FAULT on a path that never entered
//       the loop, and proving ≥1 iteration is the rotation theorem's job (R2.4).
//       Recorded as residual, not silently taken.
//   (3) TRAP-FREEDOM — the one pure instruction that can fault is division, so
//       it moves only when the divisor is a non-zero literal. C99 6.5.5p5 makes
//       division by zero undefined, and hoisting one would move the fault to a
//       path the program never took.
// Dominance is preserved by construction: the preheader dominates the header,
// which dominates the whole body, so a use inside the loop is still dominated by
// a definition placed there.
use super::*;

pub fn run(f: &mut Func) -> bool {
    let mut changed = false;
    // Preheaders first: creating one changes the CFG, so the analyses are rebuilt
    // afterwards and the motion itself runs on a stable graph.
    if preheaders(f) {
        changed = true;
    }
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    if lf.loops.is_empty() {
        return changed;
    }
    // where each value is defined, as a block
    let def_blk = |f: &Func, v: ValueId| -> Option<BlockId> {
        match f.values[v as usize].def {
            Def::FuncParam(_) => None,
            Def::Inst(b, _) | Def::Param(b, _) => Some(b),
        }
    };
    let _ = &def_blk;
    // innermost first: hoisting out of an inner loop exposes motion in the outer
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        let body: Vec<BlockId> = lf.loops[li].body.clone();
        let header = lf.loops[li].header;
        let pre = match preheader_of(f, &c, header) {
            Some(p) => p,
            None => continue,
        };
        loop {
            let mut moved = None;
            'scan: for &b in &body {
                // Only a block the header DOMINATES may give up an instruction:
                // in an irreducible region the natural-loop body can contain a
                // block reachable without passing the header, and code moved out
                // of one would no longer dominate its uses.
                if b == pre || !dt.dominates(header, b) {
                    continue;
                }
                for i in 0..f.blocks[b as usize].insts.len() {
                    if hoistable(f, &f.blocks[b as usize].insts[i], pre, &dt, &def_blk) {
                        moved = Some((b, i));
                        break 'scan;
                    }
                }
            }
            let (b, i) = match moved {
                Some(x) => x,
                None => break,
            };
            let inst = f.blocks[b as usize].insts.remove(i);
            f.blocks[pre as usize].insts.push(inst);
            refresh_defs(f);
            changed = true;
        }
    }
    changed
}

fn hoistable(
    f: &Func,
    inst: &Inst,
    pre: BlockId,
    dt: &dom::DomTree,
    def_blk: &dyn Fn(&Func, ValueId) -> Option<BlockId>,
) -> bool {
    if inst.effect() != Effect::Pure {
        return false;
    }
    // (3) the only pure instruction that can fault
    if let Inst::Bin { op, b, .. } = inst {
        if matches!(op, BinOp::SDiv | BinOp::UDiv | BinOp::SRem | BinOp::URem)
            && !matches!(b, Operand::Imm(k) if *k != 0)
        {
            return false;
        }
    }
    // (1) INVARIANCE, in the form that is actually needed: every operand's
    // definition must DOMINATE the preheader. "Defined outside the loop" is the
    // usual phrasing and is equivalent for a reducible loop, but it is not
    // equivalent in general — a block outside the natural-loop body may sit on a
    // path that never reaches the preheader — and dominance is the property the
    // verifier checks, so it is the property to test.
    let mut ok = true;
    inst.uses(|o| {
        if let Operand::Val(v) = o {
            if let Some(db) = def_blk(f, v) {
                if !dt.dominates(db, pre) {
                    ok = false;
                }
            }
        }
    });
    ok
}

/// The block that falls into `header` from outside the loop, when there is
/// exactly one such edge and its source has this header as its only successor.
fn preheader_of(f: &Func, c: &dom::Cfg, header: BlockId) -> Option<BlockId> {
    let dt = dom::domtree(f, c);
    let outside: Vec<BlockId> = c.preds[header as usize]
        .iter()
        .copied()
        .filter(|&p| !dt.dominates(header, p))
        .collect();
    match outside.as_slice() {
        [p] if c.succs[*p as usize].len() == 1 => Some(*p),
        _ => None,
    }
}

/// Give every loop header a preheader: one block outside the loop through which
/// every entry edge passes, with the header as its only successor. It is where
/// hoisted code goes, and it is the only structural change this pass makes.
///
/// The header may take PARAMETERS, and different entry edges pass different
/// arguments — so the preheader takes the same parameters and forwards them. No
/// value changes, no order changes: ⟦f⟧ = ⟦preheader f⟧ by the same argument as
/// critical-edge splitting.
fn preheaders(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let mut changed = false;
    for l in &lf.loops {
        let h = l.header;
        let outside: Vec<BlockId> = c.preds[h as usize]
            .iter()
            .copied()
            .filter(|&p| !dt.dominates(h, p))
            .collect();
        if outside.is_empty() {
            continue; // unreachable header
        }
        if outside.len() == 1 && c.succs[outside[0] as usize].len() == 1 {
            continue; // already has one
        }
        let pre = f.new_block();
        f.blocks[pre as usize].weight = f.blocks[h as usize].weight;
        let tys: Vec<Ty> = f.blocks[h as usize]
            .params
            .iter()
            .map(|p| f.ty_of(*p))
            .collect();
        let mut args = Vec::with_capacity(tys.len());
        for (k, t) in tys.iter().enumerate() {
            let v = f.new_value(*t, Def::Param(pre, k as u32));
            f.blocks[pre as usize].params.push(v);
            args.push(Operand::Val(v));
        }
        f.blocks[pre as usize].term = Term::Jmp(Target { block: h, args });
        for p in outside {
            let mut term = f.blocks[p as usize].term.clone();
            for t in term.targets_mut() {
                if t.block == h {
                    t.block = pre;
                }
            }
            f.blocks[p as usize].term = term;
        }
        changed = true;
    }
    if changed {
        refresh_defs(f);
    }
    changed
}
