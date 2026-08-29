// licm — loop-invariant code motion (MECHANISM.md §G4 row 8).
// THEORY A7b — optimization: this pass ships its commuting square
//
// UNCONDITIONAL, per §4: no register-pressure guard. The allocator owns pressure
// and now actually can — R2.2's Belady spiller splits a live range instead of
// failing on it — so a pass that refuses to hoist "in case the allocator cannot
// cope" would be pricing in a weakness that no longer exists.
//
// COMMUTING SQUARE. Moving an instruction from a loop body to its preheader
// preserves ⟦f⟧ when three things hold, and each is checked rather than assumed:
//   (1) INVARIANCE — every operand is defined outside the loop (or was itself
//       hoisted). Operands are SSA values, so "defined outside" is exactly "the
//       same value on every iteration".
//   (2) PURITY — `Effect::Pure`. A read could be hoisted too when nothing in the
//       loop writes, but a hoisted read may FAULT on a path that never entered
//       the loop, and proving ≥1 iteration is the rotation theorem's job (R2.4).
//       Recorded as residual, not silently taken.
//   (3) TRAP-FREEDOM — the one pure instruction that can fault is division, so
//       it moves only when the divisor is a non-zero literal. C99 6.5.5p5 makes
//       division by zero undefined, and hoisting one would move the fault to a
//       path the program never took.
// Dominance is preserved by construction: the preheader dominates the header,
// which dominates the whole body, so a use inside the loop is still dominated by
// a definition placed there.
use super::*;
use std::collections::HashSet;

/// THEORY A7b  SQUARE licm_hoists_an_invariant_expression_out_of_the_loop — invariance + the four fences
pub fn run(f: &mut Func, a: &mut Analyses) -> bool {
    run_with(f, &HashSet::new(), a)
}

/// `readonly` is the interprocedural purity set (`pass/purity.rs`). When it is
/// empty this is exactly the pass above; when it is not, a CALL becomes a
/// hoistable term too — see `hoist_call` for the four fences that licence it.
pub fn run_with(f: &mut Func, readonly: &HashSet<String>, a: &mut Analyses) -> bool {
    let mut changed = false;
    // Preheaders first: creating one changes the CFG, so the analyses are rebuilt
    // afterwards and the motion itself runs on a stable graph.
    if preheaders(f, a) {
        changed = true;
        a.invalidate();
    }
    let (c, dt, lf) = a.all(f);
    if lf.loops.is_empty() {
        return changed;
    }
    // where each value is defined, as a block
    let def_blk = |f: &Func, v: ValueId| -> Option<BlockId> {
        match f.values[v as usize].def {
            Def::FuncParam(_) => None,
            Def::Inst(b, _) | Def::Param(b, _) => Some(b),
        }
    };
    let _ = &def_blk;
    // innermost first: hoisting out of an inner loop exposes motion in the outer
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        let body: Vec<BlockId> = lf.loops[li].body.clone();
        let header = lf.loops[li].header;
        let pre = match preheader_of(f, &c, &dt, header) {
            Some(p) => p,
            None => continue,
        };
        // THE LOAD FENCE (2026-08-28). A load is not `Effect::Pure`, so the scalar
        // hoist above never moved one — and an invariant load is what a dispatch
        // loop pays most for: `n7_nested_subq` re-reads the same two entries of a
        // static function-pointer table on every iteration, and gcc -O1 holds one
        // of them in a register for the whole loop.
        //
        // Two conditions make the motion sound, and both are the ones the pure-call
        // hoist already states for the same reasons:
        //
        //   MEMORY-CLEAN — nothing in the loop writes memory, the calls in it
        //   being read-only (`pass/purity.rs`). A load is a function OF the memory
        //   state, so a fixed state is what makes iteration n's result iteration
        //   0's result.
        //
        //   GUARANTEED EXECUTION — the load's block dominates every latch and the
        //   loop runs at least once, so the address is one the original program
        //   dereferenced anyway. Without it a hoisted load is a speculated
        //   dereference, which is a fault the source never had.
        let mut inloop = vec![false; f.blocks.len()];
        for &b in &body {
            inloop[b as usize] = true;
        }
        let clean = body.iter().all(|&b| {
            f.blocks[b as usize].insts.iter().all(|inst| match inst.effect() {
                Effect::Pure | Effect::Read => true,
                _ => matches!(inst,
                    Inst::Call { callee: Callee::Direct(n), sret: None, .. } if readonly.contains(n)),
            })
        });
        let latches = lf.loops[li].latches.clone();
        let entered = clean && enters_body(f, &c, pre, header, &inloop);
        loop {
            let mut moved = None;
            'scan: for &b in &body {
                // Only a block the header DOMINATES may give up an instruction:
                // in an irreducible region the natural-loop body can contain a
                // block reachable without passing the header, and code moved out
                // of one would no longer dominate its uses.
                if b == pre || !dt.dominates(header, b) {
                    continue;
                }
                // a load may leave only from a block the first iteration must
                // pass through on its way to the back edge
                let loads_ok =
                    entered && (b == header || latches.iter().all(|&l| dt.dominates(b, l)));
                for i in 0..f.blocks[b as usize].insts.len() {
                    if hoistable(f, &f.blocks[b as usize].insts[i], pre, &dt, &def_blk, loads_ok) {
                        moved = Some((b, i));
                        break 'scan;
                    }
                }
            }
            let (b, i) = match moved {
                Some(x) => x,
                None => break,
            };
            let inst = f.blocks[b as usize].insts.remove(i);
            f.blocks[pre as usize].insts.push(inst);
            // Only `b` (indices after `i` shifted) and `pre` (the appended inst)
            // changed — scoped refresh, not O(function) per hoist (see mod.rs).
            super::refresh_block_defs(f, b);
            super::refresh_block_defs(f, pre);
            changed = true;
        }
        while hoist_call(f, &c, &dt, &lf, li, pre, readonly) {
            changed = true;
        }
    }
    changed
}

