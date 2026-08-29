// jam — unroll-and-jam: four iterations of an outer loop run through ONE pass of
// the inner loop.
// THEORY A7b — optimization: this pass ships its commuting square
//
// WHAT IT IS FOR, and the measurement that chose it. `perf` says zcc emits
// 1.0144x gcc -O2's STATIC instructions and executes 1.3250x its DYNAMIC ones,
// at a better average IPC — the gap is work EXECUTED, not code size. The clearest
// case is `z4_matmul_int`, 3.42x against gcc -O2 and 1.087x against gcc -O1: gcc
// -O1 emits no SIMD there, gcc -O2 emits `mla v.4s` and is 3.3x faster than its
// own -O1. Its inner loop is seven instructions for ONE value of the outer
// counter:
//
//     ldr  w20, [x19]              ; B[k][j], stride n in k
//     ldr  w21, [x2, x7, lsl #2]   ; A[i][k], INVARIANT in j
//     madd w6, w21, w20, w6
//     add  x7,#1 ; add x19,#800 ; cmp x7,#200 ; b.lt
//
// Four values of `j` at once share the `A` load, the counter, the compare and
// the branch, and pay only for their own `B` load, multiply and add. That is
// about 2.75 instructions per `j` against 7 — before a single SIMD instruction,
// and it is what MAKES the SIMD form reachable afterwards: four jammed lanes are
// one `mla v.4s`.
//
// WHY NOT ACCUMULATOR SPLITTING, which is the cheaper-looking row. Refuted
// before building: `z4`'s IPC is 5.77 against gcc's 4.38, so the `madd` chain is
// NOT the bottleneck and breaking it buys nothing. Only the instruction count is.
//
// THE TRANSFORM, and it is a SUBSTITUTION rather than an analysis. Every value
// in the inner loop that depends on the outer counter `j` is cloned three times
// with `j` replaced by `j+1`, `j+2`, `j+3`; the inner loop's accumulator becomes
// four accumulators; the outer body's store becomes four stores. No affine
// reasoning is needed and none is done: a clone of the address computation with
// `j+l` substituted computes, by construction, the address the original computes
// at `j+l`.
//
// COMMUTING SQUARE. Iterations `j`, `j+1`, `j+2`, `j+3` of the outer loop are
// INDEPENDENT — that is the refusal below, checked and not assumed: nothing the
// outer body writes is read by another of the four, because the only store is at
// an address the substitution moves by the element width and the only value
// crossing the inner loop is each lane's own accumulator. Running them together
// therefore computes what running them in order computed. What remains is the
// trip count: a runtime guard takes the jammed nest only when four iterations are
// left, and the ORIGINAL nest, untouched, is the tail and every refused case.
use super::*;
use std::collections::{HashMap, HashSet};

/// THEORY A7b — the pass ships ON, on the measurement rather than on the idea.
/// 96 programs, gcc -O2 referee, Graviton4: EXEC geomean 1.2298 -> 1.2234, INSN
/// 1.0144 -> 1.0210, 0 DIVERGE, and `z4_matmul_int` stops being the worst program
/// in the suite. `ZCC_NOJAM` turns it off.
pub fn wanted() -> bool {
    WANT.with(|c| c.get()).unwrap_or_else(|| {
        static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *W.get_or_init(|| std::env::var_os("ZCC_NOJAM").is_none())
    })
}

