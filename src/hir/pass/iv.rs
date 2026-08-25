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

// ── induction-variable WIDENING (REARCH §13l) ──────────────────────────────
//
// THE MEASUREMENT. After §13j, `mycopy`'s inner loop is six instructions against
// gcc's five, and the extra one is a `sxtw` — every iteration, widening a 32-bit
// counter so it can index memory:
//
//     zcc   sxtw x1,w0 ; ldrb w2,[x4,x1] ; strb w2,[x3,x1] ; add w0,w0,#1 ; cmp w0,w5 ; b.lt
//     gcc                ldrb w3,[x1,x2] ; strb w3,[x0,x2] ; add x2,x2,1  ; cmp x4,x2 ; bne
//
// gcc runs the counter in 64 bits and sign-extends the BOUND once, outside the
// loop. That is the whole difference, and it is available here because
// `scev::find_nowrap` already proves what it needs: the counter cannot leave its
// type, so `sext(i)` is the identity on it and a 64-bit counter takes the same
// values.
//
// THE TRANSFORM. Give the header a 64-bit parameter `w` starting at `sext(start)`
// and advancing by the same step; rewrite every `sext(i)` to `w`; rewrite the
// exit test to compare in 64 bits against the widened bound. The narrow counter
// then has no uses left and `dce` deletes it, along with its `add`. One
// instruction per iteration in every loop that subscripts an array — which is
// most of them.
//
// COMMUTING SQUARE. `w = sext(i)` at every program point: it holds at entry by
// construction, and is preserved by the step because the no-wrap fact says the
// narrow add does not overflow, so `sext(i + d) = sext(i) + d`. Every rewritten
// use therefore reads the value it read before. The comparison is the one place
// this needs care and it is why the bound is widened with the SAME signedness the
// test uses: `i <s n` and `sext(i) <s sext(n)` agree exactly, for all i and n.
//
// WHAT IS REFUSED: a counter with any use that is not the step, the exit test, or
// a sign-extension — a narrow use would need a `trunc` back, which costs the
// instruction this pass exists to remove; a loop with more than one entry edge,
// since the widened start has to be materialized on it; and an unsigned test,
// whose `zext` twin is a separate fact this does not claim.
pub fn widen(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        if widen_loop(f, &c, &dt, &lf, li) {
            return true;
        }
    }
    false
}

fn widen_loop(
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
    let entries: Vec<BlockId> = c.preds[header as usize]
        .iter()
        .copied()
        .filter(|&p| !dt.dominates(header, p))
        .collect();
    let latches: Vec<BlockId> = c.preds[header as usize]
        .iter()
        .copied()
        .filter(|&p| dt.dominates(header, p))
        .collect();
    if entries.len() != 1 || latches.is_empty() {
        return false;
    }
    let entry = entries[0];
    let mut ivs: Vec<(ValueId, scev::AddRec)> = s
        .ivs
        .iter()
        .filter(|(p, r)| r.ty == Ty::I32 && s.no_wrap_signed(**p))
        .map(|(p, r)| (*p, *r))
        .collect();
    ivs.sort_by_key(|(p, _)| *p);
    for (p, rec) in ivs {
        if let Some(plan) = plan_widen(f, c, &s, lf, li, header, &latches, p, rec) {
            apply_widen(f, header, entry, &latches, p, rec, plan);
            refresh_defs(f);
            return true;
        }
    }
    false
}

/// What the rewrite needs to know: the `sext(i)` instructions to replace, the
/// step instruction, and the exit test.
struct Plan {
    sexts: Vec<ValueId>,
    step_val: ValueId,
    cmp_at: (BlockId, usize),
    /// does the test read the counter BEFORE or AFTER the step?
    cmp_after_step: bool,
    bound: Operand,
    op: CmpOp,
    cmp_dst: ValueId,
}