// ── the invariant PURE-CALL hoist (MECHANISM.md Part F row 1) ──────────────────────
//
// COMMUTING SQUARE. Moving `x = g(a₁…aₙ)` from a loop body to its preheader
// preserves ⟦f⟧ when four things hold, and none of them is assumed:
//
//   (1) PURITY — `g` is read-only (`pass/purity.rs`): the call itself writes
//       nothing, so executing it earlier changes no state. Its RESULT still
//       depends on memory, which is what fence (3) is for.
//   (2) INVARIANCE — every argument's definition dominates the preheader, so the
//       call is applied to the same arguments on every iteration. Exactly the
//       rule the scalar hoist above uses, for exactly the same reason.
//   (3) MEMORY-CLEAN — no instruction anywhere in the loop writes memory (the
//       only calls left in it are themselves read-only). A read-only function is
//       a function OF the memory state, so equal arguments give equal results
//       only while that state is fixed; a single store in the loop would break
//       the equality and no dominance argument would notice.
//   (4) GUARANTEED EXECUTION — the call runs on the first iteration anyway, so
//       running it in the preheader adds no execution that the original did not
//       perform. This is the fence that stops a fault or a non-terminating
//       callee from being speculated onto a path that never took it, and it is
//       three conditions at once: the loop is entered at least once — free when
//       the call sits in the HEADER, since the preheader's only successor is the
//       header, and otherwise decided by evaluating the header's test under the
//       preheader's own edge arguments; the call's
//       block dominates every latch (the first iteration cannot reach the back
//       edge without it); and it dominates every other block the loop can be
//       left from (the first iteration cannot escape without it either). The
//       header is excused from the last, because leaving THERE is the loop test
//       the ≥1-trip condition has already decided.
//
// Non-termination deserves its own line, because purity does not imply it. If
// something in the loop could diverge BEFORE the call, hoisting would turn a
// hang into whatever the callee does. Fence (4) plus the memory-clean fence
// leave only two ways to diverge ahead of the call — another call, or a nested
// loop — so both are refused among the blocks that may precede it. A FAULT ahead
// of the call needs no fence: a first iteration that faults is undefined
// behaviour (C99 6.5.3.2p4), and undefined behaviour licences any continuation.
/// `ZCC_RESIDUAL=1` names, per refused loop, WHICH fence refused it. Law 4 asks
/// for the residual of every shipped theorem — the sites where it could fire and
/// did not — and answering that by reading the pass is exactly the guesswork Law
/// 2 forbids. Read once: the fences run per loop, and an environment lookup in
/// that position would be a measurement that changes what it measures.
pub fn residual_wanted() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_RESIDUAL").is_ok())
}

