// const_share (MECHANISM.md §G8, R4.6) — the constant that was already
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
// materialized.
//
// HIR carries a constant as an `Operand::Imm`, not as a value: it has no
// definition point, never interferes and never needs a value number, which is
// exactly what lets isel fold it into an immediate field without first proving
// single use. The cost of that choice appears at the one place a constant does
// NOT fold — isel mints a fresh `MovImm` at every such use, and nothing shares
// them. §13n measured it on sqlite: 14,393 `movz`/`movn` of which **9,035
// repeat an immediate already materialized in the same function**, and 8,282
// `mov ,zr` of which 7,050 repeat.
//
// The fix is the one HIR uses for every other expression — dominator-scoped
// value numbering (Briggs, Cooper & Simpson 1997) — applied to the two
// instructions that have no HIR value to be numbered as: `MovImm` and `Adrp`.
//
// COMMUTING SQUARE. `MovImm` and `Adrp` are PURE and CONSTANT: their result
// depends on nothing but their own operand, which is a literal. So if one
// dominates another with the same literal and the same width, the earlier has
// already executed on every run that reaches the later and has produced the same
// bits — the later is a copy of it. That is the identical argument `hir/pass/gvn`
// makes, and it needs no alias oracle and no effect analysis because neither
// instruction reads anything.
//
// WHY IT RUNS BEFORE THE SPILLER, NOT AFTER. Sharing lengthens a live range, so
// it trades a materialization for register pressure. That trade is the SPILLER's
// to make, and it already knows how: a spilled value whose definition is a
// `MovImm` is REMATERIALIZED rather than reloaded (`spill.rs`). Running first
// therefore offers the sharing and lets the pressure decide, instead of
// pre-empting it with a heuristic here.
//
// EXCEPT ACROSS A CALL, and that exception is not a heuristic either. AAPCS64
// §6.1.1 leaves only ten callee-saved GPRs, so a value live across a call
// competes for a register file a fifth the size of the one every other value
// draws on. A constant is the one kind of value for which that competition is
// never worth entering: re-materializing it costs ONE instruction, and holding
// it costs one of ten. Sharing across a call therefore trades the cheapest thing
// in the function for the scarcest, and csmith proved it is not merely a bad
// trade but an unaffordable one — thirteen programs where the allocator reported
// "11 call-crossing Gpr values live but only 10 callee-saved". So the dominator
// scope is cut at every `Call`: a definition on the far side of one is not
// offered.
use crate::cfg::DomTree;
use crate::mir::*;
use std::collections::HashMap;

#[derive(PartialEq, Eq, Hash)]
enum Key {
    Imm(u8, i64),
    /// A floating constant in the `fmov #imm8` form. Its bits and its width are
    /// what identify it, exactly as `Imm` above — a different width is a
    /// different register class's value, so the two never share.
    Fimm(u8, u64),
    Sym(Sym, bool),
}

