// const_share (REARCH.md §13n row (c), R4.6) — the constant that was already
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
    Sym(Sym, bool),
}

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
                    MInst::Adrp { sym, got, .. } => Key::Sym(sym.clone(), *got),
                    _ => unreachable!(),
                }));
            }
        }
    }
}