fn hoist_call(
    f: &mut Func,
    c: &dom::Cfg,
    dt: &dom::DomTree,
    lf: &dom::LoopForest,
    li: usize,
    pre: BlockId,
    readonly: &HashSet<String>,
) -> bool {
    if readonly.is_empty() {
        return false;
    }
    let resid = residual_wanted();
    let mut note = |why: &str| {
        if resid {
            let n: usize = lf.loops[li]
                .body
                .iter()
                .map(|&b| {
                    f.blocks[b as usize]
                        .insts
                        .iter()
                        .filter(|i| matches!(i, Inst::Call { callee: Callee::Direct(x), .. } if readonly.contains(x)))
                        .count()
                })
                .sum();
            if n > 0 {
                eprintln!("RESIDUAL refused {} ({} candidate calls)", why, n);
            }
        }
    };
    let header = lf.loops[li].header;
    let depth = lf.loops[li].depth;
    let latches = lf.loops[li].latches.clone();
    let mut inloop = vec![false; f.blocks.len()];
    for &b in &lf.loops[li].body {
        inloop[b as usize] = true;
    }
    // (3) memory-clean, over the WHOLE body: a store after the call changes what
    // the NEXT iteration's call would have read.
    for &b in &lf.loops[li].body {
        for inst in &f.blocks[b as usize].insts {
            match inst.effect() {
                Effect::Pure | Effect::Read => {}
                _ => match inst {
                    Inst::Call { callee: Callee::Direct(n), sret: None, .. }
                        if readonly.contains(n) => {}
                    _ => {
                        note("memory-clean");
                        return false;
                    }
                },
            }
        }
    }
    // The FIRST call in the body, in reverse postorder. Within one iteration the
    // body is acyclic (back edges only reach the header), so reverse postorder
    // is an execution order: nothing that may run before this call is a call.
    let mut site = None;
    'scan: for &b in c.rpo.iter().filter(|&&b| inloop[b as usize]) {
        for i in 0..f.blocks[b as usize].insts.len() {
            if matches!(f.blocks[b as usize].insts[i], Inst::Call { .. }) {
                site = Some((b, i));
                break 'scan;
            }
        }
    }
    let (b, i) = match site {
        Some(x) => x,
        None => return false,
    };
    // (4b) the call is REACHED on the first iteration. When it sits in the
    // header there is nothing to prove — the preheader's only successor is the
    // header, and the call precedes the header's terminator, so entering the
    // loop at all runs it. That is the ordinary shape AFTER rotation, and it is
    // why rotation was the row that unblocked this one. Otherwise the loop must
    // be shown to run at least once, by evaluating the header's test under the
    // arguments the preheader's own edge carries.
    if b != header && !enters_body(f, c, pre, header, &inloop) {
        note("trip-count");
        return false;
    }
    // (4c) the first iteration cannot finish WITHOUT the call: every way onward
    // from the body passes through it. There are exactly two ways onward — round
    // the back edge, or out of the loop — so every latch and every EXITING block
    // must be dominated by the call's block. The header is excused from the
    // second: its exit is the loop test, and (4b) has just shown that test lets
    // the first iteration in.
    //
    // This is deliberately weaker than "the header is the only exit", which is
    // what a first cut writes. Measured on sqlite, that stronger form refused
    // 1,123 of 1,816 candidate loops — every loop carrying a `break` or an early
    // `return` after the call — and every one of them was a Law-4 category-(b)
    // truncation rather than a boundary, since a `break` REACHED through the
    // call proves the call ran.
    if !latches.iter().all(|&l| dt.dominates(b, l)) {
        note("conditional-call");
        return false;
    }
    for &x in &lf.loops[li].body {
        if x == header || dt.dominates(b, x) {
            continue;
        }
        let exits = match &f.blocks[x as usize].term {
            Term::Ret(_) | Term::Unreachable => true,
            t => t.succs().iter().any(|&s| !inloop[s as usize]),
        };
        if exits {
            note("early-exit-before-call");
            return false;
        }
    }
    // No nested loop among the blocks that may precede the call: a block the
    // call dominates certainly runs after it, and every other body block may run
    // before it.
    for &x in &lf.loops[li].body {
        if !dt.dominates(b, x) && lf.depth[x as usize] != depth + 1 {
            note("nested-loop-before");
            return false;
        }
    }
    // (2) invariance
    let inst = f.blocks[b as usize].insts[i].clone();
    let mut ok = true;
    inst.uses(|o| {
        if let Operand::Val(v) = o {
            let db = match f.values[v as usize].def {
                Def::FuncParam(_) => return,
                Def::Inst(x, _) | Def::Param(x, _) => x,
            };
            if !dt.dominates(db, pre) {
                ok = false;
            }
        }
    });
    if !ok {
        note("variant-args");
        return false;
    }
    f.blocks[b as usize].insts.remove(i);
    f.blocks[pre as usize].insts.push(inst);
    // Only two blocks moved: `b`, whose instructions after `i` shifted down, and
    // `pre`, which gained one at the end. The whole-function version costs O(N)
    // per hoisted call inside a `while hoist_call(..)` loop — the same defect the
    // scalar hoist above already had, and fixed the same way.
    super::refresh_block_defs(f, b);
    super::refresh_block_defs(f, pre);
    true
}