/// THEORY A6b  SQUARE a_constant_already_materialized_is_not_materialized_again — a dominating constant
pub fn run(f: &mut MFunc) {
    // A/B while the trade is being measured: sharing buys a materialization and
    // pays in live range, and only the paired number says which wins.
    if std::env::var("ZCC_NOSHARE").is_ok() {
        return;
    }
    let cfg = crate::mir::verify::cfg(f);
    let dt = DomTree::new(&cfg, f.entry);
    // The value, its width, and the call count at its definition — a merge is
    // offered only when no call separates the two (see the header).
    let mut table: HashMap<Key, (Reg, Width, u32)> = HashMap::new();
    // `rename[v] = src` — the redundant definition and the one that already
    // dominates it. A COPY is deliberately NOT inserted: measured, that traded
    // 1,678 `movz` for 1,478 `mov` and 262 extra reloads (net +338 on sqlite),
    // because a copy's two ends are two live ranges and colouring merged them
    // only sometimes. Rewriting the USES has no such second range.
    let mut rename: Vec<Option<Reg>> = vec![None; f.vregs.len()];
    let mut dead: Vec<(usize, usize)> = Vec::new();
    // one undo entry per insertion, delimited by scope markers, so leaving a
    // dominator subtree restores exactly the table its parent had
    let mut undo: Vec<Option<Key>> = Vec::new();
    // Calls seen along the current dominator path. Saved with each scope so
    // leaving a subtree restores the parent's count, exactly like the table.
    let mut calls: u32 = 0;
    let mut stack: Vec<(u32, usize, u32)> = vec![(f.entry, 0, calls)];
    visit(f, f.entry, &mut table, &mut undo, &mut rename, &mut dead, &mut calls);
    while let Some(&mut (b, ref mut i, saved)) = stack.last_mut() {
        if *i < dt.kids[b as usize].len() {
            let k = dt.kids[b as usize][*i];
            *i += 1;
            undo.push(None); // scope marker
            let here = calls;
            visit(f, k, &mut table, &mut undo, &mut rename, &mut dead, &mut calls);
            stack.push((k, 0, here));
        } else {
            calls = saved;
            stack.pop();
            while let Some(e) = undo.pop() {
                match e {
                    Some(k) => {
                        table.remove(&k);
                    }
                    None => break,
                }
            }
        }
    }
    if dead.is_empty() {
        return;
    }
    // Chains resolve: a third copy of the constant names the second, which names
    // the first. The walk is bounded because each link points at a STRICTLY
    // dominating definition.
    let resolve = |mut r: Reg, rename: &[Option<Reg>]| -> Reg {
        for _ in 0..64 {
            match r {
                Reg::V(v) => match rename.get(v as usize).copied().flatten() {
                    Some(n) if n != r => r = n,
                    _ => return r,
                },
                Reg::P(_) => return r,
            }
        }
        r
    };
    for b in f.blocks.iter_mut() {
        for inst in b.insts.iter_mut() {
            inst.visit_mut(&mut |r, c| {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    *r = resolve(*r, &rename);
                }
            });
        }
        b.term.visit_mut(&mut |r, c| {
            if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                *r = resolve(*r, &rename);
            }
        });
    }
    dead.sort_unstable();
    for &(b, i) in dead.iter().rev() {
        f.blocks[b].insts.remove(i);
    }
}

fn visit(
    f: &mut MFunc,
    b: u32,
    table: &mut HashMap<Key, (Reg, Width, u32)>,
    undo: &mut Vec<Option<Key>>,
    rename: &mut [Option<Reg>],
    dead: &mut Vec<(usize, usize)>,
    calls: &mut u32,
) {
    for i in 0..f.blocks[b as usize].insts.len() {
        if matches!(f.blocks[b as usize].insts[i], MInst::Call { .. }) {
            *calls += 1;
        }
        let (key, dst, w) = match &f.blocks[b as usize].insts[i] {
            // A physical destination is a fixed constraint, not a value: it may
            // not be deleted, and it may not be offered as a source either (it
            // is redefined again and again).
            MInst::MovImm { w, dst: Reg::V(d), imm } => {
                (Key::Imm(*w as u8, *imm), Reg::V(*d), *w)
            }
            MInst::FMovImm { w, dst: Reg::V(d), bits } => {
                (Key::Fimm(*w as u8, *bits), Reg::V(*d), *w)
            }
            MInst::Adrp { dst: Reg::V(d), sym, got } => {
                (Key::Sym(sym.clone(), *got), Reg::V(*d), Width::W64)
            }
            _ => continue,
        };
        match table.get(&key) {
            Some(&(src, sw, at)) if sw == w && at == *calls => {
                if let Reg::V(v) = dst {
                    rename[v as usize] = Some(src);
                    dead.push((b as usize, i));
                }
            }
            _ => {
                // a definition past a call REPLACES the older one: it is the
                // nearer of the two, and the older is now unreachable as a source
                table.insert(key, (dst, w, *calls));
                undo.push(Some(match &f.blocks[b as usize].insts[i] {
                    MInst::MovImm { w, imm, .. } => Key::Imm(*w as u8, *imm),
                    MInst::FMovImm { w, bits, .. } => Key::Fimm(*w as u8, *bits),
                    MInst::Adrp { sym, got, .. } => Key::Sym(sym.clone(), *got),
                    _ => unreachable!(),
                }));
            }
        }
    }
}