fn plan_widen(
    f: &Func,
    c: &dom::Cfg,
    s: &scev::LoopScev,
    lf: &dom::LoopForest,
    li: usize,
    header: BlockId,
    latches: &[BlockId],
    p: ValueId,
    rec: scev::AddRec,
) -> Option<Plan> {
    let _ = (c, lf, li);
    // The step instruction: the one value the latch passes back.
    let mut step_val = None;
    let mut sexts: Vec<ValueId> = Vec::new();
    let mut cmp: Option<(BlockId, usize, ValueId, CmpOp, Operand, bool)> = None;
    for (bi, b) in f.blocks.iter().enumerate() {
        for (i, inst) in b.insts.iter().enumerate() {
            match inst {
                Inst::Cvt { dst, op: CvtOp::Sext, from: Ty::I32, to: Ty::I64, a }
                    if *a == Operand::Val(p) =>
                {
                    sexts.push(*dst);
                }
                Inst::Bin { dst, op: BinOp::Add, ty: Ty::I32, a, b: rhs }
                    if *a == Operand::Val(p) && *rhs == Operand::Imm(rec.step) =>
                {
                    if step_val.is_some() {
                        return None;
                    }
                    step_val = Some(*dst);
                }
                Inst::Cmp { dst, op, ty: Ty::I32, a, b: rhs } => {
                    let after = match (*a, step_val) {
                        (Operand::Val(v), Some(sv)) if v == sv => true,
                        (Operand::Val(v), _) if v == p => false,
                        _ => continue,
                    };
                    if cmp.is_some() {
                        return None;
                    }
                    cmp = Some((bi as BlockId, i, *dst, *op, *rhs, after));
                }
                _ => {}
            }
        }
    }
    let step_val = step_val?;
    let (cb, ci, cmp_dst, op, bound, cmp_after_step) = cmp?;
    if sexts.is_empty() {
        return None; // nothing to save
    }
    // A test that reads the counter AFTER the step reads the value this LATCH
    // produced, so the widened step has to exist in that same block. When the
    // test sits somewhere else there is no such value to name — and the earlier
    // cut discovered that by silently skipping the rewrite and then deleting the
    // counter anyway, which is what `use of undefined` meant on 100 csmith
    // programs. Refused here, before anything is mutated.
    if cmp_after_step && !latches.contains(&cb) {
        return None;
    }
    // The bound must be loop-invariant, and the test SIGNED — an unsigned test
    // proves a different fact than the one `find_nowrap` recorded.
    if !matches!(op, CmpOp::Slt | CmpOp::Sle | CmpOp::Sgt | CmpOp::Sge | CmpOp::Eq | CmpOp::Ne) {
        return None;
    }
    // The bound is SEXT-ed into the entry block, so it must be defined outside
    // the loop — not merely have a step of zero, which is also true of values
    // COMPUTED INSIDE it (`is_loop_invariant` vs `AddRec::is_invariant`). Asking
    // the weaker question put a `sext` of a header-defined value into the entry
    // block and broke sqlite: `%79 used in bb18 but defined in bb6`.
    match bound {
        Operand::Imm(_) => {}
        Operand::Val(v) if s.is_loop_invariant(v) => {}
        _ => return None,
    }
    // Every use of the counter and of its step must be one of: the step itself,
    // the widened sexts, the test, or the header's own edge arguments.
    let ok_use = |v: ValueId, user: &Inst| -> bool {
        match user {
            Inst::Cvt { dst, .. } => sexts.contains(dst),
            Inst::Bin { dst, .. } => *dst == step_val,
            Inst::Cmp { dst, .. } => *dst == cmp_dst,
            _ => {
                let _ = v;
                false
            }
        }
    };
    for b in &f.blocks {
        for inst in &b.insts {
            let mut bad = false;
            inst.uses(|o| {
                if (o == Operand::Val(p) || o == Operand::Val(step_val)) && !ok_use(p, inst) {
                    bad = true;
                }
            });
            if bad {
                return None;
            }
        }
        // A terminator may only carry them as arguments to the HEADER.
        for t in b.term.targets() {
            if t.block == header {
                continue;
            }
            if t.args.iter().any(|a| *a == Operand::Val(p) || *a == Operand::Val(step_val)) {
                return None;
            }
        }
        let mut bad = false;
        match &b.term {
            Term::Br(x, ..) | Term::Switch(x, ..) | Term::GotoPtr(x, _) | Term::Ret(Some(x)) => {
                if *x == Operand::Val(p) || *x == Operand::Val(step_val) {
                    bad = true;
                }
            }
            _ => {}
        }
        if bad {
            return None;
        }
    }
    Some(Plan { sexts, step_val, cmp_at: (cb, ci), cmp_after_step, bound, op, cmp_dst })
}