/// Does control reach a block INSIDE the loop the first time the header runs?
///
/// The header's parameters are bound to the arguments the preheader's edge
/// carries, the header's own instructions are constant-folded under that
/// binding, and the terminator is then read. This is a one-iteration
/// interpretation, not a range analysis: it answers the question the ≥1-trip
/// fence asks (`for (k = 0; k < 800; k++)`) and returns false whenever the entry
/// test is not decidable, which is the safe answer.
fn enters_body(f: &Func, c: &dom::Cfg, pre: BlockId, header: BlockId, inloop: &[bool]) -> bool {
    let args = match &f.blocks[pre as usize].term {
        Term::Jmp(t) if t.block == header => t.args.clone(),
        _ => return false,
    };
    if args.len() != f.blocks[header as usize].params.len() {
        return false;
    }
    let mut known: Vec<Option<Operand>> = vec![None; f.values.len()];
    for (k, &p) in f.blocks[header as usize].params.iter().enumerate() {
        known[p as usize] = resolve(f, c, pre, args[k], 4);
    }
    fn get(known: &[Option<Operand>], o: Operand) -> Operand {
        match o {
            Operand::Val(v) => known[v as usize].unwrap_or(o),
            k => k,
        }
    }
    for inst in &f.blocks[header as usize].insts {
        let mut cl = inst.clone();
        cl.uses_mut(|o| *o = get(&known, *o));
        if let (Some(d), Some(r)) = (cl.dst(), fold::fold_inst(&cl)) {
            if !matches!(r, Operand::Val(_)) {
                known[d as usize] = Some(r);
            }
        }
    }
    match &f.blocks[header as usize].term {
        // No test here at all: reaching the header IS reaching the body.
        Term::Jmp(t) => inloop[t.block as usize],
        Term::Br(cond, t, e) => match get(&known, *cond) {
            Operand::Imm(k) => inloop[(if k != 0 { t } else { e }).block as usize],
            _ => false,
        },
        _ => false,
    }
}

/// The constant `o` denotes on entry to `blk`, seeing through block parameters
/// whose incoming arguments all agree. `fuel` bounds the walk; a preheader that
/// forwards the header's parameters (the one `preheaders` builds) needs one
/// level, and nothing sensible needs four.
fn resolve(f: &Func, c: &dom::Cfg, blk: BlockId, o: Operand, fuel: u32) -> Option<Operand> {
    let v = match o {
        Operand::Val(v) => v,
        k => return Some(k),
    };
    if fuel == 0 {
        return None;
    }
    let k = f.blocks[blk as usize].params.iter().position(|&p| p == v)?;
    let mut got: Option<Operand> = None;
    if c.preds[blk as usize].is_empty() {
        return None;
    }
    for &p in &c.preds[blk as usize] {
        for t in f.blocks[p as usize].term.targets() {
            if t.block != blk {
                continue;
            }
            let r = resolve(f, c, p, *t.args.get(k)?, fuel - 1)?;
            match got {
                None => got = Some(r),
                Some(x) if x == r => {}
                _ => return None,
            }
        }
    }
    got
}

