// cost — the TIME dual of `cost = |MIR|` (MECHANISM.md R4.18, Law 3c).
//
// WHY THIS EXISTS, in one line each:
//
//   matmul   1.638x gcc -O1, instruction count IDENTICAL at seven
//   j3       1.940x, instruction count identical
//   d2       1.400x, six instructions against five
//
// `cost = |MIR|` is EXACT for size — one `MInst` is one machine instruction — and
// blind to time by that same construction. It scored both matmul loops at seven
// and one of them takes 64% longer. Law 3c says what to score instead: not how
// many instructions a loop holds, but how long its longest DEPENDENCE CHAIN is.
//
// THE MODEL. A loop's iteration cannot finish faster than its slowest recurrence:
// a value computed from itself one iteration later must wait for every latency
// on the path between. So for each loop-carried value — a header parameter `p`
// whose latch edge passes some `v` — the bound is the longest latency-weighted
// path from `p` to `v` through the body, and the loop's bound is the maximum over
// its parameters. That is the standard modulo-scheduling recurrence bound (Rau &
// Glaeser 1981), restricted to the one shape zcc needs: the body of a loop is
// acyclic once the back edges are removed, so "longest path" is a topological
// walk and not a search.
//
// WHAT IT DELIBERATELY DOES NOT MODEL: issue width, port conflicts, the
// reorder-window, cache misses, branch misprediction. Those need a machine model
// this project has no source for. The recurrence bound is a LOWER bound on
// cycles per iteration, and it is used the way `cost = |MIR|` is used — to
// compare two shapes of the same loop before either is built.
//
// THE LATENCIES ARE MEASURED, NOT INVENTED (`MEASURED M10`, `tests/bench/latency.sh`).
// Apple publishes no optimization guide for this core, so a transcribed table is
// not available; the numbers come from timing dependent chains, self-calibrated
// against a plain `add` so the clock cancels.
use super::*;
use crate::cfg::{Cfg, DomTree, LoopForest};

/// Cycles from `src` reaching `inst` to `inst`'s destination being available.
///
/// `MEASURED M10`, 2026-08-26. Every value here was timed; the ones that were
/// not are named in the `OPEN` list of that entry and take the ALU default,
/// which is the honest floor rather than a guess dressed as a fact.
pub fn latency(inst: &MInst, src: Reg) -> u32 {
    match inst {
        // A SHIFTED or EXTENDED register operand costs a second cycle, and it
        // costs it on the whole instruction rather than on that operand alone —
        // the chain was timed through the OTHER source and still read 2.02.
        // This is the row that owns j3_prefix_sum.
        MInst::Alu { b: Rhs::Shifted(..) | Rhs::Extended(..), .. } => 2,
        MInst::Alu { .. } => 1,
        // `madd` is TWO different latencies in one instruction, and the
        // difference is the whole reason `s += a*b` accumulation is not
        // multiply-bound: reaching the destination from a MULTIPLICAND takes the
        // multiplier's 3 cycles, but the ACCUMULATOR forwards late and takes 1.
        // Measured 3.02 and 1.00.
        MInst::Alu3 { c, .. } if src == *c => 1,
        MInst::Alu3 { .. } => 3,
        // An L1 hit, plain or register-offset: both timed 3.02. A miss is not
        // modelled — nothing here knows the working set.
        MInst::Load { .. } | MInst::Pair { load: true, .. } => 3,
        MInst::Cmp { .. } | MInst::CSel { .. } | MInst::CSet { .. } => 1,
        MInst::Ext { .. } | MInst::Bfx { .. } => 1,
        MInst::MovImm { .. } | MInst::FMovImm { .. } | MInst::Adrp { .. } | MInst::AddLo12 { .. } => 1,
        MInst::Copy { .. } => 1,
        // Unmeasured: the FP forms and the call. A call is not a latency, it is
        // a whole function, and a loop containing one is not recurrence-bound in
        // any sense this model can state — `recurrence` reports None for it.
        _ => 1,
    }
}

/// `sdiv`/`udiv` measured 7.05 — far off the ALU default, so they are named
/// rather than left to it.
fn div_latency(inst: &MInst) -> Option<u32> {
    match inst {
        MInst::Alu { op: AluOp::SDiv | AluOp::UDiv, .. } => Some(7),
        _ => None,
    }
}

fn lat(inst: &MInst, src: Reg) -> u32 {
    div_latency(inst).unwrap_or_else(|| latency(inst, src))
}

/// What the model can say about one loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bound {
    /// Cycles per iteration the loop-carried chains cannot beat.
    pub recurrence: u32,
    /// The longest wait before a LOAD's address is ready, within one iteration.
    ///
    /// A SECOND bound, and matmul is why it exists. Its accumulator recurrence is
    /// one cycle whichever way the address is built (`madd`'s accumulator operand
    /// forwards late), so a recurrence-only model scores the two shapes
    /// identically and reproduces `cost = |MIR|`'s exact blindness. What actually
    /// differed is that a 3-cycle multiply stood in front of a 3-cycle load,
    /// delaying every access and with it the memory-level parallelism the loop
    /// lives on. The model reports it separately rather than folding it into one
    /// number, because they bound different things.
    pub addr: u32,
}

