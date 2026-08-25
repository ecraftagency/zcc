// iv — pointer induction variables (REARCH §13f item 5; gcc's strength
// reduction over addresses, `-fivopts`' core case).
//
// THE MEASUREMENT THAT ASKED FOR THIS (§13d cause #2, sharpened by §13f).
// `g1_memcpy_loop` sits at INSN **1.000** — instruction-count parity with
// gcc -O1 — and still costs 1.542 on the clock. So what is left in that loop is
// not how MANY instructions there are; it is what they wait for:
//
//     zcc   sxtw x1,w0 ; add x1,x4,x1 ; ldrb w1,[x1]    three dependent ops,
//                                                        every iteration
//     gcc   ldrb w1,[x20],#1                            address already there
//
// Rebuilding the address from the counter puts a three-deep dependence chain in
// front of every load. Walking a POINTER instead makes the address available at
// the top of the iteration, and `auto_inc` (R3.2) then folds the bump into the
// access, which is how the second line above happens.
//
// THE TRANSFORM. For each memory access in a loop whose address has an affine
// evolution `{base + off, +, step}` (`pass/scev.rs`), introduce one header
// parameter holding that address: entry edges pass `base + off`, latch edges
// pass `q + step`, and the access reads `q`. Accesses sharing a recurrence share
// the parameter, so `a[i]` read twice costs one pointer, not two.
//
// COMMUTING SQUARE. The parameter holds `base + off + step*n` on iteration n —
// it is initialized to `base + off` and advanced by `step` on every back edge —
// which is precisely what the AddRec states the old address computation
// produced. Same address, same load, same store, so `⟦f⟧ = ⟦iv f⟧`. Pointer
// arithmetic wraps identically in both forms because both are I64 adds. The
// whole burden of proof therefore sits in `scev.rs`, which is why that analysis
// was built and battery-tested first, and why it refuses a widening it cannot
// bound instead of guessing at one.
//
// WHAT IS REFUSED:
//   * an address that is ALREADY a basic induction variable — there is nothing
//     to strength-reduce, and re-deriving it would add a second pointer walking
//     beside the first on every run of the ladder;
//   * a recurrence with no symbolic base. `{0, +, 4}` is an integer sequence, not
//     an address; rewriting it wins nothing and would fire on ordinary counters;
//   * a STORE, and a step outside the post-index immediate — see the loads-only
//     note below, which is a cost argument about A64's free scaled-index mode.
use super::*;
use std::collections::HashMap;

/// SHIPPED DEFAULT-OFF, on the measurement rather than on a doubt about the
/// theorem (§13i). Over the eight programs above the harness's noise floor this
/// row is **1 win / 1 loss / 6 flat**: g1_memcpy 74 → 48 ms, j2_histogram
/// 60 → 68 ms, everything else inside ±3%. Take the single winner out and the
/// geomean goes the WRONG way — 1.4498 → 1.4840 — for +0.8% on sqlite.
///
/// And the winner is compensating for a gap one layer down. With this pass off,
/// isel emits `sxtw ; add ; add ; ldr [x1] ; str [x2]` for `d[i] = s[i]` where
/// gcc emits `ldrb w3, [x1, x2] ; strb w3, [x0, x2]` — it does not fold the add
/// into the addressing mode when TWO accesses share one index. So most of the
/// 35% is this pass paying off an isel debt, which the addressing mode should
/// pay directly: no new parameter, no register, no size, and no post-index µop
/// to lose on (j2 regresses at IDENTICAL instruction count).
///
/// THE GATE WAS DISCHARGED, NEGATIVE (§13k). isel was fixed; re-measured on that
/// baseline this pass is **0 win / 1 loss / 7 flat** — g1_memcpy 47 → 47 ms (the
/// win was isel's all along), j2_histogram 59 → 67 ms (the loss is this pass's),
/// EXEC ≥30 ms 1.3789 → 1.4087, INSN 1.2419 → 1.2454, sqlite +1,276.
///
/// The premise is false on this target: A64's scaled-index form makes rebuilding
/// an address from a counter free, so there is nothing to strength-reduce. What
/// remains is the post-index form alone, and j2 is the counter-example to that —
/// identical instruction count, 13% slower. Re-opening needs a cost model that
/// can say WHEN a writeback pays, which is a cycle-level question the `-S`
/// harness cannot answer. `ZCC_IV=1` forces it on for that day.
const ENABLED: bool = false;

fn enabled() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| ENABLED || std::env::var("ZCC_IV").is_ok())
}

pub fn run(f: &mut Func) -> bool {
    if !enabled() {
        return false;
    }
    force(f)
}

/// The pass past the default-off gate. The batteries call this: a theorem that
/// ships disabled still owes its square.
pub fn force(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    // innermost first: the inner loop is the hot one, and rewriting it does not
    // disturb the outer loop's recurrences.
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        if strengthen(f, &c, &dt, &lf, li) {
            // The CFG is unchanged but every analysis over VALUES is stale.
            return true;
        }
    }
    false
}

