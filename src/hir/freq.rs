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

/// Relative execution frequency of every block, `ENTRY` for the entry block.
///
/// Unreachable blocks get 0, which is the honest answer and lets a caller use
/// `freq == 0` as "this code does not run".
pub fn estimate(f: &Func, c: &dom::Cfg, lf: &dom::LoopForest) -> Vec<u64> {
    let n = f.blocks.len();
    let mut freq = vec![0u64; n];
    // which blocks head a loop, and the header a back edge returns to
    let mut header = vec![false; n];
    for l in &lf.loops {
        header[l.header as usize] = true;
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
                if back_edge(c, lf, p, b) {
                    continue;
                }
                let (w, tot) = edge_weight(f, pi, b);
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
fn back_edge(c: &dom::Cfg, lf: &dom::LoopForest, p: BlockId, b: BlockId) -> bool {
    let _ = c;
    lf.loops
        .iter()
        .any(|l| l.header == b && l.latches.contains(&p))
}

/// `(weight of p -> b, total weight of p's edges)`.
///
/// Weights are integers and the total is computed the same way for every
/// successor, so the probabilities sum to one by construction and no
/// normalization step can drift.
fn edge_weight(f: &Func, p: usize, b: BlockId) -> (u64, u64) {
    let succs = f.blocks[p].term.succs();
    if succs.len() <= 1 {
        return (1, 1);
    }
    let w = |s: BlockId| -> u64 {
        match &f.blocks[s as usize].term {
            // a path the program does not take
            Term::Unreachable => 1,
            // the early-exit arm of a guard, more often than not
            Term::Ret(_) if f.blocks[s as usize].insts.is_empty() => 250,
            _ => 1_000,
        }
    };
    let total: u64 = succs.iter().map(|&s| w(s)).sum();
    // A block may appear twice among the successors (`br c, x, x` before
    // `cfg_simplify` runs); its weight then counts once per edge, which is what
    // summing over `preds` on the other side expects.
    (w(b), total)
}
