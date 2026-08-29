// vecmap — four lanes at a time through a unit-stride map loop.
// THEORY A7b — optimization: this pass ships its commuting square
//
// THE SHAPE. A counted loop whose every iteration is INDEPENDENT — nothing
// carried but the counter — reading and writing consecutive elements:
//
//     for (i=0;i<n;i++) c[i] = a[i] + b[i];      two streams
//     for (i=0;i<n;i++) a[i] = a[i] * 3;         one stream and a scalar
//
// Four `int` lanes fit one `q` register, so four iterations become `ldr q`,
// `ldr q` or `dup`, one lane operation, `str q` — and the loop runs a quarter as
// often. `MEASURED`: gcc -O2 wins its 3.3x on `z4_matmul_int` with exactly this
// register width (`mla v.4s`) while gcc -O1 emits no SIMD at all, and at TWO
// lanes the same gcc buys 3% and loses to zcc's scalar code. The prize is at
// four lanes, which is elements of 32 bits.
//
// WHY IT IS NOT A REDUCTION, and that is what makes the proof short. The four
// lanes are four DIFFERENT iterations of an independent map, not four partial
// sums of one accumulator. Nothing is reassociated, no horizontal add is needed,
// and no question about overflow or floating-point associativity arises: lane
// `l` computes exactly what iteration `i+l` computed, with the same operands.
//
// COMMUTING SQUARE, and it is two arms as `copyidiom`'s is:
//
//   * SLOW ARM — the original loop, untouched, entered whenever the guard fails
//     and also for the TAIL the vector loop could not cover. ⟦f⟧ = ⟦f'⟧ there by
//     construction.
//   * FAST ARM — entered only when at least `lanes` iterations remain AND every
//     source range is either IDENTICAL to the destination range or disjoint from
//     it. Under that condition no lane's store can change what another lane's
//     load reads, so the `lanes` iterations commute and running them at once
//     computes what running them in order computed. `⟦VecMap⟧` is written
//     lanewise in `hir::interp` for exactly this reason: the square is checkable.
//
// An IDENTICAL range is allowed and is not an oversight — `a[i] = a[i]*3` reads
// and writes lane `l` at one address, which a lanewise operation does in that
// order. It is PARTIAL overlap that the range test refuses.
use super::*;

/// THEORY A7b — the pass ships default-OFF until its A/B is on the board. A row
/// that emits SIMD where the program had none is not one to ship on an argument.
pub fn wanted() -> bool {
    WANT.with(|c| c.get()).unwrap_or_else(|| {
        static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *W.get_or_init(|| std::env::var_os("ZCC_VECMAP").is_some())
    })
}