/// One recurrence, as the key that decides which accesses share a pointer.
type Key = (ValueId, i64, i64);

fn strengthen(
    f: &mut Func,
    c: &dom::Cfg,
    dt: &dom::DomTree,
    lf: &dom::LoopForest,
    li: usize,
) -> bool {
    let s = match scev::LoopScev::analyze(f, c, dt, lf, li) {
        Some(s) => s,
        None => return false,
    };
    let header = lf.loops[li].header;
    // Which accesses want which recurrence.
    let mut groups: HashMap<Key, Vec<(BlockId, usize)>> = HashMap::new();
    for &b in &lf.loops[li].body {
        for (i, inst) in f.blocks[b as usize].insts.iter().enumerate() {
            // LOADS ONLY, and the reason is an ISA cost fact rather than a
            // safety one. A64 addresses `p[i]` with a SCALED INDEX —
            // `ldr w, [base, w, sxtw #2]` — one instruction, the arithmetic
            // free. Replacing that with a walking pointer trades a free
            // addressing mode for an explicit `add`, so it only pays when the
            // add then DISAPPEARS into a post-index (`auto_inc`, R3.2) — and
            // A64 offers post-index safely for loads alone, since `STR Xt,
            // [Xn], #imm` with t == n is CONSTRAINED UNPREDICTABLE.
            //
            // Measured, not assumed: with stores included, `j2_histogram`'s
            // zeroing loop went from four instructions per iteration to five
            // (`str wzr,[x0,w1,sxtw #2]` became `str wzr,[x4]` plus an `add`)
            // and the program lost 60 ms → 69 ms. §13h.
            let addr = match inst {
                Inst::Load { addr, vol: false, .. } => *addr,
                _ => continue,
            };
            // Already a pointer walk: leave it alone, or the ladder would grow a
            // fresh pointer beside it on every round.
            if matches!(addr, Operand::Val(v) if s.ivs.contains_key(&v)) {
                continue;
            }
            let a = match s.eval(f, addr) {
                Some(a) if a.step != 0 => a,
                _ => continue,
            };
            let base = match a.base {
                Some(b) => b,
                // an integer recurrence, not an address
                None => continue,
            };
            // The step must fit the post-index immediate, or `auto_inc` cannot
            // fold and the pointer is pure cost (DDI 0487 C6.2: an unscaled
            // signed 9-bit offset).
            if a.step < -256 || a.step > 255 {
                continue;
            }
            groups.entry((base, a.off, a.step)).or_default().push((b, i));
        }
    }
    if groups.is_empty() {
        return false;
    }
    // Deterministic order: identical IR must produce identical bytes
    // (`tests/determinism.sh`), and a HashMap walk does not.
    let mut keys: Vec<Key> = groups.keys().copied().collect();
    keys.sort();
    for k in keys {
        let sites = groups.remove(&k).unwrap();
        introduce(f, c, dt, header, k, &sites);
    }
    refresh_defs(f);
    true
}

/// Add the header parameter that walks `base + off + step*n`, and point every
/// site at it.
fn introduce(
    f: &mut Func,
    c: &dom::Cfg,
    dt: &dom::DomTree,
    header: BlockId,
    k: Key,
    sites: &[(BlockId, usize)],
) {
    let (base, off, step) = k;
    let idx = f.blocks[header as usize].params.len() as u32;
    let q = f.new_value(Ty::I64, Def::Param(header, idx));
    f.blocks[header as usize].params.push(q);

    for &p in &c.preds[header as usize] {
        // A predecessor the header DOMINATES is a latch; anything else is an
        // entry. The distinction is the whole of the initialization rule: an
        // entry supplies the start value, a latch supplies the step.
        let arg = if dt.dominates(header, p) {
            append(f, p, Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: Operand::Val(q), b: Operand::Imm(step) })
        } else if off == 0 {
            // Nothing to compute: the start IS the base.
            Operand::Val(base)
        } else {
            append(f, p, Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: Operand::Val(base), b: Operand::Imm(off) })
        };
        for t in f.blocks[p as usize].term.targets_mut() {
            if t.block == header {
                t.args.push(arg);
            }
        }
    }
    for &(b, i) in sites {
        match &mut f.blocks[b as usize].insts[i] {
            Inst::Load { addr, .. } => *addr = Operand::Val(q),
            _ => unreachable!("a site is a load"),
        }
    }
}

/// Append an instruction to a block and return the value it defines.
fn append(f: &mut Func, b: BlockId, mut inst: Inst) -> Operand {
    let i = f.blocks[b as usize].insts.len() as u32;
    let v = f.new_value(Ty::I64, Def::Inst(b, i));
    match &mut inst {
        Inst::Bin { dst, .. } => *dst = v,
        _ => unreachable!("append is used for arithmetic only"),
    }
    f.blocks[b as usize].insts.push(inst);
    Operand::Val(v)
}
