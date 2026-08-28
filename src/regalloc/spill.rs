// Spilling: reduce register pressure to ≤ k so that chordal colouring cannot
// THEORY A7 — Belady-based spilling (Braun & Hack 2009)
// fail (REARCH.md §7.2) — Braun & Hack 2009, "Register Spilling and Live-Range
// Splitting for SSA-Form Programs".
//
// WHY THIS IS THE REAL ALGORITHM NOW, AND WHAT MEASURED IT. R0/R1 shipped the
// sound base case — spill at the definition, reload before EVERY use — because
// the storage model kept every local in memory, so nothing but short-lived
// expression temporaries competed for registers and the spiller almost never
// fired. R2.2's mem2reg turns each live local into a value, and the base case
// collapsed exactly where it was predicted to: sqlite went from 12,253 frame
// memory operations to 275,665, `sqlite3VdbeExec` alone spending 83,620 of its
// 89,392 instructions reloading. The fix is not a heuristic tweak; it is the
// algorithm the milestone named as a blocking prerequisite.
//
// THE ALGORITHM. Per register class, walk each block forward carrying a WORKING
// SET `W` — the values that are in registers right now — of size at most the
// class budget:
//   * At the block head `W` is the live-in values that are not memory-resident,
//     ordered by next-use distance and truncated to the budget. What does not
//     fit becomes memory-resident (Belady's rule: evict what is needed furthest
//     in the future — provably optimal for a fixed cache size, which is what a
//     register class is).
//   * A use of a value not in `W` gets a RELOAD into a fresh virtual register,
//     which then serves every later use until it is evicted — and, since R4.1,
//     every later use in the blocks that follow as well, not only in the block
//     that reloaded it. That is the whole difference from the base case: one
//     reload per REGION of residency rather than one per use.
//   * Room is made before definitions, and — the rule that subsumes all
//     "crosses a call" reasoning — a call's clobber set counts as registers
//     already spoken for at that point, so what survives a call is at most the
//     number of callee-saved registers of its class. Two values crossing
//     DIFFERENT calls are handled by the same rule: whichever call comes first
//     has both of them live, and the ceiling binds there.
//   * A value whose producer reads no register (`MovImm`, `Adrp`, `SlotAddr`)
//     is REMATERIALIZED instead of stored and reloaded — the recomputation is
//     one instruction and the store disappears entirely.
//
// EVICTION IS A REGIONAL SPLIT (spec §4.3). "Memory-resident" is a property of a
// REGION of a value's live range, never of the whole value: a value evicted at a
// pressure peak is in memory from there to its next reload and in a register
// everywhere else, under its own name. `Sim::More` therefore asks for a memory
// HOME — a slot and a store after the definition — and not for banishment from
// the register file; see the enum's own comment for why termination is untouched
// by that. What is left whole-web memory-resident is the value that can hold a
// register nowhere at all, which the `web-none` column of `ZCC_SPILLCEIL` counts.
//
// NO SSA RECONSTRUCTION IS NEEDED — still true after R4.1, and now for a
// sharper reason than before. It used to hold because a reload's register never
// left the block that made it. R4.1 lets a copy cross an edge, but only where
// EVERY predecessor is holding that same copy; a copy has exactly one
// definition, so that condition says every path from the entry to the use runs
// through the definition, which IS dominance. The use is therefore dominated by
// its definition for the same reason as before, and no block parameter, no
// renaming and no Braun 2013 reconstruction is required to make it so. A value
// that stays in `W` under its ORIGINAL name is likewise never renamed.
//
// What is NOT carried is a copy at a LOOP HEADER: blocks are walked in reverse
// postorder, so the latch has not been simulated when the header is, and a
// predecessor that has not run holds nothing as far as the test above can tell.
// Residency therefore restarts each iteration. That is a truncation of the
// theorem, not a limit of it — lifting it needs a fixpoint over the loop — and
// under Law 4 it is measured rather than assumed: `ZCC_SPILLCEIL=1` prints the
// residual, and REARCH §13n records what it says.
//
// POST-CONDITION (what the colourer relies on): at every program point the
// number of virtual values of a class that are live, plus the allocatable
// physical registers spoken for there, is at most `isa::k(class)`.
use super::live;
use crate::mir::*;
use std::collections::{BTreeMap, BTreeSet};

/// R4-capstone (REARCH allocator-splitting spec §4.2/§4.4) — is the back-edge
/// carry ON?
///
/// THEORY A7: a residency crosses an edge only where the reaching definition
/// DOMINATES the use. Across a forward edge "every predecessor holds this copy"
/// establishes that on its own (the dominance carry in `carried` below). Across
/// a BACK edge it never can: the latch's copy and the preheader's copy are two
/// definitions of one value, and no name spans both. Reconciling them is exactly
/// what a block parameter is for, and until `reconstruct` had a caller this
/// fixpoint had nothing to say — measured, with the flag forced on, not one
/// carry fired anywhere, because a header's carry was an INTERSECTION over its
/// predecessors and the preheader holds nothing under the latch's name.
///
/// R4-capstone's reconstruction removes that intersection: the header takes a
/// PARAMETER, the latch feeds it the register it is still holding, and the
/// preheader feeds it one reload — paid once, against a reload the loop body was
/// paying every iteration. That is §4.2, and it is why this is now `true`.
///
/// It stays a `const` rather than an environment variable: the emitted code must
/// depend on it in exactly one way, and a `const` says so to the compiler as well
/// as to the reader. The A/B seam a test needs is `RECONSTRUCT` below, which
/// switches the whole of §4.1 rather than only its back-edge half.
const BACKEDGE_CARRY: bool = true;

/// THE A/B SEAM the non-vacuity obligation needs (Law 0, spec §5). A commuting
/// square proves nothing on an input where the pass never fires, and "fires" is
/// a DIFFERENCE: the same program allocated with the reconstruction and without
/// it. This is what lets a test measure that difference.
///
/// It has three settings rather than two because the reconstruction has two
/// halves that a measurement must be able to tell apart — `RECON_NONE` (0), no
/// block parameters at all; `RECON_JOINS` (1), §4.1's reconstruction at ordinary
/// joins, where every predecessor has already been simulated; `RECON_LOOPS` (2,
/// the default), §4.2 as well, where a loop header takes what its latch was
/// holding a round ago. Reporting the pair 0-vs-2 for a step that only owns
/// 1-vs-2 would be crediting it with the other half's number.
///
/// A thread-local and not a process-wide switch, because the battery runs its
/// tests in parallel threads — a global would make one test's measurement depend
/// on another test's timing, which is the measurement itself lying (Law 2's rare
/// third case, and not one to go inviting).
thread_local! {
    // THEORY A7 — the spiller's own theorem. Not a value the compiler computes
    // with: it is the INSTRUMENT that lets a test ask what each half of that
    // theorem's reconstruction actually did, which is the non-vacuity obligation.
    static RECONSTRUCT: std::cell::Cell<u8> = const { std::cell::Cell::new(RECON_LOOPS) };
}

// THEORY A7 — the three settings of the seam above, named rather than spelled as
// bare numbers at every call site.
#[cfg(test)]
pub(super) const RECON_NONE: u8 = 0;
#[cfg(test)]
pub(super) const RECON_JOINS: u8 = 1;
pub(super) const RECON_LOOPS: u8 = 2;

/// Restrict SSA reconstruction, or restore it, for the CURRENT THREAD.
#[cfg(test)]
pub(super) fn set_reconstruct(level: u8) {
    RECONSTRUCT.with(|c| c.set(level));
}

