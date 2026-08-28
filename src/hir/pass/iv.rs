// iv — pointer induction variables (MECHANISM.md Part F item 5; gcc's strength
// THEORY A7b — optimization: this pass ships its commuting square
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
//   * a STORE — see the loads-only note below, a cost argument about A64's free
//     scaled-index mode;
//   * a UNIT-STRIDE load (step == the access size) unless `ZCC_IV` forces it, and
//     then only for a step inside the post-index immediate. That half is
//     MEASURED M2, refuted on this target. A step the addressing mode cannot
//     reach is a different theorem and ships ON — see `affine` and the two cost
//     arguments in `strengthen`.
use super::*;
use std::collections::HashMap;

/// MEASURED M2 — the UNIT-STRIDE pointer-IV rewrite is negative on this target
/// SHIPPED DEFAULT-OFF, on the measurement rather than on a doubt about the
/// theorem (§13i). **Scope narrowed 2026-08-26**: this verdict is about a step
/// EQUAL to the access size, the only case A64's scaled index reaches for free.
/// A row-strided address (`B[k][j]`, step 1920) has no such mode and is rebuilt
/// with a multiply; that half ships ON and is judged on the clock (§13q). Over the eight programs above the harness's noise floor this
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

/// The Law-4 residual print R4.13 asks for BEFORE a line is written: for every
/// in-loop load this pass declines, WHY. `ZCC_IVDBG=1` turns it on; the count per
/// reason is the prediction the next amendment is judged against.
fn residual(reason: &str) {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *W.get_or_init(|| std::env::var("ZCC_IVDBG").is_ok()) {
        eprintln!("[iv-residual] {}", reason);
    }
}

fn enabled() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| ENABLED || std::env::var("ZCC_IV").is_ok())
}

/// The CONSUMER-BLIND half (`ZCC_IVX`, default off while it is measured).
///
/// The load-site scan below asks "which LOADS have an affine address"; the
/// measurement that opened this asks a different question. An address rebuilt
/// with a multiply costs the same cycles whatever reads it — a store's base, a
/// call's argument, a pointer written into a structure — and two hand edits
/// measured exactly that: `n7_nested_subq` passes `&inner[j]` to a predicate
/// through a function pointer (1.751 → 1.622 when the address walks instead),
/// and `m3_dict_rehash` builds `&pool[i]` with `madd x9,x9,#24,x22` for a pair
/// of stores (1.301 → 1.254). Neither address is a load, so neither is a site
/// the scan above can see.
///
/// So this half strengthens the ADDRESS VALUE rather than the access: any value
/// defined in the loop whose evolution is affine with a step no addressing mode
/// reaches, and whose every use is an instruction inside the loop, is replaced
/// by the walking header parameter. The commuting square is the same one — the
/// parameter holds `base + off + step·n`, which is what the multiply computed —
/// and it is now discharged at the value rather than at the access, which is
/// where it always belonged.
/// DEFAULT ON since 2026-08-28, on the measurement: over the 49-program suite,
/// interleaved, EXEC geomean 1.0281 → 1.0231 with this row alone, and
/// `n7_nested_subq` 1.759 → 1.589, `k1_dispatch` 1.323 → 1.215,
/// `k2_live_pressure` 1.216 → 1.144. `ZCC_NOIVX=1` turns it off.
fn consumer_blind() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_NOIVX").is_err())
}

/// THEORY A7b  SQUARE a_strided_load_walks_a_pointer — the AddRec IS the address it replaces
pub fn run(f: &mut Func) -> bool {
    walk(f, enabled())
}

/// The pass with BOTH halves live. `run` is now the same entry — the default-off
/// gate moved DOWN into `strengthen`, where it belongs: `ENABLED`/`ZCC_IV` gates
/// only the unit-stride half that MEASURED M2 refuted, while a stride the
/// addressing mode cannot reach is always strength-reduced. The batteries call
/// this directly because a theorem that ships half-disabled still owes its
/// square on both halves.
pub fn force(f: &mut Func) -> bool {
    walk(f, true)
}

/// `unit` = also strength-reduce the unit-stride half MEASURED M2 refuted, the
/// half that ships off. The batteries pass `true`; the ladder passes `enabled()`.
fn walk(f: &mut Func, unit: bool) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    // innermost first: the inner loop is the hot one, and rewriting it does not
    // disturb the outer loop's recurrences.
    // Counted once for the whole function: it does not depend on which loop is
    // being strengthened, and `strengthen` asks it of every loop.
    let mut uses_total: HashMap<ValueId, usize> = HashMap::new();
    for b in f.blocks.iter() {
        for inst in &b.insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    *uses_total.entry(v).or_insert(0) += 1;
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                *uses_total.entry(v).or_insert(0) += 1;
            }
        });
    }
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        if strengthen(f, &c, &dt, &lf, li, unit, &uses_total) {
            // The CFG is unchanged but every analysis over VALUES is stale.
            return true;
        }
    }
    false
}

/// One recurrence, as the key that decides which accesses share a pointer. The
/// base is a LIST of loop-invariant terms, not one value — see `affine`.
type Key = (Vec<ValueId>, i64, i64);