/// What the model can say about one loop, or `None` when it holds something this
/// model cannot score (a call).
///
/// `None` is not a failure to be papered over with a guess — it is the model
/// declining to speak, which is what keeps its numbers worth reading.
pub fn recurrence(f: &MFunc, cfg: &Cfg, lf: &LoopForest, li: usize) -> Option<Bound> {
    let lp = &lf.loops[li];
    let inloop: Vec<bool> = {
        let mut v = vec![false; f.blocks.len()];
        for &b in &lp.body {
            v[b as usize] = true;
        }
        v[lp.header as usize] = true;
        v
    };
    // A call makes the iteration's length a property of another function.
    for b in 0..f.blocks.len() {
        if inloop[b] && f.blocks[b].insts.iter().any(|i| matches!(i, MInst::Call { .. })) {
            return None;
        }
    }
    let n = f.vregs.len();
    let idx = |r: Reg| -> Option<usize> {
        match r {
            Reg::V(v) => Some(v as usize),
            _ => None,
        }
    };
    let body: Vec<MBlockId> =
        cfg.rpo.iter().copied().filter(|&b| inloop[b as usize]).collect();

    // ONE LONGEST-PATH PASS PER LOOP-CARRIED VALUE, and the "per" is the whole
    // correctness of the thing. A recurrence is a CYCLE: the path from a header
    // parameter back to the value its own latch edge hands forward. A single
    // distance array seeded with every parameter at zero measures something
    // else — the longest path from ANY parameter — and that number is not a
    // bound on anything. Measured on `tests/bench/loops.c`, which chains six
    // accumulators through one iteration, it reported 16 cycles for a loop that
    // runs in about five. Seeding one parameter at a time and leaving every
    // other value unreachable gives the cycle, which is what bounds the loop.
    let longest_from = |seed: usize| -> Vec<Option<u32>> {
        let mut dist: Vec<Option<u32>> = vec![None; n];
        dist[seed] = Some(0);
        for &b in &body {
            for inst in &f.blocks[b as usize].insts {
                let mut ready: Option<u32> = None;
                inst.visit(&mut |r, c| {
                    if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                        if let Some(d) = idx(r).and_then(|u| dist[u]) {
                            let t = d.saturating_add(lat(inst, r));
                            ready = Some(ready.map_or(t, |x: u32| x.max(t)));
                        }
                    }
                });
                if let Some(t) = ready {
                    inst.visit(&mut |r, c| {
                        if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                            if let Some(d) = idx(r) {
                                dist[d] = Some(dist[d].map_or(t, |x: u32| x.max(t)));
                            }
                        }
                    });
                }
            }
        }
        dist
    };

    // The recurrence: for each header parameter, the length of its own cycle.
    let mut bound = 1;
    for (k, p) in f.blocks[lp.header as usize].params.iter().enumerate() {
        let seed = match idx(*p) {
            Some(v) => v,
            None => continue,
        };
        let dist = longest_from(seed);
        for &pred in &cfg.preds[lp.header as usize] {
            if !inloop[pred as usize] {
                continue; // an entry edge carries no recurrence
            }
            if let Some(args) = latch_args(f, pred, lp.header) {
                match args.get(k).and_then(|r| idx(*r)) {
                    // The parameter is handed straight back: a cycle of length 0,
                    // which bounds nothing.
                    Some(a) if a == seed => {}
                    Some(a) => {
                        if let Some(d) = dist[a] {
                            bound = bound.max(d);
                        }
                    }
                    None => {}
                }
            }
        }
    }

    // The ADDRESS bound is a different question and takes a different seeding:
    // how long into the iteration a load waits for its address, counting from
    // every parameter being available at zero. matmul is why it is reported —
    // its accumulator cycle is one cycle whichever way the address is built, so
    // a recurrence-only model scores the `madd` and the pointer walk the same
    // and reproduces `cost = |MIR|`'s exact blindness.
    let mut dist0: Vec<Option<u32>> = vec![None; n];
    for p in &f.blocks[lp.header as usize].params {
        if let Some(v) = idx(*p) {
            dist0[v] = Some(0);
        }
    }
    // Anything defined outside the loop is available at zero too.
    for b in 0..f.blocks.len() {
        if inloop[b] {
            continue;
        }
        for inst in &f.blocks[b].insts {
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                    if let Some(v) = idx(r) {
                        dist0[v] = Some(0);
                    }
                }
            });
        }
    }
    let mut addr = 0u32;
    for &b in &body {
        for inst in &f.blocks[b as usize].insts {
            let mut ready: Option<u32> = None;
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    if let Some(d) = idx(r).and_then(|u| dist0[u]) {
                        let t = d.saturating_add(lat(inst, r));
                        ready = Some(ready.map_or(t, |x: u32| x.max(t)));
                    }
                }
            });
            if let Some(t) = ready {
                inst.visit(&mut |r, c| {
                    if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                        if let Some(d) = idx(r) {
                            dist0[d] = Some(dist0[d].map_or(t, |x: u32| x.max(t)));
                        }
                    }
                });
            }
            if let MInst::Load { mem, .. } | MInst::Pair { load: true, mem, .. } = inst {
                for r in addr_regs(mem) {
                    if let Some(d) = idx(r).and_then(|u| dist0[u]) {
                        addr = addr.max(d);
                    }
                }
            }
        }
    }
    Some(Bound { recurrence: bound, addr })
}