/// THE A/B SEAM FOR SPEC §4.3 — is eviction a REGIONAL split, or does a value
/// evicted at one pressure peak stay in memory for its whole web?
///
/// Same obligation as `RECONSTRUCT` above and same reason for being a
/// thread-local: a commuting square proves nothing on an input where the pass
/// never fires, and "fires" is the DIFFERENCE between the same program allocated
/// with the split and without it. Off, the residency of a memory-resident value
/// dies at every block boundary, which is the pre-§4.3 allocator exactly.
thread_local! {
    // THEORY A7 — the spiller's own theorem, instrument half. Not a value the
    // compiler computes with.
    static REGIONAL: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Turn the regional split off, or back on, for the CURRENT THREAD.
#[cfg(test)]
pub(super) fn set_regional(on: bool) {
    REGIONAL.with(|c| c.set(on));
}

/// THE A/B SEAM FOR THE PRUNING (spec §6, "block-param explosion"), and the
/// TALLY that says how many parameters a plan actually built.
///
/// A pruning pass is the one kind of pass whose absence is invisible in a
/// correctness square: prune nothing and the code is still right, only bigger.
/// So the obligation is the same as every other pass's — measure the difference
/// — and this is the switch that lets a test measure it. The tally is the same
/// argument one level up: a reconstruction that fires zero times is
/// byte-indistinguishable from one that is on and neutral, so the count is read
/// rather than inferred. `ZCC_PHICOUNT=1` prints the same two numbers per
/// function for a corpus-scale run.
thread_local! {
    // THEORY A7 — instrument half, as `RECONSTRUCT` and `REGIONAL` above.
    static PRUNE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    static PHI_TALLY: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Turn the trivial/dead-parameter pruning off, or back on, for this thread.
#[cfg(test)]
pub(super) fn set_prune(on: bool) {
    PRUNE.with(|c| c.set(on));
}

/// `(join parameters, loop-header parameters)` built since the last call, and
/// reset it — the non-vacuity instrument for §4.1/§4.2 inside the battery.
#[cfg(test)]
pub(super) fn take_phi_tally() -> (usize, usize) {
    PHI_TALLY.with(|c| c.replace((0, 0)))
}

pub fn spill(f: &mut MFunc) -> Result<usize, String> {
    let n = spill_with(f, &BTreeSet::new(), usize::MAX)?;
    check_pressure(f).map_err(PressureErr::into_string)?;
    Ok(n)
}

/// `forced` names values the caller has already decided must live in memory —
/// the colourer's answer when the chordal guarantee is not enough on its own
/// (see `regalloc::allocate`).
/// `cross_cap` lowers the ceiling on simultaneously-live CALL-CROSSING values
/// below the callee-saved count. The colourer applies "live across a call ⟹
/// callee-saved" to a VALUE over its whole range, so a value that never needed
/// the callee-saved half can still be sitting in it when a value that does needs
/// one — and no amount of spilling other values dislodges it. Tightening this cap
/// reduces the demand directly, and driving it to zero always converges: with no
/// crossing residency allowed, every value is reloaded after the call it would
/// have spanned.
pub fn spill_with(
    f: &mut MFunc,
    forced: &BTreeSet<VReg>,
    cross_cap: usize,
) -> Result<usize, String> {
    let remat = crate::compile::phase("    remat", || rematerializable(f));
    let web = crate::compile::phase("    webs", || webs(f));
    // CP2.3 (compile-speed): `spilled` is membership-tested on the per-operand
    // hot path of `simulate` and never iterated in order, so a dense `Vec<bool>`
    // over the (fixed) vreg index replaces the `BTreeSet<VReg>` — O(1) contains,
    // no log factor or pointer chase. `nsp` tracks the count the old `.len()`
    // gave. No new vregs are created during the fixpoint (reloads are added by
    // `apply`, after it), so the width is stable. Byte-identical: same membership.
    let mut spilled = vec![false; f.vregs.len()];
    let mut nsp = 0usize;
    for &v in forced {
        if !spilled[v as usize] {
            spilled[v as usize] = true;
            nsp += 1;
        }
    }
    // CP2.1 (compile-speed): the CFG's TOPOLOGY is invariant across the fixpoint
    // — a round only adds slots and rewrites block-arg lists / appends spills
    // (`ensure_slot`, `evict_params`), never a terminator's target block — so the
    // edge set, RPO and predecessor lists are the same every round. Build it once,
    // and the loop nesting derived from it with it: the spiller asks that one
    // question — is a reload placed on this edge landing in COLDER code than the
    // block it serves — which is the profitability half of §4.1's cold-edge
    // reload, and the reason a loop preheader may pay for a whole loop body.
    // Liveness DOES change each round (a value newly memory-resident stops being
    // live) so it is recomputed inside the loop; `simulate`'s `linear_positions`
    // likewise re-reads the (now longer) instruction lists off the same CFG.
    let cfg = crate::mir::verify::cfg(f);
    let lf = {
        let dt = crate::cfg::DomTree::new(&cfg, f.entry);
        crate::cfg::LoopForest::new(&cfg, &dt)
    };
    let depth = &lf.depth;

    // TERMINATION — one monotone lattice, and a SPENDING LIMIT on the other
    // (spec §4.4).
    //
    // FIRST LATTICE, memory residency. Every round that does not produce a plan
    // makes at least one more value memory-resident, and a value never leaves
    // that set, so it can climb at most |vregs| times. This one IS monotone, it
    // is the whole of the pre-restructure argument, and it is the ONLY thing
    // termination is allowed to rest on.
    //
    // SECOND LATTICE, register residency at a block EXIT — the one R4-capstone
    // adds. A round's plan says what each block was still holding when it ended
    // (`Plan.wexit`), and the next round hands that to a block whose predecessor
    // it has not reached yet: a loop header reading its latch. It is recomputed
    // from `wexit` each round rather than accumulated, and it is NOT monotone —
    // a fresh spill drops a value out of every `wexit`, so a (block, value) pair
    // can enter, be knocked out, and climb again. MEASURED, on the first attempt
    // that required this lattice to reach a fixed point before accepting a plan:
    // `sqlite3BitvecBuiltinTest` (167 blocks, 402 values) ran **113,024 rounds**
    // and was still going. It does not converge; it oscillates. No arithmetic on
    // lattice heights was ever going to make that number honest.
    //
    // SO IT IS NOT ITERATED TO A FIXED POINT. It is given a BUDGET of re-seeding
    // rounds, and the plan of the last one is accepted as it stands. That is
    // sound because a plan is SELF-CONSISTENT WITHIN ITS OWN ROUND: every phi
    // argument is resolved at the end of the walk from THIS round's exit sets
    // (see `simulate`), so nothing in an accepted plan ever refers to the
    // previous round's copy numbering. Convergence buys a BETTER plan, never a
    // valid one, and a step that only ever buys quality is a step that may be
    // stopped.
    //
    // The budget is the function's LOOP NESTING DEPTH plus one, because that is
    // what the seeding actually propagates: a round makes a latch's residency
    // visible to its own header, so a value carried around an inner loop becomes
    // visible to the enclosing loop's header one round later, and one round per
    // level is what it takes to see all of them. A function with no loop gets no
    // budget at all — there is no latch for it to learn about — which is most
    // functions and is why this costs nothing on straight-line code.
    let maxdepth = depth.iter().copied().max().unwrap_or(0) as usize;
    let carry_budget = if maxdepth > 0 { maxdepth + 1 } else { 0 };
    // The cap on the whole loop: the first lattice's height, the budget, and a
    // slack of two. Exceeding it means the FIRST claim is false — a value left
    // the spilled set — which is a Law-2 defect to be located, never a budget to
    // raise, so it is asserted in a debug build. In a RELEASE build it must not
    // be a failed compile: see the graceful last phase below.
    let bound = f.vregs.len() + carry_budget + 2;
    let mut slot_of: BTreeMap<VReg, (SlotId, Width)> = BTreeMap::new();
    let mut web_slot: BTreeMap<VReg, (SlotId, Width)> = BTreeMap::new();
    for &v in forced.iter() {
        if !remat.contains_key(&v) {
            ensure_slot(f, &web, &mut web_slot, &mut slot_of, v);
        }
    }
    evict_params(f, &slot_of);
    // evict_params mints a value for each read it materializes before an edge
    // store; every per-value vector has to grow with it.
    spilled.resize(f.vregs.len(), false);
    // The second lattice's carrier: per block, what it was still holding in a
    // register when it ENDED last round. Read only for a predecessor the current
    // round has not simulated yet. Empty on the first round, which is exactly
    // today's "a back edge holds nothing".
    let mut prev_exit: Vec<Vec<(VReg, Option<CopyId>)>> = vec![Vec::new(); f.blocks.len()];
    let mut carry = BACKEDGE_CARRY && RECONSTRUCT.with(|c| c.get()) >= RECON_LOOPS;
    let mut fell_back = false;
    let plan = {
        let mut plan = None;
        // If the loop runs out of rounds entirely the carry is DROPPED and the
        // remaining rounds are the pre-restructure fixpoint, whose |vregs|-round
        // argument is unconditional. So the allocator always converges to AT
        // LEAST the behaviour it had before the restructure: exhausting the cap
        // costs an optimization, never a compile. The cap stays a defect detector
        // — the `debug_assert` below fires on the fallback itself — but a user's
        // program is not the place to report it.
        let mut budget = carry_budget;
        let mut seeded = false;
        let mut nphi = 0usize;
        for round in 0..bound + f.vregs.len() + 2 {
            if round == bound && carry {
                carry = false;
                fell_back = true;
                prev_exit = vec![Vec::new(); f.blocks.len()];
            }
            let lv = crate::compile::phase("    lv", || live::compute(f, &cfg));
            match crate::compile::phase("    simulate", || simulate(f, &lv, &cfg, &spilled, cross_cap, &prev_exit, carry, &lf, &web, &remat))? {
                Sim::Plan(p) => {
                    // SPEND A ROUND ONLY IF IT CAN BUY SOMETHING. The seeding is
                    // worth another walk while it is still finding NEW block
                    // parameters — that is a value made visible to a header by
                    // the round before, one loop level at a time. Once the count
                    // stops growing, or the residency reproduces itself exactly,
                    // or the budget above is spent, the plan in hand is the
                    // answer: it is self-consistent on its own round, so there is
                    // nothing to wait for.
                    let n: usize = p.phis.iter().map(|ps| ps.len()).sum();
                    let stop = !carry
                        || budget == 0
                        || (seeded && (n <= nphi || p.wexit == prev_exit));
                    if stop {
                        plan = Some(p);
                        break;
                    }
                    budget -= 1;
                    seeded = true;
                    nphi = n;
                    prev_exit = p.wexit;
                }
                Sim::More(vs) => {
                    let before = nsp;
                    for &v in &vs {
                        if !spilled[v as usize] {
                            spilled[v as usize] = true;
                            nsp += 1;
                        }
                    }
                    if nsp == before {
                        return Err(format!("{}: spilling made no progress", f.name));
                    }
                    // A spilled PARAMETER has to leave the IR immediately, not at
                    // the end: while it is still there its incoming arguments are
                    // uses AT THE TERMINATOR, so every argument of a wide join
                    // needs a register simultaneously and no eviction the
                    // simulator can make relieves it. Removing the parameter
                    // turns those simultaneous uses into one store each.
                    // Slots are allocated for EVERY newly memory-resident value
                    // here, not only the parameters, so that `evict_params` can
                    // already see when an argument lives in the slot its
                    // parameter is being given — the store is then a no-op and is
                    // never emitted.
                    for v in vs {
                        if !remat.contains_key(&v) {
                            ensure_slot(f, &web, &mut web_slot, &mut slot_of, v);
                        }
                    }
                    evict_params(f, &slot_of);
                    // evict_params mints a value for each read it has to
                    // materialize before an edge store; every per-value vector
                    // has to grow with it.
                    spilled.resize(f.vregs.len(), false);
                }
            }
        }
        // The cap is a DEFECT DETECTOR, not a budget: reaching it falsifies one
        // of the two lattice-height claims above (spec §4.4), so a debug build
        // says so at the layer that owns the argument rather than letting a
        // half-spilled function quietly lose the optimization. Check the SECOND
        // claim first — that between two spills the register-residency set only
        // grows — since it is the recomputed one and the one this step added; the
        // first (a spill is never undone) is inspectable in ten lines above.
        debug_assert!(
            !fell_back,
            "{}: spilling ran {} rounds and fell back to the un-carried allocator — \
             the memory-residency claim (a value never leaves the spilled set, so \
             there are at most |vregs| plan-less rounds) is false, not the cap",
            f.name,
            bound
        );
        debug_assert!(
            plan.is_some(),
            "{}: spilling did not converge even with the back-edge carry dropped — \
             the memory-residency claim (a value never leaves the spilled set) is false",
            f.name
        );
        match plan {
            Some(p) => p,
            None => return Err(format!("{}: spilling did not converge", f.name)),
        }
    };
    let n = nsp;
    // R4-capstone MEASUREMENT — read-only, changes nothing. Counted on the
    // ACCEPTED plan and nowhere else: a rejected round's parameters are not in
    // the emitted function, and tallying them would report a reconstruction the
    // reader cannot find. `header` is a parameter at a block one of whose
    // predecessors comes after it in reverse postorder — §4.2's loop headers;
    // the rest are §4.1's ordinary joins.
    {
        let (mut fwd, mut back) = (0usize, 0usize);
        for (bi, ps) in plan.phis.iter().enumerate() {
            let bk = cfg.preds[bi].iter().any(|&p| cfg.rpo_num[p as usize] >= cfg.rpo_num[bi]);
            for _ in ps {
                if bk { back += 1 } else { fwd += 1 }
            }
        }
        PHI_TALLY.with(|c| {
            let (a, b) = c.get();
            c.set((a + fwd, b + back));
        });
        if (fwd + back) > 0 && std::env::var_os("ZCC_PHICOUNT").is_some() {
            eprintln!("PHICOUNT {} join {} header {}", f.name, fwd, back);
        }
    }
    ceiling_report(f, &plan, &remat);
    apply(f, plan, &spilled, &remat, &web, web_slot, slot_of);
    drop_redundant_spills(f);
    // The pressure post-condition (`check_pressure`) is enforced by the CALLER
    // now, not here: `spill_and_color` distinguishes its two failure kinds and
    // dissolves the recoverable one (`OverCross`) by lowering `cross_cap` and
    // retrying — a reaction `spill_with` cannot take from inside one round.
    // A reload copy is no longer confined to the block that made it, so "the
    // copy's definition dominates every use of it" stopped being true BY
    // INSPECTION and became a property to check. It is checked here, at the
    // layer that owns it, rather than three layers down as a wrong answer out of
    // a suite (Law 3, and §13n's standing caution that the allocator is where the
    // nastiest defects live). `mir::verify` in its virtual mode is exactly the
    // check: one definition per virtual register, every use dominated by it,
    // widths agreeing across every edge.
    #[cfg(debug_assertions)]
    crate::mir::verify::verify(f)?;
    Ok(n)
}

/// R4.1 CEILING MEASUREMENT (`ZCC_SPILLCEIL=1`) — read-only, changes nothing.
///
/// A reload's fresh register is used only inside the block that made it (the
/// deliberate deviation recorded at the head of this file and in REARCH §14), so
/// a value wanted in five blocks is reloaded five times: once per BLOCK-RESIDENCY
/// instead of once per program REGION. Before writing a line of Braun 2013 SSA
/// reconstruction, this asks the corpus how many reloads such a reconstruction
/// could POSSIBLY remove — the number that IS the ceiling on the whole step.
///
/// A reload of `v` in block B is counted against the ceiling when some block A
/// STRICTLY DOMINATING B also reloads `v`: a register copy of `v` provably
/// exists on every path that reaches this use, so reconstruction has something
/// to rewire the use to. It is an UPPER bound and is meant to be — it asks
/// nothing about whether the register file can hold that copy from A to B, which
/// is exactly the question the implementation would have to answer. The
/// same-block repeat column is the residue reconstruction cannot touch (the copy
/// was evicted between the two uses, so the second reload is real work), and the
/// loop columns say how much of the ceiling is in code that runs more than once.
///
/// The dominance count is the LOOSE bound. The IMPLEMENTABLE one is the last two
/// columns: a reload of `v` in B whose value is still resident at the exit of
/// EVERY predecessor of B needs no reload at all once the entry set is derived
/// from the predecessors' exit sets (Braun-Hack's block-boundary reconciliation)
/// — that column is the honest prediction. Resident at SOME predecessor only is
/// a reload moved onto the cold edges rather than removed, so it is reported
/// apart and never added into the prediction.
///
/// THE REGIONAL-SPLIT COLUMN (spec §4.3, added before Task 5 was written).
/// The columns above all ask the §4.1 question — "could a copy have been CARRIED
/// here?". §4.3 asks a different one: "was this value memory-resident here only
/// because `Sim::More` retired its WHOLE WEB on the strength of one pressure
/// peak somewhere else?". A reload of `v` in block B counts against `split` when
///   * some predecessor of B was still holding `v` in a register at its exit —
///     a register copy existed immediately before B, so there is something for a
///     regional residency to continue; and
///   * B's HEAD had slack in `v`'s class (`Plan.headroom`) — B is not itself
///     over-pressured, so nothing at B forced the value out.
/// Such a reload is pure whole-web artefact: a regional model keeps the value in
/// the register it was already in. `split-loop` is the part of it inside a loop,
/// where the reload is paid every iteration. The `webs` pair counts the same
/// thing over VALUES rather than reloads — `web-split` values are memory-resident
/// yet hold a register somewhere in the function (a regional model splits them),
/// `web-none` values hold a register nowhere at all (genuinely over-pressured
/// for their whole live range, and the ones that must still go whole-web to
/// memory so the memory lattice keeps growing and the fixpoint stays bounded).
///
/// Columns: `name total dom-ceiling same-block-repeat in-loop all-preds
/// some-preds all-preds-in-loop remat split split-loop web-split web-none`.
fn ceiling_report(f: &MFunc, plan: &Plan, remat: &BTreeMap<VReg, MInst>) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("ZCC_SPILLCEIL").is_some()) {
        return;
    }
    let cfg = crate::mir::verify::cfg(f);
    let dt = crate::cfg::DomTree::new(&cfg, f.entry);
    let lf = crate::cfg::LoopForest::new(&cfg, &dt);
    let nb = f.blocks.len();
    let mut at: Vec<BTreeSet<VReg>> = vec![BTreeSet::new(); nb];
    for (bi, rs) in plan.reloads.iter().enumerate() {
        for &(_, v, _) in rs {
            at[bi].insert(v);
        }
    }
    let ex: Vec<BTreeSet<VReg>> = plan
        .wexit
        .iter()
        .map(|w| w.iter().map(|&(v, _)| v).collect())
        .collect();
    let (mut tot, mut ceil, mut rep, mut tot_l) = (0usize, 0usize, 0usize, 0usize);
    let (mut all_p, mut some_p, mut all_p_l) = (0usize, 0usize, 0usize);
    let (mut split, mut split_l) = (0usize, 0usize);
    // how many of the reloads are REMATERIALIZATIONS (a `movz`/`adrp`/frame
    // address recomputed) rather than frame loads — the spiller's cost model
    // calls these free, and the histogram says they are 14,393 `movz`
    let mut rm = 0usize;
    for (bi, rs) in plan.reloads.iter().enumerate() {
        let inloop = lf.depth[bi] > 0;
        let preds = &cfg.preds[bi];
        let mut seen: BTreeSet<VReg> = BTreeSet::new();
        for &(_, v, _) in rs {
            let dom = (0..nb).any(|a| {
                a != bi
                    && at[a].contains(&v)
                    && dt.dominates(a as crate::cfg::Node, bi as crate::cfg::Node)
            });
            let first = seen.insert(v);
            let np = preds.iter().filter(|&&p| ex[p as usize].contains(&v)).count();
            let all = first && !preds.is_empty() && np == preds.len();
            let some = first && np > 0 && np < preds.len();
            // spec §4.3: a register held it just before this block, and this
            // block's head was not full — so nothing HERE forced it to memory
            let cl = class_of(f, Reg::V(v));
            let slack = match cl {
                Class::Gpr => plan.headroom[bi][0] > 0,
                Class::Fpr => plan.headroom[bi][1] > 0,
                Class::Flags => false,
            };
            let sp = np > 0 && slack;
            split += sp as usize;
            split_l += (sp && inloop) as usize;
            tot += 1;
            rm += remat.contains_key(&v) as usize;
            ceil += dom as usize;
            rep += !first as usize;
            tot_l += inloop as usize;
            all_p += all as usize;
            some_p += some as usize;
            all_p_l += (all && inloop) as usize;
        }
    }
    // The same question asked over VALUES: of the values this plan made
    // memory-resident, how many hold a register SOMEWHERE (regional split has an
    // interval to keep) and how many hold one nowhere at all (they must stay
    // whole-web memory-resident — that is what keeps the memory lattice growing
    // and the fixpoint bounded).
    let mut held_somewhere: BTreeSet<VReg> = BTreeSet::new();
    for w in plan.wexit.iter() {
        for &(v, _) in w.iter() {
            held_somewhere.insert(v);
        }
    }
    let mut mem: BTreeSet<VReg> = BTreeSet::new();
    for rs in plan.reloads.iter() {
        for &(_, v, _) in rs {
            if !remat.contains_key(&v) {
                mem.insert(v);
            }
        }
    }
    let web_split = mem.iter().filter(|v| held_somewhere.contains(v)).count();
    let web_none = mem.len() - web_split;
    if tot > 0 {
        eprintln!(
            "SPILLCEIL {} {} {} {} {} {} {} {} {} {} {} {} {}",
            f.name, tot, ceil, rep, tot_l, all_p, some_p, all_p_l, rm, split, split_l,
            web_split, web_none
        );
    }
}

/// The two ways the post-condition can be unmet — distinguished because the
/// caller reacts to them differently. `OverK` is a genuine over-k pressure defect
/// with nowhere to spill to. `OverCross` is the ABI-asymmetry case: more values
/// are live across a call than there are callee-saved registers to hold them. The
/// R4.1 carry and the R4.3 regional split can re-admit a crossing value at a point
/// already at that ceiling — not an over-k bug, but exactly the demand
/// `spill_and_color` dissolves by lowering `cross_cap` (driving it to zero reloads
/// every crossing value after its call, so the crossing count falls to zero).
pub enum PressureErr {
    OverK(String),
    OverCross(String),
}
impl PressureErr {
    pub fn into_string(self) -> String {
        match self {
            Self::OverK(s) | Self::OverCross(s) => s,
        }
    }
}

