// Region-resident spill promotion (MECHANISM.md §G7, R4.16).
// THEORY A7 — Belady spilling's dual: a value the allocator spilled to MEMORY,
// but which a register could have held across its whole range, is put back in a
// register.
//
// WHAT MEASURED IT. `sqlite3VdbeExec`: gcc-O1 touches the stack ZERO times in
// 6,041 instructions; zcc reloads one slot ([sp,#96], the `Vdbe *p` parameter)
// 116 times — stored once, never modified, read everywhere — while x28 sits
// UNUSED. The Braun-Hack spiller is Belady-correct within a block but its
// residency is block-local (MECHANISM.md §G7.2 note), so a value live across a wide
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
    //
    // THE INVARIANT FILTER WAS THE PROPAGATION'S SIDE CONDITION, NOT THE
    // THEOREM'S (2026-08-28). What the rename needs is that `r` still holds the
    // copied value at the use it is being moved to — and for an invariant slot
    // that is true for the whole function, which is why the filter was written
    // this way. For a LOOP-CARRIED slot it is true too, just not everywhere:
    // it holds from the reload until the next STORE, and the store is a `Copy`
    // into `r` the scan below can see. So the filter comes off and the scan
    // stops at a definition of `r` instead.
    //
    // WHAT ASKED FOR IT. `n7_nested_subq`'s inner loop, which this same pass
    // promoted on the same day: the counter's reload and store became
    // `mov x8, x28` … `mov x28, x8` around a `sub`, three instructions of
    // shuttling executed 5,760,000 times, and the invariant filter refused to
    // touch any of them. Hand-editing the loop to keep the counter in `x28`
    // throughout measured **0.8669** on the program — at ONE static instruction
    // fewer, because the two copies live in a block the static count weighs the
    // same as a cold arm.
    let promoted: HashSet<PReg> = bind.values().copied().collect();
    for b in f.blocks.iter_mut() {
        propagate_block(b, &promoted);
    }

    // 6. The function now writes these registers, so the prologue must save them.
    for r in bind.values() {
        f.saved.add(*r);
    }

    // 7. SINK THE STORE INTO ITS PRODUCER — the other half of 5b, across the
    // one edge 5b cannot see.
    //
    // `split_critical_edges` gives a loop latch its own little block, and SSA
    // destruction is long finished by the time this pass turns a spill into a
    // register, so the store lands there ALONE: `mov r, d ; b header`. The
    // producer of `d` is the last instruction of the latch, `d` dies at the
    // store, and nothing else reads either name — so the producer can simply
    // write `r`, the copy goes, and the latch branches straight back. The block
    // is then unreachable and `layout` deletes it.
    //
    // WHY IT IS WORTH A DATAFLOW CHECK. On `n7_nested_subq` this is two executed
    // instructions and one taken branch out of a fifteen-instruction inner loop
    // that runs 5,760,000 times: hand-edited, **0.8585**, at an instruction
    // count that does not move at all until `layout` collects the dead block
    // (`MEASURED M29`).
    // The static cost model is blind to it by construction — one copy in a
    // latch weighs exactly what one copy in a cold arm weighs — which is the
    // same blindness Law 3c names for chains, here for FREQUENCY.
    //
    // THE THREE SIDE CONDITIONS, and each is a real miscompile without it:
    //   * `r` must be DEAD on the latch's other edge. Writing the producer into
    //     `r` clobbers the slot on the loop-EXIT path too, where the old value
    //     would otherwise survive — sound only if nothing reads it there.
    //   * `d` must be dead after the store, or the uses left behind read a name
    //     nothing writes any more.
    //   * the producer must define `d` and nothing else, plainly. A `Call`, an
    //     `Asm` or a fixed-def form is refused: its destination is the ABI's
    //     choice, not ours.
    sink_stores(f, &promoted);
}