thread_local! {
    // THEORY A7b — instrument half. Not a value the compiler computes with: the
    // switch a battery flips to prove the jam actually happened. A thread-local
    // for the reason `spill.rs`'s seams are: the battery runs in parallel threads.
    static WANT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub fn set_wanted(on: Option<bool>) {
    WANT.with(|c| c.set(on));
}

/// MEASURED M55 — four is the lane count of a `q` register at 32 bits, which is
/// what the row exists to reach. It is not a tuning knob: a jam factor that is not the
/// vector width would have to be re-jammed before the SIMD form could be built.
const LANES: i64 = 4;

/// THEORY A7b — instrument half: nests jammed, so an A/B can tell "bought
/// nothing" from "never fired".
pub static FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct Nest {
    /// outer header, whose single parameter is `j`
    outer: BlockId,
    jp: usize,
    /// inner loop header
    inner: BlockId,
    /// index of the inner accumulator among the inner header's parameters
    accp: usize,
    /// the outer tail: stores the accumulator, steps `j`
    tail: BlockId,
    /// index of the tail parameter carrying the accumulator
    tailp: usize,
    /// the outer bound and the entry predecessor
    n: Operand,
    entry: BlockId,
    exit: Target,
}

/// THEORY A7b  SQUARE four_outer_iterations_share_one_inner_pass — unroll-and-jam
pub fn run(f: &mut Func, a: &mut Analyses) -> bool {
    if !wanted() {
        return false;
    }
    let nests = {
        let (c, _dt, lf) = a.all(f);
        let mut v = Vec::new();
        for li in 0..lf.loops.len() {
            if let Some(nst) = recognize(f, c, lf, li) {
                v.push(nst);
            }
        }
        v
    };
    if nests.is_empty() {
        return false;
    }
    // ONE nest per run: the rewrite renumbers blocks and the analysis behind the
    // others is stale the moment the first one lands. The ladder runs this row
    // again on its next turn, which is how the rest are reached.
    if std::env::var_os("ZCC_JAMDBG").is_some() {
        let n = &nests[0];
        eprintln!(
            "[jam] {} outer=b{} inner=b{} tail=b{} accp={} n={:?} entry=b{} (of {} candidates)",
            f.name, n.outer, n.inner, n.tail, n.accp, n.n, n.entry, nests.len()
        );
    }
    jam(f, &nests[0]);
    FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    refresh_defs(f);
    true
}

fn defined_in(f: &Func, b: BlockId) -> Vec<ValueId> {
    let mut v: Vec<ValueId> = f.blocks[b as usize].params.clone();
    for inst in &f.blocks[b as usize].insts {
        if let Some(d) = inst.dst() {
            v.push(d);
        }
    }
    v
}

fn recognize(f: &Func, c: &dom::Cfg, lf: &dom::LoopForest, li: usize) -> Option<Nest> {
    let l = &lf.loops[li];
    // The OUTER loop of a two-deep nest: exactly one child, and no grandchild.
    let kids: Vec<usize> = (0..lf.loops.len())
        .filter(|&x| lf.loops[x].parent == Some(li as u32))
        .collect();
    if kids.len() != 1 {
        return None;
    }
    let inner_li = kids[0];
    if lf.loops.iter().any(|x| x.parent == Some(inner_li as u32)) {
        return None;
    }
    let outer = l.header;
    let inner = lf.loops[inner_li].header;
    if lf.loops[inner_li].body.len() != 1 {
        return None;
    }
    // The outer body is exactly three blocks: the header, the inner loop, and a
    // tail. Anything else is control flow this row does not claim.
    if l.body.len() != 3 {
        return None;
    }
    let tail = *l.body.iter().find(|&&b| b != outer && b != inner)?;

    // The header may COMPUTE — `z4_matmul_int` builds `B`'s row base there — but
    // only purely: anything with an effect would be run four times by the jam.
    if f.blocks[outer as usize]
        .insts
        .iter()
        .any(|i| !matches!(i.effect(), Effect::Pure))
    {
        return None;
    }
    let to_inner = match &f.blocks[outer as usize].term {
        Term::Jmp(t) if t.block == inner => t.clone(),
        _ => return None,
    };
    if f.blocks[outer as usize].params.len() != 1 {
        return None;
    }
    let jp = 0usize;
    let j = f.blocks[outer as usize].params[jp];

    // The outer loop is entered from exactly one block outside it.
    let outside: Vec<BlockId> =
        c.preds[outer as usize].iter().copied().filter(|&p| p != tail).collect();
    if outside.len() != 1 {
        return None;
    }
    let entry = outside[0];

    // The inner loop leaves to the tail with its accumulator.
    let (icond, iback, iexit) = match &f.blocks[inner as usize].term {
        Term::Br(cd, t1, t2) if t1.block == inner && t2.block == tail => (*cd, t1, t2),
        Term::Br(cd, t1, t2) if t2.block == inner && t1.block == tail => (*cd, t2, t1),
        _ => return None,
    };
    let _ = icond;
    if iexit.args.len() != 1 || f.blocks[tail as usize].params.len() != 1 {
        return None;
    }
    let accv = match iexit.args[0] {
        Operand::Val(v) => v,
        _ => return None,
    };
    // Which inner parameter is the accumulator: the one the exit value is built
    // from, and whose back-edge argument is that same value.
    let accp = (0..f.blocks[inner as usize].params.len())
        .find(|&k| iback.args.get(k) == Some(&Operand::Val(accv)))?;

    // The tail: one store, then `j + 1` and the outer test.
    let mut stores = 0;
    for inst in &f.blocks[tail as usize].insts {
        match inst {
            Inst::Store { vol: false, .. } => stores += 1,
            Inst::Load { .. } => return None,
            other => {
                if !matches!(other.effect(), Effect::Pure) {
                    return None;
                }
            }
        }
    }
    if stores != 1 {
        return None;
    }
    let (tcond, tback, texit) = match &f.blocks[tail as usize].term {
        Term::Br(cd, t1, t2) if t1.block == outer => (*cd, t1, t2),
        Term::Br(cd, t1, t2) if t2.block == outer => (*cd, t2, t1),
        _ => return None,
    };
    // `j` steps by one, and the bound is invariant.
    let jnext = match tback.args.first() {
        Some(Operand::Val(v)) => *v,
        _ => return None,
    };
    let tdef = defined_in(f, tail);
    let steps_one = f.blocks[tail as usize].insts.iter().any(|i| {
        matches!(i, Inst::Bin { dst, op: BinOp::Add, a, b: Operand::Imm(1), .. }
            if *dst == jnext && *a == Operand::Val(j))
    });
    if !steps_one {
        return None;
    }
    let n = match tcond {
        Operand::Val(cv) => f.blocks[tail as usize].insts.iter().find_map(|i| match i {
            Inst::Cmp { dst, op: CmpOp::Slt, a, b, .. }
                if *dst == cv && *a == Operand::Val(jnext) && !tdef.contains(&match b {
                    Operand::Val(v) => *v,
                    _ => u32::MAX,
                }) =>
            {
                Some(*b)
            }
            _ => None,
        })?,
        _ => return None,
    };
    if !texit.args.is_empty() {
        return None;
    }

    // NOTHING THE NEST DEFINES IS READ AFTER IT, which is what lets four
    // iterations run together without any value of theirs escaping.
    let mut defined: Vec<ValueId> = Vec::new();
    for &b in &[outer, inner, tail] {
        defined.extend(defined_in(f, b));
    }
    for (b, blk) in f.blocks.iter().enumerate() {
        if b as BlockId == outer || b as BlockId == inner || b as BlockId == tail {
            continue;
        }
        let mut hit = false;
        let mut see = |o: Operand| {
            if let Operand::Val(v) = o {
                if defined.contains(&v) {
                    hit = true;
                }
            }
        };
        for inst in &blk.insts {
            inst.uses(&mut see);
        }
        blk.term.uses(&mut see);
        if hit {
            return None;
        }
    }

    // Which inner parameters carry something of `j`'s: each needs its OWN copy in
    // every lane, initialized at that lane's `j`.
    let iparams = f.blocks[inner as usize].params.clone();
    let dep = depends_full(f, &[outer, inner], j, &iparams, &to_inner.args);
    // The accumulator is per-lane by construction; it must not ALSO be a `j`
    // carrier, or the two roles would collide.
    if dep.contains(&iparams[accp]) {
        return None;
    }
    // The counter must be shared — a `j`-dependent trip count is a different loop
    // for every lane and this row does not claim it.
    if let Some(Operand::Val(v)) = Some(n) {
        if dep.contains(&v) {
            return None;
        }
    }

    Some(Nest {
        outer,
        jp,
        inner,
        accp,
        tail,
        tailp: 0,
        n,
        entry,
        exit: texit.clone(),
    })
}

/// Every value defined in `b` that reads `j`, directly or through another such
/// value. This is the set the jam clones, and it is a closure rather than a
/// pattern so no address shape has to be recognized.
/// The same closure with TWO roots — the outer counter and the tail's
/// accumulator parameter — which is what the tail needs.
fn depends_on_j2(
    f: &Func,
    b: BlockId,
    j: ValueId,
    k: ValueId,
    also: &HashSet<ValueId>,
) -> HashSet<ValueId> {
    let mut s: HashSet<ValueId> = HashSet::new();
    let mut again = true;
    while again {
        again = false;
        for inst in &f.blocks[b as usize].insts {
            let d = match inst.dst() {
                Some(d) => d,
                None => continue,
            };
            if s.contains(&d) {
                continue;
            }
            let mut hit = false;
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    if v == j || v == k || also.contains(&v) || s.contains(&v) {
                        hit = true;
                    }
                }
            });
            if hit {
                s.insert(d);
                again = true;
            }
        }
    }
    s
}