/// THEORY A6b  SQUARE a_constant_is_the_same_constant_every_iteration — the
/// constant a loop rebuilds
///
/// `const_share` above numbers a constant against one that DOMINATES it, which
/// is the right relation for straight-line code and the wrong one for a loop: a
/// `movz` inside the body dominates nothing outside it, so the loop rebuilds the
/// same bits on every iteration. Measured on `m2_http_parse`, an HTTP parser
/// whose states are numbers: TWELVE `movz` inside the byte loop, all of them
/// small state constants, against gcc's zero — gcc materializes them once in the
/// preheader and keeps them in registers.
///
/// SQUARE. `MovImm` and `Adrp` are pure and constant — their result depends on
/// nothing but their own literal — so evaluating one earlier changes no value.
/// The preheader dominates the header and the header dominates the body, so the
/// definition still dominates every use; SSA gives it exactly one definition, so
/// there is nothing to merge.
///
/// THE FENCES:
///   * a DEDICATED preheader — the single predecessor of the header from outside
///     the loop, and that predecessor's only successor. Anything looser
///     evaluates the constant on a path that does not enter the loop, which is
///     harmless for a pure instruction but is speculation this row is not asking
///     for.
///   * the definition must be INSIDE the loop already, or there is nothing to
///     hoist.
///   * PRESSURE is the cost, and it is why this ships behind a toggle: a
///     constant hoisted out of a loop is live across the whole loop, and a
///     program that was one value short of spilling now spills. That is a
///     measurement, not an argument (`ZCC_NOHOIST`, `MEASURED M42`).
pub fn hoist_invariant_consts(f: &mut MFunc) -> usize {
    if !hoist_wanted() {
        return 0;
    }
    let cfg = crate::mir::verify::cfg(f);
    let dt = crate::cfg::DomTree::new(&cfg, f.entry);
    let lf = crate::cfg::LoopForest::new(&cfg, &dt);
    let mut moved = 0usize;
    // innermost loops first: a constant lifted to an inner preheader can then be
    // lifted again by the enclosing loop's turn
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        let head = lf.loops[li].header;
        let inloop = |b: MBlockId| -> bool {
            lf.of[b as usize].is_some_and(|x| {
                let mut cur = Some(x);
                while let Some(y) = cur {
                    if y == li as u32 {
                        return true;
                    }
                    cur = lf.loops[y as usize].parent;
                }
                false
            })
        };
        // WHERE IT GOES, and the first cut got this wrong in a way the
        // measurement blamed on the idea. It demanded a DEDICATED preheader —
        // one outside predecessor whose only successor is the header — which a
        // rotated `for` loop does not have, since its guard branches two ways.
        // The hoist then fired on setup loops and skipped every hot one: all the
        // register pressure, none of the win.
        //
        // The immediate dominator of the header is the right block. It dominates
        // the entire loop by definition, so the definition still dominates every
        // use, and `MovImm`/`Adrp` are pure — evaluating one on a path that does
        // not enter the loop computes a value nobody reads, which costs an
        // instruction and cannot trap.
        let pre = dt.idom[head as usize];
        if pre == head || inloop(pre) {
            continue;
        }
        let mut lifted: Vec<MInst> = Vec::new();
        for b in 0..f.blocks.len() {
            if !inloop(b as MBlockId) || b == pre as usize {
                continue;
            }
            let mut keep: Vec<MInst> = Vec::with_capacity(f.blocks[b].insts.len());
            for inst in std::mem::take(&mut f.blocks[b].insts) {
                match &inst {
                    MInst::MovImm { dst: Reg::V(_), .. } | MInst::Adrp { dst: Reg::V(_), .. } => {
                        lifted.push(inst);
                        moved += 1;
                    }
                    _ => keep.push(inst),
                }
            }
            f.blocks[b].insts = keep;
        }
        // at the END of the dominator block: every value the constants might
        // (they cannot) depend on is already computed there, and the block's
        // terminator is a separate field, so this is still before the branch.
        f.blocks[pre as usize].insts.extend(lifted);
    }
    moved
}

