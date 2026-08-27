// freq — STATIC BLOCK EXECUTION FREQUENCY, the signal three decisions wanted
// and none of them had.
//
// THEORY A7b — analysis; Wu & Larus, "Static Branch Frequency and Program
// Profile Analysis", MICRO-27 (1994). Cited as `MEASURED M12` already cites it
// for the trip-count convention; this is the same paper's other half.
//
// WHY IT EXISTS, measured rather than supposed (2026-08-27):
//
//   * THE PROFITABILITY GATE. `rotate`, `licm` and `iv` add 7,728 instructions
//     to sqlite for no measurable speed, and are worth 40% of execution on the
//     42-program taxonomy suite (`MEASURED M17`). They are not wrong, they are
//     UNGATED — they fire wherever a loop exists, and most of sqlite's loops are
//     cold. The obvious gate, loop depth, is useless: 1,066 of the 1,387 loops
//     rotation touches in sqlite are OUTERMOST, and so are most of the suite's
//     hot loops. Depth cannot tell them apart. Frequency can.
//   * THE SPILLER'S DISTANCE. `spill.rs::Trace` measures Belady's distance along
//     the execution trace and charges `TRIPS = 10` per loop level to leave a
//     loop. That constant is a stand-in for exactly this analysis; `MEASURED
//     M12` says so and swept it to prove the sweep did not matter.
//   * "WE HAVE NO PROFILE." Said three times in one day as a reason a decision
//     could not be made. This is what a compiler uses instead of one.
//
// THE MODEL. Frequency is carried as an integer scaled by `ENTRY`, so the entry
// block is 1.0 and everything else is relative to it. Blocks are visited in
// reverse postorder, which reaches a block after every predecessor that is not
// a back edge — so one pass suffices and no linear system is solved:
//
//   * a LOOP HEADER takes the sum of its non-back-edge predecessors, multiplied
//     by the assumed trip count. This is where a loop becomes hot, and it is the
//     whole of the loop heuristic: the back edge needs no probability of its own
//     because the multiplication has already accounted for every iteration.
//   * every other block is the sum over its predecessors of that predecessor's
//     frequency times the probability of the edge.
//
// EDGE PROBABILITIES are integer weights normalized per predecessor. Two
// heuristics ship, both of which are facts about the CFG rather than guesses
// about the program:
//
//   * UNREACHABLE (Wu-Larus call this the "opcode/exit" family): an edge into a
//     block whose terminator is `Unreachable` is a path the program does not
//     take — `noreturn`, `abort`, the tail after a call that never returns. It
//     gets weight 1 against 1,000.
//   * RETURN: a successor that returns immediately is the early-exit arm of a
//     guard far more often than it is the common path (Wu-Larus measured 72%
//     accuracy for this one). It gets weight 1 against 4.
//
// Anything else is uniform. That is deliberate: every further heuristic in the
// paper is a statistical claim about C programs, and this compiler does not ship
// a number it has not measured. The two above are structural.
//
// DETERMINISM. Integer arithmetic, `Vec` indexed by block id, reverse postorder
// from `dom::cfg` — no hash iteration, no floating point, no allocation order.
// Identical IR gives identical frequencies, which `tests/determinism.sh` checks
// end-to-end across fresh processes.
use super::*;

/// MEASURED M19 — the fixed-point scale, not a threshold
/// The entry block's frequency. A scale, not a measurement: every other block is
/// read relative to it, and integer division needs headroom above 1.
pub const ENTRY: u64 = 10_000;

/// MEASURED M12 — assumed trips per loop level
/// The same constant `spill.rs` charges to leave a loop, for the same reason and
/// from the same paper. Kept in one place now that two analyses want it.
pub const TRIPS: u64 = 10;

/// MEASURED M19 — a saturation bound, not a threshold
/// A ceiling on frequency so a deep nest cannot overflow or swamp the arithmetic
/// downstream. Ten levels of `TRIPS` is already 10^10 relative to entry; nothing
/// real distinguishes that from "as hot as it gets".
const CEIL: u64 = ENTRY.saturating_mul(1_000_000_000);

