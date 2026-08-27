// rotate — loop rotation / header copying (REARCH §13c row 2).
// THEORY A7b — optimization: this pass ships its commuting square
//
// gcc calls this `-ftree-ch`, "copy loop headers", and enables it at -O1. It
// turns a TOP-tested loop into a BOTTOM-tested one:
//
//     while (c) { B }        ⟹      if (c) { do { B } while (c); }
//
// and the win is one branch per iteration. A top-tested loop pays a conditional
// branch at the header AND an unconditional branch at the latch to get back to
// it; a bottom-tested loop pays one conditional branch that falls through. §13d
// measured this as cause #1 of every hot-loop regression against `main`:
// `mycopy`'s inner loop is 3 instructions per iteration there and 7 here, and
// two of the four extra are the branch pair.
//
// THE TRANSFORM, on block-parameter SSA. The header is CLONED into a new `guard`
// block, and every edge that entered the loop from outside is redirected to the
// clone. The header itself keeps only its back edges, so it becomes the loop's
// bottom test; the loop's new header is the block the test falls into.
//
// COMMUTING SQUARE, by counting executions. Let the loop run n times.
//   * Before: the header runs n+1 times (n tests that enter, one that exits) and
//     the body n times.
//   * After: the guard runs once — that is the header's FIRST execution — and
//     the header runs n more, for n+1; the body still runs n.
// Every dynamic execution of the header happens exactly once, in the same order,
// with the same operand values. This is why the header's instructions need not
// be pure or trap-free: they are not SPECULATED, they are RELOCATED. A loop
// entered zero times still runs the header exactly once, now spelled `guard`.
//
// WHAT IS REFUSED, each because it is a property of the IR and not a heuristic:
//   * a header holding anything but its own exit test — copying that is PEELING,
//     not rotation. This is also what makes the pass terminate, and it is phrased
//     about data flow because the two placement-based phrasings that came before
//     it were each defeated by a later pass (§13f, §13h).
//   * a header whose definitions are used OUTSIDE the loop. After rotation the
//     header no longer dominates the exit (the guard reaches it too), so such a
//     use would leave SSA. Passing a value as an edge ARGUMENT is fine — that is
//     a parameter of the exit block, and the guard supplies its own copy.
//   * a header reached by a computed goto, or whose block identity is observable
//     (`&&label`) — the edge cannot be redirected, or the block cannot move.
//   * a header that stores, calls, or allocas — it would be written twice.
use super::*;

/// MEASURED M7 — gcc's --param, honestly labelled as not a spec
/// The largest header worth copying, in instructions. This is gcc's own -O1
/// number for the same transform (`--param max-loop-header-insns`, default 20,
/// read by `-ftree-ch`) rather than one picked here: the transform trades a
/// STATIC copy of the header for a DYNAMIC branch per iteration, so no bound
/// falls out of the theorem — it is a size/speed exchange rate, and the honest
/// thing is to take the reference compiler's rate and say where it came from.
const MAX_HEADER_INSTS: usize = 20;

/// THEORY A7b — the pass ships on, see the note above
/// Rotation was measured WORTHLESS when it first landed, and turning it on took
/// removing two things that were cancelling it — both of them elsewhere, which
/// is why the sequence is worth recording (§13e → §13f).
///
/// It makes the back edge CRITICAL: the header gains a second successor and the
/// new header a second predecessor. The edge is therefore split, and the split
/// block is exactly where SSA destruction parks the loop-carried copy. So the
/// branch rotation removes was replaced by a copy block that needs a branch of
/// its own — 10 instructions per iteration before AND after, with the guard as
/// pure addition (sqlite +2.7%, and +1,732 BRANCHES, the metric it targets).
///
/// The two fixes: `regalloc::color` now frees a DYING operand before placing the
/// instruction's destination, so `add w1,w0,#1 ; mov x0,w1` becomes
/// `add w0,w0,#1` and the copy is gone; and `mir::pass::layout` THREADS the
/// block that is then empty, so the branch to a branch is gone too. Only after
/// both does the loop become the `work ; cmp ; b.lt` the theorem promised.
const ENABLED: bool = true;

fn enabled() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| ENABLED || std::env::var("ZCC_ROTATE").is_ok())
}

/// THEORY A7b  SQUARE rotation_moves_the_test_to_the_bottom — execution-count equality
pub fn run(f: &mut Func) -> bool {
    if !enabled() {
        return false;
    }
    force(f)
}

