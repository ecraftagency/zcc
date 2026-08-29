// redjam — four iterations of a COUNTED REDUCTION loop run as four lanes, each
// with its own partial accumulator.
// THEORY A7b — optimization: this pass ships its commuting square
//
// THE MEASUREMENT THAT ASKED FOR THIS. `perf` on the Graviton box, over the
// twelve worst programs of the 96-program suite against gcc -O2, says every one
// of them is COUNT-driven and none is chain-driven: zcc's IPC is 4.1 to 6.7 and
// it retires 1.4x to 4.6x as many instructions. Reading the five worst by
// dynamic count, they are one shape written five ways:
//
//     a1_int_mix   for(i=1;i<=n;i++)  s += (long)a - b + (i%5);
//     a3_sdiv_mod  for(i=1;i<=n;i++)  s += (i/7) - (i%11);
//     d4_goto      loop: s += (i&3)? i : -i; i++; goto loop;
//     e2_many_args for(k=0;k<4000000;k++) s += mix(k,k+1,...,k+9);
//     h2_revbits   for(k=0;k<3000000u;k++) s += revbits(k);
//
// A counted loop carrying nothing but a counter and ONE accumulator, over a body
// that is otherwise a function of the counter alone. gcc -O2 runs four of them at
// a time and finishes with a horizontal add; zcc runs them one at a time and pays
// the counter's increment, compare and branch on every element.
//
// WHY IT IS NOT `unroll` AND NOT `jam`. `unroll` fully unrolls a loop whose trip
// count is a small LITERAL — these trip counts are millions and three of the five
// are runtime values. `jam` needs a two-deep nest and takes its lanes from the
// OUTER counter — these loops have no inner loop at all. The machinery is shared
// with both (the guard/tail structure is `jam`'s, the counter analysis is
// `unroll`'s) and the recognizer is neither's.
//
// WHY FOUR. The same number for the same reason as `jam` (MEASURED M55): four is
// the lane count of a `q` register at a 32-bit element, and this row exists to
// put the four lanes where a MIR pass can pack them.
//
// COMMUTING SQUARE `⟦f⟧ = ⟦redjam f⟧`, and the reassociation is the whole of it.
//
//   * SLOW ARM — the ORIGINAL loop is cloned untouched and is both the tail (a
//     trip count that is not a multiple of four) and the refused case (fewer
//     than four iterations in total). ⟦f⟧ = ⟦f'⟧ there by construction.
//   * FAST ARM — lane `l` computes the body at counter value `i+l·s`, which is
//     exactly what iteration `i+l` computed: the body reads NOTHING carried but
//     the counter (checked, not assumed), so substituting the lane's counter is
//     the identity on it.
//   * THE SUM. The four lanes' partial accumulators are added at the exit. This
//     REASSOCIATES the accumulator's additions, and the argument that it is
//     sound is not "overflow is undefined" — it is that the addition performed
//     is two's-complement modular arithmetic, which is associative and
//     commutative, so the final residue does not depend on the grouping. No
//     intermediate is observable: A64 `add` does not trap. The row is therefore
//     exact for unsigned AND for signed, and is refused for floating point,
//     where reassociation changes the value and gcc does not do it either.
use super::*;
use std::collections::{HashMap, HashSet};

/// MEASURED M55 — four is the lane count of a `q` register at a 32-bit element
/// (DDI 0487 C1.3.2), the same constant `jam` unrolls by and for the same reason.
const LANES: i64 = 4;

/// THEORY A7b — the pass ships default-OFF until its A/B is on the board, as
/// `vecmap` did. A row that reassociates an accumulator is not one to ship on an
/// argument.
pub fn wanted() -> bool {
    WANT.with(|c| c.get()).unwrap_or_else(|| {
        static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *W.get_or_init(|| std::env::var_os("ZCC_REDJAM").is_some())
    })
}

