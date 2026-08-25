// rotate — loop rotation / header copying (REARCH §13c row 2).
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

/// The largest header worth copying, in instructions. This is gcc's own -O1
/// number for the same transform (`--param max-loop-header-insns`, default 20,
/// read by `-ftree-ch`) rather than one picked here: the transform trades a
/// STATIC copy of the header for a DYNAMIC branch per iteration, so no bound
/// falls out of the theorem — it is a size/speed exchange rate, and the honest
/// thing is to take the reference compiler's rate and say where it came from.
const MAX_HEADER_INSTS: usize = 20;

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

fn rotate_one(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let pin = pinned(f);
    // innermost first: the inner loop is the hot one
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        if try_rotate(f, &c, &dt, &lf, li, &pin) {
            return true;
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
) -> bool {
    let h = lf.loops[li].header;
    if pin[h as usize] || !f.blocks[h as usize].labels.is_empty() {
        return false;
    }
    if f.blocks[h as usize].insts.len() > MAX_HEADER_INSTS {
        return false;
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
        return false;
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
        return false;
    }
    if lf.loops[li].latches.contains(&h) {
        return false;
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
        return false;
    }
    // The header must BE the exit test: a two-way branch with one arm inside.
    let (t_in, t_out) = match &f.blocks[h as usize].term {
        Term::Br(_, a, b) => match (inloop[a.block as usize], inloop[b.block as usize]) {
            (true, false) => (a.block, b.block),
            (false, true) => (b.block, a.block),
            _ => return false,
        },
        _ => return false,
    };
    // The block the test falls into becomes the new header, and it must be able
    // to take the guard's edge as a second predecessor without merging two
    // unrelated paths. Critical edges are split before the ladder runs, so a
    // single predecessor is the normal state here.
    if c.preds[t_in as usize].len() != 1 {
        return false;
    }
    // Every entry edge must be redirectable, so none may come from a computed
    // goto (whose successors are a set, not a target list).
    for &p in &c.preds[h as usize] {
        if matches!(f.blocks[p as usize].term, Term::GotoPtr(..)) {
            return false;
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
    if !out_users.is_empty() {
        // The exit block must be the one place those uses are reached through,
        // and it must be able to take a parameter — which means the header is
        // its only predecessor, so no other edge owes the new argument.
        if c.preds[t_out as usize].len() != 1 {
            return false;
        }
        if !out_users.iter().all(|&b| dt.dominates(t_out, b)) {
            return false;
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
    reparam(f, &dh, &map, h, g, t_in, &in_body);
    reparam(f, &dh, &map, h, g, t_out, &out_users);

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
    true
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
fn reparam(
    f: &mut Func,
    dh: &[ValueId],
    map: &[Option<Operand>],
    h: BlockId,
    g: BlockId,
    succ: BlockId,
    readers: &[BlockId],
) {
    let mut rw: Vec<Option<Operand>> = vec![None; f.values.len()];
    let mut any = false;
    for &v in dh {
        let mut used = false;
        for &b in readers {
            let mut see = |o: Operand| {
                if o == Operand::Val(v) {
                    used = true;
                }
            };
            for inst in &f.blocks[b as usize].insts {
                inst.uses(&mut see);
            }
            f.blocks[b as usize].term.uses(&mut see);
        }
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