/// The pass itself, past the default-off gate. The batteries call this: a
/// theorem that ships disabled still owes its commuting square, or turning it on
/// later would be turning on something unproven.
pub fn force(f: &mut Func) -> bool {
    let mut changed = false;
    // Each rotation rewrites the CFG, so the analyses are rebuilt between them.
    // The bound is the block count purely as a runaway guard: a rotated loop
    // cannot be rotated again, so the real bound is the number of loops.
    for _ in 0..f.blocks.len() {
        if !rotate_one(f) {
            break;
        }
        changed = true;
    }
    if changed {
        // The guard has two successors and both of its targets have gained a
        // predecessor, so rotation manufactures critical edges by construction.
        dom::split_critical_edges(f);
    }
    changed
}

/// `ZCC_RESIDUAL=1` names, per refused loop, WHICH condition refused it — the
/// same instrument `licm.rs` carries, for the same reason. Law 4 asks for the
/// residual of every shipped theorem, and §13n recorded rotation's absence as a
/// gap in itself: "the pass prints no residual". Reading the pass instead of
/// measuring it is the guesswork Law 2 forbids.
fn residual_wanted() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_RESIDUAL").is_ok())
}

fn rotate_one(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let pin = pinned(f);
    // innermost first: the inner loop is the hot one
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    let report = residual_wanted();
    for li in order {
        match try_rotate(f, &c, &dt, &lf, li, &pin) {
            Ok(()) => {
                if report {
                    // what a rotation COSTS, for the profitability question:
                    // the guard it duplicates is the header's own body, and the
                    // loop it buys a branch back for is `body` blocks deep.
                    let n: usize = lf.loops[li]
                        .body
                        .iter()
                        .map(|&b| f.blocks[b as usize].insts.len())
                        .sum();
                    eprintln!(
                        "rotate-did {} loop@bb{} depth{} guard{} body{}",
                        f.name,
                        lf.loops[li].header,
                        lf.loops[li].depth,
                        f.blocks[lf.loops[li].header as usize].insts.len(),
                        n
                    );
                }
                return true;
            }
            Err(why) => {
                if report && why != "already bottom-tested" {
                    eprintln!(
                        "rotate-residual {} loop@bb{} depth{}: {}",
                        f.name, lf.loops[li].header, lf.loops[li].depth, why
                    );
                }
            }
        }
    }
    false
}