/// The affine form of an address, as this pass needs it: `Σ bases + off + step·n`.
///
/// `scev::AddRec` carries ONE symbolic base, which is enough for `p[i]` and not
/// enough for `B[k][j]`. That address is `&B + k*1920 + j*8`: TWO loop-invariant
/// symbolic terms (`&B` and `j*8`) around one recurrence, so `eval` refuses the
/// whole expression and the strided load kept its multiply. Split the top-level
/// `add` and ask again — if one side carries the recurrence and the other is a
/// pure invariant, the address is still affine in `n`, and its base is the SUM of
/// the invariant terms, which is itself invariant and so computable once in the
/// preheader. Nothing about the commuting square changes: the parameter still
/// holds exactly the value the old address computation produced on iteration `n`.
fn affine(s: &scev::LoopScev, f: &Func, addr: Operand) -> Option<(Vec<ValueId>, i64, i64)> {
    if let Some(a) = s.eval(f, addr) {
        return Some((a.base.into_iter().collect(), a.off, a.step));
    }
    let v = addr.val()?;
    let (p, q) = match def_inst(f, v)? {
        Inst::Bin { op: BinOp::Add, a, b, .. } => (*a, *b),
        _ => return None,
    };
    let (x, y) = (s.eval(f, p)?, s.eval(f, q)?);
    // Exactly one side carries the recurrence; the other has to be a pure
    // invariant term. Two recurrences added together are not this shape.
    let (r, inv) = match (x.step, y.step) {
        (0, t) if t != 0 => (y, x),
        (t, 0) if t != 0 => (x, y),
        _ => return None,
    };
    let mut bases: Vec<ValueId> = r.base.into_iter().chain(inv.base).collect();
    if bases.len() != 2 || bases.iter().any(|&b| f.ty_of(b) != Ty::I64) {
        return None;
    }
    bases.sort();
    Some((bases, r.off.wrapping_add(inv.off), r.step))
}

/// Does the address `v` own the multiply that builds it — i.e. is there a `mul`
/// in its computation whose only reader is on the path to `v`, so that replacing
/// `v` kills the multiply rather than leaving it live beside the new pointer?
///
/// Two levels are enough for every shape this fires on: `v = base + i*S` and
/// `v = (base + off) + i*S`. Deeper chains are refused, which costs an
/// opportunity and never a wrong answer.
fn owns_its_multiply(f: &Func, v: ValueId, uses_total: &HashMap<ValueId, usize>) -> bool {
    fn is_mul(f: &Func, m: ValueId) -> bool {
        matches!(def_inst(f, m), Some(Inst::Bin { op: BinOp::Mul, .. }))
    }
    let sole = |m: ValueId| uses_total.get(&m).copied().unwrap_or(0) <= 1;
    let mut here = vec![v];
    for _ in 0..2 {
        let mut next = Vec::new();
        for x in here.drain(..) {
            if is_mul(f, x) && sole(x) {
                return true;
            }
            if let Some(Inst::Bin { op: BinOp::Add, a, b, .. }) = def_inst(f, x) {
                for o in [*a, *b] {
                    if let Some(y) = o.val() {
                        if sole(y) {
                            next.push(y);
                        }
                    }
                }
            }
        }
        here = next;
    }
    here.into_iter().any(|m| is_mul(f, m) && sole(m))
}

/// The instruction that defines a value, when one does.
fn def_inst(f: &Func, v: ValueId) -> Option<&Inst> {
    match f.values[v as usize].def {
        Def::Inst(b, i) => f.blocks[b as usize].insts.get(i as usize),
        _ => None,
    }
}

fn strengthen(
    f: &mut Func,
    c: &dom::Cfg,
    dt: &dom::DomTree,
    lf: &dom::LoopForest,
    li: usize,
    unit: bool,
    uses_total: &HashMap<ValueId, usize>,
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
            let (addr, acc) = match inst {
                Inst::Load { addr, ty, vol: false, .. } => (*addr, (ty.bits() / 8) as i64),
                _ => continue,
            };
            // Already a pointer walk: leave it alone, or the ladder would grow a
            // fresh pointer beside it on every round.
            if matches!(addr, Operand::Val(v) if s.ivs.contains_key(&v)) {
                continue;
            }
            let (bases, off, step) = match affine(&s, f, addr) {
                Some(a) => a,
                None => {
                    residual("scev-refused");
                    continue;
                }
            };
            if step == 0 {
                residual("invariant-address");
                continue;
            }
            // an integer recurrence, not an address
            if bases.is_empty() {
                residual("no-symbolic-base");
                continue;
            }
            // TWO COST ARGUMENTS LIVE HERE, and only one of them is the one
            // MEASURED M2 refuted. They are told apart by comparing the step to
            // the ACCESS SIZE, because that is exactly what A64's scaled index
            // can absorb (DDI 0487 C6.2.130: `ldr Xt,[Xn,Xm,lsl #3]` scales by
            // the access size and by nothing else).
            //
            //   step == access size — `p[i]`. The address rides the scaled index
            //     for free, so a walking pointer only pays if `auto_inc` then
            //     folds the add into a post-index, which needs the unscaled
            //     signed 9-bit offset. MEASURED M2 says that trade is NEGATIVE
            //     on this target (j2_histogram: identical instruction count,
            //     13% slower), so this half stays behind `ENABLED`/`ZCC_IV`.
            //
            //   step != access size — `B[k][j]` walking a 240×8-byte ROW, step
            //     1920. No addressing mode reaches it: the address is rebuilt
            //     with a MULTIPLY every iteration (`madd x12,x11,x4,x1`), and
            //     the scaled index cannot absorb a scale it does not have. The
            //     pointer replaces that multiply with an `add` — the SAME
            //     instruction count, one multiply fewer standing in front of a
            //     load. `cost = |MIR|` is blind to this by construction, so it
            //     is judged on the clock, exactly as R4.7's j3 cycle fact was:
            //     `tests/bench/matmul.c` 1.638× → 1.000× gcc -O1 (§13q).
            // LAW 3c — the thing that costs cycles is a MULTIPLY standing in
            // front of a load, not a non-unit stride as such. Two strides are
            // free on A64 and neither is worth a pointer:
            //   * step == the access size — the scaled index reaches it whole
            //     (`ldr Xt,[Xn,Xm,lsl #3]`, DDI 0487 C6.2.130). MEASURED M2.
            //   * step a POWER OF TWO — `fold::canon` has already turned
            //     `k*2^n` into `k<<n`, and isel folds `add(base, shl(k,n))` into
            //     one shifted-register `add`. Still no multiply.
            // What is left is a stride like `B[k][j]`'s 1920: no shift, no mode,
            // an honest `mul` on the address every iteration. MEASURED M9.
            let free = step.unsigned_abs() == acc.unsigned_abs()
                || step.unsigned_abs().is_power_of_two();
            if free && (!unit || step < -256 || step > 255) {
                residual("no-multiply-to-remove");
                continue;
            }
            groups.entry((bases, off, step)).or_default().push((b, i));
        }
    }
    // The CONSUMER-BLIND half, and it runs INSTEAD of the site scan above when
    // it is on: an address value it rewrites carries its loads with it, since a
    // load's address operand is one of the uses being replaced.
    if consumer_blind() {
        let vals = value_candidates(f, &s, lf, li, uses_total);
        if !vals.is_empty() {
            let mut keys: Vec<Key> = vals.keys().cloned().collect();
            keys.sort();
            for k in keys {
                let group = &vals[&k];
                let q = make_param(f, c, dt, header, k);
                for &b in &lf.loops[li].body {
                    for inst in f.blocks[b as usize].insts.iter_mut() {
                        inst.uses_mut(|o| {
                            if let Operand::Val(v) = *o {
                                if group.contains(&v) {
                                    *o = Operand::Val(q);
                                }
                            }
                        });
                    }
                }
            }
            refresh_defs(f);
            return true;
        }
    }
    if groups.is_empty() {
        return false;
    }
    // Deterministic order: identical IR must produce identical bytes
    // (`tests/determinism.sh`), and a HashMap walk does not.
    let mut keys: Vec<Key> = groups.keys().cloned().collect();
    keys.sort();
    for k in keys {
        let sites = groups.remove(&k).unwrap();
        introduce(f, c, dt, header, k, &sites);
    }
    refresh_defs(f);
    true
}