/// Which values of the nest depend on the outer counter — **parameters
/// included**, and that inclusion is the whole of a defect this pass had. A
/// strength-reduced pointer is a loop-carried PARAMETER whose initial value is
/// `base + j*w` and whose update is `+= stride`: it depends on `j` and nothing in
/// its own update says so. Following only instruction results misses it, and four
/// lanes then walk one column. `entry` names, for each parameter of `inner`, the
/// argument the outer header hands it.
fn depends_on_j(f: &Func, bs: &[BlockId], j: ValueId) -> HashSet<ValueId> {
    depends_full(f, bs, j, &[], &[])
}

fn depends_full(
    f: &Func,
    bs: &[BlockId],
    j: ValueId,
    params: &[ValueId],
    entry: &[Operand],
) -> HashSet<ValueId> {
    let mut s: HashSet<ValueId> = HashSet::new();
    let mut again = true;
    while again {
        again = false;
        for (k, &p) in params.iter().enumerate() {
            if s.contains(&p) {
                continue;
            }
            let dep = match entry.get(k) {
                Some(Operand::Val(v)) => *v == j || s.contains(v),
                _ => false,
            };
            if dep {
                s.insert(p);
                again = true;
            }
        }
        for inst in bs.iter().flat_map(|&b| f.blocks[b as usize].insts.iter()) {
            let d = match inst.dst() {
                Some(d) => d,
                None => continue,
            };
            if s.contains(&d) {
                continue;
            }
            let mut hit = false;
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    if v == j || s.contains(&v) {
                        hit = true;
                    }
                }
            });
            if hit {
                s.insert(d);
                again = true;
            }
        }
    }
    s
}

