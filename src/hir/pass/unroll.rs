// unroll — FULL unrolling of a loop whose trip count is a small literal
// (MECHANISM.md §G12 R5.9, the `#12` row; gcc's `-fcunroll` case).
// THEORY A7b — optimization: this pass ships its commuting square
//
// THE MEASUREMENT THAT ASKED FOR THIS. `n7_nested_subq` evaluates a WHERE
// clause as two predicates reached through a function-pointer table:
//
//     for (p = 0; p < 2; p++)
//         if (!plan[(i + p) & 3](o, n)) { ok = 0; break; }
//
// gcc -O1 emits the two calls straight-line. zcc emits the loop, and pays for
// it three times over: the counter's increment, compare and branch; the table
// index rebuilt from `i + p` on every arm where one of the two is loop-
// invariant; and — the expensive one — a REGISTER, because `p` is live across
// both calls and the register file is already full, which pushed the enclosing
// loop's own counter into a stack slot. Hand-edited, unrolling the two
// iterations took the program from 1.454 to 1.159 against gcc -O1, and 1.159 of
// that came from the freed register alone. The transform is worth two rows and
// it is one.
//
// WHAT IS UNROLLED, and the conditions are deliberately narrow:
//   * an INNERMOST loop (no loop nested inside it), so the clone is a flat set
//     of blocks with no back edge but its own;
//   * one latch, and a header whose terminator tests the counter against a
//     literal — `p < K` with `p` a header parameter starting at a literal and
//     advanced by a literal step on the latch, which is the whole trip-count
//     analysis this needs and is why `scev` is not asked (its trip count refuses
//     a loop with a second exit, and a `break` is exactly what the shape above
//     has);
//   * a trip count of at most `MAX_TRIPS`, and a body small enough that K copies
//     of it stay inside the instruction cache — `MAX_BODY` instructions.
//   * every OTHER exit (a `break`) is left exactly as it is: it already leaves
//     the loop, and after unrolling it leaves the copy it sits in, which is the
//     same program point.
//
// COMMUTING SQUARE `⟦f⟧ = ⟦unroll f⟧`. The loop executes its body for
// `p = a₀, a₀+s, …` while `p < K`; the unrolled form executes the SAME bodies in
// the same order, the c-th copy with `p` substituted by the constant that
// iteration held. Substituting a constant for a parameter that provably holds
// exactly that constant is the identity, and the guard `p < K` is then decided:
// true in every copy the analysis built, false where the last copy falls out, so
// the copy chain ends by taking the header's own exit edge — with the loop-
// carried arguments the latch would have passed, which is what the parameter
// would have received. No block is reordered and no memory operation moves, so
// the trace of observable events is identical. Battery: `unroll_*` in
// `hir/pass/tests.rs`.
use super::*;
use std::collections::HashMap;

/// MEASURED M22 — the unroll budgets, swept on the 49-program suite
///
/// Both are policy numbers, so Article E's question applies: the spec's number
/// or the author's convenience? Neither ISA nor ABI has anything to say about
/// how many copies of a loop are worth making, so the answer had to be measured
/// rather than cited, and `MECHANISM.md` Part F M22 records the sweep that set them.
/// Overridable at run time so the sweep needs no rebuild.
fn max_trips() -> i64 {
    static W: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *W.get_or_init(|| {
        std::env::var("ZCC_UNROLL_TRIPS").ok().and_then(|v| v.parse().ok()).unwrap_or(4)
    })
}

/// MEASURED M22 — the body budget, in HIR instructions (`n7`'s loop is 6)
fn max_body() -> usize {
    static W: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *W.get_or_init(|| {
        std::env::var("ZCC_UNROLL_BODY").ok().and_then(|v| v.parse().ok()).unwrap_or(24)
    })
}

/// DEFAULT ON since 2026-08-28, on the measurement: over the 49-program suite,
/// interleaved, EXEC geomean 1.0231 → 1.0181 with this row added to the
/// consumer-blind IV rewrite, and `n7_nested_subq` 1.747 → 1.602 with this row
/// alone. `ZCC_NOUNROLL=1` turns it off.
fn enabled() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_NOUNROLL").is_err())
}