/// The address VALUES this loop rebuilds with a multiply, grouped by recurrence.
///
/// A candidate is a value defined inside the loop, of pointer width, affine in
/// the loop's counter with a step no addressing mode reaches, and used ONLY by
/// instructions inside the loop. The last condition is what makes the rewrite a
/// substitution rather than a code motion: every reader of the old value is
/// replaced, so the old computation is dead and `dce` removes it, and nothing
/// after the loop can observe the difference.
///
/// A use in a TERMINATOR disqualifies the value outright. A branch argument is a
/// use whose replacement would have to follow the edge into the successor's
/// parameter, which is a different rewrite; refusing it costs the pass nothing
/// measured and keeps the substitution local to one block's instruction list.
fn value_candidates(
    f: &Func,
    s: &scev::LoopScev,
    lf: &dom::LoopForest,
    li: usize,
    // HOW OFTEN EACH VALUE IS READ IN THE WHOLE FUNCTION — a fact about the
    // FUNCTION, not about this loop, so it is counted once by the caller and
    // handed down. Rebuilding it here walked every block of the function, and
    // hashed every operand of every instruction, once per loop per round: on
    // `s0940` that is 10,389 walks of the same unchanging thing, and it was the
    // whole of a compile taking ten times gcc's.
    uses_total: &HashMap<ValueId, usize>,
) -> HashMap<Key, Vec<ValueId>> {
    use std::collections::HashSet;
    let body: HashSet<BlockId> = lf.loops[li].body.iter().copied().collect();

    // The access size of every load or store that reads a value AS ITS ADDRESS.
    // A step equal to it rides A64's scaled index for free (DDI 0487 C6.2.130),
    // so there is no multiply to remove — MEASURED M2 again, asked at the value.
    let mut acc_of: HashMap<ValueId, i64> = HashMap::new();
    for &b in &lf.loops[li].body {
        for inst in &f.blocks[b as usize].insts {
            let (addr, sz) = match inst {
                Inst::Load { addr, ty, .. } => (*addr, (ty.bits() / 8) as i64),
                Inst::Store { addr, ty, .. } => (*addr, (ty.bits() / 8) as i64),
                _ => continue,
            };
            if let Some(v) = addr.val() {
                let e = acc_of.entry(v).or_insert(sz);
                *e = (*e).min(sz);
            }
        }
    }

    // Where each value is read: inside the loop's instructions, or anywhere else
    // (another block, or any terminator) — the second kind disqualifies it.
    //
    // ONLY THE BODY IS WALKED, because the rest is arithmetic. "Anywhere else"
    // is every use that is not a body INSTRUCTION — a non-body instruction, a
    // non-body terminator, or a body terminator — and `uses_total` already
    // counts every use in the function. So a value escapes exactly when it is
    // read more often than the body's instructions read it, and the walk over
    // blocks this loop does not contain buys nothing. That walk was the second
    // of two over the whole function, per loop, per round.
    let mut read_in_loop: HashMap<ValueId, usize> = HashMap::new();
    for &b in &lf.loops[li].body {
        for inst in &f.blocks[b as usize].insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    *read_in_loop.entry(v).or_insert(0) += 1;
                }
            });
        }
    }
    let escaped = |v: ValueId, inside: usize| {
        uses_total.get(&v).copied().unwrap_or(0) > inside
    };

    // A probe, while the gate is being measured (`ZCC_IVXCALL`): fire only in a
    // loop that CALLS. A call forces every live value into a callee-saved
    // register or a slot anyway, so the walking pointer competes for a register
    // the multiply's inputs were already paying for — the pressure argument
    // above does not apply there.
    // AND THE LOOP MUST BE LONG ENOUGH TO AMORTIZE THE SETUP. The walking
    // pointer costs a header parameter and a preheader computation — one add per
    // invariant term, and a multiply of its own where the start value is itself
    // strided. `n2_varint_record` pays that on a blob-compare loop of a handful
    // of bytes nested inside the record loop, and lost 13%: the multiply moved
    // into the preheader instead of dying, which the static count showed
    // exactly (19 multiplies before, 19 after). A trip count SCEV can bound, and
    // bound above 32, is the evidence that the loop runs often enough for the
    // exchange to pay. A loop whose bound is a runtime length has no such
    // evidence and is refused — a residual to measure under Law 4, not a
    // theorem's limit.
    match s.trips {
        Some(t) if t >= 32 => {}
        _ => {
            residual("trip-count-unproven-or-short");
            return HashMap::new();
        }
    }
    if std::env::var("ZCC_IVXCALL").is_ok() {
        let calls = lf.loops[li].body.iter().any(|&b| {
            f.blocks[b as usize]
                .insts
                .iter()
                .any(|i| matches!(i, Inst::Call { .. }))
        });
        if !calls {
            return HashMap::new();
        }
    }
    let mut out: HashMap<Key, Vec<ValueId>> = HashMap::new();
    for &b in &lf.loops[li].body {
        for inst in &f.blocks[b as usize].insts {
            let v = match inst {
                Inst::Bin { dst, ty: Ty::I64, .. } => *dst,
                _ => continue,
            };
            // TWO uses at least, and the reason is what the first measurement
            // said. The pointer is a value live across the whole loop body, so
            // it costs a register where the multiply cost none: on
            // `k1_dispatch` and `k2_live_pressure` — the two programs in the
            // suite whose loops are already at the pressure limit — the rewrite
            // ADDED spill traffic (42 → 47 and 72 → 75 stack references) and
            // both got slower. A single-use address pays the register and buys
            // one instruction; an address read twice or more amortizes it, which
            // is `n7_nested_subq`'s shape (one `&inner[j]` handed to two
            // predicates) and where the win was measured.
            let inside = read_in_loop.get(&v).copied().unwrap_or(0);
            if escaped(v, inside) || inside < 2
                || s.ivs.contains_key(&v)
            {
                continue;
            }
            let (bases, off, step) = match affine(s, f, Operand::Val(v)) {
                Some(a) => a,
                None => continue,
            };
            if step == 0 || bases.is_empty() {
                continue;
            }
            let acc = acc_of.get(&v).copied().unwrap_or(0);
            if step.unsigned_abs().is_power_of_two() || step.unsigned_abs() == acc.unsigned_abs() {
                residual("no-multiply-to-remove");
                continue;
            }
            // AND THE MULTIPLY MUST ACTUALLY DIE. `n2_varint_record` measured
            // this one: the rewrite fired, the pointer was built, and the
            // multiply count did not move — because the same product also feeds
            // something that is not an address, so it stays live and the loop
            // pays for both. The pass then costs 7 instructions for nothing and
            // the program lost 12%. A candidate therefore has to own its
            // multiply: some `mul` in the address's own computation must have
            // this value as its ONLY reader.
            if !owns_its_multiply(f, v, &uses_total) {
                residual("multiply-shared");
                continue;
            }
            out.entry((bases, off, step)).or_default().push(v);
        }
    }
    for g in out.values_mut() {
        g.sort();
        g.dedup();
    }
    out
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
    let q = make_param(f, c, dt, header, k);
    for &(b, i) in sites {
        match &mut f.blocks[b as usize].insts[i] {
            Inst::Load { addr, .. } => *addr = Operand::Val(q),
            _ => unreachable!("a site is a load"),
        }
    }
}