/// The spiller's POST-CONDITION, checked rather than trusted (REARCH §7.6a):
/// at every program point the virtual values of a class that are live, plus the
/// allocatable physical registers spoken for there, are at most `isa::k(class)`;
/// and the call-crossing ones are at most the callee-saved count. The colourer's
/// theorem is "this cannot fail once pressure ≤ k", so a colouring failure means
/// the PRECONDITION was false — and without this check that shows up as an
/// unlocalized "no colour for v161" instead of naming the point and the count.
pub fn check_pressure(f: &MFunc) -> Result<(), PressureErr> {
    let cfg = crate::mir::verify::cfg(f);
    let lv = live::compute(f, &cfg);
    let sp = lv.sp;
    let mut lu = live::LastUse::new(sp);
    let masks = [isa::alloc_mask(Class::Gpr), isa::alloc_mask(Class::Fpr)];
    let cs = [
        (isa::callee_saved_mask(Class::Gpr) & masks[0]).count_ones() as usize,
        (isa::callee_saved_mask(Class::Fpr) & masks[1]).count_ones() as usize,
    ];
    for &b in &cfg.rpo {
        let bi = b as usize;
        live::last_use_into(f, sp, &lv, bi, &mut lu);
        let last = &lu.at;
        let mut live: BTreeSet<usize> = lv.live_in[bi].clone();
        for &p in &f.blocks[bi].params {
            live.insert(sp.idx(p));
        }
        let mut probe = |live: &BTreeSet<usize>, at: &str, extra: Option<RegSet>| -> Result<(), PressureErr> {
            for (ci, c) in [Class::Gpr, Class::Fpr].into_iter().enumerate() {
                let mut phys = 0u32;
                let (mut n, mut ncross) = (0usize, 0usize);
                for &x in live.iter() {
                    match sp.reg(x) {
                        Reg::P(p) if p.class == c => phys |= 1 << p.num,
                        Reg::V(v) if f.vregs[v as usize].class == c => {
                            n += 1;
                            if lv.crosses_call[v as usize] {
                                ncross += 1;
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(e) = extra {
                    phys |= if c == Class::Gpr { e.gpr } else { e.fpr };
                }
                let held = (phys & masks[ci]).count_ones() as usize;
                if n + held > isa::k(c) {
                    let mut who: Vec<String> = live
                        .iter()
                        .filter_map(|&x| match sp.reg(x) {
                            Reg::V(v) if f.vregs[v as usize].class == c => Some(format!("v{}", v)),
                            _ => None,
                        })
                        .collect();
                    who.sort();
                    return Err(PressureErr::OverK(format!(
                        "{}: {:?} pressure {} + {} held > k={} at {} [{}]",
                        f.name, c, n, held, isa::k(c), at, who.join(" ")
                    )));
                }
                if ncross > cs[ci] {
                    return Err(PressureErr::OverCross(format!(
                        "{}: {} call-crossing {:?} values live at {} but only {} callee-saved",
                        f.name, ncross, c, at, cs[ci]
                    )));
                }
            }
            Ok(())
        };
        probe(&live, &format!("bb{} head", bi), None)?;
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            let mut ops = Vec::new();
            inst.visit(&mut |r, c| ops.push((r, c)));
            // A PARALLEL COPY IS SIMULTANEOUS. Every source is read and every
            // destination written at one instant, so a destination may take the
            // register of a source that dies here — and when the pairs form a
            // cycle, sequentialization breaks it through the RESERVED scratch
            // (x16 = AAPCS64 IP0, v31), which is not in `alloc_order` and so
            // costs no allocatable register. Pressure at such a point is
            // therefore max(|live-in|, |live-out|), and counting live-in PLUS
            // every destination over-approximates it by the width of the copy.
            //
            // Measured (csmith c04804, 2026-08-27): the over-approximation read
            // 18 + 9 held > k=26 at a `pcopy` and aborted the compile of a
            // function the spiller had in fact converged on. For every other
            // instruction the def-and-live-through overlap is real, so the
            // relaxation is confined to this one form.
            if matches!(inst, MInst::ParallelCopy(..)) {
                let dying: Vec<usize> =
                    live.iter().copied().filter(|&x| last[x] == Some(i)).collect();
                for x in dying {
                    live.remove(&x);
                }
            }
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    live.insert(sp.idx(*r));
                }
            }
            let clob = match inst {
                MInst::Call { clobbers, .. } => Some(*clobbers),
                _ => None,
            };
            probe(&live, &format!("bb{}[{}] {:?}", bi, i, mnemonic(inst)), clob)?;
            let mut dead: Vec<usize> = live.iter().copied().filter(|&x| last[x] == Some(i)).collect();
            for (r, c) in &ops {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) && last[sp.idx(*r)].is_none() {
                    dead.push(sp.idx(*r));
                }
            }
            for x in dead {
                live.remove(&x);
            }
        }
    }
    Ok(())
}

fn mnemonic(i: &MInst) -> &'static str {
    match i {
        MInst::Alu { .. } => "alu",
        MInst::Cmp { .. } => "cmp",
        MInst::MovImm { .. } => "movimm",
        MInst::Ext { .. } => "ext",
        MInst::Load { .. } => "load",
        MInst::Store { .. } => "store",
        MInst::Adrp { .. } => "adrp",
        MInst::AddLo12 { .. } => "addlo12",
        MInst::CSel { .. } => "csel",
        MInst::Call { .. } => "call",
        MInst::Spill { .. } => "spill",
        MInst::Reload { .. } => "reload",
        MInst::ParallelCopy(..) => "pcopy",
        MInst::Copy { .. } => "copy",
        _ => "other",
    }
}

/// Values whose producer reads no register and has no fixed destination: the
/// producer can simply be re-executed wherever the value is wanted, so the value
/// needs no stack slot and no store. (`MovImm` is an immediate, `Adrp` a page
/// address, `SlotAddr` a frame offset — all three are constants of the frame.)
fn rematerializable(f: &MFunc) -> BTreeMap<VReg, MInst> {
    let mut out = BTreeMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            let ok = matches!(
                inst,
                MInst::MovImm { .. } | MInst::Adrp { .. } | MInst::SlotAddr { .. }
            );
            if !ok {
                continue;
            }
            let mut dst = None;
            let mut bad = false;
            inst.visit(&mut |r, c| match c {
                Constraint::Def => dst = r.vreg(),
                Constraint::DefFixed(_) | Constraint::Use | Constraint::UseFixed(_) => bad = true,
            });
            if let (Some(v), false) = (dst, bad) {
                out.insert(v, inst.clone());
            }
        }
    }
    out
}

// ── the plan a simulation produces ─────────────────────────────────────────

/// A reload copy, named by an index while the function is still borrowed
/// immutably; `apply` turns each one into a real virtual register.
type CopyId = u32;

#[derive(Default)]
struct Plan {
    /// per block: (before instruction i, value, copy) — `i == insts.len()` means
    /// "before the terminator"
    reloads: Vec<Vec<(usize, VReg, CopyId)>>,
    /// per block: at instruction i, read `value` from `copy` instead
    subs: Vec<Vec<(usize, VReg, CopyId)>>,
    ncopies: u32,
    /// per block: what is still resident in a register when the block ENDS,
    /// each under the name it is resident as. Not read by `apply`; it is what the
    /// successors' entry sets are built from (§ "carrying a reload"), what the
    /// R4.1 ceiling measurement reports, and — since R4-capstone — what the NEXT
    /// round hands a loop header on behalf of its latch.
    wexit: Vec<Vec<(VReg, Option<CopyId>)>>,
    /// per block: the block parameters this plan adds (spec §4.1)
    phis: Vec<Vec<Phi>>,
    /// PER BLOCK, PER CLASS (`[Gpr, Fpr]`): how many more registers of that
    /// class the block's HEAD could still have held once its working set was
    /// built. Read by `ceiling_report` alone — it is the measurement of spec
    /// §4.3's question ("was a register actually available where this value was
    /// memory-resident?") and nothing in the plan's application consults it.
    headroom: Vec<[usize; 2]>,
}

/// A block parameter the plan will add — Braun 2013's phi, spelled the way this
/// IR spells one (spec §4.1).
///
/// Downstream the phi behaves as a reload copy and nothing else: it takes a
/// `CopyId` from the same counter, the block's uses of `v` are substituted to it
/// by the ordinary `subs` machinery, and successors carry it by the ordinary
/// carry. What is different is only where its value comes from — an edge rather
/// than a `Reload` instruction.
struct Phi {
    v: VReg,
    id: CopyId,
    width: Width,
    /// `(predecessor, the name that predecessor reaches this block with)`, where
    /// `None` is the value's own name. One entry per incoming edge.
    ///
    /// Filled once the whole walk is over, not when the phi is decided. A loop
    /// header decides on the strength of what the latch held LAST round, and the
    /// latch — being after the header in reverse postorder — has no answer for
    /// THIS round until the walk reaches it. Resolving every edge at the same
    /// late moment keeps one rule instead of two.
    srcs: Vec<(MBlockId, Option<CopyId>)>,
}

/// The outcome of one simulated walk.
///
/// `More(vs)` used to be a life sentence and is not one since spec §4.3: it says
/// each value in `vs` needs a MEMORY HOME — a slot, and a store right after the
/// definition that dominates every later read — because the walk found a point
/// where no register could hold it. It does NOT say the value is banished from
/// the register file. On the next round the same value is register-resident from
/// its definition until pressure evicts it, crosses an edge under its own name
/// wherever every predecessor is still holding it, and is reloaded only in the
/// regions in between: eviction is a SPLIT, and memory residency is regional.
///
/// TERMINATION IS UNCHANGED BY THAT, and this is the claim the whole fixpoint
/// rests on. `spilled` is still what `More` grows, a value still never leaves it,
/// so there are still at most |vregs| plan-less rounds. What §4.3 changed is what
/// membership MEANS, not the lattice: a value that can hold a register nowhere —
/// 179 of sqlite's 4,549, by the `web-none` column of `ZCC_SPILLCEIL` — still
/// ends up memory-resident for its whole life, because every one of its regions
/// is a region pressure forced it out of.
enum Sim {
    Plan(Plan),
    More(Vec<VReg>),
}

/// One value's residency in a register right now.
#[derive(Clone, Copy)]
struct Res {
    v: VReg,
    /// `None` = still under its original name; `Some(id)` = a reload copy
    copy: Option<CopyId>,
    class: Class,
    /// this residency is live across some call, so it needs a callee-saved
    /// colour for its whole range
    cross: bool,
}