/// THEORY A7b  SQUARE unroll_replays_the_same_iterations — a decided guard is not a loop
pub fn run(f: &mut Func, a: &mut Analyses) -> bool {
    if !enabled() {
        return false;
    }
    force(f, a)
}

/// The pass with its gate open, for the batteries: a theorem still owes its
/// square while the row is being measured.
pub fn force(f: &mut Func, a: &mut Analyses) -> bool {
    let (c, _dt, lf) = a.all(f);
    for li in 0..lf.loops.len() {
        // innermost only
        if lf.loops.iter().any(|l| l.parent == Some(li as u32)) {
            continue;
        }
        if let Some(plan) = analyze(f, &c, &lf, li) {
            apply(f, &plan);
            refresh_defs(f);
            return true;
        }
    }
    false
}

/// WHERE the loop's own guard sits. `rotate` runs before this pass and leaves a
/// bottom-tested loop, so the latch is the ordinary case and the header-guard
/// form is what an unrotated loop still looks like.
#[derive(PartialEq, Clone, Copy)]
enum Guard {
    Header,
    Latch,
}

/// Everything the rewrite needs, decided before a single block is touched.
struct Plan {
    guard: Guard,
    header: BlockId,
    latch: BlockId,
    body: Vec<BlockId>,
    /// which header parameter is the counter
    counter: usize,
    /// the constant the counter holds on each iteration
    values: Vec<i64>,
    /// where the loop goes when the guard fails, and with what arguments
    exit: Target,
}

fn why(reason: &str) -> Option<Plan> {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *W.get_or_init(|| std::env::var("ZCC_UNROLLDBG").is_ok()) {
        eprintln!("[unroll-refused] {}", reason);
    }
    None
}