thread_local! {
    // THEORY A7b — instrument half. Not a value the compiler computes with: it is
    // the switch a battery flips to measure that a pack was actually built, which
    // is the non-vacuity obligation.
    //
    // A thread-local overlay over the environment, for the reason `spill.rs`'s
    // seams are thread-locals: the battery runs its tests in parallel threads and
    // a process-wide switch would make one test's result depend on another's
    // timing. `None` means "ask the environment", which is what every non-test
    // caller gets.
    static WANT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force the row on or off for the CURRENT THREAD, or hand the decision back to
/// the environment. A theorem that ships disabled still owes its square.
pub fn set_wanted(on: Option<bool>) {
    WANT.with(|c| c.set(on));
}

/// THEORY A7b — instrument half: loops vectorized, so an A/B can tell "bought
/// nothing" from "never fired".
pub static FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One recognized map loop.
struct Map {
    header: BlockId,
    entry: BlockId,
    ivp: usize,
    ty: Ty,
    w: u64,
    op: BinOp,
    dbase: Operand,
    abase: Operand,
    /// the second source: another stream, or a scalar to broadcast
    b: Operand,
    bmem: bool,
    n: Operand,
    exit: Target,
}

/// THEORY A7b  SQUARE four_independent_iterations_are_one_lane_operation — the vector map
pub fn run(f: &mut Func, a: &mut Analyses) -> bool {
    if !wanted() {
        return false;
    }
    let found = {
        let (c, _dt, lf) = a.all(f);
        let mut v: Vec<Map> = Vec::new();
        for (li, l) in lf.loops.iter().enumerate() {
            if lf.loops.iter().any(|x| x.parent == Some(li as u32)) {
                continue;
            }
            if let Some(m) = recognize(f, c, lf, li) {
                v.push(m);
            }
        }
        v
    };
    if found.is_empty() {
        return false;
    }
    for m in &found {
        vectorize(f, m);
        FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    refresh_defs(f);
    true
}

fn invariant(f: &Func, o: Operand, h: usize) -> bool {
    match o {
        Operand::Val(v) => match f.values[v as usize].def {
            Def::Param(b, _) | Def::Inst(b, _) => b as usize != h,
            _ => true,
        },
        _ => true,
    }
}

fn recognize(f: &Func, c: &dom::Cfg, lf: &dom::LoopForest, li: usize) -> Option<Map> {
    let l = &lf.loops[li];
    if l.body.len() != 1 {
        return None;
    }
    let h = l.header;
    let hi = h as usize;

    let outside: Vec<BlockId> = c.preds[hi].iter().copied().filter(|&p| p != h).collect();
    if outside.len() != 1 {
        return None;
    }
    let entry = outside[0];

    let (cond, back, exit) = match &f.blocks[hi].term {
        Term::Br(cd, t1, t2) if t1.block == h && t2.block != h => (*cd, t1, t2),
        Term::Br(cd, t1, t2) if t2.block == h && t1.block != h => (*cd, t2, t1),
        _ => return None,
    };
    if !exit.args.iter().all(|&o| invariant(f, o, hi)) {
        return None;
    }

    // ONE store, one or two loads, exactly one arithmetic node, nothing opaque.
    let mut loads: Vec<(Ty, Operand, ValueId)> = Vec::new();
    let mut store: Option<(Ty, Operand, Operand)> = None;
    let mut bins: Vec<(ValueId, BinOp, Ty, Operand, Operand)> = Vec::new();
    for inst in &f.blocks[hi].insts {
        match inst {
            Inst::Load { dst, ty, addr, vol: false, .. } => loads.push((*ty, *addr, *dst)),
            Inst::Store { ty, addr, val, vol: false, .. } => {
                if store.replace((*ty, *addr, *val)).is_some() {
                    return None;
                }
            }
            Inst::Bin { dst, op, ty, a, b } => bins.push((*dst, *op, *ty, *a, *b)),
            Inst::Cmp { .. } => {}
            other => {
                if !matches!(other.effect(), Effect::Pure) {
                    return None;
                }
            }
        }
    }
    let (sty, saddr, sval) = store?;
    // Only the element widths a `q` register divides into lanes this pass knows.
    if !matches!(sty, Ty::I32) {
        return None;
    }
    let w = sty.bytes() as u64;

    // Nothing the loop defines may be read after it.
    let mut defined: Vec<ValueId> = f.blocks[hi].params.clone();
    for inst in &f.blocks[hi].insts {
        if let Some(d) = inst.dst() {
            defined.push(d);
        }
    }
    for (b, blk) in f.blocks.iter().enumerate() {
        if b == hi {
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

    // The counter, and its stride must BE the element width.
    let mut ivfound = None;
    for (k, &p) in f.blocks[hi].params.iter().enumerate() {
        if let Some(Operand::Val(nx)) = back.args.get(k).copied() {
            if let Def::Inst(b, i) = f.values[nx as usize].def {
                if b == h {
                    if let Inst::Bin { op: BinOp::Add, a, b: bb, .. } =
                        &f.blocks[hi].insts[i as usize]
                    {
                        if *a == Operand::Val(p) && *bb == Operand::Imm(1) {
                            ivfound = Some((k, p, nx));
                        }
                    }
                }
            }
        }
    }
    let (ivp, iv, ivnext) = ivfound?;
    // ONE carried value: the counter. More is a dependence, not a map.
    let carried = f.blocks[hi]
        .params
        .iter()
        .enumerate()
        .filter(|(k, _)| {
            matches!(back.args.get(*k), Some(Operand::Val(v))
                if matches!(f.values[*v as usize].def, Def::Inst(b,_)|Def::Param(b,_) if b == h))
        })
        .count();
    if carried != 1 {
        return None;
    }

    let n = match cond {
        Operand::Val(cv) => match f.values[cv as usize].def {
            Def::Inst(b, i) if b == h => match &f.blocks[hi].insts[i as usize] {
                Inst::Cmp { op: CmpOp::Slt, a, b: bnd, .. }
                    if *a == Operand::Val(ivnext) && invariant(f, *bnd, hi) =>
                {
                    *bnd
                }
                _ => return None,
            },
            _ => return None,
        },
        _ => return None,
    };

    // Every address is `base + iv*w`, with the SAME scaled offset value.
    let scaled = f.blocks[hi].insts.iter().enumerate().find_map(|(i, inst)| match inst {
        Inst::Bin { dst, op: BinOp::Shl, a, b: Operand::Imm(k), .. }
            if *a == Operand::Val(iv) && (1u64 << *k) == w =>
        {
            let _ = i;
            Some(*dst)
        }
        _ => None,
    })?;
    let base_of = |o: Operand| -> Option<Operand> {
        let v = match o {
            Operand::Val(v) => v,
            _ => return None,
        };
        match f.values[v as usize].def {
            Def::Inst(b, i) if b == h => match &f.blocks[hi].insts[i as usize] {
                Inst::Bin { op: BinOp::Add, a, b: bb, .. } => match (*a, *bb) {
                    (base, Operand::Val(o)) if o == scaled && invariant(f, base, hi) => Some(base),
                    (Operand::Val(o), base) if o == scaled && invariant(f, base, hi) => Some(base),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    };
    let dbase = base_of(saddr)?;

    // The one arithmetic node, and the store must store exactly its result.
    let arith: Vec<_> = bins
        .iter()
        .filter(|(d, ..)| Operand::Val(*d) == sval)
        .collect();
    if arith.len() != 1 {
        return None;
    }
    let (_, op, bty, x, y) = *arith[0];
    if bty != sty || !matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::Xor)
    {
        return None;
    }
    // `x` must be a load of this loop's stream; `y` is a second stream or an
    // invariant scalar. A commutative op could take them the other way round,
    // but a `Sub` could not, so the order is read as written.
    let load_base = |o: Operand| -> Option<Operand> {
        let v = match o {
            Operand::Val(v) => v,
            _ => return None,
        };
        let (lty, laddr, _) = *loads.iter().find(|(_, _, d)| *d == v)?;
        if lty != sty {
            return None;
        }
        base_of(laddr)
    };
    let abase = load_base(x)?;
    let (b, bmem) = match load_base(y) {
        Some(bb) => (bb, true),
        None if invariant(f, y, hi) => (y, false),
        None => return None,
    };
    // Every load must be one of the two the arithmetic names, or the body reads
    // something this rewrite would drop.
    if loads.len() > if bmem { 2 } else { 1 } {
        return None;
    }

    Some(Map {
        header: h,
        entry,
        ivp,
        ty: sty,
        w,
        op,
        dbase,
        abase,
        b,
        bmem,
        n,
        exit: exit.clone(),
    })
}

fn vectorize(f: &mut Func, m: &Map) {
    let lanes = 16 / m.w;
    let mut entry_args: Vec<Operand> = Vec::new();
    for t in f.blocks[m.entry as usize].term.targets() {
        if t.block == m.header {
            entry_args = t.args.clone();
        }
    }
    let i0 = entry_args[m.ivp];

    let guard = f.new_block();
    let vloop = f.new_block();
    let vpost = f.new_block();

    // ── the guard ──────────────────────────────────────────────────────────
    let mut g: Vec<Inst> = Vec::new();
    let bin = |g: &mut Vec<Inst>, f: &mut Func, blk: BlockId, ty: Ty, op: BinOp, a: Operand, b: Operand| -> Operand {
        let d = f.new_value(ty, Def::Inst(blk, g.len() as u32));
        g.push(Inst::Bin { dst: d, op, ty, a, b });
        Operand::Val(d)
    };
    let cmp = |g: &mut Vec<Inst>, f: &mut Func, blk: BlockId, op: CmpOp, ty: Ty, a: Operand, b: Operand| -> Operand {
        let d = f.new_value(Ty::I32, Def::Inst(blk, g.len() as u32));
        g.push(Inst::Cmp { dst: d, op, ty, a, b });
        Operand::Val(d)
    };
    let wi = Operand::Imm(m.w as i64);
    let cnt = bin(&mut g, f, guard, Ty::I64, BinOp::Sub, m.n, i0);
    let byte = bin(&mut g, f, guard, Ty::I64, BinOp::Mul, cnt, wi);
    let off0 = bin(&mut g, f, guard, Ty::I64, BinOp::Mul, i0, wi);
    let d0 = bin(&mut g, f, guard, Ty::I64, BinOp::Add, m.dbase, off0);
    let dend = bin(&mut g, f, guard, Ty::I64, BinOp::Add, d0, byte);
    // Enough iterations for one full vector.
    let mut ok = cmp(&mut g, f, guard, CmpOp::Sge, Ty::I64, cnt, Operand::Imm(lanes as i64));
    // Each source: the SAME stream as the destination, or disjoint from it.
    let mut srcs: Vec<Operand> = vec![m.abase];
    if m.bmem {
        srcs.push(m.b);
    }
    for s in srcs {
        if s == m.dbase {
            continue; // one stream read and written lane by lane at one address
        }
        let s0 = bin(&mut g, f, guard, Ty::I64, BinOp::Add, s, off0);
        let send = bin(&mut g, f, guard, Ty::I64, BinOp::Add, s0, byte);
        let c1 = cmp(&mut g, f, guard, CmpOp::Ule, Ty::I64, dend, s0);
        let c2 = cmp(&mut g, f, guard, CmpOp::Ule, Ty::I64, send, d0);
        let dis = bin(&mut g, f, guard, Ty::I32, BinOp::Or, c1, c2);
        ok = bin(&mut g, f, guard, Ty::I32, BinOp::And, ok, dis);
    }
    f.blocks[guard as usize].insts = g;
    let ivty = f.ty_of(f.blocks[m.header as usize].params[m.ivp]);
    let vpar = f.new_value(ivty, Def::Param(vloop, 0));
    f.blocks[vloop as usize].params.push(vpar);
    let mut ventry = entry_args.clone();
    ventry.truncate(0);
    ventry.push(i0);
    f.blocks[guard as usize].term = Term::Br(
        ok,
        Target { block: vloop, args: ventry },
        Target { block: m.header, args: entry_args.clone() },
    );

    // ── the vector loop ────────────────────────────────────────────────────
    let mut v: Vec<Inst> = Vec::new();
    let j = Operand::Val(vpar);
    let voff = bin(&mut v, f, vloop, Ty::I64, BinOp::Mul, j, wi);
    let vd = bin(&mut v, f, vloop, Ty::I64, BinOp::Add, m.dbase, voff);
    let va = bin(&mut v, f, vloop, Ty::I64, BinOp::Add, m.abase, voff);
    let vb = if m.bmem {
        bin(&mut v, f, vloop, Ty::I64, BinOp::Add, m.b, voff)
    } else {
        m.b
    };
    v.push(Inst::Intrinsic {
        dst: None,
        kind: IntrinKind::VecMap { op: m.op, ty: m.ty, bmem: m.bmem },
        args: vec![vd, va, vb],
    });
    let j2 = bin(&mut v, f, vloop, ivty, BinOp::Add, j, Operand::Imm(lanes as i64));
    let room = bin(&mut v, f, vloop, ivty, BinOp::Add, j2, Operand::Imm(lanes as i64));
    let again = cmp(&mut v, f, vloop, CmpOp::Sle, Ty::I64, room, m.n);
    f.blocks[vloop as usize].insts = v;
    f.blocks[vloop as usize].term = Term::Br(
        again,
        Target { block: vloop, args: vec![j2] },
        Target { block: vpost, args: vec![j2] },
    );

    // ── the tail ───────────────────────────────────────────────────────────
    // The scalar loop is BOTTOM-tested: entering it at `j == n` would run its
    // body once at index n. So the tail is entered only when something is left.
    let tpar = f.new_value(ivty, Def::Param(vpost, 0));
    f.blocks[vpost as usize].params.push(tpar);
    let mut t: Vec<Inst> = Vec::new();
    let more = cmp(&mut t, f, vpost, CmpOp::Slt, Ty::I64, Operand::Val(tpar), m.n);
    f.blocks[vpost as usize].insts = t;
    let mut tail_args = entry_args.clone();
    tail_args[m.ivp] = Operand::Val(tpar);
    f.blocks[vpost as usize].term = Term::Br(
        more,
        Target { block: m.header, args: tail_args },
        m.exit.clone(),
    );

    // ── the entry edge now reaches the guard ───────────────────────────────
    let mut term = f.blocks[m.entry as usize].term.clone();
    for tg in term.targets_mut() {
        if tg.block == m.header {
            tg.block = guard;
            tg.args.clear();
        }
    }
    f.blocks[m.entry as usize].term = term;
}