/// THE SECOND LATTICE (spec §4.4) — what a block may assume is already in a
/// register when it is ENTERED, on behalf of a predecessor this round has not
/// simulated yet.
///
/// That carrier is now the previous round's `Plan.wexit` ITSELF, passed straight
/// through: `simulate` reads `prev_exit[latch]` exactly where it would have read
/// `exits[latch]` had the latch been simulated. An earlier shape of this step
/// pre-digested `wexit` into a per-block INTERSECTION over predecessors, which
/// was the right thing while the carry had to be a dominance carry — an
/// intersection is what proves "one reaching definition" — and is exactly the
/// reason nothing ever fired: a preheader holds no copy under the latch's name,
/// so the intersection at a loop header was always empty. With the block
/// parameter of `reconstruct` in hand the join no longer needs one name, so the
/// per-predecessor answer is what is wanted and the intersection is what threw
/// it away.
///
/// Being derived from `wexit`, the set at a block is bounded by what a register
/// file can hold, and mentions only values that block was live in — the
/// bounded-height half of the termination argument in `spill_with`.
fn simulate(
    f: &MFunc,
    lv: &live::Liveness,
    cfg: &crate::cfg::Cfg,
    spilled: &[bool],
    cross_cap: usize,
    // what the PRIOR round left resident at each block's EXIT — read only for a
    // predecessor this round has not simulated yet (spec §4.4)
    prev_exit: &[Vec<(VReg, Option<CopyId>)>],
    // is the back edge carried at all? (`BACKEDGE_CARRY`, and off in the
    // graceful last phase of `spill_with`)
    carry: bool,
    // the loop nesting — the cost side of a cold-edge reload (`lf.depth`) and
    // the trace the next-use distance is measured along (`Trace`, below)
    lf: &crate::cfg::LoopForest,
    // one root per value's SSA web — eviction is ranked over the WEB, because
    // that is the granularity at which it is PAID (`Sim::More` retires a whole
    // web to memory, § below)
    web: &[VReg],
    // values whose producer reads no register: restoring one costs a single
    // instruction and no memory traffic, which is what the eviction ranking
    // below weighs against Belady's distance
    remat: &BTreeMap<VReg, MInst>,
) -> Result<Sim, String> {
    let depth = &lf.depth;
    let reconstruct = RECONSTRUCT.with(|c| c.get()) >= 1;
    let regional = REGIONAL.with(|c| c.get());
    // WHICH VALUES STILL HAVE A DEFINITION — the fence that keeps SSA
    // reconstruction on SSA values.
    //
    // `evict_params` removes a spilled block parameter and has each incoming
    // edge store the argument into the slot instead. What is left behind is a
    // name with USES and no DEFINITION: its real definition is now that set of
    // edge stores, and backward liveness, finding nothing that kills it, reports
    // it live everywhere including the function entry. A phi may not carry such
    // a pseudo-value. Its register copy holds what the slot held at the reload;
    // the edge store then changes what the slot holds; and a carry around a back
    // edge therefore hands the next iteration the PREVIOUS iteration's value.
    // (Measured, before this fence existed: it hands the loop its previous
    // induction variable as well, and the loop never terminates —
    // `a_loop_carried_variable_survives_being_spilled` caught it as an
    // interpreter step-limit trap, which is the battery doing exactly its job.)
    //
    // For a value that DOES have a definition the question does not arise: the
    // definition dominates every block the value is live in (that is what SSA
    // means), and `webs` refuses to share a slot between two members that are
    // ever live at once, so wherever the value is live its slot holds it.
    let has_def = {
        let mut d = vec![false; f.vregs.len()];
        for b in f.blocks.iter() {
            for p in b.params.iter() {
                if let Some(v) = p.vreg() {
                    d[v as usize] = true;
                }
            }
            for inst in b.insts.iter() {
                inst.visit(&mut |r, c| {
                    if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                        if let Some(v) = r.vreg() {
                            d[v as usize] = true;
                        }
                    }
                });
            }
        }
        d
    };
    let base = crate::compile::phase("      base", || linear_positions(f, cfg));
    let uses = crate::compile::phase("      uses", || use_positions(f, lv, cfg, &base));
    let trace = crate::compile::phase("      trace", || Trace::new(f, lf, &base, &uses, lv, web));
    // Once the function contains a call the register file is PARTITIONED: a value
    // live across a call may use only the callee-saved half (AAPCS64 §6.1.1, and
    // `color.rs` applies it per VALUE over its whole range), and a value that is
    // not must therefore stay out of that half — otherwise it starves the values
    // that have nowhere else to go, and a greedy colouring in dominance order
    // (the order chordality requires) cannot go back and undo it. So there are
    // two ceilings, not one.
    let has_calls = f
        .blocks
        .iter()
        .any(|b| b.insts.iter().any(|i| matches!(i, MInst::Call { .. })));
    let nb = f.blocks.len();
    let mut plan = Plan {
        reloads: vec![Vec::new(); nb],
        subs: vec![Vec::new(); nb],
        ncopies: 0,
        wexit: vec![Vec::new(); nb],
        phis: (0..nb).map(|_| Vec::new()).collect(),
        headroom: vec![[0usize; 2]; nb],
    };
    let mut newsp: Vec<VReg> = Vec::new();
    // what each block is still holding when it ends, and whether it has been
    // simulated yet — the two things a successor's entry set is built from.
    let mut exits: Vec<Vec<Res>> = vec![Vec::new(); nb];
    // the same set as `exits`, keyed and sorted for lookup. The entry test below
    // asks "does every predecessor hold this exact copy" once per resident value
    // per predecessor, on every round of the simulation. Measured, this is NOT
    // where the step's compile time went — replacing the linear scan with the
    // binary search moved the sqlite compile not at all — so it is kept for the
    // asymptotics and named here so nobody re-measures it hoping for the 11%.
    let mut exit_keys: Vec<Vec<(VReg, CopyId)>> = vec![Vec::new(); nb];
    // spec §4.3 — the same set restricted to residencies under the value's OWN
    // name, sorted for lookup. A memory-resident value holding a register under
    // its own name is what a REGIONAL split leaves behind: it is in memory in the
    // regions pressure forced it out of and in a register in the regions it did
    // not, and this is how the next block finds out which side of that line its
    // entry is on.
    let mut exit_orig: Vec<Vec<VReg>> = vec![Vec::new(); nb];
    let mut done: Vec<bool> = vec![false; nb];
    let mut lu = live::LastUse::new(lv.sp);
    let masks = [
        isa::alloc_mask(Class::Gpr),
        isa::alloc_mask(Class::Fpr),
        0u32,
    ];
    let ki = |c: Class| match c {
        Class::Gpr => 0usize,
        Class::Fpr => 1,
        Class::Flags => 2,
    };
    // AAPCS64 §6.1.1: a value live across a call may only take a callee-saved
    // colour, and `color.rs` applies that as a property of the VALUE — over its
    // whole range, not only at the call. So the ceiling is not only "how many
    // values are live here" but "how many of them are call-crossing", and it
    // binds at every point, including points with no call in sight: two values
    // crossing DIFFERENT calls can be live together in between and still need
    // two distinct callee-saved registers.
    let cs = [
        ((isa::callee_saved_mask(Class::Gpr) & masks[0]).count_ones() as usize).min(cross_cap),
        ((isa::callee_saved_mask(Class::Fpr) & masks[1]).count_ones() as usize).min(cross_cap),
        usize::MAX,
    ];
    // the other half of the partition
    let cr = [
        isa::caller_saved_mask(Class::Gpr).count_ones() as usize,
        isa::caller_saved_mask(Class::Fpr).count_ones() as usize,
        usize::MAX,
    ];

    for &b in &cfg.rpo {
        let bi = b as usize;
        let blk = &f.blocks[bi];
        live::last_use_into(f, lv.sp, lv, bi, &mut lu);
        let last = &lu.at;
        let head = base[bi];

        // physical registers spoken for, tracked exactly like the values
        let mut physlive: BTreeSet<usize> = lv.live_in[bi]
            .iter()
            .copied()
            .filter(|&x| x >= lv.sp.nv)
            .collect();
        let phys_mask = |set: &BTreeSet<usize>, c: Class| -> u32 {
            let mut m = 0u32;
            for &x in set {
                if let Some(p) = lv.sp.reg(x).preg() {
                    if p.class == c {
                        m |= 1 << p.num;
                    }
                }
            }
            m
        };

        // (1) the working set at the head: everything live in and still
        //     available in a register, nearest next use first, truncated to the
        //     budget. Two sources, and the second is the whole of R4.1.
        //
        //     CARRYING A RELOAD ACROSS AN EDGE. A memory-resident value used in
        //     five blocks used to be reloaded five times — once per block, since
        //     a reload copy was dropped at the block boundary (the deviation this
        //     file's header records). It need not be: a copy is carried into `bi`
        //     when EVERY predecessor is still holding it, under the SAME name.
        //
        //     That one condition is also the SSA proof, which is why no Braun
        //     2013 reconstruction, no block parameter and no renaming appears
        //     here. A copy is created at exactly one place, so "every predecessor
        //     holds it" means every path from the entry to `bi` runs through that
        //     place — which is the definition of dominance. The copy's definition
        //     therefore dominates every use we redirect to it, and SSA holds by
        //     construction exactly as it did when the copy could not leave its
        //     own block. `mir::verify` checks the property rather than trusting
        //     this paragraph.
        //
        //     A predecessor not yet simulated (a back edge, since blocks are
        //     walked in reverse postorder) holds nothing as far as THIS round's
        //     `exits` can tell, so nothing was ever carried into a loop header —
        //     the residency started afresh each iteration. R4-capstone lifts that
        //     with the second fixpoint (spec §4.4): the PRIOR round did simulate
        //     the latch, and `entry_resident` is what its exit set says this
        //     block may assume. It is read only when a predecessor is missing,
        //     and only under `BACKEDGE_CARRY`, because the two reaching
        //     definitions it joins need the block parameter of `reconstruct` to
        //     be sound — see the flag at the top of this file.
        //     EVICTION IS A REGIONAL SPLIT, NOT A WHOLE-WEB SPILL (spec §4.3).
        //     The carry above is written for a reload COPY, and that is not an
        //     accident of spelling: before §4.3 a memory-resident value could
        //     only be in a register as a copy, because `Sim::More` retired the
        //     value's whole web to memory and the head candidate list drops a
        //     memory-resident name on sight. §4.3 says memory residency is
        //     REGIONAL — a value evicted at one pressure peak is in memory
        //     between that peak and its next reload and in a register everywhere
        //     else — and the register it is in over those other regions is its
        //     OWN name, minted by its own definition ((2c) below already admits
        //     it; nothing there filters on `spilled`).
        //
        //     So the carry has a second form: a memory-resident value that every
        //     predecessor is still holding under its OWN name crosses the edge
        //     with no copy, no parameter and no reload. It needs no dominance
        //     argument at all, which is why it is stronger than the copy carry
        //     rather than a special case of it: the name is the value's single
        //     SSA definition, and a value LIVE-IN to this block is by definition
        //     one whose definition dominates it. What every predecessor's exit
        //     set establishes is only that a register really holds it here, which
        //     is the pressure question and not the correctness one.
        //
        //     TWO FENCES. It must be memory-resident (a value that is not is
        //     already in the head candidate list under its own name and needs
        //     nothing), and it must still HAVE a definition — `evict_params`
        //     turns a spilled block parameter into a name whose real definition
        //     is the store each edge makes into its slot, and a register copy of
        //     such a name goes stale the moment an edge writes it (see `has_def`
        //     above; the battery caught that as a non-terminating loop). The
        //     second fence is belt-and-braces here — an evicted parameter can
        //     never enter `w` under its own name, since the head list filters
        //     memory-resident names out and (2c) only admits real definitions —
        //     and it is written down because the day that stops being true is not
        //     the day to rediscover the defect.
        let mut w: Vec<Res> = Vec::new();
        let live_here = |v: VReg| lv.live_in[bi].contains(&lv.sp.idx(Reg::V(v)));
        let all_done = cfg.preds[bi].iter().all(|&p| done[p as usize]);
        let carried: Vec<Res> = if cfg.preds[bi].is_empty() || !all_done {
            Vec::new()
        } else {
            let (first, rest) = cfg.preds[bi].split_first().unwrap();
            exits[*first as usize]
                .iter()
                .filter(|r| live_here(r.v))
                .filter(|r| match r.copy {
                    Some(id) => {
                        let key = (r.v, id);
                        rest.iter()
                            .all(|&p| exit_keys[p as usize].binary_search(&key).is_ok())
                    }
                    None => {
                        regional
                            && spilled[r.v as usize]
                            && has_def[r.v as usize]
                            && rest.iter().all(|&p| {
                                exit_orig[p as usize].binary_search(&r.v).is_ok()
                            })
                    }
                })
                .copied()
                .collect()
        };

        // (1a) SSA RECONSTRUCTION AT A JOIN (spec §4.1) — the GENERAL case of
        //      the carry above, and the reason the carry above is only a special
        //      case of it.
        //
        //      The dominance carry needs every predecessor to hold the value
        //      under ONE name, because one name is one definition and one
        //      definition reaching every path IS dominance. That condition fails
        //      for the commonest join there is: five switch arms that each
        //      reloaded the same variable hold five different copies of it, so
        //      the value is in a register on every single path and the join
        //      reloads it anyway. It fails at a loop header too, where the
        //      preheader's reaching definition and the latch's are two different
        //      things by construction.
        //
        //      Braun 2013's answer is to stop looking for one name: give the join
        //      a BLOCK PARAMETER and let each predecessor say, on its own edge,
        //      which of its values that parameter stands for. A predecessor that
        //      holds nothing supplies a reload placed ON ITS EDGE — which is not
        //      a saving there, it is a MOVE of the reload out of the join and
        //      into that one path. `reconstruct::insert_phi` builds the object;
        //      everything here is the decision of where it is worth building.
        //
        //      THREE FENCES, and each is a cost the phi must clear:
        //        * Braun's minimal-SSA PRUNING — the parameter is built only
        //          where it removes a reload: the value is memory-resident and is
        //          read at or below this block, so without the parameter that
        //          read reloads. (A value not memory-resident is already carried
        //          under its own name and needs nothing.)
        //        * PRESSURE — the parameter is a register held at the head, so it
        //          enters the working set as an ordinary copy and is subject to
        //          the same budget. One that does not fit is simply not built;
        //          it can never displace an original name, since the head sorts
        //          originals first.
        //        * The COLD EDGE — a reload moved onto an edge is paid every time
        //          that edge is taken, so it is only accepted where the edge runs
        //          in strictly SHALLOWER loop nesting than the block it serves.
        //          A preheader paying once for a body that reloads every
        //          iteration is the whole of §4.2; two arms of a diamond trading
        //          one reload for one reload is not, and is refused.
        //
        //      An edge into the join must also be a plain `b`: after
        //      `split_critical_edges` that is what every predecessor of a real
        //      join has, and it is what makes "one edge per predecessor" (which
        //      `insert_phi` fills positionally) and "the edge copy cannot clobber
        //      what the terminator reads" both true by inspection.
        let mut phi_cand: Vec<(Res, Vec<(MBlockId, Option<CopyId>)>)> = Vec::new();
        if reconstruct
            && !cfg.preds[bi].is_empty()
            && cfg.preds[bi]
                .iter()
                .all(|&p| matches!(f.blocks[p as usize].term, MTerm::B(_)))
        {
            // Who holds what, built ONCE per block by walking each predecessor's
            // exit set — O(Σ|exit|) for the whole block. The obvious spelling,
            // "for each candidate value ask each predecessor", is the
            // O(preds² × k) scan this allocator has been bitten by before.
            let mut held: BTreeMap<VReg, Vec<(MBlockId, Option<CopyId>)>> = BTreeMap::new();
            for &p in &cfg.preds[bi] {
                let pi = p as usize;
                if done[pi] {
                    for r in exits[pi].iter() {
                        held.entry(r.v).or_default().push((p, r.copy));
                    }
                } else if carry {
                    for &(v, c) in prev_exit[pi].iter() {
                        held.entry(v).or_default().push((p, c));
                    }
                }
            }
            // one register more at a predecessor's exit than it already holds
            let room_at_exit = |p: MBlockId, c: Class| -> bool {
                let pi = p as usize;
                let n = if done[pi] {
                    exits[pi].iter().filter(|r| r.class == c).count()
                } else {
                    prev_exit[pi]
                        .iter()
                        .filter(|&&(v, _)| class_of(f, Reg::V(v)) == c)
                        .count()
                };
                let mut m = 0u32;
                for &x in lv.live_out[pi].iter() {
                    if let Some(pr) = lv.sp.reg(x).preg() {
                        if pr.class == c {
                            m |= 1 << pr.num;
                        }
                    }
                }
                n + (m & masks[ki(c)]).count_ones() as usize + 1 <= isa::k(c)
            };
            for (v, hot) in held.iter() {
                let v = *v;
                // not memory-resident ⟹ already carried under its own name;
                // no definition ⟹ an evicted parameter, see `has_def` above
                if !spilled[v as usize] || !live_here(v) || !has_def[v as usize] {
                    continue;
                }
                // the dominance carry already has it, for free and with no
                // parameter — Braun's "do not build a phi whose arguments are all
                // the same value", in the form this allocator meets it
                if carried.iter().any(|r| r.v == v) {
                    continue;
                }
                let cl = class_of(f, Reg::V(v));
                if cl == Class::Flags {
                    continue;
                }
                // pruning: it must remove a read — asked of the TRACE, for the
                // two reasons S1 records. A read AT the head is the read this
                // phi removes, and the static query looks strictly after it; and
                // a value read only across the back edge answers `usize::MAX` in
                // reverse postorder, which is how the loop-invariant pointer of
                // `nestjoin.c` was refused a register and reloaded four million
                // times while its phi was available and free.
                if trace.next_use(v, bi, head) == usize::MAX {
                    continue;
                }
                // A predecessor the round has not simulated can only be believed
                // through `prev_exit`; if it did not hold the value there, this
                // round has no answer for its edge and the phi is not decidable.
                if !cfg.preds[bi]
                    .iter()
                    .all(|&p| done[p as usize] || hot.iter().any(|&(q, _)| q == p))
                {
                    continue;
                }
                let cold: Vec<MBlockId> = cfg.preds[bi]
                    .iter()
                    .copied()
                    .filter(|p| !hot.iter().any(|&(q, _)| q == *p))
                    .collect();
                if !cold.is_empty()
                    && !cold
                        .iter()
                        .all(|&p| depth[p as usize] < depth[bi] && room_at_exit(p, cl))
                {
                    continue;
                }
                let id = plan.ncopies;
                plan.ncopies += 1;
                phi_cand.push((
                    Res {
                        v,
                        copy: Some(id),
                        class: cl,
                        cross: lv.crosses_call[v as usize],
                    },
                    hot.clone(),
                ));
            }
        }

        for c in [Class::Gpr, Class::Fpr] {
            let mut names: Vec<VReg> = lv.live_in[bi]
                .iter()
                .copied()
                .filter(|&x| x < lv.sp.nv)
                .map(|x| x as VReg)
                .chain(blk.params.iter().filter_map(|p| p.vreg()))
                .filter(|v| !spilled[*v as usize] && class_of(f, Reg::V(*v)) == c)
                .collect();
            names.sort_unstable();
            names.dedup();
            let mut cand: Vec<Res> = names
                .into_iter()
                .map(|v| Res { v, copy: None, class: c, cross: lv.crosses_call[v as usize] })
                .collect();
            for r in carried
                .iter()
                .chain(phi_cand.iter().map(|(r, _)| r))
                .filter(|r| r.class == c)
            {
                if !cand.iter().any(|q| q.v == r.v) {
                    cand.push(*r);
                }
            }
            // An ORIGINAL name claims the budget before a carried copy does, even
            // when the copy is wanted sooner. The two are not symmetric: a copy
            // that does not fit is simply dropped and the value reloaded, while
            // an original that does not fit becomes memory-resident for the WHOLE
            // function and restarts the simulation — so a copy must not be able
            // to displace one. On sqlite the tie-break changes nothing (the
            // emitted bytes are identical with and without it, and the budget
            // turns out not to bind at block heads); it is here because the
            // asymmetry is real and the day it binds is not the day to discover
            // that a reload here was traded for a spill everywhere.
            // …and since §4.3 the test for "an original that does not fit
            // becomes memory-resident" is not `copy.is_none()` but "its own name
            // AND not already memory-resident": a value the regional carry
            // brought in under its own name is ALREADY in memory, so losing it
            // here costs one reload and not a whole-web spill. It must therefore
            // sort with the copies, or it could displace a name whose loss is the
            // expensive kind.
            let droppable = |r: &Res| r.copy.is_some() || spilled[r.v as usize];
            cand.sort_by_key(|r| (droppable(r), trace.rank(r.v, bi, head)));
            let hm = phys_mask(&physlive, c) & masks[ki(c)];
            let budget = isa::k(c).saturating_sub(hm.count_ones() as usize);
            let mut bcross = cs[ki(c)]
                .saturating_sub((hm & isa::callee_saved_mask(c)).count_ones() as usize);
            let mut bplain = usize::MAX;
            let mut taken = 0usize;
            for r in cand.into_iter() {
                let room = if r.cross { &mut bcross } else { &mut bplain };
                if taken < budget && *room > 0 {
                    *room -= 1;
                    taken += 1;
                    w.push(r);
                } else if !droppable(&r) {
                    // an ORIGINAL name that does not fit has to become
                    // memory-resident; a copy — or a name the regional carry
                    // brought in, which the slot already holds — is a duplicate,
                    // so dropping it costs nothing.
                    newsp.push(r.v);
                }
            }
        }

        // How many calls precede each instruction: a reload copy that will be
        // live across one is restricted to a callee-saved colour for its WHOLE
        // range (`color.rs` applies the rule per VALUE), so it has to be counted
        // as call-crossing from the moment it is created — not from the call.
        let calls_before: Vec<usize> = {
            let mut v = Vec::with_capacity(blk.insts.len() + 1);
            let mut n = 0;
            v.push(0);
            for inst in &blk.insts {
                if matches!(inst, MInst::Call { .. }) {
                    n += 1;
                }
                v.push(n);
            }
            v
        };

        // the head is a program point like any other: the ceilings bind there
        for c in [Class::Gpr, Class::Fpr] {
            loop {
                let ncross = w.iter().filter(|r| r.class == c && r.cross).count();
                if ncross <= cs[ki(c)] {
                    break;
                }
                let pick = w
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.class == c && r.cross)
                    .max_by_key(|(_, r)| trace.rank(r.v, bi, head))
                    .map(|(j, r)| (j, r.v));
                match pick {
                    Some((j, v)) => {
                        w.remove(j);
                        // already memory-resident ⟹ dropping the residency costs
                        // a reload, not a whole-web spill (spec §4.3). Pushing it
                        // anyway would report progress the fixpoint cannot make
                        // and fail the round as "spilling made no progress".
                        if !spilled[v as usize] {
                            newsp.push(v);
                        }
                    }
                    None => break,
                }
            }
        }

        // Commit the phis that SURVIVED the head — both the budget and the
        // call-crossing ceiling above. A candidate that lost its place in the
        // working set was never built: its `CopyId` is simply never mentioned
        // again and the block reloads the value exactly as it did before, which
        // is why the cold-edge reloads are minted HERE and not at the candidate
        // stage.
        for (r, _hot) in phi_cand {
            let id = r.copy.unwrap();
            if !w.iter().any(|q| q.copy == Some(id)) {
                continue;
            }
            plan.phis[bi].push(Phi {
                v: r.v,
                id,
                width: f.vregs[r.v as usize].width,
                srcs: Vec::new(),
            });
        }

        // R4-capstone MEASUREMENT for spec §4.3 (`ZCC_SPILLCEIL=1`) — read-only.
        // How many registers of each class the head could STILL have held. A
        // reload of a memory-resident value in a block whose head had slack is a
        // reload no pressure at this block ever asked for: it is there because
        // `Sim::More` retired the value's WHOLE web to memory on the strength of
        // one pressure peak somewhere else. That difference is what a REGIONAL
        // split recovers, and this is the number that says how much of it there
        // is before a line of it is written (Law 3 — predict on the model first).
        for c in [Class::Gpr, Class::Fpr] {
            let hm = phys_mask(&physlive, c) & masks[ki(c)];
            let used = w.iter().filter(|r| r.class == c).count() + hm.count_ones() as usize;
            plan.headroom[bi][ki(c)] = isa::k(c).saturating_sub(used);
        }

        // (2) walk the block
        let n = blk.insts.len();
        for i in 0..=n {
            let mut ops: Vec<(Reg, Constraint)> = Vec::new();
            let clob = if i < n {
                blk.insts[i].visit(&mut |r, c| ops.push((r, c)));
                match &blk.insts[i] {
                    MInst::Call { clobbers, .. } => Some(*clobbers),
                    _ => None,
                }
            } else {
                match &blk.term {
                    MTerm::Bcc(_, r, ..) => ops.push((*r, Constraint::Use)),
                    MTerm::Cbz { reg, .. } | MTerm::Tb { reg, .. } => {
                        ops.push((*reg, Constraint::Use))
                    }
                    MTerm::Switch { idx, .. } => ops.push((*idx, Constraint::Use)),
                    MTerm::BrReg(r, _) => ops.push((*r, Constraint::Use)),
                    _ => {}
                }
                // A JOIN can be wider than the register file: mem2reg gives a
                // block one parameter per live local, and every argument of the
                // edge is a use at this single point. No eviction relieves that —
                // the argument must be in a register HERE — so the ceiling is met
                // by removing the PARAMETER instead: the value then travels
                // through its slot, one store per argument, and the edge stops
                // demanding a register at all.
                let mut kept: Vec<(usize, usize)> = Vec::new();
                for (ti, t) in blk.term.targets().iter().enumerate() {
                    for k in 0..t.args.len() {
                        kept.push((ti, k));
                    }
                }
                for c in [Class::Gpr, Class::Fpr] {
                    loop {
                        // Everything that must hold a register at this point:
                        // the values still resident, plus the terminator's own
                        // operand and every edge argument. The arguments are the
                        // only ones no eviction can relieve, so they are what the
                        // loop below gives up.
                        let mut need: Vec<VReg> = ops
                            .iter()
                            .filter_map(|(r, _)| r.vreg())
                            .chain(kept.iter().filter_map(|&(ti, k)| {
                                blk.term.targets()[ti].args[k].vreg()
                            }))
                            .chain(w.iter().map(|r| r.v))
                            .filter(|v| class_of(f, Reg::V(*v)) == c)
                            .collect();
                        need.sort_unstable();
                        need.dedup();
                        // no call clobbers at a terminator, so only live physical
                        // registers are spoken for here
                        let ph = phys_mask(&physlive, c) & masks[ki(c)];
                        let cap = isa::k(c).saturating_sub(ph.count_ones() as usize);
                        let ncross = need.iter().filter(|&&v| lv.crosses_call[v as usize]).count();
                        let capx = cs[ki(c)]
                            .saturating_sub((ph & isa::callee_saved_mask(c)).count_ones() as usize);
                        let capp = usize::MAX;
                        if need.len() <= cap && ncross <= capx && need.len() - ncross <= capp {
                            break;
                        }
                        let pick = kept
                            .iter()
                            .enumerate()
                            .filter(|(_, tk)| {
                                blk.term.targets()[tk.0].args[tk.1]
                                    .vreg()
                                    .is_some_and(|v| class_of(f, Reg::V(v)) == c)
                            })
                            .filter_map(|(j, tk)| {
                                let tb = blk.term.targets()[tk.0].block as usize;
                                f.blocks[tb]
                                    .params
                                    .get(tk.1)
                                    .and_then(|p| p.vreg())
                                    .map(|p| (j, p))
                            })
                            .max_by_key(|&(_, p)| trace.rank(p, bi, head + i));
                        match pick {
                            Some((j, p)) => {
                                kept.remove(j);
                                newsp.push(p);
                            }
                            None => break,
                        }
                    }
                }
                for &(ti, k) in &kept {
                    ops.push((blk.term.targets()[ti].args[k], Constraint::Use));
                }
                None
            };
            let at = head + i;
            // registers this instruction pins: never a victim
            let pinned: Vec<VReg> = ops.iter().filter_map(|(r, _)| r.vreg()).collect();
            let held_mask = |set: &BTreeSet<usize>, c: Class| -> u32 {
                let mut m = phys_mask(set, c);
                if let Some(cl) = clob {
                    m |= match c {
                        Class::Gpr => cl.gpr,
                        Class::Fpr => cl.fpr,
                        Class::Flags => 0,
                    };
                }
                m & masks[ki(c)]
            };
            let held = |set: &BTreeSet<usize>, c: Class| -> usize {
                held_mask(set, c).count_ones() as usize
            };
            // `need` = registers this instruction is about to occupy;
            // `need_cross` = how many of them will carry the callee-saved
            // restriction. A RELOAD copy never does (its range ends at the
            // instruction that reads it), which is why the two counts are
            // separate — conflating them makes the ceiling reject a split that
            // is precisely what relieves it.
            let mut room = |w: &mut Vec<Res>,
                            newsp: &mut Vec<VReg>,
                            c: Class,
                            need: usize,
                            need_cross: usize,
                            physlive: &BTreeSet<usize>|
             -> Result<(), String> {
                loop {
                    let cnt = w.iter().filter(|r| r.class == c).count();
                    let ncross = w.iter().filter(|r| r.class == c && r.cross).count();
                    let hm = held_mask(physlive, c);
                    let over_k = cnt + hm.count_ones() as usize + need > isa::k(c);
                    // The physical registers actually LIVE here, without a call's
                    // clobber set: a clobber constrains what may SURVIVE the call,
                    // which is the crossing ceiling's business — counting it
                    // against the caller-saved half would declare every call
                    // over-pressured, since its clobbers ARE that half.
                    let ph = phys_mask(physlive, c) & masks[ki(c)];
                    let over_cross = ncross
                        + need_cross
                        + (ph & isa::callee_saved_mask(c)).count_ones() as usize
                        > cs[ki(c)];
                    let over_plain = false;
                    if !over_k && !over_cross && !over_plain {
                        return Ok(());
                    }
                    if c == Class::Flags {
                        // Flags are never stored: their producer is pure, so the
                        // answer is to rematerialize the compare. Reaching here
                        // means isel let two flag values overlap — a Law-2
                        // Side-I defect, not a spill problem.
                        return Err(format!(
                            "{}: two NZCV values live at once; the compare must be rematerialized",
                            f.name
                        ));
                    }
                    let pick = w
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.class == c && !pinned.contains(&r.v))
                        .filter(|(_, r)| !over_cross || r.cross)
                        .filter(|(_, r)| !(over_plain && !over_cross && !over_k) || !r.cross)
                        // DISTANCE IS NOT COST. Belady's rule — evict the value
                        // whose next read is furthest away — is exactly right when
                        // every victim costs the same to bring back. A
                        // rematerializable value does not: its producer reads no
                        // register, so restoring it is ONE instruction, no slot and
                        // no memory traffic, while every other victim pays a store
                        // and a load. Ranking the two by distance alone spends a
                        // register on the cheap value and evicts the dear one.
                        //
                        // csmith c6837 is the case that names it. `v1106` is a
                        // `SlotAddr` (`add xN, sp, #imm`) read twenty-odd times
                        // across a 600-instruction block, so its next read is always
                        // near and it never loses a distance contest — while holding
                        // one of the ten callee-saved registers, because it is live
                        // across a call. The colourer then cannot colour it, the
                        // caller forces values to memory and retries, and 168 rounds
                        // later the compile fails outright. The value that could
                        // have been rebuilt for one `add` is the one that was kept.
                        //
                        // So the key is (rematerializable, distance): a free victim
                        // outranks every paying one, and among equals Belady still
                        // decides. This is Law 3c's dual for the allocator — a
                        // decision is judged by what it COSTS, not by a count.
                        .max_by_key(|(_, r)| (remat.contains_key(&r.v), trace.rank(r.v, bi, at)))
                        .map(|(j, r)| (j, *r));
                    match pick {
                        Some((j, r)) => {
                            w.remove(j);
                            // A reload copy is a clean duplicate of what the slot
                            // already holds, so dropping it costs nothing. An
                            // original value has to become memory-resident.
                            if r.copy.is_none() && !spilled[r.v as usize] {
                                newsp.push(r.v);
                            }
                        }
                        // Every value live here is pinned by the very instruction
                        // that overflows: it reads more registers than the class
                        // has. No A64 instruction reads more than four, so this
                        // is a Law-2 defect in isel.
                        None => {
                            // THE DIAGNOSTIC IS AN INSTRUMENT, AND AN INSTRUMENT
                            // THAT LIES COSTS MORE THAN THE DEFECT IT REPORTS.
                            // The previous form printed six values under six
                            // labels rotated by one — the mnemonic appeared as
                            // "resident", the resident count as "held", and
                            // `pinned.len()` as "k". Two sessions read `k 4` on a
                            // machine whose k is 26 and diagnosed a register
                            // shortage that was not there. Each value is now
                            // named where it is computed, and the headline says
                            // WHICH ceiling was hit, because "exceeds k" was
                            // printed for a crossing overflow.
                            let ceiling = if over_k { "k" } else { "callee-saved crossing" };
                            return Err(format!(
                                "{}: {:?} pressure exceeds the {} ceiling at bb{}[{}] {} with \
                                 nothing evictable (resident {}, cross {}, held {}, need {}, \
                                 need_cross {}, pinned {}, k {}, cs {})",
                                f.name,
                                c,
                                ceiling,
                                bi,
                                i,
                                if i < n { mnemonic(&blk.insts[i]) } else { "term" },
                                cnt,
                                ncross,
                                held(physlive, c),
                                need,
                                need_cross,
                                pinned.len(),
                                isa::k(c),
                                cs[ki(c)]
                            ));
                        }
                    }
                }
            };

            // (2a0) The call-crossing ceiling, relaxed for values this
            // instruction READS. `color.rs` applies the callee-saved restriction
            // to a VALUE over its whole range, so ten is the hard limit on
            // simultaneously-live call-crossing GPR values — and a call with
            // many arguments reads more than ten of them at once, every one
            // pinned by the very instruction that needs them. Evicting a pinned
            // READ is nonetheless legal and is exactly the live-range split the
            // situation calls for: the use below reloads it into a fresh
            // register whose range ends at this instruction, so that register
            // crosses no call and needs no callee-saved home. Only a value this
            // instruction DEFINES is un-evictable, since no reload can supply it.
            let pinned_defs: Vec<VReg> = ops
                .iter()
                .filter(|(_, k)| matches!(k, Constraint::Def | Constraint::DefFixed(_)))
                .filter_map(|(r, _)| r.vreg())
                .collect();
            for c in [Class::Gpr, Class::Fpr] {
                loop {
                    let ncross = w.iter().filter(|r| r.class == c && r.cross).count();
                    if ncross <= cs[ki(c)] {
                        break;
                    }
                    let pick = w
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.class == c && r.cross)
                        .filter(|(_, r)| !pinned_defs.contains(&r.v))
                        .max_by_key(|(_, r)| trace.rank(r.v, bi, at))
                        .map(|(j, r)| (j, *r));
                    match pick {
                        Some((j, r)) => {
                            w.remove(j);
                            if r.copy.is_none() && !spilled[r.v as usize] {
                                newsp.push(r.v);
                            }
                        }
                        None => break,
                    }
                }
            }

            // (2a) every virtual use must be resident
            for (r, c) in ops.iter() {
                if !matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    continue;
                }
                let v = match r.vreg() {
                    Some(v) => v,
                    None => continue,
                };
                match w.iter().find(|x| x.v == v).map(|x| x.copy) {
                    Some(Some(id)) => plan.subs[bi].push((i, v, id)),
                    Some(None) => {}
                    None => {
                        let cl = class_of(f, Reg::V(v));
                        room(&mut w, &mut newsp, cl, 1, 0, &physlive)?;
                        let id = plan.ncopies;
                        plan.ncopies += 1;
                        plan.reloads[bi].push((i, v, id));
                        plan.subs[bi].push((i, v, id));
                        let end = match last[lv.sp.idx(Reg::V(v))] {
                            Some(j) if j < blk.insts.len() => j,
                            _ => blk.insts.len(),
                        };
                        let cross = calls_before[end] > calls_before[i.min(end)];
                        w.push(Res { v, copy: Some(id), class: cl, cross });
                        if !spilled[v as usize] {
                            newsp.push(v);
                        }
                    }
                }
            }
            if i == n {
                break;
            }
            // Everything resident BEFORE the call is live across it (whatever
            // dies at the call leaves below), so its residency inherits the
            // callee-saved restriction — including a reload copy, whose flag
            // `Liveness` cannot know until the copy exists.
            let pre_call: Vec<VReg> = if clob.is_some() {
                w.iter().map(|r| r.v).collect()
            } else {
                Vec::new()
            };
            // (2b) room for what this instruction defines, counting a call's
            //      clobber set as registers already taken
            for c in [Class::Gpr, Class::Fpr, Class::Flags] {
                let need = ops
                    .iter()
                    .filter(|(r, k)| {
                        matches!(k, Constraint::Def | Constraint::DefFixed(_))
                            && r.vreg().is_some()
                            && class_of(f, *r) == c
                    })
                    .count();
                let need_cross = ops
                    .iter()
                    .filter(|(r, k)| {
                        matches!(k, Constraint::Def | Constraint::DefFixed(_))
                            && r.vreg().is_some_and(|v| lv.crosses_call[v as usize])
                            && class_of(f, *r) == c
                    })
                    .count();
                room(&mut w, &mut newsp, c, need, need_cross, &physlive)?;
            }
            // (2c) the definitions themselves
            for (r, c) in ops.iter() {
                if !matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    continue;
                }
                match r {
                    Reg::V(v) => {
                        if !w.iter().any(|x| x.v == *v) {
                            w.push(Res {
                                v: *v,
                                copy: None,
                                class: class_of(f, *r),
                                cross: lv.crosses_call[*v as usize],
                            });
                        }
                    }
                    Reg::P(_) => {
                        physlive.insert(lv.sp.idx(*r));
                    }
                }
            }
            // (2d) whatever dies here leaves — INCLUDING a definition this block
            //      never reads and that does not escape (`last` is `None` for it)
            w.retain(|r| {
                let x = lv.sp.idx(Reg::V(r.v));
                match last[x] {
                    Some(j) => j != i,
                    None => !ops.iter().any(|(q, k)| {
                        matches!(k, Constraint::Def | Constraint::DefFixed(_)) && q.vreg() == Some(r.v)
                    }),
                }
            });
            let dead: Vec<usize> = physlive
                .iter()
                .copied()
                .filter(|&x| last[x] == Some(i))
                .collect();
            for x in dead {
                physlive.remove(&x);
            }
            if clob.is_some() {
                for r in w.iter_mut() {
                    if pre_call.contains(&r.v) {
                        r.cross = true;
                    }
                }
                for c in [Class::Gpr, Class::Fpr] {
                    room(&mut w, &mut newsp, c, 0, 0, &physlive)?;
                }
            }
        }
        plan.wexit[bi] = w.iter().map(|r| (r.v, r.copy)).collect();
        exit_keys[bi] = {
            let mut k: Vec<(VReg, CopyId)> =
                w.iter().filter_map(|r| r.copy.map(|c| (r.v, c))).collect();
            k.sort_unstable();
            k
        };
        exit_orig[bi] = {
            let mut k: Vec<VReg> = w
                .iter()
                .filter(|r| r.copy.is_none() && spilled[r.v as usize])
                .map(|r| r.v)
                .collect();
            k.sort_unstable();
            k
        };
        exits[bi] = w;
        done[bi] = true;
    }
    if newsp.is_empty() {
        // NOW THAT THE WALK IS OVER — the two things about a phi that only the
        // finished walk knows.
        //
        // (a) WHICH PHIS ARE READ AT ALL. A parameter is decided at a block head,
        //     where "the value is read at or below here" is the best that can be
        //     said; the walk can then evict it before that read is reached, and
        //     what is left is a block parameter nothing uses. That is not merely
        //     wasteful — an unread parameter is still a register held at the head
        //     of its block, so `check_pressure` counts it live across every call
        //     in that block and the colouring it was meant to help is the one it
        //     breaks. Braun's minimal-SSA pruning, applied where it is decidable.
        //     A phi read only by ANOTHER phi is read, which is why this is a
        //     fixpoint and not a single sweep: around a loop the two point at each
        //     other.
        //
        // (b) WHAT EACH EDGE SUPPLIES. Every block's exit set exists now,
        //     including the latch a header had to guess at from the previous
        //     round; a predecessor that turns out not to hold the value takes a
        //     reload on its edge like any other cold one. The two rounds
        //     disagreeing is precisely what the convergence test in `spill_with`
        //     refuses to accept a plan on.
        // (c) TRIVIAL PHIS — Braun 2013 §2.3, `removeTrivialPhi`, which this
        //     allocator could not run until §4.2 gave a header a parameter whose
        //     arguments only the finished walk knows. A phi whose every incoming
        //     edge reaches it with the SAME reaching definition (its own
        //     parameter not counted, which is how a loop header's self-reference
        //     is discounted) is not a phi at all: it IS that definition. Keeping
        //     it costs one parallel edge copy per predecessor — `destruct` emits
        //     one on every edge and only biased colouring removes any of them —
        //     and buys nothing, so the parameter is replaced by what it stands
        //     for everywhere it is mentioned.
        //
        //     The self-reference clause is what makes this a fixpoint rather than
        //     a sweep: aliasing one phi can make a second one trivial, and around
        //     a loop the two point at each other. It runs BEFORE the cold-edge
        //     reloads are minted, so a phi removed here never mints one.
        let prune = PRUNE.with(|c| c.get());
        let mut alias: BTreeMap<CopyId, Option<CopyId>> = BTreeMap::new();
        let chase = |alias: &BTreeMap<CopyId, Option<CopyId>>, mut c: Option<CopyId>| {
            let mut n = 0;
            while let Some(id) = c {
                match alias.get(&id) {
                    Some(&t) => c = t,
                    None => break,
                }
                n += 1;
                debug_assert!(n <= alias.len() + 1, "alias chain cycles");
            }
            c
        };
        while prune {
            let mut changed = false;
            for (bi, ps) in plan.phis.iter().enumerate() {
                for ph in ps.iter() {
                    if alias.contains_key(&ph.id) {
                        continue;
                    }
                    let mut uniq: Option<Option<CopyId>> = None;
                    let mut trivial = true;
                    for &p in cfg.preds[bi].iter() {
                        let held = plan.wexit[p as usize]
                            .iter()
                            .find(|&&(x, _)| x == ph.v)
                            .map(|&(_, c)| chase(&alias, c));
                        // a predecessor holding nothing takes a reload on its
                        // edge — a name of its own, so the phi is reconciling two
                        // definitions and is not trivial
                        let src = match held {
                            Some(c) => c,
                            None => {
                                trivial = false;
                                break;
                            }
                        };
                        if src == Some(ph.id) {
                            continue;
                        }
                        match uniq {
                            None => uniq = Some(src),
                            Some(u) if u == src => {}
                            _ => {
                                trivial = false;
                                break;
                            }
                        }
                    }
                    if trivial {
                        if let Some(u) = uniq {
                            alias.insert(ph.id, u);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        if !alias.is_empty() {
            for ps in plan.phis.iter_mut() {
                ps.retain(|ph| !alias.contains_key(&ph.id));
            }
            for w in plan.wexit.iter_mut() {
                for e in w.iter_mut() {
                    e.1 = chase(&alias, e.1);
                }
            }
            // A use substituted to a removed parameter reads what the parameter
            // stood for; where that is the value's own name there is nothing to
            // substitute at all, so the entry goes.
            for ss in plan.subs.iter_mut() {
                for e in ss.iter_mut() {
                    if let Some(t) = alias.get(&e.2) {
                        match chase(&alias, Some(e.2)) {
                            Some(c) => e.2 = c,
                            None => {
                                let _ = t;
                                e.2 = u32::MAX;
                            }
                        }
                    }
                }
                ss.retain(|e| e.2 != u32::MAX);
            }
        }
        let mut used: BTreeSet<CopyId> = plan
            .subs
            .iter()
            .flat_map(|ss| ss.iter().map(|&(_, _, id)| id))
            .collect();
        let owner: BTreeMap<CopyId, (usize, usize)> = plan
            .phis
            .iter()
            .enumerate()
            .flat_map(|(bi, ps)| ps.iter().enumerate().map(move |(k, ph)| (ph.id, (bi, k))))
            .collect();
        // A WORKLIST, not a re-sweep. The reachability is a graph search over the
        // phi graph, and re-scanning every phi until nothing grows makes it
        // quadratic in the phi count for no reason — the O(stores × reloads)
        // mistake in a smaller shape. Each phi is expanded at most once, so the
        // whole prune is O(phis × preds).
        let mut work: Vec<CopyId> = plan
            .phis
            .iter()
            .flatten()
            .filter(|ph| used.contains(&ph.id))
            .map(|ph| ph.id)
            .collect();
        while let Some(id) = work.pop() {
            let (bi, k) = match owner.get(&id) {
                Some(&x) => x,
                None => continue,
            };
            let v = plan.phis[bi][k].v;
            for &p in cfg.preds[bi].iter() {
                if let Some(&(_, Some(c))) =
                    plan.wexit[p as usize].iter().find(|&&(x, _)| x == v)
                {
                    if owner.contains_key(&c) && used.insert(c) {
                        work.push(c);
                    }
                }
            }
        }
        for ps in plan.phis.iter_mut() {
            ps.retain(|ph| used.contains(&ph.id));
        }
        // A dropped phi's name must not survive in any exit set: `apply` mints no
        // register for it and the next round would offer it as an edge argument.
        for w in plan.wexit.iter_mut() {
            w.retain(|&(_, c)| c.is_none_or(|c| !owner.contains_key(&c) || used.contains(&c)));
        }
        for bi in 0..nb {
            for k in 0..plan.phis[bi].len() {
                let v = plan.phis[bi][k].v;
                for pi in 0..cfg.preds[bi].len() {
                    let p = cfg.preds[bi][pi];
                    let held = plan.wexit[p as usize]
                        .iter()
                        .find(|&&(x, _)| x == v)
                        .map(|&(_, c)| c);
                    let src = match held {
                        Some(c) => c,
                        None => {
                            let rid = plan.ncopies;
                            plan.ncopies += 1;
                            plan.reloads[p as usize]
                                .push((f.blocks[p as usize].insts.len(), v, rid));
                            Some(rid)
                        }
                    };
                    plan.phis[bi][k].srcs.push((p, src));
                }
            }
        }
        Ok(Sim::Plan(plan))
    } else {
        newsp.sort_unstable();
        newsp.dedup();
        Ok(Sim::More(newsp))
    }
}

// ── applying a plan ────────────────────────────────────────────────────────

fn apply(
    f: &mut MFunc,
    plan: Plan,
    spilled: &[bool],
    remat: &BTreeMap<VReg, MInst>,
    web: &[VReg],
    mut web_slot: BTreeMap<VReg, (SlotId, Width)>,
    mut slot_of: BTreeMap<VReg, (SlotId, Width)>,
) {
    // CP2.3: iterate the set in ascending vreg order — identical to the old
    // `BTreeSet<VReg>` iteration, so slots are minted in the same order and the
    // output is byte-identical.
    for v in 0..spilled.len() as VReg {
        if !spilled[v as usize] || remat.contains_key(&v) {
            continue;
        }
        ensure_slot(f, web, &mut web_slot, &mut slot_of, v);
    }
    evict_params(f, &slot_of);

    // Every copy's register is minted BEFORE any block is rewritten. A copy is
    // no longer confined to the block that reloads it (§ "carrying a reload"), so
    // a use of it can sit in a block with a lower index than its definition —
    // assigning the register as the reload is emitted would then have the use
    // read a placeholder. Block INDEX order is not dominance order and never was.
    let mut copy_reg: Vec<Reg> = vec![Reg::P(isa::ZR); plan.ncopies as usize];
    for rs in plan.reloads.iter() {
        for &(_, v, id) in rs {
            let w = f.vregs[v as usize].width;
            copy_reg[id as usize] = f.new_vreg(w);
        }
    }

    // SSA RECONSTRUCTION (spec §4.1) — the plan's block parameters, built here
    // and nowhere else. A phi is a reload copy whose value arrives on an edge
    // instead of out of a slot, so once its name is in `copy_reg` the rewriting
    // below substitutes uses to it exactly as it does for a reload, and nothing
    // else in `apply` needs to know a phi exists.
    //
    // EVERY parameter is created before ANY edge is fed. A phi's argument on one
    // edge can be another phi's parameter, and around a loop the header's
    // argument from the latch and the latch's own parameter point at each other —
    // there is no order in which "resolve the arguments, then build" terminates.
    // Building all the names first dissolves the knot without any ordering
    // cleverness (`reconstruct::new_param` then `reconstruct::feed_phi`, the two
    // halves of `insert_phi`). Within one block the two passes keep the SAME
    // order, which is what makes `feed_phi`'s positional edge-filling line up.
    let mut params: Vec<(usize, usize, VReg)> = Vec::new();
    for b in 0..plan.phis.len() {
        for (k, ph) in plan.phis[b].iter().enumerate() {
            let p = super::reconstruct::new_param(f, b as MBlockId, ph.width.class(), ph.width);
            copy_reg[ph.id as usize] = Reg::V(p);
            params.push((b, k, p));
        }
    }
    for (b, k, p) in params {
        let args: Vec<(MBlockId, Reg)> = plan.phis[b][k]
            .srcs
            .iter()
            .map(|&(pred, c)| {
                let r = match c {
                    Some(id) => copy_reg[id as usize],
                    // the predecessor still holds the value under its own name,
                    // whose definition dominates that predecessor because the
                    // value is live there — so the name is the reaching
                    // definition and needs no copy
                    None => Reg::V(plan.phis[b][k].v),
                };
                (pred, r)
            })
            .collect();
        super::reconstruct::feed_phi(f, b as MBlockId, p, &args);
    }
    for b in 0..f.blocks.len() {
        let reloads = &plan.reloads[b];
        let subs = &plan.subs[b];
        if reloads.is_empty() && subs.is_empty() && !has_spilled_def(f, b, spilled, remat) {
            continue;
        }
        let insts = std::mem::take(&mut f.blocks[b].insts);
        let n = insts.len();
        let mut out: Vec<MInst> = Vec::with_capacity(n + reloads.len() * 2);
        let mut term = f.blocks[b].term.clone();
        for (i, inst) in insts.into_iter().enumerate().map(|(i, x)| (i, Some(x))).chain(
            std::iter::once((n, None)),
        ) {
            for &(at, v, id) in reloads.iter().filter(|(at, ..)| *at == i) {
                let d = copy_reg[id as usize];
                let _ = at;
                match remat.get(&v) {
                    Some(src) => {
                        let mut c = src.clone();
                        c.visit_mut(&mut |r, k| {
                            if matches!(k, Constraint::Def) {
                                *r = d;
                            }
                        });
                        out.push(c);
                    }
                    None => {
                        let (slot, sw) = slot_of[&v];
                        out.push(MInst::Reload { slot, dst: d, w: sw });
                    }
                }
            }
            let rewrite = |r: &mut Reg, k: Constraint| {
                if !matches!(k, Constraint::Use | Constraint::UseFixed(_)) {
                    return;
                }
                if let Some(v) = r.vreg() {
                    if let Some(&(_, _, id)) = subs.iter().find(|(at, x, _)| *at == i && *x == v) {
                        *r = copy_reg[id as usize];
                    }
                }
            };
            match inst {
                Some(mut inst) => {
                    inst.visit_mut(&mut |r, k| rewrite(r, k));
                    let mut defs: Vec<Reg> = Vec::new();
                    inst.visit(&mut |r, k| {
                        if matches!(k, Constraint::Def | Constraint::DefFixed(_)) {
                            if let Some(v) = r.vreg() {
                                if slot_of.contains_key(&v) {
                                    defs.push(r);
                                }
                            }
                        }
                    });
                    out.push(inst);
                    for d in defs {
                        let (slot, w) = slot_of[&d.vreg().unwrap()];
                        out.push(MInst::Spill { slot, src: d, w });
                    }
                }
                None => {
                    term.visit_mut(&mut |r, _| rewrite(r, Constraint::Use));
                }
            }
        }
        f.blocks[b].insts = out;
        f.blocks[b].term = term;
    }
}

fn has_spilled_def(
    f: &MFunc,
    b: usize,
    spilled: &[bool],
    remat: &BTreeMap<VReg, MInst>,
) -> bool {
    f.blocks[b].insts.iter().any(|inst| {
        let mut hit = false;
        inst.visit(&mut |r, k| {
            if matches!(k, Constraint::Def | Constraint::DefFixed(_)) {
                if let Some(v) = r.vreg() {
                    if spilled[v as usize] && !remat.contains_key(&v) {
                        hit = true;
                    }
                }
            }
        });
        hit
    })
}

/// A spilled BLOCK PARAMETER cannot be stored "at its definition": its
/// definition IS the block head, and a value occupying a register there is
/// exactly the pressure the spill was meant to relieve. Braun & Hack handle a
/// spilled phi the only way it can be handled — the parameter stops existing,
/// and each predecessor writes the value it would have passed straight into the
/// slot.
///
/// The square is the ordinary spill one: on every edge into the block the slot
/// afterwards holds exactly the operand that edge carried, and every use reloads
/// from it. Writing the slot in a predecessor that ALSO branches elsewhere is
/// harmless — the slot belongs to this value alone, so a path that never reads
/// it cannot observe the write.
fn evict_params(f: &mut MFunc, slot_of: &BTreeMap<VReg, (SlotId, Width)>) {
    for b in 0..f.blocks.len() {
        let keep: Vec<bool> = f.blocks[b]
            .params
            .iter()
            .map(|p| p.vreg().is_none_or(|v| !slot_of.contains_key(&v)))
            .collect();
        if keep.iter().all(|k| *k) {
            continue;
        }
        let dropped: Vec<(usize, SlotId, Width)> = f.blocks[b]
            .params
            .iter()
            .enumerate()
            .filter(|(i, _)| !keep[*i])
            .map(|(i, p)| {
                let (slot, w) = slot_of[&p.vreg().unwrap()];
                (i, slot, w)
            })
            .collect();
        let mut i = 0;
        f.blocks[b].params.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        for p in 0..f.blocks.len() {
            let mut term = f.blocks[p].term.clone();
            let mut stores: Vec<MInst> = Vec::new();
            let mut pre: Vec<MInst> = Vec::new();
            let mut edited = false;
            for t in term.targets_mut() {
                if t.block as usize != b {
                    continue;
                }
                edited = true;
                // THE EDGE IS A PARALLEL COPY, AND A SLOT IS ONE OF ITS
                // LOCATIONS. The stores below all write slots at the END of this
                // edge; any argument that is ITSELF resident in one of those
                // slots must be read BEFORE they run, or it reads the value the
                // edge has just overwritten. Read-before-write is the defining
                // property of a parallel copy, and it is not something the
                // ordering of two separately-scheduled phases provides on its
                // own — so the read is materialized here, into a fresh value the
                // spiller will never send back to memory, and the argument is
                // rewritten to name it. This is the same move `seq_copy` makes
                // for a register cycle, with a slot as the location.
                //
                // Measured: `int *pt[3]` rotated across a loop back edge
                // (`t=pt[0]; pt[0]=pt[1]; pt[1]=pt[2]; pt[2]=t;`) emitted
                // `str x13,[sp,#88]` and then `ldr x13,[sp,#88]`, so the reload
                // returned the value just stored and the rotation lost a
                // pointer. sqlite's `wherePathSolver` picks its join order with
                // exactly that rotation, which is how a two-cursor query came to
                // dereference a NULL cursor and SIGSEGV.
                let written: Vec<SlotId> = dropped
                    .iter()
                    .filter(|&&(k, slot, _)| {
                        !t.args[k]
                            .vreg()
                            .and_then(|v| slot_of.get(&v))
                            .is_some_and(|&(s2, _)| s2 == slot)
                    })
                    .map(|&(_, slot, _)| slot)
                    .collect();
                for j in 0..t.args.len() {
                    let Some(v) = t.args[j].vreg() else { continue };
                    let Some(&(s2, w2)) = slot_of.get(&v) else {
                        continue;
                    };
                    if !written.contains(&s2) {
                        continue;
                    }
                    let fresh = f.new_vreg(w2);
                    pre.push(MInst::Reload {
                        slot: s2,
                        dst: fresh,
                        w: w2,
                    });
                    t.args[j] = fresh;
                }
                for &(k, slot, w) in &dropped {
                    // If the argument is itself memory-resident IN THIS SLOT, the
                    // value is already where the parameter's uses will look for
                    // it, and the store is a no-op. That is the ordinary case
                    // after slot coalescing — a variable flowing through a join
                    // does not move — and skipping it is what stops a
                    // pass-through block from emitting `ldr d,[S]; str d,[S]`.
                    // Nothing can have overwritten the slot in between: the only
                    // other values that use it are web members, and a member
                    // defined while the argument is live would INTERFERE with it,
                    // which is exactly what `webs` refused to coalesce.
                    if t.args[k]
                        .vreg()
                        .and_then(|v| slot_of.get(&v))
                        .is_some_and(|&(s2, _)| s2 == slot)
                    {
                        continue;
                    }
                    stores.push(MInst::Spill { slot, src: t.args[k], w });
                }
                let mut i = 0;
                t.args.retain(|_| {
                    i += 1;
                    keep[i - 1]
                });
            }
            // The rewritten terminator goes back even when NO store was needed:
            // dropping the arguments is the point, and the store is only the
            // occasional extra. Writing it back conditionally left the edge
            // carrying arguments for parameters that no longer existed.
            if edited {
                f.blocks[p].insts.extend(pre);
                f.blocks[p].insts.extend(stores);
                f.blocks[p].term = term;
            }
        }
    }
}

// ── shared indices ─────────────────────────────────────────────────────────

/// Absolute position of each block's first instruction, in reverse postorder.
/// `usize::MAX` marks an unreachable block. A block occupies
/// `base[b] ..= base[b] + insts.len()`, the last slot being its terminator.
fn linear_positions(f: &MFunc, cfg: &crate::cfg::Cfg) -> Vec<usize> {
    let mut base = vec![usize::MAX; f.blocks.len()];
    let mut at = 0usize;
    for &b in &cfg.rpo {
        base[b as usize] = at;
        at += f.blocks[b as usize].insts.len() + 1;
    }
    base
}

/// Every position at which each value is READ, ascending — built once per
/// simulation. Belady's rule needs "the next use after this point", and with
/// this index that is a binary search instead of a scan of the whole function.
fn use_positions(
    f: &MFunc,
    lv: &live::Liveness,
    cfg: &crate::cfg::Cfg,
    base: &[usize],
) -> Vec<Vec<usize>> {
    let mut uses = vec![Vec::new(); lv.sp.len()];
    for &b in &cfg.rpo {
        let bi = b as usize;
        if base[bi] == usize::MAX {
            continue;
        }
        for (i, inst) in f.blocks[bi].insts.iter().enumerate() {
            let at = base[bi] + i;
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    uses[lv.sp.idx(r)].push(at);
                }
            });
        }
        let at = base[bi] + f.blocks[bi].insts.len();
        f.blocks[bi].term.visit(&mut |r, _| uses[lv.sp.idx(r)].push(at));
    }
    for u in uses.iter_mut() {
        u.sort_unstable();
        u.dedup();
    }
    uses
}

/// The next position at which value `x` is read, strictly after `from`;
/// `usize::MAX` when it is never used again — in the STATIC order.
fn next_use(uses: &[Vec<usize>], x: usize, from: usize) -> usize {
    let u = &uses[x];
    match u.partition_point(|&p| p <= from) {
        i if i < u.len() => u[i],
        _ => usize::MAX,
    }
}

/// The first position ≥ `lo` at which value `x` is read; `usize::MAX` if none.
fn use_from(uses: &[Vec<usize>], x: usize, lo: usize) -> usize {
    let u = &uses[x];
    match u.partition_point(|&p| p < lo) {
        i if i < u.len() => u[i],
        _ => usize::MAX,
    }
}

/// Assumed trips per loop level (`MEASURED M12`). Belady's distance counts
/// DYNAMIC steps, so leaving a loop costs the iterations still to run, and no
/// static analysis knows that number. Only the ORDER this factor induces is
/// used — a value wanted inside this loop must outrank one wanted after it — so
/// the question Article E makes mandatory ("the spec's number, or my
/// convenience's number?") is answered by measuring how much the number
/// matters: swept 1…1000 over sqlite and the taxonomy suite, the whole range
/// moves sqlite by 72 instructions (0.04%) and the suite not at all, and the
/// output saturates from 100 upward. Ten is the value every other compiler uses
/// (gcc's `10^depth` block frequency), so it is the one a reader can check.
const TRIPS: usize = 10;

/// THE TRACE — Belady's rule measured along the EXECUTION order, not the text.
///
/// `linear_positions` numbers instructions in reverse postorder, and a back edge
/// runs BACKWARDS in that numbering. So for a value carried around a loop —
/// the induction variable, the pointer, the accumulator — the static
/// `next_use` from the latch finds no later use and answers `usize::MAX`,
/// "never used again", which is the strongest possible reason to evict it.
/// The value is in fact read by the very next dynamic instruction. Measured on
/// `tests/bench/nestjoin.c`, that is exactly what happened: the three hot values
/// were spilled out of a four-million-iteration inner loop while twenty-four
/// cold ones, whose uses lie at higher positions, kept their registers.
///
/// Belady's MIN is a theorem about a TRACE (THEORY I — optimal replacement is
/// furthest-next-use *in time*), so the distance it ranks by has to be counted
/// in time. This walks out through the loop nest from the point of the
/// question and answers with the first of:
///
/// * a use still ahead in this iteration → its static distance;
/// * a use behind, inside the same loop → one wrap: to the latch, then in from
///   the header;
/// * neither → the remaining trips of this loop, then the same question of the
///   enclosing loop.
///
/// The result is `usize::MAX` only for a value that is genuinely dead, which is
/// what the sentinel meant all along.
///
/// ONE MORE THING THE VREG CANNOT SEE — the question is asked of a WEB.
/// mem2reg splits one C variable into a chain of SSA values joined by block
/// parameters, and each link of that chain has exactly ONE use: being passed to
/// the next link. Asked of the vreg, "how far to the next use of `c0`?" and
/// "how far to the next use of `j`?" both answer 1, because both are about to
/// be handed to the next block — the twenty-four cold values and the three hot
/// ones become indistinguishable at precisely the edge where the choice is
/// made. So the distance is measured over the web: the next use of the
/// VARIABLE, not of this link. That is also the granularity at which the
/// decision is PAID, since a value evicted here retires its whole web to memory
/// (`Sim::More`, above) — ranking by anything finer prices something the
/// allocator never buys.
struct Trace<'a> {
    lf: &'a crate::cfg::LoopForest,
    /// R5.1-C — `(first position, weight)` per block, ascending by position, so
    /// the block holding a use is a binary search rather than a scan. Empty when
    /// weights are off, and `rank` then never asks.
    starts: Vec<(usize, u32)>,
    /// whether `rank` scales at all (`ZCC_WEIGHTS`); off, it IS `next_use` and
    /// the allocator makes exactly the decisions it made before.
    weighted: bool,
    /// each loop's body in POSITION space. A natural loop's body is contiguous
    /// in reverse postorder for a reducible CFG, so min..max is that span; where
    /// irreducibility makes it wider the model over-estimates a wrap distance,
    /// which costs a ranking, never a correctness obligation.
    span: Vec<(usize, usize)>,
    /// `uses`, unioned over each web and indexed by the web's root value.
    wuses: Vec<Vec<usize>>,
    /// the root of each value's web, extended to values minted after `webs`
    /// ran (they are their own root: a fresh name nothing has joined yet).
    root: Vec<VReg>,
}