/// The header parameter that walks `base + off + step·n`: entry edges supply the
/// start value, latch edges the step. Shared by both halves of the pass — the
/// load-site rewrite and the consumer-blind value substitution — because the
/// recurrence is the same theorem either way.
fn make_param(f: &mut Func, c: &dom::Cfg, dt: &dom::DomTree, header: BlockId, k: Key) -> ValueId {
    let (bases, off, step) = k;
    let idx = f.blocks[header as usize].params.len() as u32;
    let q = f.new_value(Ty::I64, Def::Param(header, idx));
    f.blocks[header as usize].params.push(q);

    for &p in &c.preds[header as usize] {
        // A predecessor the header DOMINATES is a latch; anything else is an
        // entry. The distinction is the whole of the initialization rule: an
        // entry supplies the start value, a latch supplies the step.
        let arg = if dt.dominates(header, p) {
            append(f, p, Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: Operand::Val(q), b: Operand::Imm(step) })
        } else {
            // The start value. Every term is loop-invariant, so summing them on
            // the entry edge computes ONCE what the old address recomputed on
            // every iteration; when there is a single term and no offset there
            // is nothing to compute and the start IS the base.
            let mut acc = Operand::Val(bases[0]);
            for &t in &bases[1..] {
                acc = append(f, p, Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: acc, b: Operand::Val(t) });
            }
            if off != 0 {
                acc = append(f, p, Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: acc, b: Operand::Imm(off) });
            }
            acc
        };
        for t in f.blocks[p as usize].term.targets_mut() {
            if t.block == header {
                t.args.push(arg);
            }
        }
    }
    q
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