/// R5's loop-constant seam. ON by default since `MEASURED M42`; `ZCC_NOHOIST`
/// turns it off.
///
/// WHY IT SHIPPED OFF FOR A YEAR AND WHY THAT REASON NO LONGER HOLDS. The
/// standing argument was: EXEC about four tenths of a percent, "inside the
/// run-to-run spread", bought with 2.3% of instructions that is not — and THE
/// ULTIMATUM asks for 1x on BOTH axes, so the row traded the axis zcc wins for
/// the one it loses. Two things retired that argument.
///
///   * `M38` measured `corr(INSN, EXEC) = 0.196` over ninety programs. "1x on
///     both axes" reads as one goal only while the axes are believed to track;
///     they do not, so INSN cannot veto an EXEC row on the grounds of being the
///     same question asked twice. Law 0 ranks `exec > size` and there is nothing
///     left to set against it.
///   * The old reading could not resolve four tenths of a percent because the
///     42-program suite could not (`M38`: a suite of ~45 resolves to ±0.03 at
///     best). The 96-program suite and a THREE-PAIR interleaved design can.
///
/// THE MEASUREMENT THAT DECIDED IT (`M42`), one frozen binary, both arms from
/// the same build so no rebuild difference enters the comparison:
///
///     pair      OFF      ON        OFF median   ON median   >1.1x  OFF/ON
///     1       1.0774   1.0618        1.059       1.037       34 / 34
///     2       1.0762   1.0731        1.063       1.044       36 / 31
///     3       1.0749   1.0696        1.060       1.043       37 / 31
///     mean    1.0762   1.0682        1.0607      1.0413
///
/// EXEC geomean −0.74%, the sign the same in all three pairs. The geomean is the
/// noisier statistic here — the ON arm's spread is 0.0113 against the OFF arm's
/// 0.0025, because hoisting raises pressure and a program that spills is more
/// sensitive to cache state than one that does not. **The MEDIAN is the reading
/// to trust**: −1.83%, with a within-condition spread of 0.004 to 0.007, and the
/// count of programs above 1.1x falls from 37 to 31.
///
/// ON THE DETERMINISTIC AXIS, where there is no spread at all (dynamic
/// instructions, `callgrind` Ir, against gcc -O1):
///
///     k2_live_pressure  1.331 -> 1.054      m3_dict_rehash  1.161 -> 1.210
///     v2_freelist       1.738 -> 1.266      n6_pcache_lru   1.196 -> 1.217
///     o2_fp_stencil     1.546 -> 1.346
///     w2_tagged         1.125 -> 1.063
///
/// THE RESIDUAL, and it is named rather than hidden (Law 3): the cost is
/// PRESSURE. A constant hoisted out of a loop is live across it, so a function
/// one value short of spilling now spills — `m3_dict_rehash` and
/// `n6_pcache_lru` are the two programs of ninety-six that go backwards, both
/// by that mechanism, and INSN geomean pays 1.0757 -> 1.0935. Filtering by the
/// constant's `movz/movk` chain length was built to collect that back and was
/// REFUTED (`M41`): it removes almost all the static cost and gives back the two
/// largest wins, because chain length does not know what the loop is bound by.
/// A guard that works has to read pressure, and that is the row's open frontier.
pub fn hoist_wanted() -> bool {
    HOIST.with(|c| c.get()).unwrap_or_else(|| {
        static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENV.get_or_init(|| std::env::var_os("ZCC_NOHOIST").is_none())
    })
}

thread_local! {
    // THEORY A6b — instrument half, as the seams in `spill.rs`. Not a value the
    // compiler computes with.
    static HOIST: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force the hoist on or off for the CURRENT THREAD.
#[cfg(test)]
pub fn set_hoist(on: Option<bool>) {
    HOIST.with(|c| c.set(on));
}
