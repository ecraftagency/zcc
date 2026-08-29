// loopmem — the memory cell a loop reads and writes every iteration
// (MECHANISM.md §G4; the residual `mem.rs` names and defers).
// THEORY A7b — optimization: this pass ships its commuting square
//
// WHAT `mem.rs` CANNOT SEE, AND SAYS SO. Its store→load forwarding is block-local
// plus one edge, because that is where its oracle is exact without a memory SSA.
// The commonest accumulator in C is out of that reach by one edge — the wrong one:
//
//     void accumulate(int n){ for (int i=0;i<n;i++) gsum += gtab[i&255]; }
//
// The store at the end of iteration k feeds the load at the start of iteration
// k+1, so the forward crosses the BACK EDGE. Nothing block-local can make it, and
// zcc therefore emits `ldr` and `str` on `gsum` EVERY iteration, putting a round
// trip through memory on the loop-carried dependence. gcc keeps the value in a
// register for the whole loop.
//
// WHY IT WAS INVISIBLE FOR A YEAR. On an Apple M1 Pro store-to-load forwarding is
// fast enough to hide the whole cost: `i1_global_acc` measured **0.709 — zcc
// FASTER than gcc -O1**. On a Neoverse V2, the core almost every real
// AArch64-Linux machine has, the same binary reads **4.51** (`MEASURED M46`).
// Hand-edited there, the loop goes to **1.015**.
//
// WHAT THIS PASS DOES, and it is deliberately the smaller half of the transform.
// It forwards the store across the back edge into the next iteration's load, and
// LEAVES THE STORE ALONE. Memory is therefore written exactly as before, at every
// point, so no exit needs a fix-up and no path can observe a difference. What the
// loop loses is the LOAD — the half that sits on the dependence chain. Sinking the
// store out of the loop as well is a further row and needs the value at every exit
// edge, which is a different proof; this one is complete without it.
//
// COMMUTING SQUARE. Memory is a function, and reading a location no intervening
// write may touch returns what was last written there — the identical statement
// `mem.rs` makes about two adjacent instructions, applied across one back edge.
// The side conditions are what turn the analogy into a proof:
//
//   1. **ONE reader, ONE writer, in that order.** Exactly one load and exactly one
//      store to the location in the loop, the load before the store in the SAME
//      block, and that block dominates every latch. So each iteration reads then
//      writes, once, unconditionally — the value entering iteration k+1 is the one
//      iteration k stored, on every path.
//   2. **NOTHING ELSE MAY TOUCH IT.** Every other memory access in the loop is
//      `disjoint` from the location by `mem.rs`'s oracle, and the loop contains no
//      call, no `alloca`, no `memcpy`/`memset` and no volatile access — each of
//      which may write anything.
//   3. **THE ADDRESS IS LOOP-INVARIANT**, defined outside the loop, so "the same
//      location" means the same location on every iteration.
//   4. **THE LOCATION IS A LINKER SYMBOL.** A global object exists for the whole
//      program, so the load this pass places in the preheader cannot fault even if
//      the loop body never runs. That is what removes the `entered` obligation
//      `licm` needs for its own hoists — and it is a restriction, not an
//      observation: a `Ptr` location would need the loop proven to execute.
use super::mem::{disjoint, same, Loc};
use super::*;

/// THEORY A7b  SQUARE loopmem_forwards_a_global_accumulator_across_the_back_edge —
/// the loop-carried memory cell
pub fn run(f: &mut Func) -> bool {
    if std::env::var_os("ZCC_NOLOOPMEM").is_some() {
        return false;
    }
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let mut changed = false;
    // innermost first: an outer loop's body still contains the inner one, and a
    // cell promoted in the inner loop is no longer a candidate outside it.
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        if promote_one(f, &c, &dt, &lf, li) {
            changed = true;
        }
    }
    if changed {
        refresh_defs(f);
    }
    changed
}

