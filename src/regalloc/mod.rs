// Register allocation ON SSA — the core of the re-architecture (MECHANISM.md §G7).
// THEORY A7 — register allocation on SSA
//
// The order is the whole point, and it is the exact inverse of rc3's:
//
//   split critical edges → SPILL to pressure ≤ k → COLOR (chordal, optimal)
//                        → destruct SSA → sequentialize copies
//
// rc3 ran `to_ssa ▸ passes ▸ out_of_ssa ▸ abi_alloc`: allocation AFTER SSA was
// destroyed, on a graph that is not chordal, where coloring is NP-hard and the
// heuristic gives every value one home for its whole life — so live-range
// splitting was structurally impossible and sqlite carried 27,403 frame-slot
// memory operations. Allocating while the program is still in SSA makes the
// interference graph chordal (Hack 2007), so the greedy walk in dominance
// preorder is optimal and cannot fail once the spiller has done its job.
pub mod color;
pub mod destruct;
pub mod live;
pub mod promote;
pub mod reconstruct;
pub mod spill;
#[cfg(test)]
mod tests;
pub mod verify;

use crate::cfg::DomTree;
use crate::mir::*;

pub fn allocate(f: &mut MFunc) -> Result<(), String> {
    use crate::compile::phase;
    prune_unreachable(f);
    prune_dead_params(f);
    destruct::split_critical_edges(f);
    let (col, lv) = spill_and_color(f)?;
    coalesce_report(f, &lv, &col);
    phase("  colour-check", || color::check(f, &lv, &col))?;
    phase("  destruct", || {
        destruct::apply_colors(f, &col.color)?;
        let edge_pairs = destruct::destruct(f);
        destruct::sequentialize(f, edge_pairs);
        Ok::<(), String>(())
    })?;
    // The prologue must preserve exactly the callee-saved registers this
    // function actually writes — no more (AAPCS64 §6.1.1). `frame` reads this.
    let mut saved = RegSet::default();
    for p in col.used.iter() {
        if isa::is_callee_saved(p) {
            saved.add(p);
        }
    }
    f.saved = saved;
    f.physical = true;
    // R4.16: a value the spiller sent to memory, but which a wholly-free
    // callee-saved register could hold across its range, goes back into a
    // register (adds to `f.saved`, which is why it runs here, after it is set).
    promote::run(f);
    Ok(())
}