/// Copy a group of blocks, renaming every value they define and rewriting every
/// reference that stays inside the group. References OUT of the group keep
/// pointing where they pointed — which is what makes the copy a second, scalar
/// version of the same nest rather than a second entry into the first.
fn clone_group(f: &mut Func, group: &[BlockId]) -> HashMap<BlockId, BlockId> {
    let mut bmap: HashMap<BlockId, BlockId> = HashMap::new();
    for &b in group {
        bmap.insert(b, f.new_block());
    }
    let mut vmap: HashMap<ValueId, ValueId> = HashMap::new();
    // Parameters first: a phi's argument on one edge can be another phi's
    // parameter, and around a loop those two point at each other.
    for &b in group {
        let nb = bmap[&b];
        for k in 0..f.blocks[b as usize].params.len() {
            let old = f.blocks[b as usize].params[k];
            let nv = f.new_value(f.ty_of(old), Def::Param(nb, k as u32));
            f.blocks[nb as usize].params.push(nv);
            vmap.insert(old, nv);
        }
    }
    for &b in group {
        let nb = bmap[&b];
        let insts = f.blocks[b as usize].insts.clone();
        for (k, inst) in insts.into_iter().enumerate() {
            let mut c = inst.clone();
            if let Some(old) = inst.dst() {
                let nv = f.new_value(f.ty_of(old), Def::Inst(nb, k as u32));
                set_dst(&mut c, nv);
                vmap.insert(old, nv);
            }
            f.blocks[nb as usize].insts.push(c);
        }
        let mut t = f.blocks[b as usize].term.clone();
        for tg in t.targets_mut() {
            if let Some(&x) = bmap.get(&tg.block) {
                tg.block = x;
            }
        }
        f.blocks[nb as usize].term = t;
    }
    // Now every reference inside the copy is renamed. A value the group does NOT
    // define is left alone: it dominates both copies.
    for &b in group {
        let nb = bmap[&b] as usize;
        for inst in f.blocks[nb].insts.iter_mut() {
            inst.uses_mut(|o| {
                if let Operand::Val(v) = o {
                    if let Some(&nv) = vmap.get(v) {
                        *o = Operand::Val(nv);
                    }
                }
            });
        }
        let mut t = f.blocks[nb].term.clone();
        t.uses_mut(|o| {
            if let Operand::Val(v) = o {
                if let Some(&nv) = vmap.get(v) {
                    *o = Operand::Val(nv);
                }
            }
        });
        for tg in t.targets_mut() {
            for a in tg.args.iter_mut() {
                if let Operand::Val(v) = a {
                    if let Some(&nv) = vmap.get(v) {
                        *a = Operand::Val(nv);
                    }
                }
            }
        }
        f.blocks[nb].term = t;
    }
    bmap
}

