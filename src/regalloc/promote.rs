// Region-resident spill promotion (REARCH.md §13p, R4.16).
// THEORY A7 — Belady spilling's dual: a value the allocator spilled to MEMORY,
// but which a register could have held across its whole range, is put back in a
// register.
//
// WHAT MEASURED IT. `sqlite3VdbeExec`: gcc-O1 touches the stack ZERO times in
// 6,041 instructions; zcc reloads one slot ([sp,#96], the `Vdbe *p` parameter)
// 116 times — stored once, never modified, read everywhere — while x28 sits
// UNUSED. The Braun-Hack spiller is Belady-correct within a block but its
// residency is block-local (REARCH §7.2 note), so a value live across a wide
// switch is reloaded once per arm. gcc keeps it in a callee-saved register: the
// register IS the residency. This pass realizes that where it is provably free.
//
// THE SOUND SUBSET. A callee-saved register `r` that appears NOWHERE in the
// function is free across EVERY program point, so binding a spilled value to it
// cannot conflict with anything the colourer decided — no interference test, no
// SSA reconstruction. For a spill slot with exactly one store (a single value,
// invariant in memory) and many reloads, the store becomes `mov r, src` and each
// reload is a read of `r`.
//
// COMMUTING SQUARE `⟦before⟧ = ⟦after⟧`. `r` is written exactly once (the former
// store) and never elsewhere, so from that point `r` holds the stored value at
// every later point; a reload read the same value from memory, so replacing it by
// a read of `r` is the identity. Deleting a reload whose destination dies inside
// its block — every use between the reload and the destination's next definition
// renamed to `r` — is likewise the identity, since `r` carries the value there.
// `r` is added to `f.saved`, so `frame` preserves it (AAPCS64 §6.1.1) exactly as
// it would any other callee-saved register the function now uses. Battery:
// `promote_*` in `regalloc/tests.rs`.
use crate::cfg::DomTree;
use crate::mir::*;
use std::collections::{HashMap, HashSet};

/// THEORY A7 — the cost dual of Belady spilling: putting the value back in a
/// callee-saved register ADDS its prologue save and epilogue restore (two
/// instructions — one pair once `frame_fold` runs), so the promotion pays only
/// when it removes strictly more memory traffic than that. Three reloads is the
/// first count that clears it with margin.
const MIN_RELOADS: usize = 3;