fn hoistable(
    f: &Func,
    inst: &Inst,
    pre: BlockId,
    dt: &dom::DomTree,
    def_blk: &dyn Fn(&Func, ValueId) -> Option<BlockId>,
    loads_ok: bool,
) -> bool {
    match inst.effect() {
        Effect::Pure => {}
        // a non-volatile load, under the caller's two fences (C99 6.7.3: a
        // volatile access may not be moved at all)
        Effect::Read if loads_ok && matches!(inst, Inst::Load { vol: false, .. }) => {}
        _ => return false,
    }
    // (3) the only pure instruction that can fault
    if let Inst::Bin { op, b, .. } = inst {
        if matches!(op, BinOp::SDiv | BinOp::UDiv | BinOp::SRem | BinOp::URem)
            && !matches!(b, Operand::Imm(k) if *k != 0)
        {
            return false;
        }
    }
    // (1) INVARIANCE, in the form that is actually needed: every operand's
    // definition must DOMINATE the preheader. "Defined outside the loop" is the
    // usual phrasing and is equivalent for a reducible loop, but it is not
    // equivalent in general — a block outside the natural-loop body may sit on a
    // path that never reaches the preheader — and dominance is the property the
    // verifier checks, so it is the property to test.
    let mut ok = true;
    inst.uses(|o| {
        if let Operand::Val(v) = o {
            if let Some(db) = def_blk(f, v) {
                if !dt.dominates(db, pre) {
                    ok = false;
                }
            }
        }
    });
    ok
}

/// The block that falls into `header` from outside the loop, when there is
/// exactly one such edge and its source has this header as its only successor.
///
/// The dominator tree is HANDED IN rather than built. Building one here costs a
/// whole-function analysis per loop, for a tree the caller already holds and
/// which cannot have changed — `preheaders` runs before the loop below and
/// nothing inside it touches the graph.
fn preheader_of(f: &Func, c: &dom::Cfg, dt: &dom::DomTree, header: BlockId) -> Option<BlockId> {
    let _ = f;
    let outside: Vec<BlockId> = c.preds[header as usize]
        .iter()
        .copied()
        .filter(|&p| !dt.dominates(header, p))
        .collect();
    match outside.as_slice() {
        [p] if c.succs[*p as usize].len() == 1 => Some(*p),
        _ => None,
    }
}

/// Give every loop header a preheader: one block outside the loop through which
/// every entry edge passes, with the header as its only successor. It is where
/// hoisted code goes, and it is the only structural change this pass makes.
///
/// The header may take PARAMETERS, and different entry edges pass different
/// arguments — so the preheader takes the same parameters and forwards them. No
/// value changes, no order changes: ⟦f⟧ = ⟦preheader f⟧ by the same argument as
/// critical-edge splitting.
fn preheaders(f: &mut Func, a: &mut Analyses) -> bool {
    let (c, dt, lf) = a.all(f);
    let mut changed = false;
    for l in &lf.loops {
        let h = l.header;
        let outside: Vec<BlockId> = c.preds[h as usize]
            .iter()
            .copied()
            .filter(|&p| !dt.dominates(h, p))
            .collect();
        if outside.is_empty() {
            continue; // unreachable header
        }
        if outside.len() == 1 && c.succs[outside[0] as usize].len() == 1 {
            continue; // already has one
        }
        let pre = f.new_block();
        f.blocks[pre as usize].weight = f.blocks[h as usize].weight;
        let tys: Vec<Ty> = f.blocks[h as usize]
            .params
            .iter()
            .map(|p| f.ty_of(*p))
            .collect();
        let mut args = Vec::with_capacity(tys.len());
        for (k, t) in tys.iter().enumerate() {
            let v = f.new_value(*t, Def::Param(pre, k as u32));
            f.blocks[pre as usize].params.push(v);
            args.push(Operand::Val(v));
        }
        f.blocks[pre as usize].term = Term::Jmp(Target { block: h, args });
        for p in outside {
            let mut term = f.blocks[p as usize].term.clone();
            for t in term.targets_mut() {
                if t.block == h {
                    t.block = pre;
                }
            }
            f.blocks[p as usize].term = term;
        }
        changed = true;
    }
    if changed {
        refresh_defs(f);
    }
    changed
}