fn analyze(f: &Func, c: &dom::Cfg, lf: &dom::LoopForest, li: usize) -> Option<Plan> {
    let l = &lf.loops[li];
    let header = l.header;
    if l.latches.len() != 1 {
        return why("many-latches");
    }
    let latch = l.latches[0];
    let body: Vec<BlockId> = l.body.clone();
    if body.iter().map(|&b| f.blocks[b as usize].insts.len()).sum::<usize>() > max_body() {
        return why("body-too-big");
    }
    // THE GUARD, wherever it sits. A rotated loop tests at the LATCH — `p+s < K`
    // on the value the back edge is about to pass — and an unrotated one tests
    // at the header. Both name the same iteration set, so both are read here and
    // only the rewiring below tells them apart.
    let mut found = None;
    for (g, blk) in [(Guard::Latch, latch), (Guard::Header, header)] {
        let (cond, t_a, t_b) = match &f.blocks[blk as usize].term {
            Term::Br(c, a, b) => (*c, a, b),
            _ => continue,
        };
        // exactly one side leaves the loop, and for the latch the staying side
        // is the header itself
        let (inside, outside) = match (body.contains(&t_a.block), body.contains(&t_b.block)) {
            (true, false) => (t_a.clone(), t_b.clone()),
            (false, true) => (t_b.clone(), t_a.clone()),
            _ => continue,
        };
        if g == Guard::Latch && inside.block != header {
            continue;
        }
        found = Some((g, cond, inside, outside));
        break;
    }
    let Some((guard, cond, _inside, t_out)) = found else {
        return why("no-guard-edge");
    };
    let (cmp_a, cmp_b, op) = match def_inst(f, cond.val()?)? {
        Inst::Cmp { a, b, op, .. } => (*a, *b, *op),
        _ => return why("guard-not-a-compare"),
    };
    // `p < K`, signed or unsigned; `K` a literal
    if !matches!(op, CmpOp::Slt | CmpOp::Ult) {
        return why("guard-not-lt");
    }
    let limit = match cmp_b {
        Operand::Imm(k) => k,
        _ => return why("bound-not-literal"),
    };
    // The compared value is either the counter itself (header test) or the
    // counter's next value (latch test); both identify the same parameter.
    let cv = cmp_a.val()?;
    let hp = &f.blocks[header as usize].params;
    let counter = match hp.iter().position(|&x| x == cv) {
        Some(i) => i,
        None => match def_inst(f, cv) {
            Some(Inst::Bin { op: BinOp::Add, a, b, .. }) => {
                let base = match (a, b) {
                    (Operand::Val(x), Operand::Imm(_)) => *x,
                    (Operand::Imm(_), Operand::Val(x)) => *x,
                    _ => return why("guard-value-not-a-step"),
                };
                match hp.iter().position(|&x| x == base) {
                    Some(i) => i,
                    None => return why("guard-value-not-a-param"),
                }
            }
            _ => return why("guard-value-not-a-param"),
        },
    };
    let p = f.blocks[header as usize].params[counter];

    // the start value, from every entry edge, and the step, from the latch
    let mut start: Option<i64> = None;
    let mut step: Option<i64> = None;
    for &pred in &c.preds[header as usize] {
        let arg = arg_to(f, pred, header, counter)?;
        if pred == latch {
            // `p + s`, with `p` the parameter itself
            let (a, b) = match def_inst(f, arg.val()?)? {
                Inst::Bin { op: BinOp::Add, a, b, .. } => (*a, *b),
                _ => return None,
            };
            let s = match (a, b) {
                (Operand::Val(x), Operand::Imm(s)) if x == p => s,
                (Operand::Imm(s), Operand::Val(x)) if x == p => s,
                _ => return None,
            };
            if step.replace(s).is_some() {
                return None;
            }
        } else {
            match arg {
                Operand::Imm(v) if start.is_none_or(|s| s == v) => start = Some(v),
                _ => return None,
            }
        }
    }
    // NOTHING THE LOOP DEFINES MAY BE READ OUTSIDE IT. HIR scopes a value by
    // dominance, so a block after the loop can name a parameter or an
    // instruction of the loop directly; after unrolling, the value that reaches
    // that block is the LAST copy's, and every earlier exit would need its own
    // version merged in — SSA reconstruction this row does not do. `20071029-1`
    // is the case that proves the refusal is not theoretical: a counter read
    // after its loop turned into copy 0's literal and the program aborted.
    let inside: std::collections::HashSet<BlockId> = body.iter().copied().collect();
    let mut defined: std::collections::HashSet<ValueId> = std::collections::HashSet::new();
    for &bb in &body {
        for prm in &f.blocks[bb as usize].params {
            defined.insert(*prm);
        }
        for inst in &f.blocks[bb as usize].insts {
            if let Some(d) = inst.dst() {
                defined.insert(d);
            }
        }
    }
    let mut escapes = false;
    for (bi, blk) in f.blocks.iter().enumerate() {
        if inside.contains(&(bi as BlockId)) {
            continue;
        }
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
    }
    if escapes {
        return why("value-read-after-the-loop");
    }

    let (mut v, step) = (start?, step?);
    if step <= 0 || v >= limit {
        return why("start-or-step");
    }
    let mut values = Vec::new();
    while v < limit {
        values.push(v);
        if values.len() as i64 > max_trips() {
            return None;
        }
        v += step;
    }
    Some(Plan {
        guard,
        header,
        latch,
        body,
        counter,
        values,
        exit: t_out.clone(),
    })
}

/// The argument a predecessor supplies for one header parameter.
fn arg_to(f: &Func, pred: BlockId, header: BlockId, k: usize) -> Option<Operand> {
    let mut out = None;
    for t in f.blocks[pred as usize].term.targets() {
        if t.block == header {
            if out.is_some() {
                return None; // two edges to the same header: not this shape
            }
            out = Some(*t.args.get(k)?);
        }
    }
    out
}

fn def_inst(f: &Func, v: ValueId) -> Option<&Inst> {
    match f.values[v as usize].def {
        Def::Inst(b, i) => f.blocks[b as usize].insts.get(i as usize),
        _ => None,
    }
}