/// THE SEAM ANOTHER PASS'S MEASUREMENT NEEDS. This pass runs last and can hide
/// the frame traffic a SPILLER row was measuring — the loop-header carry's
/// A/B test saw its difference vanish the moment this one learned to promote a
/// loop-carried slot. A battery that measures the layer below therefore turns
/// this off around its count, in the thread it runs in, rather than through an
/// environment variable two parallel tests would share.
thread_local! {
    // THEORY A7 — instrument half, as `RECONSTRUCT` and `PRUNE` in `spill.rs`.
    static PROMOTE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

pub(super) fn set_enabled(on: bool) {
    PROMOTE.with(|c| c.set(on));
}

/// THEORY A7  SQUARE promote_moves_a_spilled_value_out_of_memory — a wholly-free register is the residency
pub fn run(f: &mut MFunc) {
    if std::env::var("ZCC_NOPROMOTE").is_ok() || !PROMOTE.with(|c| c.get()) {
        return;
    }
    // 1. Every physical register the function already mentions.
    let mut used: HashSet<PReg> = HashSet::new();
    for b in &f.blocks {
        for i in &b.insts {
            i.visit(&mut |r, _| {
                if let Reg::P(p) = r {
                    used.insert(p);
                }
            });
        }
        b.term.visit(&mut |r, _| {
            if let Reg::P(p) = r {
                used.insert(p);
            }
        });
    }

    // 2. The callee-saved registers that appear nowhere — free across the whole
    // function (AAPCS64 §6.1.1 names the sets; `isa::is_callee_saved` is the
    // table).
    let free_of = |class: Class| -> Vec<PReg> {
        let range: Vec<u8> = if class == Class::Gpr {
            (19..=28).collect()
        } else {
            (8..=15).collect()
        };
        range
            .into_iter()
            .map(|n| PReg { class, num: n })
            .filter(|p| !used.contains(p))
            .collect()
    };
    let mut free_gpr = free_of(Class::Gpr);
    let mut free_fpr = free_of(Class::Fpr);
    if free_gpr.is_empty() && free_fpr.is_empty() {
        return;
    }

    // 3. Per spill slot: store count, reload count, the value's class and width,
    // and the single store's source register. (`frame` has not run, so the only
    // slots are the allocator's spills — the callee-save slots do not exist yet.)
    struct Slot {
        stores: usize,
        reloads: usize,
        class: Class,
        store_at: Option<(u32, usize)>,
        reload_at: Vec<(u32, usize)>,
        /// every access's width; a slot read at two widths is not one register
        widths: Vec<Width>,
    }
    let mut info: HashMap<SlotId, Slot> = HashMap::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (ii, i) in b.insts.iter().enumerate() {
            match i {
                MInst::Spill { slot, src, w } => {
                    let e = info.entry(*slot).or_insert_with(|| Slot {
                        stores: 0,
                        reloads: 0,
                        class: class_of_reg(f, *src),
                        store_at: None,
                        reload_at: Vec::new(),
                        widths: Vec::new(),
                    });
                    e.stores += 1;
                    e.store_at = Some((bi as u32, ii));
                    e.widths.push(*w);
                }
                MInst::Reload { slot, dst, w } => {
                    let e = info.entry(*slot).or_insert_with(|| Slot {
                        stores: 0,
                        reloads: 0,
                        class: class_of_reg(f, *dst),
                        store_at: None,
                        reload_at: Vec::new(),
                        widths: Vec::new(),
                    });
                    e.reloads += 1;
                    e.reload_at.push((bi as u32, ii));
                    e.widths.push(*w);
                }
                _ => {}
            }
        }
    }

    // 4. Candidates: a single store that DOMINATES every reload (so `r` provably
    // holds the value at each read — a loop-carried value stored at the latch does
    // NOT qualify), enough reloads, and a free register of its class. Rank by
    // reloads so the hottest slot claims the first free register.
    let cfg = crate::mir::verify::cfg(f);
    let dt = DomTree::new(&cfg, f.entry);
    // A RELOAD IN A LOOP IS NOT ONE RELOAD. `MIN_RELOADS` weighs the promotion
    // against the prologue save it adds, and that comparison is about executions,
    // not about instructions in the listing: `n7_nested_subq`'s inner-loop
    // counter is ONE static reload and one static store, paid 2,400 times per
    // outer iteration, and the static count refused it while two callee-saved
    // registers sat unused. Depth is the only frequency this layer has, so a
    // reload counts 10^depth — the same estimate the spiller's own next-use
    // weighting uses, capped where the number stops meaning anything.
    let lf = crate::cfg::LoopForest::new(&cfg, &dt);
    let weight = |b: u32| -> usize {
        let d = lf.depth.get(b as usize).copied().unwrap_or(0).min(3);
        10usize.pow(d)
    };
    let dominates_all = |s: &Slot| -> bool {
        let (sb, si) = match s.store_at {
            Some(x) => x,
            None => return false,
        };
        s.reload_at.iter().all(|&(rb, ri)| {
            if rb == sb {
                si < ri
            } else {
                dt.dominates(sb, rb)
            }
        })
    };
    // THE SLOT IS THE REGISTER, and the single-store rule was never what the
    // theorem required (2026-08-28). A spill slot is addressed by nothing but
    // its own `Spill` and `Reload` instructions — no C object lives there, its
    // address is never taken — so a register that appears NOWHERE ELSE in the
    // function can stand in for the slot wholesale: every store to the slot
    // writes the register, every load reads it, and the two are then the same
    // location under two names. That holds for ANY number of stores, in any
    // order, on any path; dominance was needed only for the copy propagation
    // below, which treats the register as an invariant, and that half stays
    // behind the old test.
    //
    // WHAT ASKED FOR IT. `n7_nested_subq`'s inner loop counter is stored and
    // reloaded on EVERY iteration — a loop-carried value, so it has one store
    // per round and the old filter refused it — while x27 and x28 sat unused
    // across the whole function. The allocator had spilled a value it had two
    // free registers for.
    //
    // One condition the width adds: a slot written and read at one width is one
    // register; a slot accessed at two is not, since a 32-bit write zeroes the
    // upper half (DDI 0487 B1.2.1) and a 64-bit read would then see that zero
    // rather than the stack bytes.
    let one_width = |s: &Slot| s.widths.windows(2).all(|w| w[0] == w[1]);
    let hotness = |s: &Slot| -> usize {
        s.reload_at.iter().map(|&(b, _)| weight(b)).sum::<usize>()
            + s.store_at.map(|(b, _)| weight(b)).unwrap_or(0)
    };
    let mut cand: Vec<(SlotId, usize, Class, bool)> = info
        .iter()
        .filter(|(_, s)| s.stores >= 1 && s.reloads >= 1 && hotness(s) >= MIN_RELOADS && one_width(s))
        .map(|(id, s)| (*id, hotness(s), s.class, s.stores == 1 && dominates_all(s)))
        .collect();
    cand.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // The invariant ones — a single store that dominates every reload — are also
    // the only ones the propagation below may rewrite.
    let mut invariant: HashSet<SlotId> = HashSet::new();
    let mut bind: HashMap<SlotId, PReg> = HashMap::new();
    for (id, _, class, inv) in cand {
        if inv {
            invariant.insert(id);
        }
        let pool = if class == Class::Gpr { &mut free_gpr } else { &mut free_fpr };
        if let Some(r) = pool.pop() {
            bind.insert(id, r);
        }
    }
    if bind.is_empty() {
        return;
    }

    // 5a. Rewrite the store to `mov r, src` and each reload to `mov dst, r`.
    for b in f.blocks.iter_mut() {
        for i in b.insts.iter_mut() {
            match i {
                MInst::Spill { slot, src, w } if bind.contains_key(slot) => {
                    let r = bind[slot];
                    *i = MInst::Copy { w: *w, dst: Reg::P(r), src: *src };
                }
                MInst::Reload { slot, dst, w } if bind.contains_key(slot) => {
                    let r = bind[slot];
                    *i = MInst::Copy { w: *w, dst: *dst, src: Reg::P(r) };
                }
                _ => {}
            }
        }
    }

    // 5b. Local copy propagation of the now-invariant registers. Within a block,
    // once `dst` is set to `r` and neither is written again before `dst`'s use,
    // the use reads `r` directly; if `dst` is redefined later in the block it does
    // not escape, so the `mov dst, r` is dead and dropped.
    let promoted: HashSet<PReg> = bind
        .iter()
        .filter(|(id, _)| invariant.contains(id))
        .map(|(_, r)| *r)
        .collect();
    for b in f.blocks.iter_mut() {
        propagate_block(b, &promoted);
    }

    // 6. The function now writes these registers, so the prologue must save them.
    for r in bind.values() {
        f.saved.add(*r);
    }
}