/// R5.1's A/B SEAM, and it is one switch for the whole row: whether the estimate
/// is written into `Block.weight` at all, whether `layout` chains blocks by it,
/// and whether the spiller's eviction ranking scales by it. Off, `weight` keeps
/// the `1` it has always held, and all three read today's behaviour — which the
/// refactor gate checks byte for byte.
///
/// A thread-local overlay over the environment, for the reason `spill.rs`'s seams
/// are thread-locals: the battery runs in parallel threads, and a process-wide
/// switch would make one test's measurement depend on another's timing. `None`
/// means "ask the environment", which is what every non-test caller gets.
pub fn weights_wanted() -> bool {
    WEIGHTS.with(|c| c.get()).unwrap_or_else(env_weights)
}

thread_local! {
    // THEORY A7b — instrument half. Not a value the compiler computes with: it
    // is what lets a test ask what the weights DID, which is the non-vacuity
    // obligation Law 0 puts on every pass.
    static WEIGHTS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force the weights on or off for the CURRENT THREAD, or hand the decision back
/// to the environment with `None`.
#[cfg(test)]
pub fn set_weights(on: Option<bool>) {
    WEIGHTS.with(|c| c.set(on));
}

/// `ZCC_WEIGHTS` read once. The spiller asks this per function and `layout` per
/// function; an environment lookup on each is a cost the answer cannot change.
fn env_weights() -> bool {
    static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENV.get_or_init(|| {
        std::env::var_os("ZCC_WEIGHTS").is_some()
            || std::env::var_os("ZCC_WEIGHTS_LAYOUT").is_some()
            || std::env::var_os("ZCC_WEIGHTS_SPILL").is_some()
    })
}

/// R5.1 IS TWO CONSUMERS UNDER ONE SWITCH, and the first measurement said the
/// pair loses — EXEC 1.0206 → 1.0873, INSN 1.0719 → 1.0880 on the 42-program
/// taxonomy suite (2026-08-28, this machine). A pair that loses says nothing
/// about which half lost, so each consumer gets its own seam: `ZCC_WEIGHTS`
/// still turns both on, and `ZCC_WEIGHTS_LAYOUT` / `ZCC_WEIGHTS_SPILL` turn on
/// exactly one. This is Law 2's "locate mechanically first" applied to a
/// performance defect rather than a correctness one.
pub fn layout_wanted() -> bool {
    weights_wanted() && sub("ZCC_WEIGHTS_SPILL", "ZCC_WEIGHTS_LAYOUT")
}

/// The same, for the spiller's eviction ranking.
pub fn spill_wanted() -> bool {
    weights_wanted() && sub("ZCC_WEIGHTS_LAYOUT", "ZCC_WEIGHTS_SPILL")
}

/// A consumer is on unless the OTHER consumer was named alone.
fn sub(other: &str, own: &str) -> bool {
    std::env::var_os(own).is_some() || std::env::var_os(other).is_none()
}

/// Stamp every block with its estimated frequency, so the layers BELOW HIR can
/// read a real number instead of re-deriving loop depth.
///
/// `Block.weight` has existed since the HIR was written and `isel` already
/// copies it into `MBlock.weight`, but nothing ever COMPUTED it: it defaults to
/// `1` and the nine sites that touch it only propagate a neighbour's value. The
/// field was plumbed and dead. This is the missing line.
///
/// Run AFTER the pass ladder and before `isel`: the passes reshape the CFG, and
/// a frequency computed before them describes a program that no longer exists.
///
/// Saturating to `u32` is not a truncation of the model. `CEIL` is `10^13`,
/// reached only by a nest ten levels deep, and everything above `u32::MAX` is
/// already "as hot as it gets" — the consumers compare weights, they do not do
/// arithmetic that the clamp could bias.
pub fn annotate(f: &mut Func) {
    if !weights_wanted() {
        return;
    }
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let fq = estimate(f, &c, &lf);
    for (b, &w) in fq.iter().enumerate() {
        f.blocks[b].weight = w.min(u32::MAX as u64) as u32;
    }
}

/// Relative execution frequency of every block, `ENTRY` for the entry block.
///
/// Unreachable blocks get 0, which is the honest answer and lets a caller use
/// `freq == 0` as "this code does not run".
pub fn estimate(f: &Func, c: &dom::Cfg, lf: &dom::LoopForest) -> Vec<u64> {
    let n = f.blocks.len();
    let mut freq = vec![0u64; n];
    // which blocks head a loop, and the header a back edge returns to
    let mut header = vec![false; n];
    // The latches of every loop headed HERE, indexed by header. `back_edge`
    // asked the same question by scanning the whole loop list once per CFG
    // EDGE, which is `edges × loops` for a fact that is a table: a loop's
    // latches are known before the walk starts and do not change during it.
    let mut latches_of: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    for l in &lf.loops {
        header[l.header as usize] = true;
        latches_of[l.header as usize].extend(l.latches.iter().copied());
    }
    freq[f.entry as usize] = ENTRY;

    for &b in &c.rpo {
        let bi = b as usize;
        if bi != f.entry as usize {
            // Sum the predecessors that reach this block WITHOUT a back edge. A
            // back edge's contribution is already inside the loop multiplier
            // below, and counting it here would need the fixpoint this pass
            // exists to avoid.
            let mut sum: u64 = 0;
            for &p in &c.preds[bi] {
                let pi = p as usize;
                if back_edge(&latches_of, p, b) {
                    continue;
                }
                let (w, tot) = edge_weight(f, lf, pi, b);
                sum = sum.saturating_add(freq[pi].saturating_mul(w) / tot.max(1));
            }
            freq[bi] = sum;
        }
        if header[bi] {
            freq[bi] = freq[bi].saturating_mul(TRIPS).min(CEIL);
        }
    }
    freq
}

/// Is `p -> b` a back edge? It is exactly the edge a loop's latch takes to its
/// own header, which `LoopForest` already knows.
fn back_edge(latches_of: &[Vec<BlockId>], p: BlockId, b: BlockId) -> bool {
    latches_of[b as usize].contains(&p)
}

/// `(weight of p -> b, total weight of p's edges)`.
///
/// Weights are integers and the total is computed the same way for every
/// successor, so the probabilities sum to one by construction and no
/// normalization step can drift.
///
/// THE LOOP-BRANCH HEURISTIC, and leaving it out was a defect rather than an
/// omission (found by measurement, 2026-08-28). A loop that runs `TRIPS` times
/// leaves ONCE: the exit edge is taken on one iteration in `TRIPS`, not on one
/// in two. Scoring it uniformly gave a loop's exit block half the body's
/// frequency — so the "not found" return of a search loop scored as hot as the
/// search — and `layout` duly chained that cold block into the middle of the hot
/// one. EXEC over the 42-program taxonomy suite went 1.0206 to 1.0925 with block
/// weights on, `d3_early_exit` 1.00 to 1.98, and the reason was here rather than
/// in the consumer.
///
/// Wu-Larus call this the LOOP BRANCH heuristic and measure it the most accurate
/// of the family (88%); it is also the one heuristic that is a fact about the
/// CFG rather than a claim about C programs, since `TRIPS` is already the trip
/// count every other part of this model assumes.
fn edge_weight(f: &Func, lf: &dom::LoopForest, p: usize, b: BlockId) -> (u64, u64) {
    let succs = f.blocks[p].term.succs();
    if succs.len() <= 1 {
        return (1, 1);
    }
    // the innermost loop containing the SOURCE; an edge leaving it is an exit
    let inner = lf.of[p];
    // `s` is inside loop `a` when a walk up its loop ancestry reaches `a` — the
    // successor may sit in a loop NESTED inside this one, which is still not an
    // exit.
    let inside = |a: u32, s: BlockId| -> bool {
        let mut cur = lf.of[s as usize];
        while let Some(x) = cur {
            if x == a {
                return true;
            }
            cur = lf.loops[x as usize].parent;
        }
        false
    };
    let stays = |s: BlockId| -> bool {
        match inner {
            Some(a) => inside(a, s),
            // outside every loop, no edge is an exit
            None => true,
        }
    };
    let w = |s: BlockId| -> u64 {
        match &f.blocks[s as usize].term {
            // a path the program does not take
            Term::Unreachable => 1,
            // the early-exit arm of a guard, more often than not
            Term::Ret(_) if f.blocks[s as usize].insts.is_empty() => 250,
            // one iteration in TRIPS leaves; the rest stay
            _ if !stays(s) => 1_000 / TRIPS,
            _ => 1_000,
        }
    };
    let total: u64 = succs.iter().map(|&s| w(s)).sum();
    // A block may appear twice among the successors (`br c, x, x` before
    // `cfg_simplify` runs); its weight then counts once per edge, which is what
    // summing over `preds` on the other side expects.
    (w(b), total)
}