fn addr_regs(m: &AddrMode) -> Vec<Reg> {
    match m {
        AddrMode::BaseImm { base, .. } => vec![*base],
        AddrMode::BaseReg { base, idx, .. } => vec![*base, *idx],
        AddrMode::PreIdx { base, .. } | AddrMode::PostIdx { base, .. } => vec![*base],
        AddrMode::SymLo12 { base, .. } => vec![*base],
        _ => Vec::new(),
    }
}

fn latch_args(f: &MFunc, from: MBlockId, to: MBlockId) -> Option<Vec<Reg>> {
    for t in f.blocks[from as usize].term.targets() {
        if t.block == to {
            return Some(t.args.clone());
        }
    }
    None
}

/// The THIRD model, and the one `MEASURED M29` asked for (built: `MEASURED M30`): **executions, not
/// instructions**.
///
/// `cost = |MIR(f)|` is exact for size and Law 3c names its blindness to
/// dependence chains. There is a simpler blindness beside it: a static count
/// weighs an instruction in a latch executed 5,760,000 times exactly as it
/// weighs one in a cold arm. `n7_nested_subq` is the proof — removing two
/// executed instructions and a taken branch from its inner loop moved the
/// program 1.370 → 1.195 while the suite's INSN geomean moved 0.0008.
///
/// So this sums each block's instruction count against the frequency
/// `hir::freq` already computed and `isel` already copied into `MBlock.weight`:
///
/// ```text
///     wcost(f) = Σ_b  weight(b) · |insts(b)|
/// ```
///
/// It is an ESTIMATE of executions, not a cycle count — it prices every
/// instruction at one and knows nothing of latency or issue width, so it is the
/// dual of `cost = |MIR|` rather than of `recurrence`. Its job is RANKING: it
/// says which blocks a codegen row should be read in, which is the question the
/// static count answers wrongly.
pub fn weighted(f: &MFunc) -> u64 {
    f.blocks
        .iter()
        .map(|b| b.weight as u64 * b.insts.len() as u64)
        .sum()
}

/// `ZCC_WCOST=1` — the weighted count per function, and the blocks that carry
/// it. The list is the worklist: a block near the top is where an instruction
/// costs something, and every parity win this project has recorded came from
/// reading one loop body against gcc's.
pub fn wreport(f: &MFunc) {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*W.get_or_init(|| std::env::var("ZCC_WCOST").is_ok()) {
        return;
    }
    let total = weighted(f);
    if total == 0 {
        return;
    }
    let mut rows: Vec<(u64, usize, usize, u32)> = f
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| !b.insts.is_empty())
        .map(|(i, b)| (b.weight as u64 * b.insts.len() as u64, i, b.insts.len(), b.weight))
        .collect();
    rows.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    eprintln!("WCOST {} total={} blocks={}", f.name, total, f.blocks.len());
    for (w, b, n, wt) in rows.iter().take(6) {
        eprintln!(
            "  WCOST {} b{} {} insts x weight {} = {} ({:.0}%)",
            f.name,
            b,
            n,
            wt,
            w,
            100.0 * *w as f64 / total as f64
        );
    }
}

/// `ZCC_CYCLES=1` — print each loop's recurrence bound beside its instruction
/// count, so the two models can be read against each other. A loop whose count
/// is at parity with gcc while its bound is not is exactly the case `cost=|MIR|`
/// cannot see, and it is the worklist R4.18 exists to produce.
pub fn report(f: &MFunc) {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*W.get_or_init(|| std::env::var("ZCC_CYCLES").is_ok()) {
        return;
    }
    let cfg = crate::mir::verify::cfg(f);
    let dt = DomTree::new(&cfg, f.entry);
    let lf = LoopForest::new(&cfg, &dt);
    for li in 0..lf.loops.len() {
        let lp = &lf.loops[li];
        let insts: usize = std::iter::once(lp.header)
            .chain(lp.body.iter().copied())
            .collect::<std::collections::BTreeSet<_>>()
            .iter()
            .map(|&b| f.blocks[b as usize].insts.len())
            .sum();
        match recurrence(f, &cfg, &lf, li) {
            Some(c) => eprintln!(
                "[cycles] {} loop@{} depth {}: {} insns, recurrence {} cyc/iter, addr {}",
                f.name, lp.header, lp.depth, insts, c.recurrence, c.addr
            ),
            None => eprintln!(
                "[cycles] {} loop@{} depth {}: {} insns, recurrence UNSCORED (call)",
                f.name, lp.header, lp.depth, insts
            ),
        }
    }
}