fn class_of_reg(f: &MFunc, r: Reg) -> Class {
    match r {
        Reg::P(p) => p.class,
        Reg::V(_) => f.class_of(r),
    }
}

/// Within one block, forward-propagate every `Copy{dst, src=r}` whose `r` is a
/// promoted (globally-invariant) register: rename later uses of `dst` to `r`
/// until `dst` is redefined, and drop the copy when `dst` is redefined later in
/// the block (so it is dead at the block's exit).
fn propagate_block(b: &mut MBlock, promoted: &HashSet<PReg>) {
    let n = b.insts.len();
    let mut drop: Vec<bool> = vec![false; n];
    for i in 0..n {
        let (dst, r) = match &b.insts[i] {
            MInst::Copy { dst: Reg::P(dst), src: Reg::P(src), .. } if promoted.contains(src) => {
                (*dst, *src)
            }
            _ => continue,
        };
        // Rename PLAIN uses of dst to r until dst is redefined. A FIXED use — an
        // ABI-pinned call argument or result register — must stay in dst (renaming
        // `mov x0,r; bl f` to `bl f` reading r breaks the calling convention, the
        // segfault this guards), and its presence means the `mov` cannot be
        // dropped. Track both.
        let mut redefined = false;
        let mut fixed_use = false;
        for j in (i + 1)..n {
            if redefined {
                break;
            }
            let mut defs_dst = false;
            b.insts[j].visit_mut(&mut |rr, c| match c {
                Constraint::Use => {
                    if *rr == Reg::P(dst) {
                        *rr = Reg::P(r);
                    }
                }
                Constraint::UseFixed(_) => {
                    if *rr == Reg::P(dst) {
                        fixed_use = true;
                    }
                }
                Constraint::Def | Constraint::DefFixed(_) => {
                    if *rr == Reg::P(dst) {
                        defs_dst = true;
                    }
                }
            });
            if defs_dst {
                redefined = true;
            }
        }
        // The terminator (a conditional branch) never redefines a register, so
        // its plain use of dst is this value ONLY if no earlier instruction
        // redefined it.
        if !redefined {
            b.term.visit_mut(&mut |rr, c| {
                if matches!(c, Constraint::Use) && *rr == Reg::P(dst) {
                    *rr = Reg::P(r);
                }
            });
        }
        // Drop the copy only when dst dies inside the block (redefined) AND no
        // fixed use needed dst on the way — otherwise the copy still feeds either
        // a live-out value or an ABI-pinned register.
        if redefined && !fixed_use {
            drop[i] = true;
        }
    }
    let mut k = 0;
    b.insts.retain(|_| {
        let keep = !drop[k];
        k += 1;
        keep
    });
}