fn try_rotate(
    f: &mut Func,
    c: &dom::Cfg,
    dt: &dom::DomTree,
    lf: &dom::LoopForest,
    li: usize,
    pin: &[bool],
) -> Result<(), &'static str> {
    let h = lf.loops[li].header;
    // A block whose IDENTITY is observable cannot move: `&&label` puts its
    // address in a static initializer, and a computed goto names it. `pinned`
    // knows exactly which those are — a plain C label does NOT pin anything,
    // because a `goto` to it is an ordinary edge and rotation redirects edges.
    // This used to refuse EVERY labelled header, which §13n measured as the
    // reason d3 and d4 keep two branches per iteration.
    //
    // The one case a plain label still blocks: `emit` writes `mov sp, x29` at a
    // labelled block of a function with a dynamic frame (C99 6.8.6.1, a `goto`
    // leaving a VLA's scope). The guard would not get that instruction, so a
    // VLA function keeps the old refusal.
    if pin[h as usize] {
        return Err("header address-taken or a computed-goto target");
    }
    if !f.blocks[h as usize].labels.is_empty() && f.has_vla {
        return Err("labelled header in a VLA function (sp restore at the label)");
    }
    if f.blocks[h as usize].insts.len() > MAX_HEADER_INSTS {
        return Err("header larger than max-loop-header-insns");
    }
    // The header is COPIED, so nothing in it may be observable twice in the
    // source text. Reads are fine — `while (*p)` is the shape this transform
    // exists for, and each dynamic execution still happens exactly once — but a
    // store, a call or an `alloca` would appear at two program points. This is
    // also the second half of the termination argument: after rotating, the new
    // header holds the loop BODY, and a body that calls or stores refuses to be
    // copied again, so the peeling cannot chain.
    if f.blocks[h as usize]
        .insts
        .iter()
        .any(|i| matches!(i.effect(), Effect::Write | Effect::Call))
    {
        return Err("header stores, calls or allocas");
    }
    let mut inloop = vec![false; f.blocks.len()];
    for &b in &lf.loops[li].body {
        inloop[b as usize] = true;
    }
    // Already bottom-tested (and the termination argument).
    let exits = |b: BlockId| -> bool {
        match &f.blocks[b as usize].term {
            Term::Ret(_) | Term::Unreachable => true,
            t => t.succs().iter().any(|&s| !inloop[s as usize]),
        }
    };
    if lf.loops[li].latches.iter().any(|&l| exits(l)) {
        return Err("already bottom-tested");
    }
    if lf.loops[li].latches.contains(&h) {
        return Err("already bottom-tested");
    }
    // THE HEADER MUST BE THE TEST AND NOTHING ELSE — gcc's pass is called "copy
    // loop HEADERS" for this reason. Copying a header that also holds body work
    // is not rotation, it is PEELING: the work appears at two program points and
    // the loop runs one fewer time.
    //
    // This is also the termination argument, and it is the THIRD one written
    // here — the first two were both defeated by a later pass, which is the
    // lesson worth keeping. "The latch exits" died to `split_critical_edges`,
    // which puts an empty block on the back edge so the latch stops exiting.
    // "Some block outside the header has work" died to `sink`, which moves an
    // instruction down into that empty block and makes it non-empty. Both were
    // statements about WHERE INSTRUCTIONS SIT, and any pass may move an
    // instruction. This one is a statement about DATA FLOW: after rotation the
    // header holds the body, the body does not feed the exit condition, and no
    // amount of code motion changes that.
    if !header_is_only_the_test(f, h) {
        return Err("header holds body work (copying it would be peeling)");
    }
    // The header must BE the exit test: a two-way branch with one arm inside.
    let (t_in, t_out) = match &f.blocks[h as usize].term {
        Term::Br(_, a, b) => match (inloop[a.block as usize], inloop[b.block as usize]) {
            (true, false) => (a.block, b.block),
            (false, true) => (b.block, a.block),
            _ => return Err("header's branch has both arms inside the loop"),
        },
        _ => return Err("header does not end in a two-way branch"),
    };
    // The block the test falls into becomes the new header, and it must be able
    // to take the guard's edge as a second predecessor without merging two
    // unrelated paths. Critical edges are split before the ladder runs, so a
    // single predecessor is the normal state here.
    if c.preds[t_in as usize].len() != 1 {
        return Err("the block the test falls into has other predecessors");
    }
    // Every entry edge must be redirectable, so none may come from a computed
    // goto (whose successors are a set, not a target list).
    for &p in &c.preds[h as usize] {
        if matches!(f.blocks[p as usize].term, Term::GotoPtr(..)) {
            return Err("an entry edge comes from a computed goto");
        }
    }

    // What the header defines: its parameters and its instruction results.
    let mut dh: Vec<ValueId> = f.blocks[h as usize].params.clone();
    for inst in &f.blocks[h as usize].insts {
        if let Some(d) = inst.dst() {
            dh.push(d);
        }
    }
    let mut isdh = vec![false; f.values.len()];
    for &v in &dh {
        isdh[v as usize] = true;
    }
    // A header definition READ AFTER THE LOOP — the accumulator in
    // `for (...) s += ...; return s;` — is the ordinary case, not an obstacle.
    // It cannot stay a direct reference: after rotation the header no longer
    // dominates the exit, because the guard reaches it too. So it leaves through
    // the exit block as a PARAMETER, and each of the two predecessors supplies
    // its own value. This is loop-closed SSA, built here for exactly the values
    // that need it.
    let mut out_users: Vec<BlockId> = Vec::new();
    for b in 0..f.blocks.len() {
        if inloop[b] || !c.reachable(b as BlockId) {
            continue;
        }
        let mut hit = false;
        let mut see = |o: Operand| {
            if let Operand::Val(v) = o {
                if isdh[v as usize] {
                    hit = true;
                }
            }
        };
        for inst in &f.blocks[b].insts {
            inst.uses(&mut see);
        }
        f.blocks[b].term.uses(&mut see);
        if hit {
            out_users.push(b as BlockId);
        }
    }
    // LOOP-CLOSED SSA, over EVERY exit of the loop (R4.11). The first version of
    // this refused unless `t_out` was the single door — one predecessor and
    // dominating every reader. The residual print measured what that costs on
    // sqlite: **1,837 loops refused for "the exit block is a merge" and 221 more
    // for "outside the exit's dominance"**, the two biggest fixable reasons and
    // between them d3 and j5. A loop with an early `return` has TWO exits, and a
    // `while (a && b)` reaches one exit from two different in-loop blocks; both
    // are ordinary C, not obstacles.
    //
    // The general construction: for each EXIT BLOCK e — outside the loop, with
    // at least one predecessor inside it — a header value read below e leaves
    // through e as a parameter, and every predecessor of e supplies the value it
    // itself can see:
    //   * `h` still defines the value, so it passes it directly;
    //   * `g`, the guard, passes its own clone;
    //   * any other in-loop predecessor sits BELOW the new header, so it passes
    //     the parameter that `reparam` gives the new header — which is why the
    //     value must get one whether or not the body reads it.
    // Every predecessor of e must be inside the loop (or the guard): an edge
    // from outside owes an argument it has no value for.
    let mut exit_of: Vec<(BlockId, BlockId)> = Vec::new(); // (reader, its exit)
    if !out_users.is_empty() {
        let mut exits: Vec<BlockId> = Vec::new();
        for &b in &lf.loops[li].body {
            for &sx in f.blocks[b as usize].term.succs().iter() {
                if !inloop[sx as usize] && !exits.contains(&sx) {
                    exits.push(sx);
                }
            }
        }
        for &u in &out_users {
            // the exit this reader is reached through; dominators of a node are
            // totally ordered, so "the deepest one that dominates it" is unique
            let mut best: Option<BlockId> = None;
            for &e in &exits {
                if dt.dominates(e, u) && best.is_none_or(|b| dt.dominates(b, e)) {
                    best = Some(e);
                }
            }
            match best {
                Some(e) => exit_of.push((u, e)),
                None => {
                    return Err("a header value is read where no single loop exit dominates it");
                }
            }
        }
        for &(_, e) in &exit_of {
            if c.preds[e as usize].iter().any(|&p| !inloop[p as usize]) {
                return Err("an exit block carrying a header value has a predecessor outside the loop");
            }
        }
    }

    // ── the clone ──────────────────────────────────────────────────────────
    let g = f.new_block();
    f.blocks[g as usize].weight = f.blocks[h as usize].weight;
    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    for (k, p) in f.blocks[h as usize].params.clone().iter().enumerate() {
        let np = f.new_value(f.ty_of(*p), Def::Param(g, k as u32));
        f.blocks[g as usize].params.push(np);
        map[*p as usize] = Some(Operand::Val(np));
    }
    for inst in f.blocks[h as usize].insts.clone() {
        let mut ci = inst.clone();
        ci.uses_mut(|o| *o = sub(&map, *o));
        if let Some(d) = inst.dst() {
            let nd = f.new_value(f.ty_of(d), Def::Inst(g, 0));
            set_dst(&mut ci, nd);
            map[d as usize] = Some(Operand::Val(nd));
        }
        f.blocks[g as usize].insts.push(ci);
    }
    let mut term = f.blocks[h as usize].term.clone();
    match &mut term {
        Term::Br(x, ..) | Term::Switch(x, ..) | Term::GotoPtr(x, _) => *x = sub(&map, *x),
        Term::Ret(Some(x)) => *x = sub(&map, *x),
        _ => {}
    }
    for t in term.targets_mut() {
        for a in t.args.iter_mut() {
            *a = sub(&map, *a);
        }
    }
    f.blocks[g as usize].term = term;

    // ── the new header takes over the header's definitions ─────────────────
    // A value the header defines and the BODY reads was dominated by the header
    // before; now the body is reachable from the guard too, so the value travels
    // as a block parameter and each of the two predecessors supplies its own.
    let in_body: Vec<BlockId> =
        lf.loops[li].body.iter().copied().filter(|&b| b != h).collect();
    // Values that must reach the new header whether or not the BODY reads them:
    // an in-loop predecessor of an exit passes the value on, and below the new
    // header the parameter is the only name it has.
    let forced: Vec<ValueId> = if exit_of.is_empty() {
        Vec::new()
    } else {
        dh.iter()
            .copied()
            .filter(|&v| {
                exit_of.iter().any(|&(u, e)| {
                    uses_value(f, u, v)
                        && c.preds[e as usize].iter().any(|&p| p != h && inloop[p as usize])
                })
            })
            .collect()
    };
    let rw_body = reparam(f, &dh, &map, h, g, t_in, &in_body, &forced);
    // …then every exit, each predecessor supplying the name it can see.
    let mut by_exit: Vec<(BlockId, Vec<BlockId>)> = Vec::new();
    for &(u, e) in &exit_of {
        match by_exit.iter_mut().find(|(x, _)| *x == e) {
            Some((_, rs)) => rs.push(u),
            None => by_exit.push((e, vec![u])),
        }
    }
    for (e, readers) in by_exit {
        close_exit(f, &dh, &map, &rw_body, h, g, e, &readers, &inloop);
    }

    // ── every edge from outside now enters the guard instead ───────────────
    let outside: Vec<BlockId> = c.preds[h as usize]
        .iter()
        .copied()
        .filter(|&p| !dt.dominates(h, p))
        .collect();
    for p in outside {
        for t in f.blocks[p as usize].term.targets_mut() {
            if t.block == h {
                t.block = g;
            }
        }
    }
    refresh_defs(f);
    Ok(())
}