/// R4.2 PREDICTION (`ZCC_COALESCE=1`) — read-only, changes nothing.
///
/// 84% of the copies that survive to the emitter are EDGE copies (measured by
/// `destruct::movkind_report`), so a coalescer is aimed at the right thing. This
/// asks the second question, the one that decides whether the step is worth
/// building: of the parameter/argument pairs that biased colouring FAILED to
/// give one colour, how many could have had one?
///
/// The test is the SSA interference test, not a heuristic. A block parameter `p`
/// and the argument `a` supplied on one edge can share a register exactly when
/// their live ranges do not overlap, and on an edge that means `a` must not
/// still be live INSIDE the successor: if it is, both names are needed there at
/// once and no colouring can merge them. So the columns are
///
///   SAME  — biased colouring already gave the pair one colour; the copy is gone
///           before this point and is not a coalescing opportunity, it is a
///           coalescing SUCCESS,
///   FREE  — different colours, and `a` dies on the edge: a merge is available
///           and biased colouring simply did not find it. **This column is
///           Boissinot's ceiling**, and it is the number R4.2 must beat,
///   BOUND — different colours and `a` is live in the successor: the two names
///           genuinely coexist, so the copy is REAL and no coalescer removes it.
///
/// A prediction taken from the first two columns alone would be an over-claim,
/// which is exactly what §13n's "classify before building" is there to prevent.
///
/// C0 (2026-08-28) adds the two columns the first draft SKIPPED in silence, and
/// they are the reason its three columns summed to 229 where `destruct` emitted
/// 518 edge copies. A pair only entered the count when BOTH ends were virtual;
/// a pair with a physical end — an ABI-pinned argument or result that isel
/// wrote into a block argument, or a parameter the spiller pinned — fell out
/// through a bare `continue` and was invisible. It is a different question, not
/// a smaller one: a physical end cannot be RECOLOURED, so no colouring-side
/// coalescer reaches it, and counting it inside the ceiling would overstate the
/// row. Split so the ratio decides which front the campaign opens on:
///
///   PSAME — a physical pair that already agrees; the copy is gone,
///   PDIFF — a physical pair that disagrees; only argument TARGETING (C4) or a
///           split (C3) moves it, never biased colouring.
fn coalesce_report(f: &MFunc, lv: &live::Liveness, col: &color::Coloring) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("ZCC_COALESCE").is_some()) {
        return;
    }
    let phys = |r: Reg| -> Option<PReg> {
        match r {
            Reg::P(p) => Some(p),
            Reg::V(v) => col.color.get(v as usize).copied().flatten(),
        }
    };
    let (mut same, mut free, mut bound) = (0usize, 0usize, 0usize);
    let (mut psame, mut pdiff) = (0usize, 0usize);
    // which END is the pinned one: the PARAMETER (C3's subject) or the
    // ARGUMENT (C4's), counted separately because they are opposite fixes
    let (mut ppar, mut parg) = (0usize, 0usize);
    // of PDIFF, the subset the ABI FORBIDS rather than the colourer missing
    let mut pabi = 0usize;
    // of PDIFF, the subset whose argument is the ZERO REGISTER: a constant
    // materialization wearing a register move's spelling
    let mut pzr = 0usize;
    for b in 0..f.blocks.len() {
        for (ti, t) in f.blocks[b].term.targets().iter().enumerate() {
            let _ = ti;
            let succ = t.block as usize;
            for (k, arg) in t.args.iter().enumerate() {
                let praw = match f.blocks[succ].params.get(k) {
                    Some(p) => *p,
                    None => continue,
                };
                let (p, a) = match (praw.vreg(), arg.vreg()) {
                    (Some(p), Some(a)) => (p, a),
                    // at least one end is ABI-pinned and cannot be recoloured
                    _ => {
                        match (phys(praw), phys(*arg)) {
                            (Some(x), Some(y)) if x == y => psame += 1,
                            _ => {
                                pdiff += 1;
                                if praw.vreg().is_none() {
                                    ppar += 1;
                                }
                                if arg.vreg().is_none() {
                                    parg += 1;
                                }
                                // Why the hint was unreachable. A parameter that
                                // is live across a call may take only a
                                // callee-saved colour (AAPCS64 §6.1.1), and the
                                // physical argument it is partnered with is a
                                // call RESULT in the caller-saved half — so the
                                // merge is forbidden by the ABI, not missed by
                                // the colourer. That is Law-4 category (a),
                                // FUNDAMENTAL, and it must be told apart from a
                                // pair the colourer could have merged.
                                let banned = praw.vreg().is_some_and(|p| {
                                    lv.crosses_call[p as usize]
                                }) && arg.vreg().is_none()
                                    && matches!(*arg, Reg::P(x) if !isa::is_callee_saved(x));
                                if banned {
                                    pabi += 1;
                                }
                                // …and the one that is not a COPY at all. An
                                // edge carrying the constant zero passes
                                // `Reg::P(ZR)` as its argument, and SSA
                                // destruction emits `mov wN, wzr` for it — which
                                // materializes a constant, exactly as gcc's
                                // `mov w0, 0` does in the same place. It reaches
                                // the `.s` spelled like a register move and is
                                // what made the first census read it as
                                // coalescer excess on zcc's side and as constant
                                // materialization on gcc's, once each.
                                if matches!(*arg, Reg::P(x) if x == isa::ZR) {
                                    pzr += 1;
                                }
                            }
                        }
                        continue;
                    }
                };
                let cp = col.color.get(p as usize).copied().flatten();
                let ca = col.color.get(a as usize).copied().flatten();
                if cp.is_some() && cp == ca {
                    same += 1;
                } else if lv.live_in[succ].contains(&lv.sp.idx(Reg::V(a))) {
                    bound += 1;
                } else {
                    free += 1;
                }
            }
        }
    }
    if same + free + bound + psame + pdiff > 0 {
        eprintln!(
            "COALESCE {} {} {} {} {} {} {} {} {} {}",
            f.name, same, free, bound, psame, pdiff, ppar, parg, pabi, pzr
        );
    }
}