// ── induction-variable WIDENING (MECHANISM.md Part F) ──────────────────────────────
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
/// `ZCC_FVDBG=1` counts the loops a FINAL-VALUE pass could close: a known trip
/// count, and a body that computes nothing but affine accumulators and the
/// counter. Law 4 asks for the size of an opportunity before it is built.
pub fn fv_wanted() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_FVDBG").is_ok())
}

pub fn fv_opportunity(f: &Func) {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    for li in 0..lf.loops.len() {
        let s = match scev::LoopScev::analyze(f, &c, &dt, &lf, li) {
            Some(s) => s,
            None => continue,
        };
        let n = s.trips;
        // every instruction in the body must be affine over the loop
        // The loop's own exit test is CONTROL, not data — a closed form replaces
        // the accumulators and deletes the test, so requiring the compare to be
        // affine would report every loop as unclosable. (It did: the first cut
        // of this counter said 0 everywhere, including on a hand-written
        // known-positive. Validate the oracle before believing the verdict.)
        let mut conds: Vec<ValueId> = Vec::new();
        for &b in &lf.loops[li].body {
            if let Term::Br(Operand::Val(v), ..) = &f.blocks[b as usize].term {
                conds.push(*v);
            }
        }
        let mut closed = true;
        let mut insts = 0;
        for &b in &lf.loops[li].body {
            for inst in &f.blocks[b as usize].insts {
                let d = match inst.dst() {
                    Some(d) => d,
                    None => {
                        closed = false;
                        continue;
                    }
                };
                if conds.contains(&d) {
                    continue;
                }
                insts += 1;
                if s.eval(f, Operand::Val(d)).is_none() {
                    closed = false;
                }
            }
        }
        if closed && insts > 0 {
            eprintln!("FV closable trips={:?} insts={} fn={}", n, insts, f.name);
        }
    }
}

pub fn widen(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    // Counted once for the whole function: it does not depend on which loop is
    // being strengthened, and `strengthen` asks it of every loop.
    let mut uses_total: HashMap<ValueId, usize> = HashMap::new();
    for b in f.blocks.iter() {
        for inst in &b.insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    *uses_total.entry(v).or_insert(0) += 1;
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                *uses_total.entry(v).or_insert(0) += 1;
            }
        });
    }
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
        Operand::Val(v) if s.is_loop_invariant(f, v) => {}
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

// ── INDUCTION-VARIABLE SUBSTITUTION (§13q ii; gcc's IV canonicalization, the
//    half of `-ftree-slsr` that owns d2_nested_loops) ────────────────────────
//
// THE MEASUREMENT. `for (k=0;k<n;k++) s += (i*j+k) & 31;`
//
//     zcc  add w7,w5,w4 ; and w7,w7,#31 ; add x6,x6,x7 ; add w4,w4,#1 ; cmp w4,w0 ; b.lt
//     gcc  and x2,x1,31 ; add x0,x0,x2  ; add w1,w1,1 ; cmp w1,w3    ; bne
//
// gcc runs `i*j + k` AS the induction variable: it starts at `i*j`, steps by
// one, and the exit bound becomes `n + i*j`. The add that rebuilt the value on
// every iteration is gone, and the mask reads its input a cycle earlier. Six
// instructions against five, and 1.556 against 1.000 on the clock — Law 3c's
// second kind of gap, where the count moves by ONE and the time by half.
//
// THE TRANSFORM. For a counted loop with an I32 counter `k` and one or more
// values `t = inv + k` where `inv` is loop-invariant, introduce a 64-bit header
// parameter `q` holding `sext(inv) + sext(k)`, rewrite the exit test onto it,
// and let `k` die. Each `t` becomes `trunc(q)` in place.
//
// COMMUTING SQUARE, and it is why the parameter is WIDE. `SEMANTICS.md` defines
// signed overflow as WRAPPING, so the C-level "signed overflow is undefined"
// argument gcc uses is not available here and the rewrite has to be exact under
// wrapping. In I32 it is not: shifting `k <s bound` by `inv` flips at the sign
// boundary, and the corner is REACHABLE (`inv + bound - 1 == INT_MAX` exits on
// the first test instead of the last). In I64 it is exact, unconditionally:
//   * `sext(k_n) = sext(k_0) + step·n` — this is `no_wrap_signed(k)`, the fact
//     `widen` already needs and `scev::find_nowrap` already proves;
//   * `q_n = sext(inv) + sext(k_n)` cannot overflow — both terms are 32-bit
//     ranged, so the sum needs 33 bits;
//   * `t_n = trunc32(q_n)` holds with NO side condition, because truncating an
//     exact sum yields the wrapping sum, which is what `t` meant;
//   * `k <s bound ⟺ sext(k) <s sext(bound) ⟺ q <s sext(bound)+sext(inv)`,
//     adding one constant to both sides of a comparison in ℤ, with no 64-bit
//     wrap to spoil it. The bound is computed ONCE, in the entry block.
//
// WHAT IS REFUSED: a loop where the counter also feeds a `sext` — that is
// `widen`'s row and running both would grow a second wide counter beside this
// one; an unsigned exit test, whose `zext` twin is a fact this does not claim;
// a bound that is not defined outside the loop; and any other use of the
// counter, since a surviving `k` means the `add` this pass removes comes
// straight back as the counter's own step.

/// THEORY A7b  SQUARE an_invariant_plus_the_counter_becomes_the_counter — Law 3c
pub fn substitute(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    // Counted once for the whole function: it does not depend on which loop is
    // being strengthened, and `strengthen` asks it of every loop.
    let mut uses_total: HashMap<ValueId, usize> = HashMap::new();
    for b in f.blocks.iter() {
        for inst in &b.insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    *uses_total.entry(v).or_insert(0) += 1;
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                *uses_total.entry(v).or_insert(0) += 1;
            }
        });
    }
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        if substitute_loop(f, &c, &dt, &lf, li) {
            return true;
        }
    }
    false
}