impl<'a> Trace<'a> {
    fn new(
        f: &MFunc,
        lf: &'a crate::cfg::LoopForest,
        base: &[usize],
        uses: &[Vec<usize>],
        lv: &live::Liveness,
        web: &[VReg],
    ) -> Trace<'a> {
        let nv = f.vregs.len();
        let root: Vec<VReg> =
            (0..nv as VReg).map(|v| web.get(v as usize).copied().unwrap_or(v)).collect();
        let mut wuses: Vec<Vec<usize>> = vec![Vec::new(); nv];
        for v in 0..nv as VReg {
            let u = &uses[lv.sp.idx(Reg::V(v))];
            if u.is_empty() {
                continue;
            }
            wuses[root[v as usize] as usize].extend_from_slice(u);
        }
        for u in wuses.iter_mut() {
            u.sort_unstable();
            u.dedup();
        }
        let weighted = crate::hir::freq::spill_wanted();
        let mut starts: Vec<(usize, u32)> = Vec::new();
        if weighted {
            starts = (0..f.blocks.len())
                .filter(|&b| base[b] != usize::MAX)
                .map(|b| (base[b], f.blocks[b].weight))
                .collect();
            starts.sort_unstable();
        }
        Trace { lf, span: Self::spans(f, lf, base), wuses, root, starts, weighted }
    }

    fn spans(
        f: &MFunc,
        lf: &crate::cfg::LoopForest,
        base: &[usize],
    ) -> Vec<(usize, usize)> {
        lf.loops
            .iter()
            .map(|l| {
                let (mut lo, mut hi) = (usize::MAX, 0usize);
                for &b in l.body.iter() {
                    let bi = b as usize;
                    if base[bi] == usize::MAX {
                        continue;
                    }
                    lo = lo.min(base[bi]);
                    hi = hi.max(base[bi] + f.blocks[bi].insts.len());
                }
                (lo, hi)
            })
            .collect()
    }

    /// Dynamic distance from `from` (a position in block `bi`) to the next read
    /// of value `x`'s WEB. This is the number the eviction rule ranks by.
    fn next_use(&self, x: VReg, bi: usize, from: usize) -> usize {
        self.next_use_at(x, bi, from).0
    }

    /// `(dynamic distance, the position of the use it settled on)`. The second
    /// half is what `rank` needs and the walk already knows: which use answered
    /// is not recoverable from the distance once a loop wrap has folded a
    /// backwards step into a forwards one.
    fn next_use_at(&self, x: VReg, bi: usize, from: usize) -> (usize, usize) {
        let uses: &[Vec<usize>] = match self.root.get(x as usize) {
            Some(&r) => std::slice::from_ref(&self.wuses[r as usize]),
            None => return (usize::MAX, usize::MAX),
        };
        let x = 0usize;
        let mut pos = from;
        let mut acc = 0usize;
        let mut cur = self.lf.of.get(bi).copied().flatten();
        while let Some(li) = cur {
            let (lo, hi) = self.span[li as usize];
            if lo == usize::MAX {
                break;
            }
            let u = next_use(uses, x, pos);
            if u <= hi {
                return (acc.saturating_add(u - pos), u);
            }
            let w = use_from(uses, x, lo);
            if w < hi {
                // one wrap: out to the latch, then in from the header
                return (
                    acc.saturating_add(hi.saturating_sub(pos)).saturating_add(w - lo),
                    w,
                );
            }
            // not wanted anywhere in this loop: the trips still to run are the
            // distance, and the question moves outward
            acc = acc.saturating_add(TRIPS.saturating_mul(hi.saturating_sub(pos)));
            pos = hi;
            cur = self.lf.loops[li as usize].parent;
        }
        match next_use(uses, x, pos) {
            usize::MAX => (usize::MAX, usize::MAX),
            u => (acc.saturating_add(u - pos), u),
        }
    }

    /// R5.1-C — WHAT AN EVICTION COSTS, not only how far away it is paid.
    ///
    /// Belady ranks by distance because in the cache his theorem is about, every
    /// miss costs the same. Here it does not: evicting a value inserts a reload
    /// AT ITS NEXT USE, so the price is one load times the number of times that
    /// block runs. Two values whose next reads are equally distant — one in the
    /// body of a hot loop, one on an error path that runs once — are not equally
    /// cheap to evict, and distance alone cannot tell them apart. `Trace` already
    /// charges `TRIPS` per loop LEVEL, which is the same idea at the resolution
    /// loop depth can express; block frequency is that idea at the resolution the
    /// CFG can express, and R5.1-A finally computes it.
    ///
    /// The rank is `distance × ENTRY / weight(block of the next use)`: an entry-
    /// block use leaves the distance as it was (`weight == ENTRY` there by
    /// construction), a hot use shrinks it — the value becomes harder to evict —
    /// and a cold one stretches it. A dead value stays `usize::MAX`, which every
    /// caller reads as "evict this first".
    ///
    /// This composes with, and does not replace, the rematerialization term that
    /// `4de446c` put in front of the distance: a value that can be rebuilt in one
    /// instruction is still the first victim whatever the frequencies say.
    ///
    /// SOUNDNESS. It is a RANKING, not a fact: any order this produces is a legal
    /// eviction order, so no correctness obligation rides on the arithmetic. What
    /// rides on it is speed, which is why the whole thing is behind the toggle
    /// and off by default until measured on a machine.
    fn rank(&self, x: VReg, bi: usize, from: usize) -> usize {
        if !self.weighted {
            return self.next_use(x, bi, from);
        }
        let (d, at) = self.next_use_at(x, bi, from);
        if d == usize::MAX {
            return usize::MAX;
        }
        let w = self.weight_at(at).max(1) as u128;
        let r = (d as u128) * (crate::hir::freq::ENTRY as u128) / w;
        // one below the sentinel: a live value must never rank as dead
        r.min(usize::MAX as u128 - 1) as usize
    }

    /// The weight of the block holding position `at`, by binary search over the
    /// block starts — the positions are numbered blockwise, so the block is the
    /// last one that starts at or before `at`.
    fn weight_at(&self, at: usize) -> u32 {
        match self.starts.partition_point(|&(p, _)| p <= at) {
            0 => 1,
            i => self.starts[i - 1].1,
        }
    }
}

