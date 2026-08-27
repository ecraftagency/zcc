// sched — BASIC-BLOCK LIST SCHEDULING (REARCH §16 #9; Gibbons & Muchnick 1986).
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
//
// Law 3c is the reason this pass exists at all: `cost(f) = |MIR(f)|` is exact
// for SIZE and blind to TIME by the same construction, so a row that leaves a
// long dependence chain in front of a use costs cycles the size model cannot
// see. Reordering does not change how many instructions are executed; it changes
// how much of the chain is exposed.
//
// WHAT IS BEING REORDERED, and it is the narrow, safe half. This runs POST
// ALLOCATION, so every register is physical and no schedule can create a new
// live range or a new spill — the price is that the anti-dependences the
// allocator introduced (two unrelated values sharing one register) are now real
// constraints, which is exactly what the WAR and WAW edges below encode. The
// pre-allocation variant, which schedules for pressure and then colours, is a
// different pass with a different risk and is not this one.
//
// THE DAG. One node per instruction, edges for every ordering the machine
// requires:
//
//   * RAW — a read after the write that produced it;
//   * WAR / WAW — the anti-dependences that make a physical register a resource
//     rather than a value. NZCV is an ordinary register here (`flags: Reg`), so
//     a compare and the branch that reads it are ordered by the same rule and
//     need no special case;
//   * MEMORY — two accesses are ordered whenever at least one writes. This
//     oracle is deliberately the crude one: `hir::mem` disambiguated at the
//     layer where the C types are still visible, and re-deriving that here from
//     an `AddrMode` would be a second, weaker copy of it;
//   * BARRIERS — a call, a stack adjustment, a barrier instruction and the
//     load/store-exclusive pair order EVERYTHING. A call's clobber set is on the
//     instruction, but its arguments are established by a fixed-register
//     protocol whose order is the ABI's; `SpAdj` moves the frame under every
//     stack access at once.
//
// The schedule is a topological order of that DAG, so the two sides of the
// square execute the same instructions with every ordering the machine can
// observe preserved. That IS the proof — `⟦m⟧ = ⟦sched m⟧` is not established
// by running programs but by the construction of the order, and the battery
// confirms it rather than discovering it.
//
// THE PRIORITY is the longest latency-weighted path from a node to the end of
// the block, computed once bottom-up with `cost::latency` — the MEASURED table
// (`MEASURED M10`), never an invented number. A node on the critical path issues
// first; ties go to the earlier original position, which keeps the schedule a
// function of the input and keeps a def close to its use where nothing else
// decides. Priority is a heuristic and carries no correctness obligation: any
// topological order is legal, and this one is chosen to shorten the exposed
// chain.
use crate::mir::*;
use crate::mir::cost;

/// R5.4's A/B SEAM (`ZCC_SCHED`). Off, the block keeps the order the layers
/// above produced, which the byte-identical gate checks.
pub fn wanted() -> bool {
    SCHED.with(|c| c.get()).unwrap_or_else(|| {
        static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENV.get_or_init(|| std::env::var_os("ZCC_SCHED").is_some())
    })
}