fn substitute_loop(
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
        .filter(|(p, r)| r.ty == Ty::I32 && r.base.is_none() && s.no_wrap_signed(**p))
        .map(|(p, r)| (*p, *r))
        .collect();
    ivs.sort_by_key(|(p, _)| *p);
    for (p, rec) in ivs {
        if let Some((plan, shift)) = plan_substitute(f, &s, header, &latches, p, rec) {
            apply_substitute(f, header, entry, &latches, p, rec, plan, shift);
            refresh_defs(f);
            return true;
        }
    }
    false
}

/// The counter's uses, sorted into the ones this rewrite absorbs and the ones
/// that refuse it. Returns the plan and the invariant term the new parameter
/// carries.
fn plan_substitute(
    f: &Func,
    s: &scev::LoopScev,
    header: BlockId,
    latches: &[BlockId],
    p: ValueId,
    rec: scev::AddRec,
) -> Option<(Plan, ValueId)> {
    let mut step_val = None;
    let mut cmp: Option<(BlockId, usize, ValueId, CmpOp, Operand, bool)> = None;
    // `inv` → the values `inv + p` computed in this loop. Deterministic order:
    // identical IR must produce identical bytes.
    let mut groups: std::collections::BTreeMap<ValueId, Vec<ValueId>> =
        std::collections::BTreeMap::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for (i, inst) in b.insts.iter().enumerate() {
            match inst {
                // The counter feeding a widening is `widen`'s row, not this one.
                Inst::Cvt { op: CvtOp::Sext, a, .. } if *a == Operand::Val(p) => return None,
                Inst::Bin { dst, op: BinOp::Add, ty: Ty::I32, a, b: rhs } => {
                    if *a == Operand::Val(p) && *rhs == Operand::Imm(rec.step) {
                        if step_val.is_some() {
                            return None;
                        }
                        step_val = Some(*dst);
                        continue;
                    }
                    let other = match (*a, *rhs) {
                        (Operand::Val(x), o) if x == p => o,
                        (o, Operand::Val(x)) if x == p => o,
                        _ => continue,
                    };
                    if let Some(v) = other.val() {
                        if s.is_loop_invariant(f, v) {
                            groups.entry(v).or_default().push(*dst);
                        }
                    }
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
    // The invariant term with the most sites; the smallest id breaks a tie so
    // the choice does not depend on a hash walk.
    let (shift, subs) = groups.into_iter().max_by_key(|(v, ss)| (ss.len(), std::cmp::Reverse(*v)))?;
    if cmp_after_step && !latches.contains(&cb) {
        return None;
    }
    if !matches!(op, CmpOp::Slt | CmpOp::Sle | CmpOp::Sgt | CmpOp::Sge | CmpOp::Eq | CmpOp::Ne) {
        return None;
    }
    // Defined OUTSIDE the loop, not merely unchanging inside it — the shifted
    // bound and the widened start are both materialized in the entry block.
    match bound {
        Operand::Imm(_) => {}
        Operand::Val(v) if s.is_loop_invariant(f, v) => {}
        _ => return None,
    }
    if !s.is_loop_invariant(f, shift) || f.ty_of(shift) != Ty::I32 {
        return None;
    }
    // Every reader of the counter and of its step must be one this rewrite
    // moves: a substituted add, the step, the test, or a header edge argument.
    let ok_use = |user: &Inst| -> bool {
        match user {
            Inst::Bin { dst, .. } => *dst == step_val || subs.contains(dst),
            Inst::Cmp { dst, .. } => *dst == cmp_dst,
            _ => false,
        }
    };
    for b in &f.blocks {
        for inst in &b.insts {
            let mut bad = false;
            inst.uses(|o| {
                if (o == Operand::Val(p) || o == Operand::Val(step_val)) && !ok_use(inst) {
                    bad = true;
                }
            });
            if bad {
                return None;
            }
        }
        for t in b.term.targets() {
            if t.block == header {
                continue;
            }
            if t.args.iter().any(|a| *a == Operand::Val(p) || *a == Operand::Val(step_val)) {
                return None;
            }
        }
        match &b.term {
            Term::Br(x, ..) | Term::Switch(x, ..) | Term::GotoPtr(x, _) | Term::Ret(Some(x)) => {
                if *x == Operand::Val(p) || *x == Operand::Val(step_val) {
                    return None;
                }
            }
            _ => {}
        }
    }
    Some((
        Plan { sexts: subs, step_val, cmp_at: (cb, ci), cmp_after_step, bound, op, cmp_dst },
        shift,
    ))
}

fn apply_substitute(
    f: &mut Func,
    header: BlockId,
    entry: BlockId,
    latches: &[BlockId],
    p: ValueId,
    rec: scev::AddRec,
    plan: Plan,
    shift: ValueId,
) {
    let idx = f.blocks[header as usize].params.len() as u32;
    let q = f.new_value(Ty::I64, Def::Param(header, idx));
    f.blocks[header as usize].params.push(q);

    // entry: `sext(inv)`, then the start `sext(inv) + k0` and the shifted bound.
    // Both are loop-invariant and are computed exactly once.
    let winv = append_cvt(f, entry, CvtOp::Sext, Ty::I32, Ty::I64, Operand::Val(shift));
    let start = append(
        f,
        entry,
        Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: winv, b: Operand::Imm(rec.off) },
    );
    for t in f.blocks[entry as usize].term.targets_mut() {
        if t.block == header {
            t.args.push(start);
        }
    }
    let wide_bound = match plan.bound {
        Operand::Imm(k) => append(
            f,
            entry,
            Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: winv, b: Operand::Imm(k) },
        ),
        o => {
            let wb = append_cvt(f, entry, CvtOp::Sext, Ty::I32, Ty::I64, o);
            append(f, entry, Inst::Bin { dst: 0, op: BinOp::Add, ty: Ty::I64, a: winv, b: wb })
        }
    };

    // latches: the wide step, placed before the test in the block that holds it.
    let (cb, mut ci) = plan.cmp_at;
    let mut wstep = Vec::new();
    for &l in latches {
        let at = if l == cb { ci } else { f.blocks[l as usize].insts.len() };
        let v = insert_bin(f, l, at, BinOp::Add, Ty::I64, Operand::Val(q), Operand::Imm(rec.step));
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

    // Each `inv + k` becomes a truncation of the wide parameter, IN PLACE: same
    // value, same block, same index, so every reader keeps reading what it read.
    // The truncation is what `mir/pass/ext.rs` then absorbs into a consumer that
    // masks or re-widens (`and w,w,#31` + `sext` becomes one `and x,q,#31`).
    for &t in &plan.sexts {
        for b in f.blocks.iter_mut() {
            for inst in b.insts.iter_mut() {
                if inst.dst() == Some(t) {
                    *inst = Inst::Cvt {
                        dst: t,
                        op: CvtOp::Trunc,
                        from: Ty::I64,
                        to: Ty::I32,
                        a: Operand::Val(q),
                    };
                }
            }
        }
    }

    let lhs = if plan.cmp_after_step {
        Operand::Val(
            wstep
                .iter()
                .find(|(l, _)| *l == cb)
                .expect("the after-step test is in a latch")
                .1,
        )
    } else {
        Operand::Val(q)
    };
    f.blocks[cb as usize].insts[ci] =
        Inst::Cmp { dst: plan.cmp_dst, op: plan.op, ty: Ty::I64, a: lhs, b: wide_bound };

    // The counter is dead by construction, and is deleted here for the same
    // reason `apply_widen` deletes its own: what is left is a CYCLE the use-count
    // sweep reads as live, and the loop would keep a second counter for nothing.
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

// ── COUNT DOWN, so the branch reads the flags the step already set ─────────
//
// THE MEASUREMENT. `revbits`: `for (i=0;i<32;i++) { r=(r<<1)|(x&1); x>>=1; }`
//
//     zcc  and w3,w2,#1 ; orr w1,w3,w1,lsl 1 ; lsr w2,w2,#1 ; add w0,w0,#1 ; cmp w0,#32 ; b.lt
//     gcc  and w3,w1,1  ; orr w0,w3,w0,lsl 1 ; lsr w1,w1,1  ; subs w2,w2,#1 ;             bne
//
// Six instructions against five, and 6/5 = 1.20 against a measured EXEC of
// 1.194 — the whole gap is the separate `cmp`. gcc counts DOWN, so the step sets
// the flags the branch needs. `cmp w0,#32` cannot be fused by `cmp_elim`: it
// compares against a bound, not against zero.
//
// What the TIME model said about this loop is the instructive part: recurrence
// 2, and gcc's is 2 as well. The gap was never latency. The model earned its
// keep by ruling the recurrence OUT — this row is a count fix, not a chain fix.
//
// THE TRANSFORM. When a counted loop's index is read by NOTHING but its own step
// and the exit test, its VALUES are unobservable; only the number of iterations
// is. So run it from `trips` down to zero and test against zero.
//
// COMMUTING SQUARE. `scev::trips` is the number of times the BODY runs, claimed
// only for a single-exit loop testing a literal-bounded induction variable from
// a literal start. The body runs `trips` times before and after: the counter
// takes `trips, trips-1, …, 1` and the step reaches 0 exactly once. Nothing else
// can observe the difference, because the fence below establishes that no other
// instruction reads the index — that IS the argument.
//
// IDEMPOTENCE IS PART OF THE PROOF, not a detail. The pass ladder is a FIXPOINT:
// it re-runs until nothing changes, so a rewrite that does not recognize its own
// output runs again on it. The first cut checked `step == -1 && off == trips`,
// which looks sufficient and is not — on the second round `scev` re-derives a
// trip count from the REWRITTEN `!= 0` test and returned 1, so the loop was
// rebuilt to start at 1 and `revbits` ran a single iteration. That is a
// MISCOMPILE (6442439334100992 became 1500000), caught by the batteries and by
// comparing output against gcc before anything was banked. The guard is now the
// SHAPE of the exit test — already testing `!= 0` means already done — which
// cannot be re-derived into something else.
//
// WHAT IS REFUSED: a TOP-tested loop, where the test reads the index before the
// step; an index with any other reader, a `sext` included (that is `widen`'s
// row); a trip count the analysis will not state; a count that does not fit the
// index's type.

/// THEORY A7b  SQUARE a_counted_loop_counts_down_and_the_compare_disappears
pub fn countdown(f: &mut Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    // Counted once for the whole function: it does not depend on which loop is
    // being strengthened, and `strengthen` asks it of every loop.
    let mut uses_total: HashMap<ValueId, usize> = HashMap::new();
    for b in f.blocks.iter() {
        for inst in &b.insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    *uses_total.entry(v).or_insert(0) += 1;
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                *uses_total.entry(v).or_insert(0) += 1;
            }
        });
    }
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    for li in order {
        if countdown_loop(f, &c, &dt, &lf, li) {
            refresh_defs(f);
            return true;
        }
    }
    false
}