fn sub(map: &[Option<Operand>], o: Operand) -> Operand {
    match o {
        Operand::Val(v) => map[v as usize].unwrap_or(o),
        k => k,
    }
}

fn set_dst(inst: &mut Inst, nd: ValueId) {
    match inst {
        Inst::Bin { dst, .. }
        | Inst::Un { dst, .. }
        | Inst::Cmp { dst, .. }
        | Inst::Cvt { dst, .. }
        | Inst::Load { dst, .. }
        | Inst::SlotAddr { dst, .. }
        | Inst::SymAddr { dst, .. }
        | Inst::Select { dst, .. }
        | Inst::Alloca { dst, .. } => *dst = nd,
        Inst::Call { dst, .. } | Inst::Intrinsic { dst, .. } => *dst = Some(nd),
        Inst::Store { .. } | Inst::MemCpy { .. } | Inst::MemSet { .. } => {}
    }
}

/// Route every value in `dh` that `readers` reference through a PARAMETER of
/// `succ`, fed by the header and by its clone.
///
/// The header itself is never a reader here: its terminator arguments are what
/// feed the new parameters, so rewriting them would tie each parameter to
/// itself. Nor is the guard, for the same reason.
/// Does this block read this value, in an instruction or its terminator?
fn uses_value(f: &Func, b: BlockId, v: ValueId) -> bool {
    let mut hit = false;
    let mut see = |o: Operand| {
        if o == Operand::Val(v) {
            hit = true;
        }
    };
    for inst in &f.blocks[b as usize].insts {
        inst.uses(&mut see);
    }
    f.blocks[b as usize].term.uses(&mut see);
    hit
}

