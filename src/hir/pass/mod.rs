// The HIR pass ladder (MECHANISM.md §G4) — the tree-SSA half of gcc -O1, re-realized
// THEORY A7b — optimization, and what proving each pass requires
// on this architecture.
//
// Every pass in here is an HIR→HIR function shipping the commuting square
// ⟦f⟧ = ⟦P f⟧ (Law 3). The square is not a comment: `hir::tests` runs the WHOLE
// battery corpus through both sides of it, so a pass that changes an observable
// value fails `cargo test` before any machine layer exists to hide it.
//
// Order mirrors gcc -O1's `-ftree-*` sequence, bounded to a small fixpoint —
// each pass exposes work for the next (sccp folds a branch, cfg_simplify deletes
// the arm, gvn re-numbers what the deletion merged), and the rounds stop when a
// round changes nothing or the bound is reached.
pub mod cfg;
pub mod copyidiom;
pub mod vecprobe;
pub mod copyprobe;
pub mod dce;
pub mod fold;
pub mod gvn;
pub mod ifconv;
pub mod inline;
pub mod iv;
pub mod divmagic;
pub mod licm;
pub mod tailjump;
pub mod loopmem;
pub mod mem;
pub mod purity;
pub mod rotate;
pub mod scev;
#[cfg(test)]
mod tests;
pub mod sccp;
pub mod sink;
pub mod sroa;
pub mod unroll;
pub mod vrp;

use super::*;

/// THEORY A7b — the ladder's fixpoint bound
/// Rounds of the ladder. Three is gcc's own practical bound for -O1-class
/// pipelines: the interesting cascades (fold → branch removal → merge →
/// re-number) are two levels deep, and the third is the confirmation round that
/// usually changes nothing. It is a TERMINATION bound, not a correctness one —
/// each pass is individually meaning-preserving, so any number of rounds is
/// sound and this one only decides how much is left on the table.
pub const ROUNDS: u32 = 3;

pub fn run_module(m: &mut Module) {
    run_module_with(m, &std::collections::HashSet::new());
}

/// `pinned` names functions a STATIC INITIALIZER refers to (`static void (*p)() =
/// &f;`). HIR has no view of the data segment, so the caller supplies it — and
/// without it the inliner would delete a function the linker still needs.
pub fn run_module_with(m: &mut Module, pinned: &std::collections::HashSet<String>) {
    // The purity set is INTERPROCEDURAL, so it is computed on the module and
    // handed to the per-function ladder rather than rediscovered inside it. It
    // is recomputed after inlining, which changes both the call graph and the
    // set of functions.
    // FIRST, because it is what unfences the rows below: a block carrying a C
    // label is refused by cfg_simplify's threading and merging identities and by
    // the inliner, and on a function with no `&&label`, no `goto *e` and no VLA
    // that label is unobservable (cfg.rs, SQUARE labels_are_not_observable).
    if on("delabel") {
        timed("delabel", || cfg::delabel(m, pinned));
    }
    let ro = readonly(m);
    for f in m.funcs.iter_mut() {
        run_with(f, &ro);
    }
    // Inlining is the one INTERPROCEDURAL row, so it runs between two
    // intra-procedural sweeps rather than inside one: the callee must already be
    // optimized when it is spliced in (its locals promoted, its constants
    // folded), and the caller must be re-optimized afterwards, because a call
    // replaced by a body is exactly the shape the other rows feed on.
    // ON BY DEFAULT. It was off for one day (2026-08-28) because it multiplied
    // the cost of the superlinear passes below it — sqlite 22 s against 4.7 s
    // without — and the fuzzing campaigns that find miscompiles are gated on
    // compile time. The growth was not the row's: it was one of the row's three
    // rules, and that rule is gone (`inline.rs`, REFUSED THIRD RULE). What is
    // left grows sqlite by 2.0% and costs 2.2 s, so the row pays for itself
    // again. `ZCC_NOPASS=inline` turns it off.
    if inline_wanted() && on("inline") && timed("inline", || inline::run_module(m, pinned)) {
        let ro = readonly(m);
        for f in m.funcs.iter_mut() {
            run_with(f, &ro);
        }
    }
    report_ladder();
}