/// Spill, then colour — and if the colouring still runs out of registers, force
/// the value it failed on into memory and start over.
///
/// THE THEOREM STILL HOLDS, and this loop is not a retreat from it. Hack 2007
/// guarantees that a chordal greedy cannot get stuck once pressure ≤ k, and the
/// spiller's post-condition (`spill::check_pressure`) establishes exactly that.
/// What the theorem does not cover is the ABI's asymmetry: AAPCS64 §6.1.1 splits
/// the register file into caller- and callee-saved, a value live across a call
/// may only use the second half, and a value that is NOT live across a call may
/// still end up occupying it. That makes the allowed sets NESTED rather than
/// equal, and greedy in dominance order — the order chordality requires — is not
/// free to colour the most-constrained value first. The honest answer is
/// Chaitin's: when the colouring fails, spill and try again. Each round makes one
/// more value memory-resident, so it terminates, and the common case still
/// colours on the first pass.
fn spill_and_color(f: &mut MFunc) -> Result<(color::Coloring, live::Liveness), String> {
    use crate::compile::phase;
    let mut forced: std::collections::BTreeSet<VReg> = std::collections::BTreeSet::new();
    let mut cross_cap = usize::MAX;
    let snapshot = f.clone();
    let cs_gpr = isa::callee_saved_mask(Class::Gpr).count_ones() as usize;
    // The well-founded measure is the pair (cross_cap, |forced|): cross_cap falls
    // at most cs_gpr+1 times, and between two falls `forced` grows at most vregs
    // times before it must either colour or fall again. This product is a hard
    // ceiling the common case (which colours on the first pass) never approaches.
    let bound = (f.vregs.len() + 1) * (cs_gpr + 2);
    for round in 0..bound {
        let _ = round;
        phase("  spill", || spill::spill_with(f, &forced, cross_cap))?;
        // POST-CONDITION (§7.6a), enforced here so the two failure kinds can be
        // told apart. `OverCross` — more values live across a call than there are
        // callee-saved registers — is the ABI asymmetry this loop already dissolves
        // for the colourer: lower the crossing ceiling and retry. Driving cross_cap
        // to 0 reloads every crossing value after its call (ncross → 0), so the
        // check must eventually pass; the convergence is the None branch's below.
        match spill::check_pressure(f) {
            Ok(()) => {}
            Err(spill::PressureErr::OverCross(ref w)) if cross_cap > 0 => {
                let cur = if cross_cap == usize::MAX { cs_gpr } else { cross_cap };
                cross_cap = cur.saturating_sub(1);
                if std::env::var("ZCC_XCAP").is_ok() {
                    eprintln!("XCAP {} cross_cap -> {} ({})", f.name, cross_cap, w);
                }
                forced.clear();
                *f = snapshot.clone();
                continue;
            }
            Err(e) => return Err(e.into_string()),
        }
        let cfg = phase("  cfg", || crate::mir::verify::cfg(f));
        let lv = phase("  liveness", || live::compute(f, &cfg));
        let dt = phase("  domtree", || DomTree::new(&cfg, f.entry));
        match phase("  colour", || color::color(f, &lv, &dt)) {
            Ok(col) => return Ok((col, lv)),
            Err(color::ColorErr::NoColour(v, holders, m)) => {
                let nholders = holders.len();
                let forcible = holders
                    .iter()
                    .filter(|&&x| (x as usize) < snapshot.vregs.len())
                    .count();
                // A value the spiller INVENTED (a reload copy) cannot itself be
                // forced — it does not exist in the snapshot we restart from — so
                // the relief comes from one of the values HOLDING a register of
                // that class instead. Either way one more value becomes
                // memory-resident, which is what makes the loop terminate.
                let pick = std::iter::once(v)
                    .chain(holders)
                    .find(|&x| (x as usize) < snapshot.vregs.len() && !forced.contains(&x));
                match pick {
                    Some(x) => {
                        forced.insert(x);
                        *f = snapshot.clone();
                        continue;
                    }
                    // Nothing left to force: every holder is either already
                    // memory-resident or a reload copy the spiller invented. The
                    // demand for callee-saved registers is what has to come down,
                    // so lower its ceiling and start over.
                    None if cross_cap > 0 => {
                        let cur = if cross_cap == usize::MAX {
                            isa::callee_saved_mask(Class::Gpr).count_ones() as usize
                        } else {
                            cross_cap
                        };
                        cross_cap = cur.saturating_sub(1);
                        forced.clear();
                        *f = snapshot.clone();
                        continue;
                    }
                    None => {
                        return Err(format!(
                            "{} (after {} forced-spill rounds, {} values forced, {} holders \
                             of which {} are pre-existing values)",
                            m,
                            round,
                            forced.len(),
                            nholders,
                            forcible
                        ));
                    }
                }
            }
            Err(color::ColorErr::Other(m)) => return Err(m),
        }
    }
    Err(format!("{}: register allocation did not converge", f.name))
}