thread_local! {
    // THEORY A6b — instrument half: the switch a test flips to measure that the
    // schedule moved something (the non-vacuity obligation).
    static SCHED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force the pass on or off for the CURRENT THREAD, or hand it back to the
/// environment with `None`.
#[cfg(test)]
pub fn set_sched(on: Option<bool>) {
    SCHED.with(|c| c.set(on));
}

/// THEORY A6b  SQUARE sched_is_a_topological_order_of_the_dependence_dag
pub fn run(f: &mut MFunc) -> bool {
    if !wanted() {
        return false;
    }
    let mut moved = false;
    for b in 0..f.blocks.len() {
        if f.blocks[b].insts.len() < 3 {
            continue;
        }
        let order = schedule(&f.blocks[b]);
        if order.iter().enumerate().any(|(i, &j)| i != j) {
            let old = std::mem::take(&mut f.blocks[b].insts);
            let mut new = Vec::with_capacity(old.len());
            // `old` is consumed in the scheduled order without cloning: each
            // slot is taken exactly once, which the permutation guarantees.
            let mut slots: Vec<Option<MInst>> = old.into_iter().map(Some).collect();
            for &j in &order {
                new.push(slots[j].take().expect("the schedule is a permutation"));
            }
            f.blocks[b].insts = new;
            moved = true;
        }
    }
    moved
}

/// Does this instruction order every memory access around it, and every other
/// barrier?
fn is_barrier(i: &MInst) -> bool {
    matches!(
        i,
        MInst::Call { .. }
            | MInst::SpAdj { .. }
            | MInst::Dmb
            | MInst::LdAxr { .. }
            | MInst::StlXr { .. }
            | MInst::Stlr { .. }
            | MInst::ParallelCopy(_)
            | MInst::Load { vol: true, .. }
            | MInst::Store { vol: true, .. }
    )
}

/// `(reads memory, writes memory)`.
fn mem_effect(i: &MInst) -> (bool, bool) {
    match i {
        MInst::Load { .. } | MInst::Reload { .. } => (true, false),
        MInst::Store { .. } | MInst::Spill { .. } => (false, true),
        MInst::Pair { load, .. } => (*load, !*load),
        _ => (false, false),
    }
}

/// The block's instructions in a legal order, as a permutation of their indices.
fn schedule(blk: &MBlock) -> Vec<usize> {
    let n = blk.insts.len();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    // A DUPLICATE EDGE IS HARMLESS and is not filtered: the count and the walk
    // agree — each insertion raises `indeg` once and the walk lowers it once —
    // so the only cost is a slightly longer list. Filtering would mean a
    // `contains` per edge, a scan where the whole point is to avoid one.
    let mut edge = |succ: &mut Vec<Vec<usize>>, indeg: &mut Vec<usize>, a: usize, b: usize| {
        if a == b {
            return;
        }
        succ[a].push(b);
        indeg[b] += 1;
    };

    // THE LAST WRITER AND THE READERS SINCE IT, per register — a lookup, not a
    // scan back through the block. The block that made this matter is real: the
    // s7876 fixture holds 600-instruction blocks, and a backwards scan per
    // operand is the defect class `678e700` removed from four other places.
    let mut last_def: std::collections::HashMap<Reg, usize> = Default::default();
    let mut readers: std::collections::HashMap<Reg, Vec<usize>> = Default::default();
    let mut last_mem_write: Option<usize> = None;
    let mut mem_reads: Vec<usize> = Vec::new();
    let mut last_barrier: Option<usize> = None;

    for i in 0..n {
        let inst = &blk.insts[i];
        let (mut uses, mut defs) = (Vec::new(), Vec::new());
        inst.visit(&mut |r, c| match c {
            Constraint::Use | Constraint::UseFixed(_) => uses.push(r),
            Constraint::Def | Constraint::DefFixed(_) => defs.push(r),
        });
        for r in &uses {
            if let Some(&d) = last_def.get(r) {
                edge(&mut succ, &mut indeg, d, i); // RAW
            }
            readers.entry(*r).or_default().push(i);
        }
        for r in &defs {
            if let Some(&d) = last_def.get(r) {
                edge(&mut succ, &mut indeg, d, i); // WAW
            }
            if let Some(rs) = readers.get(r) {
                for &u in rs {
                    edge(&mut succ, &mut indeg, u, i); // WAR
                }
            }
        }
        for r in &defs {
            last_def.insert(*r, i);
            readers.insert(*r, Vec::new());
        }

        let bar = is_barrier(inst);
        let (reads, writes) = mem_effect(inst);
        if bar {
            for j in 0..i {
                edge(&mut succ, &mut indeg, j, i);
            }
            last_barrier = Some(i);
            last_mem_write = Some(i);
            mem_reads.clear();
        } else {
            if let Some(b) = last_barrier {
                edge(&mut succ, &mut indeg, b, i);
            }
            if reads || writes {
                if let Some(w) = last_mem_write {
                    edge(&mut succ, &mut indeg, w, i);
                }
            }
            if writes {
                for &r in &mem_reads {
                    edge(&mut succ, &mut indeg, r, i);
                }
                last_mem_write = Some(i);
                mem_reads.clear();
            } else if reads {
                mem_reads.push(i);
            }
        }
    }

    // PRIORITY: the longest latency-weighted path to the end of the block.
    // Computed backwards over the instruction order, which is a topological
    // order of the DAG by construction — every edge runs from a lower index to a
    // higher one.
    let mut height = vec![0u32; n];
    for i in (0..n).rev() {
        let w = weight(&blk.insts[i]);
        let mut h = 0;
        for &s in &succ[i] {
            h = h.max(height[s]);
        }
        height[i] = h + w;
    }

    let mut order = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut indeg = indeg;
    while !ready.is_empty() {
        // highest priority, then the earliest original position — so a block
        // with nothing to choose between comes out exactly as it went in
        let pick = ready
            .iter()
            .enumerate()
            .max_by_key(|&(_, &i)| (height[i], std::cmp::Reverse(i)))
            .map(|(k, _)| k)
            .expect("ready is non-empty");
        let i = ready.remove(pick);
        order.push(i);
        for k in 0..succ[i].len() {
            let s = succ[i][k];
            indeg[s] -= 1;
            if indeg[s] == 0 {
                ready.push(s);
            }
        }
    }
    // A cycle would leave nodes unscheduled, and a dependence DAG cannot have
    // one — every edge runs forward in the original order. Falling back to that
    // order rather than panicking keeps a defect here from being a miscompile.
    if order.len() != n {
        return (0..n).collect();
    }
    order
}

/// The latency to charge a node: the longest of its input-to-output latencies,
/// from the MEASURED table. An instruction with no register source takes the
/// table's answer for a source it does not have, which is the ALU default.
fn weight(inst: &MInst) -> u32 {
    let mut w = 0;
    let mut any = false;
    inst.visit(&mut |r, c| {
        if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
            any = true;
            w = w.max(cost::latency(inst, r));
        }
    });
    if any {
        w
    } else {
        cost::latency(inst, Reg::P(crate::mir::isa::SCRATCH_GPR))
    }
}