/// Where each value's address comes from — `mem.rs`'s map, PLUS the base-symbol
/// walk it does not need and this pass cannot work without.
///
/// WHY THE WALK. `mem.rs` reads an address only where a `SymAddr` or `SlotAddr`
/// defines it directly, and calls anything else a `Ptr`. That is enough for its
/// block-local pairs, and it is not enough here: the very first loop this pass was
/// built for reads `gtab[i&255]`, whose address is `SymAddr(gtab)` plus an offset,
/// so the oracle saw a `Ptr` against a `Sym` and answered "may alias" — refusing
/// the promotion because a DIFFERENT global might be the same global.
///
/// WHY IT IS SOUND. `Loc::Sym` already means the WHOLE object, offset untracked
/// (`mem.rs`'s own comment). An access at `&g + k` is an access to `g`, or it is
/// undefined (C99 6.5.6p8: only pointers within an object, or one past its end,
/// have defined arithmetic). So folding `SymAddr(g) + k` to `Sym(g)` names the
/// same object the C standard names, and two DIFFERENT symbols remain disjoint —
/// which is the only question this pass asks of it.
///
/// One side only. `&a - &b` is two symbols and is not an address into either, so a
/// sum with a symbol on both sides stays a `Ptr`.
fn addr_map(f: &Func) -> Vec<Option<Loc>> {
    let mut addr: Vec<Option<Loc>> = vec![None; f.values.len()];
    for b in &f.blocks {
        for inst in &b.insts {
            match inst {
                Inst::SlotAddr { dst, slot, off } => {
                    addr[*dst as usize] = Some(Loc::Slot(*slot, *off))
                }
                Inst::SymAddr { dst, sym } => addr[*dst as usize] = Some(Loc::Sym(sym.clone())),
                _ => {}
            }
        }
    }
    // Propagate through address arithmetic to a fixpoint. HIR is SSA so a
    // definition precedes its uses, but a block's order alone does not guarantee
    // the base was seen first across blocks — the loop is cheap and terminates
    // because each round only ever fills in a `None`.
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for inst in &b.insts {
                let Inst::Bin { dst, op: BinOp::Add | BinOp::Sub, a, b: rhs, .. } = inst else {
                    continue;
                };
                if addr[*dst as usize].is_some() {
                    continue;
                }
                let base = |o: &Operand| match o {
                    Operand::Val(v) => addr[*v as usize].clone(),
                    _ => None,
                };
                let (l, r) = (base(a), base(rhs));
                let found = match (l, r) {
                    (Some(Loc::Sym(g)), None) | (None, Some(Loc::Sym(g))) => Some(Loc::Sym(g)),
                    // A slot's offset IS tracked, so folding one here would have to
                    // do the arithmetic; leave it to `mem.rs`'s exact form.
                    _ => None,
                };
                if let Some(loc) = found {
                    addr[*dst as usize] = Some(loc);
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    addr
}

macro_rules! why { ($($t:tt)*) => { if std::env::var_os("ZCC_LMDEBUG").is_some() { eprintln!($($t)*); } } }

fn promote_one(f: &mut Func, c: &dom::Cfg, dt: &dom::DomTree, lf: &dom::LoopForest, li: usize) -> bool {
    let header = lf.loops[li].header;
    let body: Vec<BlockId> = lf.loops[li].body.clone();
    let latches: Vec<BlockId> = lf.loops[li].latches.clone();
    if body.is_empty() || latches.is_empty() {
        why!("lm: no body/latch");
        return false;
    }
    // The preheader: the one predecessor of the header from outside the loop. Two
    // of them and there is no single place to put the load.
    let outside: Vec<BlockId> =
        c.preds[header as usize].iter().copied().filter(|p| !body.contains(p)).collect();
    if outside.len() != 1 {
        why!("lm: {} outside preds", outside.len());
        return false;
    }
    let pre = outside[0];

    // CONDITION 2, first half: a call or an untyped bulk write may touch anything.
    for &b in &body {
        for inst in &f.blocks[b as usize].insts {
            match inst {
                Inst::Call { .. } | Inst::Alloca { .. } | Inst::MemCpy { .. } | Inst::MemSet { .. } => {
                    why!("lm: call/alloca/memcpy in loop");
                    return false;
                }
                Inst::Load { vol: true, .. } | Inst::Store { vol: true, .. } => {
                    why!("lm: volatile in loop");
                    return false;
                }
                _ => {}
            }
        }
    }

    let addr = addr_map(f);
    let loc_of = |o: Operand, ty: Ty, ac: AClass| -> Option<(Loc, u32, AClass)> {
        match o {
            Operand::Val(v) => {
                Some((addr[v as usize].clone().unwrap_or(Loc::Ptr(v, 0)), ty.bytes(), ac))
            }
            _ => None,
        }
    };
    // Every memory access in the loop, with the block and index it sits at.
    let mut accesses: Vec<(BlockId, usize, bool, (Loc, u32, AClass), Ty, Operand)> = Vec::new();
    for &b in &body {
        for (i, inst) in f.blocks[b as usize].insts.iter().enumerate() {
            match inst {
                Inst::Load { ty, addr: a, aclass, .. } => {
                    let Some(l) = loc_of(*a, *ty, *aclass) else { why!("lm: unnamed load addr"); return false };
                    accesses.push((b, i, false, l, *ty, *a));
                }
                Inst::Store { ty, addr: a, aclass, .. } => {
                    let Some(l) = loc_of(*a, *ty, *aclass) else { return false };
                    accesses.push((b, i, true, l, *ty, *a));
                }
                _ => {}
            }
        }
    }

    // Try each store as the candidate writer.
    for si in 0..accesses.len() {
        if !accesses[si].2 {
            continue;
        }
        let (sb, sidx, _, ref sloc, sty, saddr) = accesses[si].clone();
        // CONDITION 4: a linker symbol, so the preheader load cannot fault.
        if !matches!(sloc.0, Loc::Sym(_)) {
            why!("lm: store loc not a symbol");
            continue;
        }
        // CONDITION 3: the address is computed outside the loop.
        let Operand::Val(av) = saddr else { continue };
        match f.values[av as usize].def {
            Def::Inst(db, _) | Def::Param(db, _) if body.contains(&db) => { why!("lm: addr defined in loop"); continue }
            _ => {}
        }
        // CONDITION 1: exactly one reader and one writer of this location.
        let readers: Vec<usize> =
            (0..accesses.len()).filter(|&j| !accesses[j].2 && same(&accesses[j].3, sloc)).collect();
        let writers: Vec<usize> =
            (0..accesses.len()).filter(|&j| accesses[j].2 && same(&accesses[j].3, sloc)).collect();
        if readers.len() != 1 || writers.len() != 1 {
            why!("lm: {} readers {} writers", readers.len(), writers.len());
            continue;
        }
        let (lb, lidx, _, _, lty, _) = accesses[readers[0]].clone();
        if lb != sb || lidx >= sidx || lty != sty {
            why!("lm: order lb={} sb={} lidx={} sidx={}", lb, sb, lidx, sidx);
            continue;
        }
        // …and that block runs on every iteration.
        if !latches.iter().all(|&l| dt.dominates(sb, l)) {
            why!("lm: store block does not dominate every latch");
            continue;
        }
        // CONDITION 2, second half: nothing else in the loop may touch it.
        if !(0..accesses.len())
            .filter(|&j| j != si && j != readers[0])
            .all(|j| disjoint(&accesses[j].3, sloc))
        {
            why!("lm: another access may alias");
            continue;
        }
        let Inst::Store { val, .. } = f.blocks[sb as usize].insts[sidx].clone() else {
            continue;
        };
        let Inst::Load { dst: ldst, .. } = f.blocks[lb as usize].insts[lidx].clone() else {
            continue;
        };
        do_promote(f, c, header, pre, &body, sty, saddr, ldst, val, lb, lidx);
        return true;
    }
    false
}

/// The rewrite. Split out so `promote_one` reads as the predicate it is.
#[allow(clippy::too_many_arguments)]
fn do_promote(
    f: &mut Func,
    c: &dom::Cfg,
    header: BlockId,
    pre: BlockId,
    body: &[BlockId],
    ty: Ty,
    addr: Operand,
    ldst: ValueId,
    stored: Operand,
    lb: BlockId,
    lidx: usize,
) {
    // The header's new parameter carries the cell across the back edge.
    let acc = f.new_value(ty, Def::Param(header, f.blocks[header as usize].params.len() as u32));
    f.blocks[header as usize].params.push(acc);

    // The preheader loads it once. A global always exists, so this cannot fault
    // even on a path where the loop body never runs.
    let v0 = f.new_value(ty, Def::Inst(pre, f.blocks[pre as usize].insts.len() as u32));
    f.blocks[pre as usize]
        .insts
        .push(Inst::Load { dst: v0, ty, addr, aclass: ACLASS_ANY, vol: false });

    // Every edge into the header gets its argument: the preheader hands over what
    // it just read, and a latch hands over what this iteration stored.
    for &p in &c.preds[header as usize] {
        let arg = if p == pre { Operand::Val(v0) } else { stored };
        for t in f.blocks[p as usize].term.targets_mut() {
            if t.block == header {
                t.args.push(arg);
            }
        }
    }

    // The load is now the parameter. `rewrite_values` is the same substitution
    // `gvn` uses, so uses in terminators and edge arguments are covered too — the
    // argument this pass just appended among them, which is why the map is applied
    // AFTER the edges are written and the value it maps to is not `ldst`.
    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    map[ldst as usize] = Some(Operand::Val(acc));
    rewrite_values(f, &map);
    // The store stays exactly where it was: memory is written as before, at every
    // point, so no exit needs a fix-up.
    f.blocks[lb as usize].insts.remove(lidx);
    let _ = body;
}