/// A header value leaves through `exit` as a parameter, and every predecessor of
/// `exit` — all of them inside the loop, checked by the caller — supplies the
/// name IT can see: the guard its clone, the old header the value itself, and a
/// body block the new header's parameter (`rw_body`).
fn close_exit(
    f: &mut Func,
    dh: &[ValueId],
    map: &[Option<Operand>],
    rw_body: &[Option<Operand>],
    h: BlockId,
    g: BlockId,
    exit: BlockId,
    readers: &[BlockId],
    inloop: &[bool],
) {
    let mut rw: Vec<Option<Operand>> = vec![None; f.values.len()];
    let mut any = false;
    for &v in dh {
        if !readers.iter().any(|&b| uses_value(f, b, v)) {
            continue;
        }
        let preds: Vec<BlockId> = (0..f.blocks.len() as BlockId)
            .filter(|&p| {
                // `inloop` was sized before the guard block existed
                (p == g || inloop.get(p as usize).copied().unwrap_or(false))
                    && f.blocks[p as usize].term.succs().contains(&exit)
            })
            .collect();
        let k = f.blocks[exit as usize].params.len() as u32;
        let np = f.new_value(f.ty_of(v), Def::Param(exit, k));
        f.blocks[exit as usize].params.push(np);
        for p in preds {
            let arg = if p == g {
                map[v as usize].expect("a header definition has a clone")
            } else if p == h {
                Operand::Val(v)
            } else {
                rw_body[v as usize].unwrap_or(Operand::Val(v))
            };
            for t in f.blocks[p as usize].term.targets_mut() {
                if t.block == exit {
                    t.args.push(arg);
                }
            }
        }
        rw[v as usize] = Some(Operand::Val(np));
        any = true;
    }
    if !any {
        return;
    }
    for &b in readers {
        let blk = &mut f.blocks[b as usize];
        let repl = |o: &mut Operand| {
            if let Operand::Val(v) = *o {
                if let Some(n) = rw[v as usize] {
                    *o = n;
                }
            }
        };
        for inst in blk.insts.iter_mut() {
            inst.uses_mut(repl);
        }
        match &mut blk.term {
            Term::Br(x, ..) | Term::Switch(x, ..) | Term::GotoPtr(x, _) => repl(x),
            Term::Ret(Some(x)) => repl(x),
            _ => {}
        }
        for t in blk.term.targets_mut() {
            for a in t.args.iter_mut() {
                repl(a);
            }
        }
    }
}