fn class_of(f: &MFunc, r: Reg) -> Class {
    match r {
        Reg::V(v) => f.vregs[v as usize].class,
        Reg::P(p) => p.class,
    }
}

// ── one slot per SSA web ───────────────────────────────────────────────────

/// Union-find over "values that are the same C variable and never hold two
/// different things at once": a block parameter and the arguments its edges pass
/// it, merged only when the two classes do not INTERFERE. mem2reg splits one
/// local into a web of SSA values joined exactly by those edges, and in the
/// ordinary case each value dies exactly where the next is defined — so the web
/// collapses to one slot. Where it does not (a value still live after the next
/// definition), the merge is refused and the two keep separate slots: sharing
/// there would overwrite a live value, which is a miscompile, not a size cost.
///
/// WHY IT MATTERS, measured. Giving each spilled VALUE its own slot makes a
/// spilled parameter copy between slots on every incoming edge — sqlite's
/// `sqlite3VdbeExec` grew 110,000 stores that way, moving one variable from one
/// slot to another. With one slot per web, the incoming value has usually just
/// been reloaded FROM that slot, and `drop_redundant_spills` deletes the store
/// outright: the variable simply stays where it was.
fn webs(f: &MFunc) -> Vec<VReg> {
    let cfg = crate::mir::verify::cfg(f);
    let lv = live::compute(f, &cfg);
    let sp = lv.sp;
    let nv = f.vregs.len();
    // definition point of every value, on a 1-based scale where 0 is the block
    // head (a parameter) and instruction i sits at i + 1
    let mut def_blk = vec![u32::MAX; nv];
    let mut def_idx = vec![0u32; nv];
    let mut last: Vec<BTreeMap<VReg, usize>> = vec![BTreeMap::new(); f.blocks.len()];
    let mut lu = live::LastUse::new(sp);
    for b in 0..f.blocks.len() {
        if !cfg.reachable(b as MBlockId) {
            continue;
        }
        for &p in &f.blocks[b].params {
            if let Some(v) = p.vreg() {
                def_blk[v as usize] = b as u32;
                def_idx[v as usize] = 0;
            }
        }
        for (i, inst) in f.blocks[b].insts.iter().enumerate() {
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    if let Some(v) = r.vreg() {
                        def_blk[v as usize] = b as u32;
                        def_idx[v as usize] = i as u32 + 1;
                    }
                }
            });
        }
        live::last_use_into(f, sp, &lv, b, &mut lu);
        // WHAT THIS BLOCK WROTE, not the whole value space. `last_use_into` only
        // ever records a register the block's own instructions read, and it keeps
        // the list of those entries for exactly this reason — walking `0..nv` per
        // block is O(blocks × values), which `live.rs` itself calls out as
        // "invisible on a small function and the dominant cost on a real one".
        // A virtual register's index IS its number and the physical ones sit
        // above `nv`, so the vreg entries are the ones below it. `last[b]` is
        // ordered by key, so the order they arrive in does not matter.
        for &i in lu.touched() {
            if i < nv {
                if let Some(j) = lu.at[i] {
                    last[b].insert(i as VReg, j);
                }
            }
        }
    }
    // `v` holds a value at the point just after position `idx` in block `b`
    let live_at = |v: VReg, b: u32, idx: u32| -> bool {
        if b == u32::MAX {
            return false;
        }
        let bi = b as usize;
        let mine = def_blk[v as usize] == b;
        if mine {
            if def_idx[v as usize] > idx {
                return false; // not defined yet
            }
            if def_idx[v as usize] == idx {
                return true; // defined right here
            }
        } else if !lv.live_in[bi].contains(&sp.idx(Reg::V(v))) {
            return false;
        }
        if lv.live_out[bi].contains(&sp.idx(Reg::V(v))) {
            return true;
        }
        match last[bi].get(&v) {
            // a use at instruction j is at position j + 1
            Some(&j) => j == usize::MAX || j as u32 + 1 > idx,
            None => false,
        }
    };
    let interfere = |x: VReg, y: VReg| -> bool {
        x == y
            || live_at(y, def_blk[x as usize], def_idx[x as usize])
            || live_at(x, def_blk[y as usize], def_idx[y as usize])
    };

    let mut parent: Vec<VReg> = (0..nv as VReg).collect();
    fn find(p: &mut Vec<VReg>, mut x: VReg) -> VReg {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let mut members: BTreeMap<VReg, Vec<VReg>> = BTreeMap::new();
    for b in &f.blocks {
        for t in b.term.targets() {
            let want = &f.blocks[t.block as usize].params;
            for (a, q) in t.args.iter().zip(want) {
                let (a, q) = match (a.vreg(), q.vreg()) {
                    (Some(a), Some(q)) => (a, q),
                    _ => continue,
                };
                if f.vregs[a as usize].width != f.vregs[q as usize].width {
                    continue;
                }
                let (ra, rq) = (find(&mut parent, a), find(&mut parent, q));
                if ra == rq {
                    continue;
                }
                // Transitivity is not free: merging two classes makes EVERY
                // member share one slot, so the check is class against class,
                // not just the pair that proposed the merge.
                //
                // READ THE CLASSES, do not copy them. Both were cloned here on
                // every edge argument in the function — including the ones the
                // check below rejects at the first pair, and including the
                // singletons — so a variable threaded through many joins paid a
                // heap allocation per attempt. The two are only taken apart once
                // the merge is agreed.
                let (sa, sq) = ([ra], [rq]);
                let clash = {
                    let ma: &[VReg] = members.get(&ra).map(Vec::as_slice).unwrap_or(&sa);
                    let mq: &[VReg] = members.get(&rq).map(Vec::as_slice).unwrap_or(&sq);
                    ma.iter().any(|&x| mq.iter().any(|&y| interfere(x, y)))
                };
                if clash {
                    continue;
                }
                parent[ra as usize] = rq;
                let mut all = members.remove(&ra).unwrap_or_else(|| vec![ra]);
                all.extend(members.remove(&rq).unwrap_or_else(|| vec![rq]));
                members.insert(rq, all);
            }
        }
    }
    (0..nv as VReg).map(|v| find(&mut parent, v)).collect()
}