fn apply_widen(
    f: &mut Func,
    header: BlockId,
    entry: BlockId,
    latches: &[BlockId],
    p: ValueId,
    rec: scev::AddRec,
    plan: Plan,
) {
    let idx = f.blocks[header as usize].params.len() as u32;
    let w = f.new_value(Ty::I64, Def::Param(header, idx));
    f.blocks[header as usize].params.push(w);

    // entry: the widened start
    let start = match rec.base {
        None => Operand::Imm(rec.off),
        Some(v) => append_cvt(f, entry, CvtOp::Sext, Ty::I32, Ty::I64, Operand::Val(v)),
    };
    for t in f.blocks[entry as usize].term.targets_mut() {
        if t.block == header {
            t.args.push(start);
        }
    }
    // latches: the widened step. In the block that also holds the exit test it
    // goes immediately BEFORE that test, which is what reads it — appending at
    // the end would place the definition after its use.
    let (cb, mut ci) = plan.cmp_at;
    let mut wstep = Vec::new();
    for &l in latches {
        let at = if l == cb { ci } else { f.blocks[l as usize].insts.len() };
        let v = insert_bin(f, l, at, BinOp::Add, Ty::I64, Operand::Val(w), Operand::Imm(rec.step));
        if l == cb {
            ci += 1;
        }
        wstep.push((l, v));
        for t in f.blocks[l as usize].term.targets_mut() {
            if t.block == header {
                t.args.push(Operand::Val(v));
            }
        }
    }
    // every `sext(i)` becomes the wide counter
    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    for d in &plan.sexts {
        map[*d as usize] = Some(Operand::Val(w));
    }
    rewrite_values(f, &map);

    // the exit test, in 64 bits against the widened bound
    let wide_bound = match plan.bound {
        Operand::Imm(k) => Operand::Imm(k),
        o => append_cvt(f, entry, CvtOp::Sext, Ty::I32, Ty::I64, o),
    };
    // Placing the widened bound in the ENTRY block keeps it loop-invariant; the
    // step value the test reads is the one this latch just produced.
    let lhs = if plan.cmp_after_step {
        // `plan_widen` has already refused the case where this is absent.
        Operand::Val(
            wstep
                .iter()
                .find(|(l, _)| *l == cb)
                .expect("the after-step test is in a latch")
                .1,
        )
    } else {
        Operand::Val(w)
    };
    f.blocks[cb as usize].insts[ci] = Inst::Cmp {
        dst: plan.cmp_dst,
        op: plan.op,
        ty: Ty::I64,
        a: lhs,
        b: wide_bound,
    };
    // The narrow counter is now dead BY CONSTRUCTION — `plan_widen` proved its
    // only readers were the sign-extensions and the test, and both now read the
    // wide one. It is deleted here rather than left to `dce` because what is
    // left is a CYCLE: the parameter feeds its own step, and the step feeds the
    // parameter back along the edge, so a use-count sweep sees both as live and
    // the loop keeps a second counter for nothing. (Widening then costs an `add`
    // exactly where it saved a `sxtw`, which is how this was caught: the loop
    // stayed six instructions.)
    let pi = f.blocks[header as usize]
        .params
        .iter()
        .position(|&x| x == p)
        .expect("the counter is a header parameter");
    f.blocks[header as usize].params.remove(pi);
    let mut preds: Vec<BlockId> = latches.to_vec();
    preds.push(entry);
    for b in preds {
        for t in f.blocks[b as usize].term.targets_mut() {
            if t.block == header && pi < t.args.len() {
                t.args.remove(pi);
            }
        }
    }
    for b in f.blocks.iter_mut() {
        b.insts.retain(|i| i.dst() != Some(plan.step_val));
    }
}

fn insert_bin(
    f: &mut Func,
    b: BlockId,
    at: usize,
    op: BinOp,
    ty: Ty,
    a: Operand,
    rhs: Operand,
) -> ValueId {
    let v = f.new_value(ty, Def::Inst(b, at as u32));
    f.blocks[b as usize].insts.insert(at, Inst::Bin { dst: v, op, ty, a, b: rhs });
    v
}

fn append_cvt(f: &mut Func, b: BlockId, op: CvtOp, from: Ty, to: Ty, a: Operand) -> Operand {
    let i = f.blocks[b as usize].insts.len() as u32;
    let v = f.new_value(to, Def::Inst(b, i));
    f.blocks[b as usize].insts.push(Inst::Cvt { dst: v, op, from, to, a });
    Operand::Val(v)
}