thread_local! {
    // THEORY A7b — instrument half. Not a value the compiler computes with: it is
    // the switch a battery flips to build the same function BOTH ways, which is
    // the only shape this pass's square can take.
    //
    // A thread-local overlay over the environment, for the reason `vecmap`'s is
    // one: the battery runs its tests in parallel threads and a process-wide
    // switch would make one test's result depend on another's timing. `None`
    // means "ask the environment", which is what every non-test caller gets.
    static WANT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force the row on or off for the CURRENT THREAD, or hand the decision back to
/// the environment. A theorem that ships disabled still owes its square.
pub fn set_wanted(on: Option<bool>) {
    WANT.with(|c| c.set(on));
}

/// THEORY A7b — instrument half: loops jammed, so an A/B can tell "bought
/// nothing" from "never fired".
pub static FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Everything the rewrite needs, decided before a single block is touched.
pub struct Red {
    /// the loop's single block: header, body and latch at once
    pub header: BlockId,
    /// the one block outside the loop that enters it
    pub entry: BlockId,
    /// header parameter index of the counter
    pub ip: usize,
    /// header parameter index of the accumulator
    pub accp: usize,
    /// the counter's literal step
    pub step: i64,
    /// the bound the counter is tested against, and the test
    pub n: Operand,
    pub cmp: CmpOp,
    /// the type the counter and the bound are compared in
    pub cty: Ty,
    /// where the loop goes when the test fails
    pub exit: Target,
}

fn why(f: &Func, b: BlockId, reason: &str) -> Option<Red> {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *W.get_or_init(|| std::env::var_os("ZCC_REDJAMDBG").is_some()) {
        eprintln!("[redjam-refused] {} b{} {}", f.name, b, reason);
    }
    None
}

/// The census half: report what the recognizer accepts, changing nothing. Run
/// with `ZCC_REDJAMDBG=1` to see every refusal beside it.
pub fn census(f: &Func, a: &mut Analyses) {
    if !std::env::var_os("ZCC_REDJAMPROBE").is_some() {
        return;
    }
    let (c, _dt, lf) = a.all(f);
    eprintln!("[redjam] {} loops={}", f.name, lf.loops.len());
    for li in 0..lf.loops.len() {
        if let Some(r) = recognize(f, &c, &lf, li) {
            eprintln!(
                "[redjam] {} loop@b{} counter=p{} acc=p{} step={} bound={:?} cmp={:?} body={}",
                f.name, r.header, r.ip, r.accp, r.step, r.n, r.cmp,
                f.blocks[r.header as usize].insts.len()
            );
        }
    }
}

/// THE SHAPE, and every condition is a refusal that prints its own name.
///
/// A single-block innermost loop is the whole of the first cut: all five
/// programs that asked for the row have one, and a multi-block body needs the
/// clone to carry control flow the lane substitution would have to follow. What
/// it refuses is recorded as residual (Law 3, exhaustion), not as an oversight.
pub fn recognize(f: &Func, c: &dom::Cfg, lf: &dom::LoopForest, li: usize) -> Option<Red> {
    let l = &lf.loops[li];
    let header = l.header;
    // INNERMOST and single-block: header, body and latch are one block, so the
    // clone is a flat instruction list and no edge inside a lane exists to
    // rewrite.
    if lf.loops.iter().any(|x| x.parent == Some(li as u32)) {
        return why(f, header, "not-innermost");
    }
    if l.body.len() != 1 {
        return why(f, header, "multi-block-body");
    }
    if l.latches.len() != 1 || l.latches[0] != header {
        return why(f, header, "latch-is-not-the-header");
    }

    // PURE. Four copies of the body run where one ran, and they run interleaved
    // with each other; anything with an effect would be duplicated and reordered.
    // A load is pure only because the loop has no store: with no store in the
    // loop, no lane's read can observe another lane's write.
    for inst in &f.blocks[header as usize].insts {
        if !matches!(inst.effect(), Effect::Pure) {
            return why(f, header, "body-not-pure");
        }
    }

    // THE TEST, at the latch — `rotate` runs long before this pass and leaves a
    // bottom-tested loop, so the staying side of the terminator is the header
    // itself.
    let (cond, t_back, t_exit) = match &f.blocks[header as usize].term {
        Term::Br(cd, t1, t2) if t1.block == header && t2.block != header => {
            (*cd, t1.clone(), t2.clone())
        }
        Term::Br(cd, t1, t2) if t2.block == header && t1.block != header => {
            (*cd, t2.clone(), t1.clone())
        }
        _ => return why(f, header, "no-two-way-latch"),
    };
    let (cmp_a, cmp_b, cmp, cty) = match def_inst(f, cond.val()?)? {
        Inst::Cmp { a, b, op, ty, .. } => (*a, *b, *op, *ty),
        _ => return why(f, header, "test-not-a-compare"),
    };
    if cty.is_float() {
        return why(f, header, "counter-is-floating-point");
    }
    // `i < n` or `i <= n`, signed or unsigned. The bound is whatever the compare
    // reads and it may be a runtime value — that is the difference from `unroll`,
    // which needs a literal because it decides the guard rather than guarding it.
    //
    // POLARITY IS NOT PART OF THE SHAPE. `d4_goto` writes its loop as
    // `if (i >= n) goto done;`, so the compare is `Sge` and the STAYING side is
    // the false one. That is the same loop with the same bound; refusing it would
    // be refusing a spelling. The test is normalized here — to the predicate that
    // is true while the loop RUNS — and everything downstream reads only that.
    let (cmp, t_back, t_exit) = match cmp {
        CmpOp::Slt | CmpOp::Ult | CmpOp::Sle | CmpOp::Ule => (cmp, t_back, t_exit),
        // the exit is the TRUE side: the loop runs while the negation holds
        CmpOp::Sge if t_exit_is_true(f, header) => (CmpOp::Slt, t_back, t_exit),
        CmpOp::Uge if t_exit_is_true(f, header) => (CmpOp::Ult, t_back, t_exit),
        CmpOp::Sgt if t_exit_is_true(f, header) => (CmpOp::Sle, t_back, t_exit),
        CmpOp::Ugt if t_exit_is_true(f, header) => (CmpOp::Ule, t_back, t_exit),
        _ => return why(f, header, "test-not-lt-or-le"),
    };
    // The bound must be available where the loop is ENTERED, since the guard
    // reads it there. A literal always is; a value must be defined outside the
    // loop.
    if let Operand::Val(v) = cmp_b {
        if def_block(f, v) == Some(header) {
            return why(f, header, "bound-computed-in-the-loop");
        }
    }

    // THE TWO CARRIED VALUES. Exactly two header parameters take a back-edge
    // argument computed inside the loop: the counter and the accumulator. A third
    // is a dependence this row does not break.
    let params = f.blocks[header as usize].params.clone();
    let mut carried: Vec<usize> = Vec::new();
    for (k, _) in params.iter().enumerate() {
        if let Some(Operand::Val(v)) = t_back.args.get(k).copied() {
            if def_block(f, v) == Some(header) {
                carried.push(k);
            }
        }
    }
    if carried.len() != 2 {
        return why(f, header, "carried-is-not-exactly-two");
    }

    // WHICH IS THE COUNTER. The compared value is the counter's NEXT value —
    // `i + s` on the back edge — or the parameter itself; both name the same
    // parameter.
    let cv = cmp_a.val()?;
    let ip = match params.iter().position(|&x| x == cv) {
        Some(i) => i,
        None => match def_inst(f, cv) {
            Some(Inst::Bin { op: BinOp::Add, a, b, .. }) => {
                let base = match (a, b) {
                    (Operand::Val(x), Operand::Imm(_)) => *x,
                    (Operand::Imm(_), Operand::Val(x)) => *x,
                    _ => return why(f, header, "test-value-is-not-a-step"),
                };
                params.iter().position(|&x| x == base)?
            }
            _ => return why(f, header, "test-value-is-not-a-parameter"),
        },
    };
    if !carried.contains(&ip) {
        return why(f, header, "counter-is-not-carried");
    }
    let accp = *carried.iter().find(|&&k| k != ip)?;

    // THE STEP, a literal, from the back edge.
    let i = params[ip];
    let step = match def_inst(f, t_back.args[ip].val()?)? {
        Inst::Bin { op: BinOp::Add, a, b, .. } => match (a, b) {
            (Operand::Val(x), Operand::Imm(s)) if *x == i => *s,
            (Operand::Imm(s), Operand::Val(x)) if *x == i => *s,
            _ => return why(f, header, "step-is-not-a-literal"),
        },
        _ => return why(f, header, "counter-does-not-step"),
    };
    if step <= 0 {
        return why(f, header, "step-not-positive");
    }
    // THE TESTED VALUE IS THE ONE THE BACK EDGE PASSES. That is what makes the
    // loop's meaning "run the body at `v`, then continue iff `cmp(v+s, n)`", and
    // every guard below is derived from that sentence. A test on the parameter
    // itself is a different loop and is refused rather than guessed at.
    if t_back.args[ip].val() != Some(cv) {
        return why(f, header, "test-is-not-on-the-back-edge-value");
    }

    // THE ACCUMULATION, and its type decides whether the row may reassociate.
    // `acc + x` on an INTEGER type: two's-complement modular addition is
    // associative, so the four partial sums may be combined in any order. A
    // floating-point accumulator is refused — the square would not close.
    let acc = params[accp];
    let accnext = t_back.args[accp].val()?;
    match def_inst(f, accnext)? {
        Inst::Bin { op: BinOp::Add, ty, a, b, .. } => {
            if ty.is_float() {
                return why(f, header, "accumulator-is-not-an-integer");
            }
            let reads_acc = matches!(a, Operand::Val(x) if *x == acc)
                || matches!(b, Operand::Val(x) if *x == acc);
            if !reads_acc {
                return why(f, header, "accumulator-does-not-read-itself");
            }
        }
        _ => return why(f, header, "accumulation-is-not-an-add"),
    }

    // NOTHING THE LOOP DEFINES MAY BE READ OUTSIDE IT, for the reason
    // `unroll` states: HIR scopes a value by dominance, so a block after the loop
    // can name one directly, and after the rewrite there are four versions of it.
    // What LEAVES the loop leaves as an exit ARGUMENT, which is rewritten here.
    let mut defined: HashSet<ValueId> = params.iter().copied().collect();
    for inst in &f.blocks[header as usize].insts {
        if let Some(d) = inst.dst() {
            defined.insert(d);
        }
    }
    for (bi, blk) in f.blocks.iter().enumerate() {
        if bi == header as usize {
            continue;
        }
        let mut escapes = false;
        for inst in &blk.insts {
            inst.uses(|o| {
                if let Operand::Val(x) = o {
                    if defined.contains(&x) {
                        escapes = true;
                    }
                }
            });
        }
        blk.term.uses(|o| {
            if let Operand::Val(x) = o {
                if defined.contains(&x) {
                    escapes = true;
                }
            }
        });
        if escapes {
            return why(f, header, "value-read-after-the-loop");
        }
    }
    // The exit edge may carry only the accumulator and values that do not depend
    // on the counter — anything else is a second result the sum cannot restore.
    for (k, a) in t_exit.args.iter().enumerate() {
        if let Operand::Val(v) = a {
            if defined.contains(v) && *v != accnext && *v != acc {
                return why(f, header, "exit-carries-a-second-loop-value");
            }
            let _ = k;
        }
    }

    // ONE entry into the loop from outside, where the guard goes.
    let outside: Vec<BlockId> =
        c.preds[header as usize].iter().copied().filter(|&p| p != header).collect();
    if outside.len() != 1 {
        return why(f, header, "many-entries");
    }

    Some(Red {
        header,
        entry: outside[0],
        ip,
        accp,
        step,
        n: cmp_b,
        cmp,
        cty,
        exit: t_exit,
    })
}

/// THEORY A7b  SQUARE four_lanes_share_one_counted_reduction — four partial
/// accumulators sum to the one the sequential loop reached
pub fn run(f: &mut Func, a: &mut Analyses) -> bool {
    if !wanted() {
        return false;
    }
    force(f, a)
}

/// The pass with its gate open, for the batteries: a theorem still owes its
/// square while the row is being measured.
pub fn force(f: &mut Func, a: &mut Analyses) -> bool {
    let (c, _dt, lf) = a.all(f);
    for li in 0..lf.loops.len() {
        if let Some(r) = recognize(f, &c, &lf, li) {
            apply(f, &r);
            FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return true;
        }
    }
    false
}

/// The counter and the bound, widened to `I64` in the block being built.
///
/// WHY THE GUARD IS NOT COMPUTED IN THE COUNTER'S OWN TYPE. Every guard below
/// asks whether `i + 3s` is still an iteration, and for a 32-bit counter that
/// sum can leave the type: a loop whose bound sits within `3s` of `INT_MAX`
/// would compute a wrapped, NEGATIVE `i + 3s`, and `Slt` against the bound would
/// then be TRUE — the jammed block would run four bodies where three remained.
/// That is a wrong answer, not a missed optimization, so the arithmetic is done
/// where it cannot wrap. Sign-extension preserves signed order and zero-extension
/// preserves unsigned order, so the widened predicate is the same predicate; the
/// widened bound is loop-invariant and `licm` lifts it out.
fn widen(f: &mut Func, b: BlockId, v: Operand, ty: Ty, signed: bool) -> Operand {
    if ty == Ty::I64 {
        return v;
    }
    if let Operand::Imm(k) = v {
        return Operand::Imm(k);
    }
    let at = f.blocks[b as usize].insts.len() as u32;
    let d = f.new_value(Ty::I64, Def::Inst(b, at));
    f.blocks[b as usize].insts.push(Inst::Cvt {
        dst: d,
        op: if signed { CvtOp::Sext } else { CvtOp::Zext },
        from: ty,
        to: Ty::I64,
        a: v,
    });
    Operand::Val(d)
}

fn push_bin(f: &mut Func, b: BlockId, op: BinOp, ty: Ty, a: Operand, c: Operand) -> ValueId {
    let at = f.blocks[b as usize].insts.len() as u32;
    let d = f.new_value(ty, Def::Inst(b, at));
    f.blocks[b as usize].insts.push(Inst::Bin { dst: d, op, ty, a, b: c });
    d
}

fn push_cmp(f: &mut Func, b: BlockId, op: CmpOp, ty: Ty, a: Operand, c: Operand) -> ValueId {
    let at = f.blocks[b as usize].insts.len() as u32;
    let d = f.new_value(Ty::I32, Def::Inst(b, at));
    f.blocks[b as usize].insts.push(Inst::Cmp { dst: d, op, ty, a, b: c });
    d
}

/// `cmp(v + k·s, n)`, computed where it cannot wrap. Returns the condition.
fn room_test(f: &mut Func, b: BlockId, r: &Red, v: Operand, k: i64) -> ValueId {
    let signed = matches!(r.cmp, CmpOp::Slt | CmpOp::Sle);
    let w = widen(f, b, v, r.cty, signed);
    let n = widen(f, b, r.n, r.cty, signed);
    let last = if k == 0 {
        w
    } else {
        Operand::Val(push_bin(f, b, BinOp::Add, Ty::I64, w, Operand::Imm(k * r.step)))
    };
    push_cmp(f, b, r.cmp, Ty::I64, last, n)
}

/// Build the jammed block and wire the two guards around it. The ORIGINAL loop
/// is left exactly as it is and becomes both the tail and the refused case.
fn apply(f: &mut Func, r: &Red) {
    let h = r.header as usize;
    let params = f.blocks[h].params.clone();
    let insts = f.blocks[h].insts.clone();
    let ivty = f.ty_of(params[r.ip]);
    let accty = f.ty_of(params[r.accp]);
    let t_back: Target = {
        let t = f.blocks[h]
            .term
            .targets()
            .iter()
            .find(|t| t.block == r.header)
            .cloned();
        t.expect("recognize accepted a self edge").clone()
    };

    // ── the jammed block: the original parameters, plus three accumulators ──
    let j = f.new_block();
    let mut jp: Vec<ValueId> = Vec::new();
    for (k, &p) in params.iter().enumerate() {
        let v = f.new_value(f.ty_of(p), Def::Param(j, k as u32));
        f.blocks[j as usize].params.push(v);
        jp.push(v);
    }
    let mut lanacc: Vec<ValueId> = vec![jp[r.accp]];
    for l in 1..LANES as usize {
        let k = f.blocks[j as usize].params.len();
        let v = f.new_value(accty, Def::Param(j, k as u32));
        f.blocks[j as usize].params.push(v);
        lanacc.push(v);
        let _ = l;
    }

    // ── four copies of the body, lane `l` at counter `i + l·s` ─────────────
    //
    // Lane `l` reads the lane's counter and the lane's accumulator and NOTHING
    // else the loop defines — `recognize` established that the only carried
    // values are those two, so every other operand is either an invariant
    // parameter (mapped to the jammed block's own copy of it) or a value defined
    // outside the loop, which dominates the jammed block unchanged.
    let accnext_orig = t_back.args[r.accp].val().expect("recognize checked the accumulator");
    let mut lane_next: Vec<Operand> = Vec::new();
    for l in 0..LANES as usize {
        let mut vmap: HashMap<ValueId, Operand> = HashMap::new();
        for (k, &p) in params.iter().enumerate() {
            vmap.insert(p, Operand::Val(jp[k]));
        }
        vmap.insert(params[r.accp], Operand::Val(lanacc[l]));
        if l > 0 {
            let d = push_bin(
                f,
                j,
                BinOp::Add,
                ivty,
                Operand::Val(jp[r.ip]),
                Operand::Imm(l as i64 * r.step),
            );
            vmap.insert(params[r.ip], Operand::Val(d));
        }
        for inst in &insts {
            let mut c = inst.clone();
            c.uses_mut(|o| {
                if let Operand::Val(v) = o {
                    if let Some(&nv) = vmap.get(v) {
                        *o = nv;
                    }
                }
            });
            if let Some(old) = inst.dst() {
                let at = f.blocks[j as usize].insts.len() as u32;
                let nv = f.new_value(f.ty_of(old), Def::Inst(j, at));
                set_dst(&mut c, nv);
                vmap.insert(old, Operand::Val(nv));
            }
            f.blocks[j as usize].insts.push(c);
        }
        lane_next.push(*vmap.get(&accnext_orig).expect("the accumulation is in the body"));
    }

    // ── the latch: advance four, and stay only while four more remain ──────
    let inext = push_bin(
        f,
        j,
        BinOp::Add,
        ivty,
        Operand::Val(jp[r.ip]),
        Operand::Imm(LANES * r.step),
    );
    let stay = room_test(f, j, r, Operand::Val(inext), LANES - 1);

    // G2 collects what the jammed block reached; it is built before the
    // terminator that names it.
    let g2 = f.new_block();
    let mut g2p: Vec<ValueId> = Vec::new();
    let g2i = f.new_value(ivty, Def::Param(g2, 0));
    f.blocks[g2 as usize].params.push(g2i);
    for l in 0..LANES as usize {
        let v = f.new_value(accty, Def::Param(g2, 1 + l as u32));
        f.blocks[g2 as usize].params.push(v);
        g2p.push(v);
        let _ = l;
    }

    let mut back_args: Vec<Operand> = t_back.args.clone();
    back_args[r.ip] = Operand::Val(inext);
    back_args[r.accp] = lane_next[0];
    // an invariant parameter passes itself
    for (k, a) in back_args.iter_mut().enumerate() {
        if k != r.ip && k != r.accp {
            *a = Operand::Val(jp[k]);
        }
    }
    for l in 1..LANES as usize {
        back_args.push(lane_next[l]);
    }
    let mut g2_args: Vec<Operand> = vec![Operand::Val(inext)];
    for l in 0..LANES as usize {
        g2_args.push(lane_next[l]);
    }
    f.blocks[j as usize].term = Term::Br(
        Operand::Val(stay),
        Target { block: j, args: back_args },
        Target { block: g2, args: g2_args },
    );

    // ── G2: the horizontal sum, then the tail or the exit ──────────────────
    //
    // THE SUM IS WHERE THE SQUARE IS SPENT. Four partial accumulators are
    // combined in a balanced tree; two's-complement addition is associative, so
    // the residue is the one the sequential loop would have reached.
    let s01 = push_bin(f, g2, BinOp::Add, accty, Operand::Val(g2p[0]), Operand::Val(g2p[1]));
    let s23 = push_bin(f, g2, BinOp::Add, accty, Operand::Val(g2p[2]), Operand::Val(g2p[3]));
    let sum = push_bin(f, g2, BinOp::Add, accty, Operand::Val(s01), Operand::Val(s23));
    let more = room_test(f, g2, r, Operand::Val(g2i), 0);

    // The tail is the ORIGINAL loop, entered with the counter the jammed block
    // reached and the sum as its accumulator; every other parameter is invariant
    // and is whatever the entry edge supplied.
    let entry_args: Vec<Operand> = f.blocks[r.entry as usize]
        .term
        .targets()
        .iter()
        .find(|t| t.block == r.header)
        .map(|t| t.args.clone())
        .unwrap_or_default();
    let mut tail_args = entry_args.clone();
    tail_args[r.ip] = Operand::Val(g2i);
    tail_args[r.accp] = Operand::Val(sum);
    // Leaving straight out: the exit edge's arguments, with the accumulator's
    // in-loop value replaced by the sum. `recognize` refused every other
    // loop-defined value on this edge, so nothing else needs rewriting.
    let mut exit = r.exit.clone();
    for a in exit.args.iter_mut() {
        if matches!(*a, Operand::Val(v) if v == accnext_orig || v == params[r.accp]) {
            *a = Operand::Val(sum);
        }
    }
    f.blocks[g2 as usize].term = Term::Br(
        Operand::Val(more),
        Target { block: r.header, args: tail_args },
        exit,
    );

    // ── G1: in front of everything, because the jammed block needs a whole
    //    group and must not run when fewer than four iterations exist ────────
    let g1 = f.new_block();
    let enough = room_test(f, g1, r, entry_args[r.ip], LANES - 1);
    let mut j_args = entry_args.clone();
    for _ in 0..LANES - 1 {
        j_args.push(Operand::Imm(0));
    }
    f.blocks[g1 as usize].term = Term::Br(
        Operand::Val(enough),
        Target { block: j, args: j_args },
        Target { block: r.header, args: entry_args.clone() },
    );
    let mut eterm = f.blocks[r.entry as usize].term.clone();
    for t in eterm.targets_mut() {
        if t.block == r.header {
            t.block = g1;
        }
    }
    f.blocks[r.entry as usize].term = eterm;
}

/// Lanes 1..3 start their partial sums at the identity for `+`, which is why
/// `G1` pushes `Imm(0)`: the sequential accumulator's initial value stays with
/// lane 0, and the sum at the exit restores it exactly once.
fn set_dst(i: &mut Inst, d: ValueId) {
    match i {
        Inst::Bin { dst, .. }
        | Inst::Un { dst, .. }
        | Inst::Cmp { dst, .. }
        | Inst::Cvt { dst, .. }
        | Inst::Load { dst, .. }
        | Inst::SlotAddr { dst, .. }
        | Inst::SymAddr { dst, .. }
        | Inst::Select { dst, .. } => *dst = d,
        _ => {}
    }
}

/// Whether the terminator's TRUE arm is the one that leaves the loop — the
/// `if (i >= n) goto done;` spelling, where the compare states the exit
/// condition rather than the staying one.
fn t_exit_is_true(f: &Func, header: BlockId) -> bool {
    matches!(&f.blocks[header as usize].term, Term::Br(_, t1, _) if t1.block != header)
}

fn def_inst(f: &Func, v: ValueId) -> Option<&Inst> {
    match f.values[v as usize].def {
        Def::Inst(b, i) => f.blocks[b as usize].insts.get(i as usize),
        _ => None,
    }
}

fn def_block(f: &Func, v: ValueId) -> Option<BlockId> {
    match f.values[v as usize].def {
        Def::Inst(b, _) | Def::Param(b, _) => Some(b),
        _ => None,
    }
}

const _: i64 = LANES;
