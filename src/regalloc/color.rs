// Chordal coloring of the SSA interference graph (REARCH.md §7.3).
//
// THE theorem this whole re-architecture rests on (Hack 2007): the interference
// graph of a program in SSA form is CHORDAL, and a preorder of the dominator
// tree is a perfect elimination order for it. Greedy coloring along that order
// is therefore OPTIMAL — it uses exactly ω(G) = the maximum register pressure
// colors, and it CANNOT get stuck once pressure ≤ k. No graph is ever built, no
// node is ever merged, and there is no coalesce/simplify/spill iteration:
// rc3 ran Chaitin-Briggs AFTER SSA destruction, on a graph that is not chordal,
// where the same greedy walk has no such guarantee.
//
// Two rules cover everything AAPCS64 asks for, with no special cases:
//   * a value live across a call may not take a caller-saved colour (§6.1.1) —
//     `Liveness::crosses_call`;
//   * argument and result registers are ordinary physical registers made live
//     by the parallel copies isel places around a call, so they interfere the
//     normal way.
//
// Coalescing is BIASED COLORING (§7.4): at a definition that is a copy, prefer
// the partner's colour when it is free. It never merges nodes and so can never
// break the pressure guarantee — the property Chaitin-Briggs coalescing has to
// be careful about.
use super::live::{Liveness, Space};
use super::live;
use crate::cfg::DomTree;
use crate::mir::*;
use std::collections::BTreeSet;

pub struct Coloring {
    /// colour of each virtual register, once assigned
    pub color: Vec<Option<PReg>>,
    /// physical registers this function actually defines (the callee-saved
    /// subset of which the prologue must preserve)
    pub used: RegSet,
}

fn class_of(f: &MFunc, r: Reg) -> Class {
    match r {
        Reg::V(v) => f.vregs[v as usize].class,
        Reg::P(p) => p.class,
    }
}

pub fn color(f: &MFunc, lv: &Liveness, dt: &DomTree) -> Result<Coloring, String> {
    let sp = lv.sp;
    let mut color: Vec<Option<PReg>> = vec![None; sp.nv];
    let mut used = RegSet::default();
    let mut lu = super::live::LastUse::new(sp);

    for &b in &dt.preorder {
        let bi = b as usize;
        let blk = &f.blocks[bi];
        // Colours occupied at the current point: every live value's colour.
        // Values live-in are already coloured — their definitions dominate this
        // block, which is exactly what the preorder guarantees.
        let mut occupied: Vec<PReg> = Vec::new();
        let mut live_here: BTreeSet<usize> = lv.live_in[bi].clone();
        for &i in &live_here {
            if let Some(p) = color_of(&color, sp, i) {
                occupied.push(p);
            }
        }

        super::live::last_use_into(f, sp, lv, bi, &mut lu);
        let last = &lu.at;

        // block parameters are defined at the block's entry
        for &p in &blk.params {
            assign(f, lv, &mut color, &mut used, &mut occupied, p, None)?;
            live_here.insert(sp.idx(p));
        }

        for (i, inst) in blk.insts.iter().enumerate() {
            // the coalescing hint: a copy wants its partner's colour
            let hint = match inst {
                MInst::Copy { src, .. } | MInst::FMov { src, .. } => phys_of(&color, sp, *src),
                _ => None,
            };
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    assign(f, lv, &mut color, &mut used, &mut occupied, *r, hint)?;
                    live_here.insert(sp.idx(*r));
                }
            }
            // Free colours only AFTER the definitions of this instruction are
            // placed. Reusing a dying operand's colour for the result is legal
            // on A64 but NOT for a parallel copy, whose assignments are
            // simultaneous; taking the conservative order costs at most one
            // register and removes the case analysis entirely.
            let dead: Vec<usize> = live_here
                .iter()
                .copied()
                .filter(|&x| last[x] == Some(i))
                .collect();
            for x in dead {
                live_here.remove(&x);
                if let Some(p) = color_of(&color, sp, x) {
                    if let Some(k) = occupied.iter().position(|q| *q == p) {
                        occupied.swap_remove(k);
                    }
                }
            }
        }
    }
    Ok(Coloring { color, used })
}

fn color_of(color: &[Option<PReg>], sp: Space, i: usize) -> Option<PReg> {
    if i < sp.nv {
        color[i]
    } else {
        match sp.reg(i) {
            Reg::P(p) => Some(p),
            Reg::V(_) => None,
        }
    }
}

fn phys_of(color: &[Option<PReg>], sp: Space, r: Reg) -> Option<PReg> {
    match r {
        Reg::P(p) => Some(p),
        Reg::V(v) => {
            let _ = sp;
            color[v as usize]
        }
    }
}

fn assign(
    f: &MFunc,
    lv: &Liveness,
    color: &mut [Option<PReg>],
    used: &mut RegSet,
    occupied: &mut Vec<PReg>,
    r: Reg,
    hint: Option<PReg>,
) -> Result<(), String> {
    let v = match r {
        // A physical definition occupies its own register.
        Reg::P(p) => {
            used.add(p);
            if !occupied.contains(&p) {
                occupied.push(p);
            }
            return Ok(());
        }
        Reg::V(v) => v,
    };
    if color[v as usize].is_some() {
        return Ok(()); // already coloured (a value defined once, visited once)
    }
    let class = f.vregs[v as usize].class;
    if class == Class::Flags {
        // k = 1: NZCV is the only member, and `spill` has already guaranteed no
        // two flag values are live at once.
        color[v as usize] = Some(PReg::NZCV);
        return Ok(());
    }
    let avoid_caller_saved = lv.crosses_call[v as usize];
    // AAPCS64 §6.1.2: only the LOW 64 bits of v8–v15 are preserved across a
    // call, so a 128-bit value live across one has NO legal colour. isel
    // guarantees this never happens (a quad is parked in memory before any
    // call), and saying so here turns a future violation into a loud failure
    // instead of a silently truncated long double.
    if avoid_caller_saved && class == Class::Fpr && f.vregs[v as usize].width == Width::Q {
        return Err(format!(
            "{}: v{} is a 128-bit value live across a call — v8–v15 preserve only \
             their low half (AAPCS64 §6.1.2), so it must be parked in memory",
            f.name, v
        ));
    }
    let conflict = lv.phys_conflict[v as usize];
    let free = |p: PReg, occupied: &Vec<PReg>| -> bool {
        !occupied.contains(&p)
            && !conflict.has(p)
            && !(avoid_caller_saved && !isa::is_callee_saved(p))
    };
    let pick = hint
        .filter(|h| h.class == class && free(*h, occupied))
        .or_else(|| {
            isa::alloc_order(class)
                .iter()
                .map(|&n| PReg { class, num: n })
                .find(|p| free(*p, occupied))
        });
    match pick {
        Some(p) => {
            color[v as usize] = Some(p);
            used.add(p);
            occupied.push(p);
            Ok(())
        }
        // Unreachable once `spill` holds pressure ≤ k — and that is the theorem,
        // so reaching here is a Law-2 defect in the spiller, not a condition to
        // paper over with a fallback.
        None => Err(format!(
            "{}: no colour for v{} in class {:?} — pressure exceeds k",
            f.name, v, class
        )),
    }
}
