// vecmla — four jammed lanes become one lane operation.
// THEORY A6b — MIR; THEORY A7b — optimization: this pass ships its commuting square
//
// WHERE IT SITS. `hir::pass::jam` runs four outer iterations through one pass of
// an inner loop, and `iv`'s displacement rule then puts their four loads on ONE
// base at offsets 0, 4, 8, 12 — sixteen contiguous bytes, which is a `q`
// register. What is left is the width, and this is the pass that takes it.
//
// THE SHAPE, exactly as `z4_matmul_int` leaves it:
//
//     Load W  d0 <- [p +  0]        Madd a0' = s * d0 + a0
//     Load W  d1 <- [p +  4]        Madd a1' = s * d1 + a1
//     Load W  d2 <- [p +  8]        Madd a2' = s * d2 + a2
//     Load W  d3 <- [p + 12]        Madd a3' = s * d3 + a3
//
// with `s` ONE value shared by all four, and `a0..a3` four block parameters of
// this loop. Eight instructions become four: one `ldr q`, one `dup`, one
// `mul v.4s`, one `add v.4s`.
//
// WHY MIR AND NOT HIR. The accumulator crosses the back edge as a VECTOR, and a
// vector WIDTH is target knowledge — Article B keeps that in the ISA tables and
// the emitter. MIR already carries it: `Width::Q` is a real width and a `Q` vreg
// is a legal block parameter, so nothing new has to be invented to hold it.
//
// COMMUTING SQUARE. Lane `l` of the vector holds exactly what `a_l` held: the
// load is the same sixteen bytes the four scalar loads read (they are contiguous
// and in ascending lane order, which is what the offsets 0,4,8,12 say), `dup`
// puts the shared multiplier in every lane, and lanewise `mul`+`add` is the four
// `Madd`s written once. `mir::interp` executes all four lanewise, so this is
// checkable rather than asserted. At the exit the four lanes are four DIFFERENT
// outer iterations, not partial sums — so they are EXTRACTED with `umov`, once,
// in the successor, not added.
use crate::mir::*;

/// THEORY A7b — the pass ships ON, on the measurement rather than on the idea.
/// 96 programs, gcc -O2 referee, Graviton4: EXEC geomean 1.2245 -> 1.2168, INSN
/// 1.0154 -> 1.0147, 0 DIVERGE, and `z4_matmul_int` — the worst program in the
/// suite — goes 2.93x -> 1.43x. `ZCC_NOVECMLA` turns it off.
pub fn wanted() -> bool {
    WANT.with(|c| c.get()).unwrap_or_else(|| {
        static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *W.get_or_init(|| std::env::var_os("ZCC_NOVECMLA").is_none())
    })
}