/// Drop block parameters nothing reads, together with the arguments every edge
/// passes them. SSA destruction turns a parameter into a real copy on every
/// incoming edge, so an unread parameter is not merely a wasted colour — it is a
/// `mov` per edge, and it forces the colourer to keep a register live for it.
/// mem2reg produces them in quantity: a variable live around one join is given a
/// parameter at every join in its iterated dominance frontier, whether or not
/// that particular join is where it is read.
fn prune_dead_params(f: &mut MFunc) {
    loop {
        let mut used = vec![false; f.vregs.len()];
        for b in &f.blocks {
            for inst in &b.insts {
                inst.visit(&mut |r, c| {
                    if let (Reg::V(v), Constraint::Use | Constraint::UseFixed(_)) = (r, c) {
                        used[v as usize] = true;
                    }
                });
            }
            b.term.visit(&mut |r, _| {
                if let Reg::V(v) = r {
                    used[v as usize] = true;
                }
            });
            // an argument is a use of the value, but only while the PARAMETER it
            // feeds survives — so it is counted in the second pass below
            for t in b.term.targets() {
                for a in &t.args {
                    if let Reg::V(v) = a {
                        used[*v as usize] = true;
                    }
                }
            }
        }
        let mut changed = false;
        for b in 0..f.blocks.len() {
            let keep: Vec<bool> = f.blocks[b]
                .params
                .iter()
                .map(|p| p.vreg().is_none_or(|v| used[v as usize]))
                .collect();
            if keep.iter().all(|k| *k) {
                continue;
            }
            let mut i = 0;
            f.blocks[b].params.retain(|_| {
                i += 1;
                keep[i - 1]
            });
            for p in 0..f.blocks.len() {
                let mut term = f.blocks[p].term.clone();
                let mut edited = false;
                for t in term.targets_mut() {
                    if t.block as usize != b {
                        continue;
                    }
                    edited = true;
                    let mut i = 0;
                    t.args.retain(|_| {
                        i += 1;
                        keep[i - 1]
                    });
                }
                if edited {
                    f.blocks[p].term = term;
                }
            }
            changed = true;
        }
        if !changed {
            return;
        }
    }
}

/// Empty every block the entry cannot reach. `hir::build` leaves such blocks
/// behind by construction (statements after a `return`, a label no `goto`
/// mentions), and their instructions define values the colourer — which walks
/// the dominator tree, i.e. exactly the reachable blocks — never sees.
fn prune_unreachable(f: &mut MFunc) {
    let cfg = crate::mir::verify::cfg(f);
    for b in 0..f.blocks.len() {
        if !cfg.reachable(b as MBlockId) {
            f.blocks[b].params.clear();
            f.blocks[b].insts.clear();
            f.blocks[b].term = MTerm::Unreachable;
        }
    }
}

pub fn allocate_module(m: &mut MModule) -> Result<(), String> {
    for f in m.funcs.iter_mut() {
        allocate(f)?;
    }
    color::hint_report();
    Ok(())
}