fn apply(f: &mut Func, p: &Plan) {
    // Copy 0 is the loop as it stands; copies 1.. are clones. Each copy hands
    // control to the next, and the last one leaves through the loop's own exit
    // edge.
    let mut headers = vec![p.header];
    let mut latches = vec![p.latch];
    let mut owned = vec![p.body.clone()];
    for _ in 1..p.values.len() {
        let (h, l, blocks) = clone_body(f, p);
        headers.push(h);
        latches.push(l);
        owned.push(blocks);
    }
    let last = p.values.len() - 1;

    for c in 0..p.values.len() {
        let (h, latch) = (headers[c], latches[c]);
        // The counter is this copy's constant everywhere inside the copy.
        let param = f.blocks[h as usize].params[p.counter];
        substitute(f, &owned[c], param, Operand::Imm(p.values[c]));

        match p.guard {
            // Unrotated: the header's test is decided (true — the analysis built
            // exactly the passing iterations), so the header becomes the jump it
            // always takes, and the latch closes on the next copy or, at the
            // end, on the exit edge carrying what it was about to pass.
            Guard::Header => {
                let taken = match &f.blocks[h as usize].term {
                    Term::Br(_, a, _) => a.clone(),
                    _ => unreachable!("analyze accepted a non-Br header"),
                };
                f.blocks[h as usize].term = Term::Jmp(taken);
                let next = if c == last { None } else { Some(headers[c + 1]) };
                retarget_latch(f, latch, h, next, p);
            }
            // Rotated: the LATCH holds the test. It is true for every copy but
            // the last, where it is false — so each latch becomes a jump, to the
            // next copy's header or out through the exit target it already
            // carries, with no argument rewriting at all: both targets were
            // cloned with the copy and already name the copy's own values.
            Guard::Latch => {
                let (t_hdr, t_exit) = match &f.blocks[latch as usize].term {
                    Term::Br(_, a, b) => {
                        if a.block == h {
                            (a.clone(), b.clone())
                        } else {
                            (b.clone(), a.clone())
                        }
                    }
                    _ => unreachable!("analyze accepted a non-Br latch"),
                };
                f.blocks[latch as usize].term = if c == last {
                    Term::Jmp(t_exit)
                } else {
                    let mut t = t_hdr;
                    t.block = headers[c + 1];
                    Term::Jmp(t)
                };
            }
        }
    }
}

/// Replace every use of `v` by `k` INSIDE the given blocks.
///
/// The first cut walked the whole function on the claim that a header parameter
/// is in scope only inside its loop. That claim is false: HIR scopes a value by
/// DOMINANCE, so any block the header dominates may read the parameter, and the
/// blocks after the loop are exactly such blocks. `gcc.c-torture` case
/// `20071029-1` caught it — a counter read after its loop became copy 0's
/// literal, and the program aborted where -O0 and gcc -O1 both returned 0.
/// `analyze` now refuses a loop whose values are read outside it, and this walk
/// is confined to the copy regardless: two fences, because the one that failed
/// was an argument rather than a check.
fn substitute(f: &mut Func, blocks: &[BlockId], v: ValueId, k: Operand) {
    for bi in blocks.iter().map(|&b| b as usize) {
        let b = &mut f.blocks[bi];
        for inst in b.insts.iter_mut() {
            inst.uses_mut(|o| {
                if matches!(*o, Operand::Val(x) if x == v) {
                    *o = k;
                }
            });
        }
        let mut t = std::mem::replace(&mut b.term, Term::Unreachable);
        match &mut t {
            Term::Br(c, ..) | Term::Switch(c, ..) | Term::GotoPtr(c, _) => {
                if matches!(*c, Operand::Val(x) if x == v) {
                    *c = k;
                }
            }
            Term::Ret(Some(r)) => {
                if matches!(*r, Operand::Val(x) if x == v) {
                    *r = k;
                }
            }
            _ => {}
        }
        for tt in t.targets_mut() {
            for a in tt.args.iter_mut() {
                if matches!(*a, Operand::Val(x) if x == v) {
                    *a = k;
                }
            }
        }
        b.term = t;
    }
}

/// The latch of copy `c`: it jumped back to `h`; now it enters `next`, or leaves
/// through the loop's exit edge when there is no next copy.
fn retarget_latch(f: &mut Func, latch: BlockId, h: BlockId, next: Option<BlockId>, p: &Plan) {
    // What the latch was passing to the header's parameters.
    let mut args: Option<Vec<Operand>> = None;
    for t in f.blocks[latch as usize].term.targets_mut() {
        if t.block == h {
            args = Some(t.args.clone());
            match next {
                Some(n) => t.block = n,
                None => {}
            }
        }
    }
    let Some(args) = args else { return };
    if next.is_some() {
        return;
    }
    // The last copy falls out of the loop. It takes the header's exit edge, and
    // every argument that edge computed from a header parameter is now the value
    // the latch was about to supply for that parameter.
    let params = f.blocks[h as usize].params.clone();
    let map: HashMap<ValueId, Operand> =
        params.iter().copied().zip(args.iter().copied()).collect();
    let mut exit = p.exit.clone();
    for a in exit.args.iter_mut() {
        if let Operand::Val(v) = *a {
            if let Some(&r) = map.get(&v) {
                *a = r;
            }
        }
    }
    for t in f.blocks[latch as usize].term.targets_mut() {
        if t.block == h {
            *t = exit.clone();
        }
    }
}