fn jam(f: &mut Func, nst: &Nest) {
    // THE TAIL COMES FIRST, and it is not optional: after the jam the outer
    // counter advances four at a time, so a trip count that is not a multiple of
    // four leaves one to three iterations the jammed nest never runs. The copy
    // made here is the ORIGINAL nest, untouched, and it computes exactly them.
    let group = [nst.outer, nst.inner, nst.tail];
    let bmap = clone_group(f, &group);
    let scalar = bmap[&nst.outer];

    let j = f.blocks[nst.outer as usize].params[nst.jp];
    let ivty = f.ty_of(j);
    let acc = f.blocks[nst.inner as usize].params[nst.accp];
    // The SAME closure `recognize` used, parameters included — computed here from
    // the same two inputs so the two can never disagree about which values a lane
    // must own.
    let dep = {
        let ps = f.blocks[nst.inner as usize].params.clone();
        let ea = match &f.blocks[nst.outer as usize].term {
            Term::Jmp(t) => t.args.clone(),
            _ => unreachable!("recognize accepted a Jmp into the inner loop"),
        };
        depends_full(f, &[nst.outer, nst.inner], j, &ps, &ea)
    };

    // ── `j+1`, `j+2`, `j+3`, computed once in the outer header ─────────────
    let mut jl: Vec<ValueId> = vec![j];
    for l in 1..LANES {
        let d = f.new_value(ivty, Def::Inst(nst.outer, f.blocks[nst.outer as usize].insts.len() as u32));
        f.blocks[nst.outer as usize].insts.push(Inst::Bin {
            dst: d,
            op: BinOp::Add,
            ty: ivty,
            a: Operand::Val(j),
            b: Operand::Imm(l),
        });
        jl.push(d);
    }

    // ── the inner body: three more accumulators and three more dependence
    //    chains, each the original with `j` substituted ──────────────────────
    let inner = nst.inner as usize;
    let iback_args: Vec<Operand> = match &f.blocks[inner].term {
        Term::Br(_, t1, t2) => {
            if t1.block == nst.inner { t1.args.clone() } else { t2.args.clone() }
        }
        _ => unreachable!("recognize accepted a two-way inner terminator"),
    };
    let accnext = iback_args[nst.accp];

    let iparams = f.blocks[inner].params.clone();
    let to_inner_args: Vec<Operand> = match &f.blocks[nst.outer as usize].term {
        Term::Jmp(t) => t.args.clone(),
        _ => unreachable!("recognize accepted a Jmp into the inner loop"),
    };
    // Every parameter that carries something of `j`'s — a strength-reduced
    // pointer, typically — needs its own copy per lane, started at that lane's
    // `j`. The accumulator is one such copy by construction and is handled beside
    // them; the counter and the bound are SHARED, which is the whole point.
    let carriers: Vec<usize> = (0..iparams.len())
        .filter(|&k| k != nst.accp && dep.contains(&iparams[k]))
        .collect();

    // The per-lane substitution SURVIVES into the tail: the store's address is
    // built in the outer HEADER (`z4` computes `j<<2` there), so the tail's clone
    // must know the lane's copy of it or all four lanes store to one address.
    let mut lane_maps: Vec<HashMap<ValueId, ValueId>> = Vec::new();
    let mut lane_back: Vec<Vec<Operand>> = Vec::new();
    let mut lane_exit: Vec<Operand> = Vec::new();
    let mut lane_init: Vec<Vec<Operand>> = Vec::new();
    for l in 1..LANES as usize {
        let mut map: HashMap<ValueId, ValueId> = HashMap::new();
        map.insert(j, jl[l]);
        // (a) the lane's accumulator, and (b) its copy of every carrier — both
        // created BEFORE anything is cloned, because the body reads them.
        let a = f.new_value(f.ty_of(acc), Def::Param(nst.inner, 0));
        f.blocks[inner].params.push(a);
        map.insert(acc, a);
        let mut cp: Vec<ValueId> = Vec::new();
        for &k in &carriers {
            let old = iparams[k];
            let np = f.new_value(f.ty_of(old), Def::Param(nst.inner, 0));
            f.blocks[inner].params.push(np);
            map.insert(old, np);
            cp.push(np);
        }
        // (c) the header first and (d) the body second: the header builds the
        // bases the body's addresses read.
        for blk in [nst.outer, nst.inner] {
            let bi = blk as usize;
            let src: Vec<Inst> = f.blocks[bi]
                .insts
                .iter()
                .filter(|i| i.dst().is_some_and(|d| dep.contains(&d)))
                .cloned()
                .collect();
            for inst in src {
                let mut c = inst.clone();
                c.uses_mut(|o| {
                    if let Operand::Val(v) = o {
                        if let Some(&nv) = map.get(v) {
                            *o = Operand::Val(nv);
                        }
                    }
                });
                let old = inst.dst().expect("filtered on dst");
                let nd = f.new_value(f.ty_of(old), Def::Inst(blk, f.blocks[bi].insts.len() as u32));
                set_dst(&mut c, nd);
                map.insert(old, nd);
                f.blocks[bi].insts.push(c);
            }
        }
        let sub = |o: Operand| -> Operand {
            match o {
                Operand::Val(v) => Operand::Val(*map.get(&v).unwrap_or(&v)),
                other => other,
            }
        };
        // (e) what the entry edge hands this lane, and (f) what its back edge does
        let mut init = vec![to_inner_args[nst.accp]];
        for &k in &carriers {
            init.push(sub(to_inner_args[k]));
        }
        lane_init.push(init);
        let mut back = vec![sub(accnext)];
        for &k in &carriers {
            back.push(sub(iback_args[k]));
        }
        lane_exit.push(back[0]);
        lane_back.push(back);
        lane_maps.push(map);
        let _ = cp;
    }
    // thread them around the back edge, out the exit, and in on the entry edge
    let mut term = f.blocks[inner].term.clone();
    for t in term.targets_mut() {
        if t.block == nst.inner {
            for b in &lane_back {
                t.args.extend(b.iter().copied());
            }
        } else {
            t.args.extend(lane_exit.iter().copied());
        }
    }
    f.blocks[inner].term = term;
    if let Term::Jmp(t) = &mut f.blocks[nst.outer as usize].term {
        for i in &lane_init {
            t.args.extend(i.iter().copied());
        }
    }

    // ── the tail: three more parameters and three more stores ──────────────
    let tail = nst.tail as usize;
    let mut tparams: Vec<ValueId> = Vec::new();
    for _ in 1..LANES {
        let p = f.new_value(f.ty_of(acc), Def::Param(nst.tail, 0));
        f.blocks[tail].params.push(p);
        tparams.push(p);
    }
    // THE TAIL'S OWN COMPUTATION, cloned per lane. `z4_matmul_int` stores
    // `t + r`, not `t`, and its address is built from `j` — so neither the value
    // nor the address may be assumed. Both are obtained the same way the inner
    // loop's were: clone the closure with `j` and the accumulator substituted,
    // and let the clone compute what the original computes at `j+l`.
    let tp = f.blocks[tail].params[0];
    let (sty, saddr, sval) = f.blocks[tail]
        .insts
        .iter()
        .find_map(|i| match i {
            Inst::Store { ty, addr, val, vol: false, .. } => Some((*ty, *addr, *val)),
            _ => None,
        })
        .expect("recognize counted exactly one store");
    let tdep = depends_on_j2(f, nst.tail, j, tp, &dep);
    for (l, &p) in tparams.iter().enumerate() {
        let mut map = lane_maps[l].clone();
        map.insert(j, jl[l + 1]);
        map.insert(tp, p);
        let src: Vec<Inst> = f.blocks[tail]
            .insts
            .iter()
            .filter(|i| i.dst().is_some_and(|d| tdep.contains(&d)))
            .cloned()
            .collect();
        for inst in src {
            let mut c = inst.clone();
            c.uses_mut(|o| {
                if let Operand::Val(v) = o {
                    if let Some(&nv) = map.get(v) {
                        *o = Operand::Val(nv);
                    }
                }
            });
            let old = inst.dst().expect("filtered on dst");
            let nd = f.new_value(f.ty_of(old), Def::Inst(nst.tail, f.blocks[tail].insts.len() as u32));
            set_dst(&mut c, nd);
            map.insert(old, nd);
            f.blocks[tail].insts.push(c);
        }
        let sub = |o: Operand| -> Operand {
            match o {
                Operand::Val(v) => Operand::Val(*map.get(&v).unwrap_or(&v)),
                other => other,
            }
        };
        f.blocks[tail].insts.push(Inst::Store {
            ty: sty,
            addr: sub(saddr),
            val: sub(sval),
            aclass: 0,
            vol: false,
        });
    }
    let _ = sty.bytes();

    // `j` now steps by LANES, and the outer test asks for a whole group.
    let jnext = f.blocks[tail]
        .insts
        .iter_mut()
        .find_map(|i| match i {
            Inst::Bin { dst, op: BinOp::Add, a, b, .. }
                if *a == Operand::Val(j) && *b == Operand::Imm(1) =>
            {
                *b = Operand::Imm(LANES);
                Some(*dst)
            }
            _ => None,
        })
        .expect("recognize proved `j` steps by one");
    for inst in f.blocks[tail].insts.iter_mut() {
        if let Inst::Cmp { op: CmpOp::Slt, a, b, .. } = inst {
            if *a == Operand::Val(jnext) {
                // room for a whole group: `j' + (LANES-1) < n`
                *a = Operand::Val(jnext);
                let _ = b;
            }
        }
    }
    // The group test is `j' + LANES <= n`, which `Slt` on `j' + LANES - 1`
    // expresses without a new comparison operator.
    let room = f.new_value(ivty, Def::Inst(nst.tail, f.blocks[tail].insts.len() as u32));
    f.blocks[tail].insts.push(Inst::Bin {
        dst: room,
        op: BinOp::Add,
        ty: ivty,
        a: Operand::Val(jnext),
        b: Operand::Imm(LANES - 1),
    });
    let newc = f.new_value(Ty::I32, Def::Inst(nst.tail, f.blocks[tail].insts.len() as u32));
    f.blocks[tail].insts.push(Inst::Cmp {
        dst: newc,
        op: CmpOp::Slt,
        ty: ivty,
        a: Operand::Val(room),
        b: nst.n,
    });
    let mut tterm = f.blocks[tail].term.clone();
    if let Term::Br(cd, _, _) = &mut tterm {
        *cd = Operand::Val(newc);
    }
    f.blocks[tail].term = tterm;

    // ── the two edges that make the tail reachable ─────────────────────────
    //
    // G1, in front of everything: the jammed nest needs a whole group, so when
    // fewer than LANES iterations exist in total it must not run at all.
    // G2, after it: whatever the jam left is finished by the scalar copy, and
    // nothing is left when the counter has reached the bound.
    let entry_args: Vec<Operand> = f.blocks[nst.entry as usize]
        .term
        .targets()
        .iter()
        .find(|t| t.block == nst.outer)
        .map(|t| t.args.clone())
        .unwrap_or_default();
    let j0 = entry_args[nst.jp];

    let g1 = f.new_block();
    let last = f.new_value(ivty, Def::Inst(g1, 0));
    let c1 = f.new_value(Ty::I32, Def::Inst(g1, 1));
    f.blocks[g1 as usize].insts = vec![
        Inst::Bin { dst: last, op: BinOp::Add, ty: ivty, a: j0, b: Operand::Imm(LANES - 1) },
        Inst::Cmp { dst: c1, op: CmpOp::Slt, ty: ivty, a: Operand::Val(last), b: nst.n },
    ];
    f.blocks[g1 as usize].term = Term::Br(
        Operand::Val(c1),
        Target { block: nst.outer, args: entry_args.clone() },
        Target { block: scalar, args: entry_args.clone() },
    );
    let mut eterm = f.blocks[nst.entry as usize].term.clone();
    for t in eterm.targets_mut() {
        if t.block == nst.outer {
            t.block = g1;
        }
    }
    f.blocks[nst.entry as usize].term = eterm;

    let g2 = f.new_block();
    let gp = f.new_value(ivty, Def::Param(g2, 0));
    f.blocks[g2 as usize].params.push(gp);
    let c2 = f.new_value(Ty::I32, Def::Inst(g2, 0));
    f.blocks[g2 as usize].insts = vec![Inst::Cmp {
        dst: c2,
        op: CmpOp::Slt,
        ty: ivty,
        a: Operand::Val(gp),
        b: nst.n,
    }];
    let mut sargs = entry_args.clone();
    sargs[nst.jp] = Operand::Val(gp);
    f.blocks[g2 as usize].term = Term::Br(
        Operand::Val(c2),
        Target { block: scalar, args: sargs },
        nst.exit.clone(),
    );
    // the jammed tail leaves to G2, carrying the counter it reached
    let mut tt = f.blocks[nst.tail as usize].term.clone();
    for t in tt.targets_mut() {
        if t.block != nst.outer {
            t.block = g2;
            t.args = vec![Operand::Val(jnext)];
        }
    }
    f.blocks[nst.tail as usize].term = tt;

    let _ = nst.tailp;
}

/// Rewrite an instruction's destination. `Inst` has no setter, and the jam needs
/// one for every node it clones.
fn set_dst(i: &mut Inst, d: ValueId) {
    match i {
        Inst::Bin { dst, .. }
        | Inst::Un { dst, .. }
        | Inst::Cmp { dst, .. }
        | Inst::Cvt { dst, .. }
        | Inst::Load { dst, .. }
        | Inst::SlotAddr { dst, .. }
        | Inst::SymAddr { dst, .. }
        | Inst::Alloca { dst, .. }
        | Inst::Select { dst, .. } => *dst = d,
        other => unreachable!("jam cloned a node with no destination: {:?}", other),
    }
}