fn reparam(
    f: &mut Func,
    dh: &[ValueId],
    map: &[Option<Operand>],
    h: BlockId,
    g: BlockId,
    succ: BlockId,
    readers: &[BlockId],
    forced: &[ValueId],
) -> Vec<Option<Operand>> {
    let mut rw: Vec<Option<Operand>> = vec![None; f.values.len()];
    let mut any = false;
    for &v in dh {
        let used = forced.contains(&v) || readers.iter().any(|&b| uses_value(f, b, v));
        if !used {
            continue;
        }
        let k = f.blocks[succ as usize].params.len() as u32;
        let np = f.new_value(f.ty_of(v), Def::Param(succ, k));
        f.blocks[succ as usize].params.push(np);
        for t in f.blocks[h as usize].term.targets_mut() {
            if t.block == succ {
                t.args.push(Operand::Val(v));
            }
        }
        let gv = map[v as usize].expect("a header definition has a clone");
        for t in f.blocks[g as usize].term.targets_mut() {
            if t.block == succ {
                t.args.push(gv);
            }
        }
        rw[v as usize] = Some(Operand::Val(np));
        any = true;
    }
    if !any {
        return rw;
    }
    for &b in readers {
        let blk = &mut f.blocks[b as usize];
        let repl = |o: &mut Operand| {
            if let Operand::Val(v) = *o {
                if let Some(n) = rw[v as usize] {
                    *o = n;
                }
            }
        };
        for inst in blk.insts.iter_mut() {
            inst.uses_mut(repl);
        }
        match &mut blk.term {
            Term::Br(x, ..) | Term::Switch(x, ..) | Term::GotoPtr(x, _) => repl(x),
            Term::Ret(Some(x)) => repl(x),
            _ => {}
        }
        for t in blk.term.targets_mut() {
            for a in t.args.iter_mut() {
                repl(a);
            }
        }
    }
    rw
}

/// Is every instruction in the header part of computing its exit condition?
///
/// The backward slice of the branch condition, taken inside the header, must
/// cover the whole block. A `while (*p)` header — a load and a compare — passes;
/// a header that also holds a loop body does not, and that is precisely an
/// already-rotated loop.
fn header_is_only_the_test(f: &Func, h: BlockId) -> bool {
    let cond = match &f.blocks[h as usize].term {
        Term::Br(c, ..) => *c,
        _ => return false,
    };
    let blk = &f.blocks[h as usize];
    let mut keep = vec![false; blk.insts.len()];
    // Where each value this block defines sits, so the slice can walk operands
    // back to their defining instruction without consulting `values`, whose def
    // records a mid-pass caller may not have refreshed.
    let mut at: std::collections::HashMap<ValueId, usize> = std::collections::HashMap::new();
    for (i, inst) in blk.insts.iter().enumerate() {
        if let Some(d) = inst.dst() {
            at.insert(d, i);
        }
    }
    let mut work: Vec<ValueId> = cond.val().into_iter().collect();
    while let Some(v) = work.pop() {
        let i = match at.get(&v) {
            Some(&i) => i,
            None => continue, // a parameter, or defined elsewhere
        };
        if keep[i] {
            continue;
        }
        keep[i] = true;
        blk.insts[i].uses(|o| {
            if let Operand::Val(x) = o {
                work.push(x);
            }
        });
    }
    keep.iter().all(|&x| x)
}