fn readonly(m: &Module) -> std::collections::HashSet<String> {
    match on("purecall") {
        true => {
            let r = purity::readonly_functions(m);
            if licm::residual_wanted() {
                eprintln!("RESIDUAL readonly={} of {}", r.len(), m.funcs.len());
            }
            r
        }
        false => std::collections::HashSet::new(),
    }
}

/// THEORY A7b  SQUARE ladder_is_idempotent_at_the_fixpoint — the ladder reaches
/// a fixpoint, and running it again changes nothing
pub fn run(f: &mut Func) {
    run_with(f, &std::collections::HashSet::new())
}

pub fn run_with(f: &mut Func, ro: &std::collections::HashSet<String>) {
    // Critical edges are split once, up front: sccp and gvn both want to place a
    // value on an edge, and a critical edge offers nowhere to put it.
    dom::split_critical_edges(f);
    // THE ANALYSIS LAYER (`hir::analysis`). `cfg`, `DomTree` and `LoopForest` are
    // owned here for the whole ladder instead of rebuilt at the head of each of
    // the twenty-four rows below, and INVALIDATED by the pass that changes the
    // function rather than recomputed by the one that reads it next.
    //
    // THE INVALIDATION IS CONSERVATIVE ON PURPOSE, and that is a first cut rather
    // than the answer: any pass reporting a change drops the whole handle, even
    // where it rewrote instructions and left every edge alone. What it collects
    // is the sharing WITHIN a pass and across the rows that report nothing, which
    // is most of them after round one. Narrowing a row's declaration to "I did
    // not touch the CFG" is a separate row, and `analysis::checking` is what
    // makes each such claim provable rather than asserted.
    let mut a = Analyses::new();
    let a = &mut a;
    for _ in 0..ROUNDS {
        let mut changed = false;
        if on("cfg") {
            if timed("cfg", || cfg::run(f)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("sroa") {
            if timed("sroa", || sroa::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("sccp") {
            if timed("sccp", || sccp::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // AFTER sccp: a value the point lattice settles is an interval of one,
        // so the cheaper analysis goes first and this one reasons about what is
        // left. BEFORE gvn, which is what removes the comparisons this decides.
        if on("vrp") {
            if timed("vrp", || vrp::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("gvn") {
            if fold::canon(f) | fold::narrow_mask(f) {
                changed = true;
                a.invalidate();
            }
            if timed("gvn", || gvn::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("mem") {
            if timed("mem", || mem::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("ifconv") {
            if timed("ifconv", || ifconv::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // Rotation runs BEFORE licm, not after: a bottom-tested loop is what
        // makes "the loop runs at least once" structural rather than
        // arithmetic, and that is the fence licm's call hoist was refusing on.
        if on("rotate") {
            if timed("rotate", || rotate::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // BEFORE licm and the IV rows: unrolling decides a guard and deletes a
        // back edge, so what it leaves is straight-line code those rows can see
        // through — and a loop it removes is one they no longer have to reason
        // about.
        if on("unroll") {
            if timed("unroll", || unroll::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("licm") {
            if timed("licm", || licm::run_with(f, ro, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // BEFORE gvn's next turn and before dce: the sequence divmagic writes is
        // ordinary arithmetic, so everything downstream folds, numbers and
        // schedules it like any other. AFTER licm, so a divisor that was loop
        // invariant has already moved and the multiply lands where the divide was.
        if on("divmagic") {
            if timed("divmagic", || divmagic::run(f)) {
                changed = true;
                a.invalidate();
            }
        }
        // AFTER licm, which is what moves the address computation out of the loop
        // — the invariance this pass requires of it is a property of that shape.
        if on("loopmem") {
            if timed("loopmem", || loopmem::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // LAST of the loop rows: it duplicates blocks, so every analysis above it
        // sees the smaller CFG, and the copies it leaves are ordinary code that
        // `gvn` and `dce` clean up on the next turn.
        if on("copyidiom") {
            if timed("copyidiom", || copyidiom::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("tailjump") {
            if timed("tailjump", || tailjump::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // AFTER licm and rotation: the loop must already be in its final shape,
        // because the recurrence this reads is a property of that shape. Before
        // dce, which is what removes the address chain it replaces.
        if on("iv") {
            if timed("iv", || iv::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // Widening is a SEPARATE row from the pointer walk above and is ON: it
        // removes the per-iteration `sxtw` that stands between an `a[i]` loop
        // and gcc's (§13l).
        if on("widen") {
            if timed("iv", || iv::widen(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // IV SUBSTITUTION is a third row again (§13q ii): `widen` removes a
        // `sxtw` from an `a[i]` loop, this removes the ADD that rebuilds
        // `inv + k` every iteration by making that value the counter. After
        // `widen`, because a loop whose counter already feeds a `sext` belongs
        // to that row and this one refuses it.
        if on("subst") {
            if timed("iv", || iv::substitute(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        // LAST of the IV rows, because it DESTROYS the index: once the loop
        // counts down, `widen` and `substitute` have nothing left to recognize.
        if on("countdown") {
            if timed("iv", || iv::countdown(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("sink") {
            if timed("sink", || sink::run(f, a)) {
                changed = true;
                a.invalidate();
            }
        }
        if on("dce") {
            if timed("dce", || dce::run(f)) {
                changed = true;
                a.invalidate();
            }
        }
        if !changed {
            break;
        }
    }
    if on("cfg") {
        cfg::run(f);
    }
    copyprobe::census(f, a);
    vecprobe::census(f, a);
    if iv::fv_wanted() {
        iv::fv_opportunity(f, a);
    }
}

/// `ZCC_NOPASS=gvn,mem` disables the named rows. This is a BISECTION tool, not a
/// tuning knob: when a differential suite reports a wrong answer, the first
/// question is which theorem's square is false, and answering it by rebuilding
/// the compiler six times is the slow path Law 2 warns about. No shipped
/// configuration reads it.
/// Is the interprocedural row wanted? Off by default — see `run_module_with`.
///
/// A battery that MEASURES inlining asks for it in the thread it runs in, not
/// through the environment: `cargo test` runs tests in parallel in one process,
/// so an environment variable is shared by every test at once and one battery
/// would silently decide another's input. Same reason `regalloc::promote` keeps
/// its switch this way.
thread_local! {
    /// THEORY A7b — the interprocedural row's switch, an instrument half in the
    /// sense `promote.rs` carries: a battery that measures a row WITHOUT
    /// inlining turns it off in its own thread, so a single-function square is
    /// read on the function it was written for. The row itself is on by
    /// default (`run_module_with` above).
    static INLINE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn inline_wanted() -> bool {
    INLINE.with(|c| c.get())
}

#[cfg(test)]
pub(crate) fn set_inline(on: bool) {
    INLINE.with(|c| c.set(on));
}

fn on(name: &str) -> bool {
    match std::env::var("ZCC_NOPASS") {
        Ok(v) => !v.split(',').any(|x| x == name),
        Err(_) => true,
    }
}


/// WHAT EACH ROW OF THE LADDER COSTS, under `ZCC_TIME`.
///
/// Turning a row off and timing the difference does not answer this: a row that
/// is off leaves more code for every row after it, so the deltas are confounded
/// and can even come out negative (removing `cfg` makes this compile SLOWER).
/// The honest instrument is a clock around each row, summed over every function.
/// It is off unless `ZCC_TIME` is set, and the row below it reads the same
/// environment variable the pipeline's stage timers already use.
thread_local! {
    /// THEORY A7b — instrument half, as `RECONSTRUCT` and `PRUNE` in `spill.rs`.
    /// It records where the ladder's time went and decides nothing; a row that
    /// is not measured is a row optimized by guess, and this session spent
    /// itself proving that on/off deltas confound one pass with the next.
    static LADDER: std::cell::RefCell<Vec<(&'static str, std::time::Duration)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn timed<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if std::env::var_os("ZCC_TIME").is_none() {
        return f();
    }
    let t = std::time::Instant::now();
    let r = f();
    let d = t.elapsed();
    LADDER.with(|l| {
        let mut l = l.borrow_mut();
        match l.iter_mut().find(|(n, _)| *n == name) {
            Some((_, acc)) => *acc += d,
            None => l.push((name, d)),
        }
    });
    r
}

/// Print the per-row totals, worst first. Called once the module is done.
pub fn report_ladder() {
    if std::env::var_os("ZCC_TIME").is_none() {
        return;
    }
    LADDER.with(|l| {
        let mut rows = l.borrow().clone();
        rows.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
        for (n, d) in rows {
            eprintln!("[ladder] {:<10} {:>8.1} ms", n, d.as_secs_f64() * 1e3);
        }
    });
}

// ── shared plumbing every pass needs ───────────────────────────────────────

/// Recompute every `ValueInfo::def` from where the definition actually sits.
/// Any pass that moves, deletes or inserts an instruction calls this instead of
/// maintaining def records by hand — the verifier checks the record against the
/// layout, so one mechanical recomputation is both shorter and impossible to get
/// subtly wrong. Values whose definition has disappeared keep a stale record and
/// are simply never reached again (the same status a DCE'd value has).
pub fn refresh_defs(f: &mut Func) {
    for bi in 0..f.blocks.len() {
        refresh_block_defs(f, bi as BlockId);
    }
}

/// Re-stamp the `Def` of every value one block defines. Moving an instruction
/// between two blocks only invalidates those two — the source (every inst after
/// the removed one shifts index) and the destination (the appended inst) — so a
/// mover that knows them updates just those, not the whole function. `refresh_defs`
/// over the entire `Func` after each single hoist was O(insts) PER hoist, minutes
/// on a large function that hoists many invariants (yarpgen `test`).
pub fn refresh_block_defs(f: &mut Func, bi: BlockId) {
    let b = bi as usize;
    for k in 0..f.blocks[b].params.len() {
        let p = f.blocks[b].params[k];
        f.values[p as usize].def = Def::Param(bi, k as u32);
    }
    for i in 0..f.blocks[b].insts.len() {
        if let Some(d) = f.blocks[b].insts[i].dst() {
            f.values[d as usize].def = Def::Inst(bi, i as u32);
        }
    }
}

/// Blocks whose IDENTITY is observable, so no pass may delete, merge or thread
/// them. Two EXT(gcc) constructs pin a block: `&&label` takes its address as a
/// datum (`Sym::Label`), and `goto *e` names it as a CFG successor. The entry is
/// pinned because the ABI materializes parameters there.
pub fn pinned(f: &Func) -> Vec<bool> {
    let mut p = vec![false; f.blocks.len()];
    p[f.entry as usize] = true;
    for b in &f.blocks {
        for inst in &b.insts {
            if let Inst::SymAddr { sym: Sym::Label(t), .. } = inst {
                p[*t as usize] = true;
            }
        }
        if let Term::GotoPtr(_, bs) = &b.term {
            for &t in bs {
                p[t as usize] = true;
            }
        }
    }
    p
}

/// Apply a value substitution everywhere: instruction operands, terminator
/// operands and block arguments. `map[v]` is the value `v` becomes; the walk is
/// transitive (a chain v→w→x resolves to x) and stops at a self-map.
pub fn rewrite_values(f: &mut Func, map: &[Option<Operand>]) {
    let resolve = |o: Operand| -> Operand {
        let mut cur = o;
        // A substitution chain is acyclic by construction (a value is only ever
        // mapped to one that already existed), but the bound keeps a corrupted
        // map from hanging the compiler instead of failing a test.
        for _ in 0..64 {
            match cur {
                Operand::Val(v) => match map.get(v as usize).and_then(|x| *x) {
                    Some(n) if n != cur => cur = n,
                    _ => return cur,
                },
                _ => return cur,
            }
        }
        cur
    };
    for b in f.blocks.iter_mut() {
        for inst in b.insts.iter_mut() {
            inst.uses_mut(|o| *o = resolve(*o));
        }
        match &mut b.term {
            Term::Br(c, ..) | Term::Switch(c, ..) | Term::GotoPtr(c, _) => *c = resolve(*c),
            Term::Ret(Some(v)) => *v = resolve(*v),
            _ => {}
        }
        for t in b.term.targets_mut() {
            for a in t.args.iter_mut() {
                *a = resolve(*a);
            }
        }
    }
}