/// The slot a spilled value uses: one per WEB, allocated on first demand.
///
/// `slot_of` is keyed by VALUE and holds an entry only for values that are
/// actually spilled — `web_slot` is the per-class index. Keeping the two apart
/// matters: `evict_params` decides which parameters to remove by asking whether
/// `slot_of` names them, and a web root that merely gave its name to a slot must
/// never answer yes.
fn ensure_slot(
    f: &mut MFunc,
    web: &[VReg],
    web_slot: &mut BTreeMap<VReg, (SlotId, Width)>,
    slot_of: &mut BTreeMap<VReg, (SlotId, Width)>,
    v: VReg,
) -> (SlotId, Width) {
    let root = web[v as usize];
    if let Some(&e) = web_slot.get(&root) {
        slot_of.insert(v, e);
        return e;
    }
    let w = f.vregs[v as usize].width;
    // a `q` spill needs 16 bytes AND 16-byte alignment (DDI 0487 C3.2: the
    // unsigned offset form scales the immediate by the access size)
    let slot = f.new_slot(w.bytes().max(8), w.bytes().max(8), SlotKind::Spill);
    web_slot.insert(root, (slot, w));
    slot_of.insert(v, (slot, w));
    (slot, w)
}

/// A store of exactly what the slot already holds. Spill slots are compiler
/// private — their address is never taken and no C-level access can reach them —
/// so within a block the last `Spill`/`Reload` of a slot names the register that
/// equals its contents, and storing that same register again is a no-op. This is
/// what turns a spilled parameter from "copy the variable into its slot on every
/// edge" into "leave the variable where it already is".
fn drop_redundant_spills(f: &mut MFunc) {
    for b in f.blocks.iter_mut() {
        let mut holds: BTreeMap<SlotId, Reg> = BTreeMap::new();
        let mut keep: Vec<bool> = Vec::with_capacity(b.insts.len());
        for inst in &b.insts {
            let k = match inst {
                MInst::Reload { slot, dst, .. } => {
                    holds.insert(*slot, *dst);
                    true
                }
                MInst::Spill { slot, src, .. } => {
                    if holds.get(slot) == Some(src) {
                        false
                    } else {
                        holds.insert(*slot, *src);
                        true
                    }
                }
                _ => true,
            };
            keep.push(k);
        }
        let mut i = 0;
        b.insts.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    }
}