/// One more copy of the loop's blocks, with fresh values. Returns the copy's
/// header, its latch, and the blocks it owns — the last so the constant
/// substitution stays inside the copy that decided it.
fn clone_body(f: &mut Func, p: &Plan) -> (BlockId, BlockId, Vec<BlockId>) {
    let mut bmap: HashMap<BlockId, BlockId> = HashMap::new();
    for &b in &p.body {
        let nb = f.new_block();
        f.blocks[nb as usize].weight = f.blocks[b as usize].weight;
        bmap.insert(b, nb);
    }
    let mut vmap: HashMap<ValueId, Operand> = HashMap::new();
    // parameters first: an instruction in the copy may read a parameter of a
    // block that has not been cloned yet.
    for &b in &p.body {
        let nb = bmap[&b];
        let params = f.blocks[b as usize].params.clone();
        for (i, &v) in params.iter().enumerate() {
            let ty = f.ty_of(v);
            let nv = f.new_value(ty, Def::Param(nb, i as u32));
            f.blocks[nb as usize].params.push(nv);
            vmap.insert(v, Operand::Val(nv));
        }
    }
    for &b in &p.body {
        let nb = bmap[&b];
        let insts = f.blocks[b as usize].insts.clone();
        for (i, mut inst) in insts.into_iter().enumerate() {
            if let Some(d) = inst.dst() {
                let ty = f.ty_of(d);
                let nv = f.new_value(ty, Def::Inst(nb, i as u32));
                vmap.insert(d, Operand::Val(nv));
                set_dst(&mut inst, nv);
            }
            f.blocks[nb as usize].insts.push(inst);
        }
        let term = f.blocks[b as usize].term.clone();
        f.blocks[nb as usize].term = term;
    }
    // now rewrite the clones' operands and edges
    for &b in &p.body {
        let nb = bmap[&b];
        let mut insts = std::mem::take(&mut f.blocks[nb as usize].insts);
        for inst in insts.iter_mut() {
            inst.uses_mut(|o| remap(o, &vmap));
        }
        f.blocks[nb as usize].insts = insts;
        let mut t = std::mem::replace(&mut f.blocks[nb as usize].term, Term::Unreachable);
        match &mut t {
            Term::Br(c, ..) | Term::Switch(c, ..) | Term::GotoPtr(c, _) => remap(c, &vmap),
            Term::Ret(Some(r)) => remap(r, &vmap),
            _ => {}
        }
        for tt in t.targets_mut() {
            for a in tt.args.iter_mut() {
                remap(a, &vmap);
            }
            if let Some(&nt) = bmap.get(&tt.block) {
                tt.block = nt;
            }
        }
        f.blocks[nb as usize].term = t;
    }
    let mut owned: Vec<BlockId> = bmap.values().copied().collect();
    owned.sort();
    (bmap[&p.header], bmap[&p.latch], owned)
}

fn set_dst(inst: &mut Inst, nv: ValueId) {
    match inst {
        Inst::Bin { dst, .. }
        | Inst::Un { dst, .. }
        | Inst::Cmp { dst, .. }
        | Inst::Cvt { dst, .. }
        | Inst::Load { dst, .. }
        | Inst::SlotAddr { dst, .. }
        | Inst::SymAddr { dst, .. }
        | Inst::Select { dst, .. }
        | Inst::Alloca { dst, .. } => *dst = nv,
        Inst::Call { dst, .. } | Inst::Intrinsic { dst, .. } => *dst = Some(nv),
        _ => {}
    }
}

fn remap(o: &mut Operand, vmap: &HashMap<ValueId, Operand>) {
    if let Operand::Val(v) = *o {
        if let Some(&n) = vmap.get(&v) {
            *o = n;
        }
    }
}