fn countdown_loop(
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
    let trips = match s.trips {
        Some(t) if t >= 1 => t,
        _ => return false,
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
    if entries.len() != 1 || latches.len() != 1 {
        return false;
    }
    let mut ivs: Vec<(ValueId, scev::AddRec)> =
        s.ivs.iter().filter(|(_, r)| r.base.is_none()).map(|(p, r)| (*p, *r)).collect();
    ivs.sort_by_key(|(p, _)| *p);
    for (p, rec) in ivs {
        let ty = f.ty_of(p);
        if ty.is_float() {
            continue;
        }
        let limit: u64 = if ty.bits() == 32 { i32::MAX as u64 } else { i64::MAX as u64 };
        if trips > limit {
            continue;
        }
        let mut step_val = None;
        let mut bad = false;
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Bin { dst, op: BinOp::Add, a, b: rhs, .. } = inst {
                    if *a == Operand::Val(p) && *rhs == Operand::Imm(rec.step) {
                        if step_val.is_some() {
                            bad = true;
                        }
                        step_val = Some(*dst);
                    }
                }
            }
        }
        let sv = match step_val {
            Some(v) if !bad => v,
            _ => continue,
        };
        // The exit test, which must read the STEPPED value: that is the
        // bottom-tested shape, the one whose arithmetic this proof is about.
        let mut cmp = None;
        for (bi, b) in f.blocks.iter().enumerate() {
            for (i, inst) in b.insts.iter().enumerate() {
                if let Inst::Cmp { dst, op, a, b: rhs, .. } = inst {
                    if *a == Operand::Val(sv) || *a == Operand::Val(p) {
                        if cmp.is_some() {
                            bad = true;
                        }
                        cmp = Some((bi as BlockId, i, *dst, *a == Operand::Val(sv), *op, *rhs));
                    }
                }
            }
        }
        let (cb, ci, cmp_dst, after_step, op, rhs) = match cmp {
            Some(x) if !bad && x.3 => x,
            _ => continue,
        };
        let _ = after_step;
        // THE COMPARE MUST BE THE LOOP'S EXIT TEST, and asserting that is not a
        // formality — it is the fence this pass shipped without and the torture
        // corpus caught within the hour (`961017-2`). That program runs TWO
        // induction variables: `z`, whose `while (z > 0)` is the real exit, and
        // `i`, whose only compare is an overflow guard `if (i > 0x40000) abort()`
        // in the middle of the body. A scan that takes the first compare it finds
        // on an index rewrote the GUARD into `!= 0`, and the program aborted
        // immediately. Being read only by a step and a compare is not enough; the
        // compare has to be the one that ends the loop.
        let leaves_loop = match &f.blocks[cb as usize].term {
            Term::Br(Operand::Val(c), t, e) if *c == cmp_dst => {
                let ins = |b: BlockId| b == header || lf.loops[li].body.contains(&b);
                ins(t.block) != ins(e.block)
            }
            _ => false,
        };
        if !leaves_loop {
            continue;
        }
        // ALREADY a countdown. The ladder is a fixpoint and re-runs this pass on
        // its own output; see the note above for what happened when the guard
        // was a property of the trip count instead of the shape of the test.
        if op == CmpOp::Ne && rhs == Operand::Imm(0) {
            continue;
        }
        let pi = match f.blocks[header as usize].params.iter().position(|&x| x == p) {
            Some(i) => i,
            None => continue,
        };
        for b in &f.blocks {
            for inst in &b.insts {
                inst.uses(|o| {
                    if o == Operand::Val(p) || o == Operand::Val(sv) {
                        let d = inst.dst();
                        if d != Some(sv) && d != Some(cmp_dst) {
                            bad = true;
                        }
                    }
                });
            }
            for t in b.term.targets() {
                // A SIBLING HEADER PHI FED BY THE SAME LATCH VALUE is the hole
                // this fence closes (`c9330`, 2026-08-27). The rewrite below is
                // done in place on the step instruction, so every header param
                // whose latch argument is that value starts counting down —
                // but only slot `pi` gets its entry argument restamped to the
                // trip count. A duplicate index phi (one slot read by the exit
                // test, the other by `&a[i][j]`) therefore kept its 0 start and
                // then followed the descending step: the loop wrote `a[1][7]`
                // where the source says `a[4][7]`. Being read only by a step and
                // a compare is not enough; the step must flow into NO header
                // slot but our own.
                if t.block == header {
                    if t.args.iter().enumerate().any(|(k, a)| {
                        k != pi && (*a == Operand::Val(p) || *a == Operand::Val(sv))
                    }) {
                        bad = true;
                    }
                } else if t.args.iter().any(|a| *a == Operand::Val(p) || *a == Operand::Val(sv)) {
                    bad = true;
                }
            }
            match &b.term {
                Term::Br(x, ..) | Term::Switch(x, ..) | Term::GotoPtr(x, _) | Term::Ret(Some(x)) => {
                    if *x == Operand::Val(p) || *x == Operand::Val(sv) {
                        bad = true;
                    }
                }
                _ => {}
            }
        }
        if bad {
            continue;
        }
        for t in f.blocks[entries[0] as usize].term.targets_mut() {
            if t.block == header {
                t.args[pi] = Operand::Imm(trips as i64);
            }
        }
        for b in f.blocks.iter_mut() {
            for inst in b.insts.iter_mut() {
                if inst.dst() == Some(sv) {
                    *inst = Inst::Bin {
                        dst: sv,
                        op: BinOp::Sub,
                        ty,
                        a: Operand::Val(p),
                        b: Operand::Imm(1),
                    };
                }
            }
        }
        f.blocks[cb as usize].insts[ci] =
            Inst::Cmp { dst: cmp_dst, op: CmpOp::Ne, ty, a: Operand::Val(sv), b: Operand::Imm(0) };
        return true;
    }
    false
}