/// Step 7 — see `run`. Read-write, and every edit is guarded by the liveness of
/// the two physical registers involved.
fn sink_stores(f: &mut MFunc, promoted: &HashSet<PReg>) {
    let cfg = crate::mir::verify::cfg(f);
    let lv = super::live::compute(f, &cfg);
    let sp = lv.sp;
    // (latch, its instruction index, the split block, its target, r, d)
    let mut work: Vec<(usize, usize, usize, MBlockId, PReg, PReg)> = Vec::new();
    for b in 0..f.blocks.len() {
        let blk = &f.blocks[b];
        let (r, d) = match (blk.insts.len(), blk.insts.first()) {
            (1, Some(MInst::Copy { dst: Reg::P(r), src: Reg::P(d), .. }))
                if promoted.contains(r) && r != d =>
            {
                (*r, *d)
            }
            _ => continue,
        };
        let target = match blk.term {
            MTerm::B(MTarget { block, ref args }) if args.is_empty() => block,
            _ => continue,
        };
        if cfg.preds[b].len() != 1 {
            continue;
        }
        let p = cfg.preds[b][0] as usize;
        if p == b {
            continue;
        }
        // The producer: the last instruction of the latch that defines `d`.
        // It need not be the final one — `n7_nested_subq`'s latch decrements
        // the counter and then bumps two pointers — but everything AFTER it
        // must leave both `d` and `r` alone, or moving the definition onto `r`
        // would either clobber a live `r` or read a `d` that no longer exists.
        let pi = match f.blocks[p]
            .insts
            .iter()
            .rposition(|i| {
                let mut hit = false;
                i.visit(&mut |rr, c| {
                    if matches!(c, Constraint::Def | Constraint::DefFixed(_)) && rr == Reg::P(d) {
                        hit = true;
                    }
                });
                hit
            }) {
            Some(i) => i,
            None => continue,
        };
        let tail_clear = f.blocks[p].insts[pi + 1..].iter().all(|i| {
            let mut clear = true;
            i.visit(&mut |rr, _| {
                if rr == Reg::P(d) || rr == Reg::P(r) {
                    clear = false;
                }
            });
            clear
        });
        if !tail_clear {
            continue;
        }
        let mut defs: Vec<Reg> = Vec::new();
        let mut fixed = false;
        f.blocks[p].insts[pi].visit(&mut |rr, c| match c {
            Constraint::Def => defs.push(rr),
            Constraint::DefFixed(_) => {
                defs.push(rr);
                fixed = true;
            }
            _ => {}
        });
        if fixed || defs.len() != 1 || defs[0] != Reg::P(d) {
            continue;
        }
        // `r` dead on every OTHER edge out of the latch, and `d` dead after the
        // store — both asked of the successors, which is where a value that
        // survives this block would have to appear.
        let ri = sp.idx(Reg::P(r));
        let di = sp.idx(Reg::P(d));
        //
        // AND `d` MUST DIE AT THE STORE ON EVERY EDGE, not only on the one that
        // reaches the store. The first cut asked it of `target` alone and three
        // allocator batteries answered with `⟦mir_v⟧ ≠ ⟦mir_p⟧`: a loop-carried
        // ACCUMULATOR is read on the loop's EXIT edge, so renaming its
        // definition onto `r` left the exit reading a `d` nothing writes any
        // more. Law 3 at the middle, doing exactly its job.
        let other_succs = || cfg.succs[p].iter().filter(|&&s| s as usize != b);
        let r_safe = other_succs().all(|&s| !lv.live_in[s as usize].contains(&ri));
        let d_safe = other_succs().all(|&s| !lv.live_in[s as usize].contains(&di))
            && !lv.live_in[target as usize].contains(&di);
        if !r_safe || !d_safe {
            continue;
        }
        // One latch, one rewrite: two split blocks can share a predecessor, and
        // the second edit would be computed from a scan the first invalidated.
        if work.iter().any(|&(q, ..)| q == p) {
            continue;
        }
        work.push((p, pi, b, target, r, d));
    }
    for (p, pi, b, target, r, d) in work {
        f.blocks[p].insts[pi].visit_mut(&mut |rr, c| {
            if matches!(c, Constraint::Def) && *rr == Reg::P(d) {
                *rr = Reg::P(r);
            }
        });
        f.blocks[p].term.visit_mut(&mut |rr, c| {
            if matches!(c, Constraint::Use) && *rr == Reg::P(d) {
                *rr = Reg::P(r);
            }
        });
        let mut term = f.blocks[p].term.clone();
        for t in term.targets_mut() {
            if t.block as usize == b {
                t.block = target;
            }
        }
        f.blocks[p].term = term;
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
        // …and `r` itself may be REDEFINED, by the store that ends this slot's
        // current value (a `Copy` into `r`). Past that point `r` no longer holds
        // what `dst` holds, so the rename must stop — reads before writes, so
        // the defining instruction's OWN uses are still renamed and the stop
        // takes effect after it.
        let mut r_redefined = false;
        for j in (i + 1)..n {
            if redefined || r_redefined {
                break;
            }
            let mut defs_dst = false;
            let mut defs_r = false;
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
                    if *rr == Reg::P(r) {
                        defs_r = true;
                    }
                }
            });
            if defs_dst {
                redefined = true;
            }
            if defs_r {
                r_redefined = true;
            }
        }
        // The terminator (a conditional branch) never redefines a register, so
        // its plain use of dst is this value ONLY if no earlier instruction
        // redefined it — and only while `r` still carries it.
        if !redefined && !r_redefined {
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