thread_local! {
    // THEORY A7b — instrument half. Not a value the compiler computes with: the
    // switch a battery flips to build the same function BOTH ways and compare
    // them, which is the only shape this pass's square can take. A thread-local
    // for the reason `spill.rs`'s seams are: the battery runs in parallel threads.
    static WANT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub fn set_wanted(on: Option<bool>) {
    WANT.with(|c| c.set(on));
}

/// THEORY A7b — instrument half: loops widened, so an A/B can tell "bought
/// nothing" from "never fired".
pub static FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// THEORY A6b  SQUARE four_jammed_lanes_are_one_lane_operation — the widened jam
pub fn run(f: &mut MFunc) -> bool {
    if !wanted() {
        return false;
    }
    let mut hit = false;
    for b in 0..f.blocks.len() {
        if widen(f, b as MBlockId) {
            hit = true;
            FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    hit
}

/// The four `(load, madd)` pairs of one group, in lane order.
struct Group {
    /// index in the block of each load and each madd
    at: Vec<usize>,
    base: Reg,
    /// the shared multiplier
    s: Reg,
    /// each lane's accumulator parameter position, and the value it becomes
    accp: Vec<usize>,
    next: Vec<Reg>,
}

fn find(f: &MFunc, b: MBlockId) -> Option<Group> {
    let bi = b as usize;
    let blk = &f.blocks[bi];
    // A self-loop: the four accumulators must be this block's own parameters.
    if !blk.term.targets().iter().any(|t| t.block == b) {
        return None;
    }
    // Loads of a 32-bit word at one base, at the four lane offsets.
    let mut lanes: Vec<(usize, Reg, i64)> = Vec::new();
    for (i, inst) in blk.insts.iter().enumerate() {
        if let MInst::Load { op: MemOp::W, dst, mem: AddrMode::BaseImm { base, off }, vol: false } =
            inst
        {
            lanes.push((i, *dst, *off as i64 * 0 + *off as i64));
            let _ = base;
        }
    }
    // Group by base.
    for (_, _, _) in lanes.iter() {}
    let mut best: Option<Group> = None;
    let bases: Vec<Reg> = blk
        .insts
        .iter()
        .filter_map(|i| match i {
            MInst::Load { op: MemOp::W, mem: AddrMode::BaseImm { base, .. }, vol: false, .. } => {
                Some(*base)
            }
            _ => None,
        })
        .collect();
    for base in bases {
        let mut at: Vec<usize> = Vec::new();
        let mut dsts: Vec<Reg> = Vec::new();
        let mut ok = true;
        for l in 0..4i64 {
            let found = blk.insts.iter().enumerate().find(|(_, i)| {
                matches!(i,
                    MInst::Load { op: MemOp::W, mem: AddrMode::BaseImm { base: bb, off }, vol: false, .. }
                    if *bb == base && *off as i64 == l * 4)
            });
            match found {
                Some((i, MInst::Load { dst, .. })) => {
                    at.push(i);
                    dsts.push(*dst);
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        // Each load feeds exactly one `Madd`, and all four share the other factor.
        let mut madds: Vec<usize> = Vec::new();
        let mut s: Option<Reg> = None;
        let mut accs: Vec<Reg> = Vec::new();
        let mut nexts: Vec<Reg> = Vec::new();
        for &d in &dsts {
            let m = blk.insts.iter().enumerate().find(|(_, i)| {
                matches!(i, MInst::Alu3 { op: Alu3Op::Madd, w: Width::W32, a, b, .. }
                    if *a == d || *b == d)
            });
            match m {
                Some((i, MInst::Alu3 { dst, a, b, c, .. })) => {
                    let other = if *a == d { *b } else { *a };
                    if *s.get_or_insert(other) != other {
                        ok = false;
                        break;
                    }
                    madds.push(i);
                    accs.push(*c);
                    nexts.push(*dst);
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok || madds.len() != 4 {
            continue;
        }
        // The four accumulators must be this block's parameters, in lane order.
        let accp: Vec<usize> = accs
            .iter()
            .filter_map(|a| blk.params.iter().position(|p| p == a))
            .collect();
        if accp.len() != 4 {
            continue;
        }
        at.extend(madds);
        best = Some(Group { at, base, s: s.unwrap(), accp, next: nexts });
        break;
    }
    best
}

fn widen(f: &mut MFunc, b: MBlockId) -> bool {
    let g = match find(f, b) {
        Some(g) => g,
        None => return false,
    };
    let bi = b as usize;
    // THE EXIT MUST BE PRIVATE. The four lanes are extracted in the successor so
    // the `umov`s run once per group instead of once per iteration — which is
    // only sound if nothing else reaches that block.
    let exit = match f.blocks[bi].term.targets().iter().find(|t| t.block != b) {
        Some(t) => t.block,
        None => return false,
    };
    let preds = f
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, blk)| blk.term.targets().iter().any(|t| t.block == exit))
        .count();
    if preds != 1 {
        return false;
    }

    // ── the loop body ──────────────────────────────────────────────────────
    let qacc = f.new_vreg(Width::Q);
    let qload = f.new_vreg(Width::Q);
    let qdup = f.new_vreg(Width::Q);
    let qmul = f.new_vreg(Width::Q);
    let qnext = f.new_vreg(Width::Q);
    let ins = vec![
        MInst::Load {
            op: MemOp::Q,
            dst: qload,
            mem: AddrMode::BaseImm { base: g.base, off: 0 },
            vol: false,
        },
        MInst::VDup { arr: Arr::V4S, dst: qdup, src: g.s },
        MInst::VInt { op: VIntOp::Mul, arr: Arr::V4S, dst: qmul, a: qload, b: qdup },
        MInst::VInt { op: VIntOp::Add, arr: Arr::V4S, dst: qnext, a: qacc, b: qmul },
    ];
    // Drop the eight instructions the four replace, highest index first.
    let mut drop = g.at.clone();
    drop.sort_unstable();
    for &i in drop.iter().rev() {
        f.blocks[bi].insts.remove(i);
    }
    // WHERE THE FOUR GO, and putting them at the first hole was this pass's
    // second defect: `dup` reads the shared multiplier, which a load LATER in the
    // block defines. The verifier said so — `v103 used before its definition` —
    // which is Law 3 doing its job at the middle. They go after whatever defines
    // the multiplier, or at the top when it is a block parameter.
    let at = f.blocks[bi]
        .insts
        .iter()
        .position(|i| {
            let mut d = false;
            i.visit(&mut |r, c| {
                if matches!(c, Constraint::Def) && r == g.s {
                    d = true;
                }
            });
            d
        })
        .map(|k| k + 1)
        .unwrap_or(0)
        .min(f.blocks[bi].insts.len());
    for (k, i) in ins.into_iter().enumerate() {
        f.blocks[bi].insts.insert(at + k, i);
    }

    // ── the parameters, and the two edges that carry them ──────────────────
    let mut order: Vec<usize> = g.accp.clone();
    order.sort_unstable();
    for &p in order.iter().rev() {
        f.blocks[bi].params.remove(p);
    }
    f.blocks[bi].params.push(qacc);
    // THE TWO EDGES DO NOT SHARE A LAYOUT, and assuming they did was this pass's
    // first defect. On the BACK edge the arguments line up with this block's own
    // parameters, so the accumulator positions are `accp`. On the EXIT edge they
    // line up with the SUCCESSOR's parameters, and the accumulators sit wherever
    // their next-values happen to be — found by value, not by position.
    let exit_pos: Vec<usize> = {
        let t = f.blocks[bi].term.targets().iter().find(|t| t.block == exit).cloned();
        let args = t.map(|t| t.args.clone()).unwrap_or_default();
        g.next
            .iter()
            .filter_map(|n| args.iter().position(|a| a == n))
            .collect()
    };
    if exit_pos.len() != 4 {
        return false;
    }
    let mut term = f.blocks[bi].term.clone();
    for t in term.targets_mut() {
        let drop_at: &[usize] = if t.block == b { &g.accp } else { &exit_pos };
        let keep: Vec<Reg> = t
            .args
            .iter()
            .enumerate()
            .filter(|(k, _)| !drop_at.contains(k))
            .map(|(_, r)| *r)
            .collect();
        t.args = keep;
        t.args.push(qnext);
    }
    f.blocks[bi].term = term;

    // Every OTHER predecessor of the loop hands it the four initial accumulators;
    // they become one vector, built by broadcasting the value they share.
    let entries: Vec<usize> = f
        .blocks
        .iter()
        .enumerate()
        .filter(|(k, blk)| *k != bi && blk.term.targets().iter().any(|t| t.block == b))
        .map(|(k, _)| k)
        .collect();
    for e in entries {
        let args: Vec<Reg> = f.blocks[e]
            .term
            .targets()
            .iter()
            .find(|t| t.block == b)
            .map(|t| t.args.clone())
            .unwrap_or_default();
        let init: Vec<Reg> = g.accp.iter().map(|&k| args[k]).collect();
        if init.iter().any(|r| *r != init[0]) {
            return false; // four different starts is a different transform
        }
        let qz = f.new_vreg(Width::Q);
        f.blocks[e].insts.push(MInst::VDup { arr: Arr::V4S, dst: qz, src: init[0] });
        let mut t = f.blocks[e].term.clone();
        for tg in t.targets_mut() {
            if tg.block == b {
                let keep: Vec<Reg> = tg
                    .args
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| !g.accp.contains(k))
                    .map(|(_, r)| *r)
                    .collect();
                tg.args = keep;
                tg.args.push(qz);
            }
        }
        f.blocks[e].term = t;
    }

    // ── the exit: four lanes back into four registers, once ────────────────
    let ei = exit as usize;
    let mut order2: Vec<usize> = exit_pos.clone();
    order2.sort_unstable();
    // The exit block's parameters line up with the exit edge's arguments, which
    // this pass has just rewritten; the four scalars it expected are extracted
    // from the vector that replaced them.
    let scal: Vec<Reg> = order2.iter().map(|&k| f.blocks[ei].params[k]).collect();
    for &p in order2.iter().rev() {
        f.blocks[ei].params.remove(p);
    }
    let qp = f.new_vreg(Width::Q);
    f.blocks[ei].params.push(qp);
    let mut pre: Vec<MInst> = Vec::new();
    for (l, &d) in scal.iter().enumerate() {
        pre.push(MInst::VExt { arr: Arr::V4S, lane: l as u8, dst: d, src: qp });
    }
    for (k, i) in pre.into_iter().enumerate() {
        f.blocks[ei].insts.insert(k, i);
    }
    let _ = &g.next;
    true
}
