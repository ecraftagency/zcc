// copyidiom — the element-at-a-time copy loop, given a `memcpy` fast path.
// THEORY A7b — optimization: this pass ships its commuting square
//
// WHAT IT IS FOR. `for (i=0;i<n;i++) d[i]=s[i];` compiles here to five
// instructions per ELEMENT — `ldrb, strb, add, cmp, b.lt` — while gcc -O2 emits
// `bl memcpy` and the libc routine moves 16 to 64 bytes per iteration with SIMD.
// `MEASURED` on the Graviton box: `g1_memcpy_loop` runs at 30.2x gcc -O2, the
// largest single outlier in the 96-program suite, and `perf` says the whole
// remaining gap has this shape — zcc emits 1.0045x gcc's STATIC instructions and
// executes 1.3250x its DYNAMIC ones.
//
// The shape is not an artefact of the benchmark. `ZCC_COPYPROBE` counted it
// across the corpus before this was written (Article A: demand is DETECTED):
// 16 in `sqlite3.c`, 11 in zlib, 2 in lua, 15 in the suite — and **0 in musl**,
// which is what makes calling `memcpy` safe from recursion into the libc that
// implements it. The refusal below makes that structural rather than lucky.
//
// WHY A RUNTIME GUARD, AND WHY IT IS NOT OPTIONAL. `d[i]=s[i]` on overlapping
// objects is DEFINED — it is a sequence of ordinary array assignments — while
// `memcpy` on overlapping objects is UNDEFINED (C99 7.21.2.1p2). Rewriting one
// into the other therefore turns defined behaviour into undefined unless the
// regions are known disjoint, and inside `void mycopy(char *d, const char *s,
// int n)` nothing static can know that. So the loop is not replaced: it is
// VERSIONED. A guard tests disjointness on the actual pointers, the `memcpy`
// runs only when it holds, and the original loop is still there for every other
// case. This is the standard "versioning for alias" a vectorizer needs anyway.
//
// COMMUTING SQUARE. Two arms, and each is an equality on its own guard:
//
//   * SLOW ARM — the original loop, unchanged, reached whenever the guard fails.
//     ⟦f⟧ = ⟦f'⟧ on that path by construction: no instruction moved.
//   * FAST ARM — reached only when `n > i0` and the two ranges
//     `[d+i0*w, d+n*w)` and `[s+i0*w, s+n*w)` are disjoint. Under disjointness
//     no store in the loop can change what a later load reads, so the loop's
//     iterations are independent and its whole effect is: every byte of the
//     source range is written to the corresponding byte of the destination
//     range, and nothing else in memory changes. That is exactly `memcpy`'s
//     specification over the same range (C99 7.21.2.1p1), and `n > i0` is what
//     makes the range non-empty and its length positive.
//
// Nothing else in the loop may be observable for the fast arm to be allowed to
// skip it, which is what the four refusals enforce: no value the loop defines is
// read after it, no side effect other than the one load and the one store, one
// entry edge, and exit arguments that name nothing the loop computed.
use super::*;

/// THEORY A7b — the pass ships ON, on the measurement rather than on the idea.
/// `ZCC_NOCOPYIDIOM` turns it off. 96 programs, gcc -O2 referee, Graviton4:
/// EXEC geomean 1.2799 -> 1.2290, `g1_memcpy_loop` 30.4x -> 1.0x, the count above
/// 1.1x from 52 to 48, INSN 1.0045 -> 1.0144 (the guard), 0 DIVERGE.
///
/// IT EMITS A CALL THE PROGRAM DID NOT WRITE, which is a real cost in a
/// freestanding build — the same one `gcc` pays for the same transform, and the
/// same switch answers it.
pub fn wanted() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var_os("ZCC_NOCOPYIDIOM").is_none())
}

/// THEORY A7b — instrument half: how many loops were versioned, so the A/B can
/// tell "the row bought nothing" from "the row never fired".
pub static FIRED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The libc routines a `memcpy` call must never be planted inside. A copy loop
/// in `memcpy` itself is the implementation, not a candidate; rewriting it is an
/// infinite recursion. musl holds none of this shape today, so the fence costs
/// nothing and removes the whole failure mode rather than relying on that.
fn is_libc_copy(name: &str) -> bool {
    matches!(
        name,
        "memcpy" | "memmove" | "mempcpy" | "memccpy" | "wmemcpy" | "wmemmove"
            | "strcpy" | "strncpy" | "stpcpy" | "stpncpy" | "bcopy"
    )
}

/// One recognized loop, with everything the rewrite needs read out of it.
struct Copy {
    header: BlockId,
    /// the outside predecessor, and its position in `preds`
    entry: BlockId,
    /// index of the induction variable among the header's parameters
    ivp: usize,
    /// element width in bytes
    w: u64,
    sbase: Operand,
    dbase: Operand,
    /// the exit test's bound
    n: Operand,
    /// the exit target, verbatim
    exit: Target,
}

/// THEORY A7b  SQUARE a_disjoint_copy_loop_is_memcpy — the versioned copy
pub fn run(f: &mut Func, a: &mut Analyses) -> bool {
    if !wanted() || is_libc_copy(&f.name) {
        return false;
    }
    let found = {
        let (c, _dt, lf) = a.all(f);
        let mut v: Vec<Copy> = Vec::new();
        for (li, l) in lf.loops.iter().enumerate() {
            if lf.loops.iter().any(|x| x.parent == Some(li as u32)) {
                continue; // innermost only
            }
            if let Some(cp) = recognize(f, c, lf, li) {
                v.push(cp);
            }
        }
        v
    };
    if found.is_empty() {
        return false;
    }
    for cp in &found {
        version(f, cp);
        FIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    refresh_defs(f);
    true
}

/// Is `o` available OUTSIDE the loop — an immediate, or a value the loop does
/// not define? A header parameter of this loop counts as defined by it.
fn invariant(f: &Func, o: Operand, inl: &[bool]) -> bool {
    match o {
        Operand::Val(v) => match f.values[v as usize].def {
            Def::Param(b, _) | Def::Inst(b, _) => !inl[b as usize],
            _ => true,
        },
        _ => true,
    }
}

fn recognize(f: &Func, c: &dom::Cfg, lf: &dom::LoopForest, li: usize) -> Option<Copy> {
    let l = &lf.loops[li];
    // ONE BLOCK. A copy loop with control flow inside it is a different shape and
    // a different proof; this row does not claim it.
    if l.body.len() != 1 {
        return None;
    }
    let h = l.header;
    let hi = h as usize;
    let mut inl = vec![false; f.blocks.len()];
    inl[hi] = true;

    // ONE ENTRY EDGE, so the initial value of the counter is a single operand.
    let outside: Vec<BlockId> = c.preds[hi].iter().copied().filter(|&p| p != h).collect();
    if outside.len() != 1 {
        return None;
    }
    let entry = outside[0];

    // The terminator: a two-way branch, one arm back to the header.
    let (cond, back, exit) = match &f.blocks[hi].term {
        Term::Br(cd, t1, t2) if t1.block == h && t2.block != h => (*cd, t1, t2),
        Term::Br(cd, t1, t2) if t2.block == h && t1.block != h => (*cd, t2, t1),
        _ => return None,
    };
    // EXIT ARGUMENTS may name nothing the loop computed — the fast arm jumps
    // straight there and has no value of the loop's to give.
    if !exit.args.iter().all(|&o| invariant(f, o, &inl)) {
        return None;
    }

    // The body: pure address arithmetic, exactly one load and one store.
    let mut load: Option<(Ty, Operand, ValueId)> = None;
    let mut store: Option<(Ty, Operand, Operand)> = None;
    for inst in &f.blocks[hi].insts {
        match inst {
            Inst::Load { dst, ty, addr, vol: false, .. } => {
                if load.replace((*ty, *addr, *dst)).is_some() {
                    return None;
                }
            }
            Inst::Store { ty, addr, val, vol: false, .. } => {
                if store.replace((*ty, *addr, *val)).is_some() {
                    return None;
                }
            }
            other => {
                if !matches!(other.effect(), Effect::Pure) {
                    return None;
                }
            }
        }
    }
    let (lty, laddr, ldst) = load?;
    let (sty, saddr, sval) = store?;
    if lty != sty || sval != Operand::Val(ldst) {
        return None;
    }
    let w = lty.bytes() as u64;

    // NOTHING THE LOOP DEFINES IS READ AFTER IT. Same fence `unroll.rs` states:
    // the fast arm skips every definition the body makes, so a reader outside
    // would have no value to read.
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
        for inst in &blk.insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    if defined.contains(&v) {
                        hit = true;
                    }
                }
            });
        }
        blk.term.uses(|o| {
            if let Operand::Val(v) = o {
                if defined.contains(&v) {
                    hit = true;
                }
            }
        });
        if hit {
            return None;
        }
    }

    // THE COUNTER. A header parameter whose back-edge argument is itself plus
    // one, and whose stride matches the addressing below.
    let mut ivp = None;
    for (k, &p) in f.blocks[hi].params.iter().enumerate() {
        if let Some(Operand::Val(nx)) = back.args.get(k).copied() {
            if let Def::Inst(b, i) = f.values[nx as usize].def {
                if b == h {
                    if let Inst::Bin { op: BinOp::Add, a, b: bb, .. } = &f.blocks[hi].insts[i as usize] {
                        if *a == Operand::Val(p) && *bb == Operand::Imm(1) {
                            ivp = Some((k, p, nx));
                        }
                    }
                }
            }
        }
    }
    let (ivp, iv, ivnext) = ivp?;

    // THE EXIT TEST: `i+1 <cmp> n`, with `n` invariant. Signed or unsigned; both
    // say the body runs while the counter is below the bound, which is the only
    // property the length below uses.
    let n = match cond {
        Operand::Val(cv) => match f.values[cv as usize].def {
            Def::Inst(b, i) if b == h => match &f.blocks[hi].insts[i as usize] {
                Inst::Cmp { op: CmpOp::Slt | CmpOp::Ult, a, b: bnd, .. }
                    if *a == Operand::Val(ivnext) && invariant(f, *bnd, &inl) =>
                {
                    *bnd
                }
                _ => return None,
            },
            _ => return None,
        },
        _ => return None,
    };

    // THE ADDRESSES: `base + i*w`, the same offset value on both sides.
    let addr_of = |o: Operand| -> Option<(Operand, ValueId)> {
        let v = match o {
            Operand::Val(v) => v,
            _ => return None,
        };
        match f.values[v as usize].def {
            Def::Inst(b, i) if b == h => match &f.blocks[hi].insts[i as usize] {
                Inst::Bin { op: BinOp::Add, a, b: bb, .. } => match (*a, *bb) {
                    (base, Operand::Val(off)) if invariant(f, base, &inl) => Some((base, off)),
                    (Operand::Val(off), base) if invariant(f, base, &inl) => Some((base, off)),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    };
    let (sbase, soff) = addr_of(laddr)?;
    let (dbase, doff) = addr_of(saddr)?;
    if soff != doff {
        return None;
    }
    // The offset is the counter scaled by the element width, and the scale must
    // BE the width — an `i*w` that does not match the access is a different
    // access pattern and this row does not claim it.
    let ok_off = if w == 1 {
        soff == iv
    } else {
        match f.values[soff as usize].def {
            Def::Inst(b, i) if b == h => matches!(
                &f.blocks[hi].insts[i as usize],
                Inst::Bin { op: BinOp::Shl, a, b: Operand::Imm(k), .. }
                    if *a == Operand::Val(iv) && (1u64 << *k) == w
            ),
            _ => false,
        }
    };
    if !ok_off {
        return None;
    }

    Some(Copy { header: h, entry, ivp, w, sbase, dbase, n, exit: exit.clone() })
}

/// Build the guard and the fast arm, and hand the original loop the failing edge.
fn version(f: &mut Func, cp: &Copy) {
    let hi = cp.header as usize;
    // The arguments the entry edge was passing — they dominate the new blocks,
    // because the new blocks sit ON that edge.
    let mut entry_args: Vec<Operand> = Vec::new();
    for t in f.blocks[cp.entry as usize].term.targets() {
        if t.block == cp.header {
            entry_args = t.args.clone();
        }
    }
    let i0 = entry_args[cp.ivp];

    let guard = f.new_block();
    let fast = f.new_block();

    // ── the guard: `n > i0` and the two ranges are disjoint ────────────────
    let mut g: Vec<Inst> = Vec::new();
    // `Cmp` yields I32 and address arithmetic is I64; the verifier is what says
    // so, and it said so on the first build of this pass.
    let bin = |g: &mut Vec<Inst>, f: &mut Func, ty: Ty, op: BinOp, a: Operand, b: Operand| -> Operand {
        let d = f.new_value(ty, Def::Inst(guard, g.len() as u32));
        g.push(Inst::Bin { dst: d, op, ty, a, b });
        Operand::Val(d)
    };
    // len = (n - i0) * w, in bytes; src = s + i0*w; dst = d + i0*w
    let cnt = bin(&mut g, f, Ty::I64, BinOp::Sub, cp.n, i0);
    let len = if cp.w == 1 { cnt } else { bin(&mut g, f, Ty::I64, BinOp::Mul, cnt, Operand::Imm(cp.w as i64)) };
    let base_off = if cp.w == 1 { i0 } else { bin(&mut g, f, Ty::I64, BinOp::Mul, i0, Operand::Imm(cp.w as i64)) };
    let src = bin(&mut g, f, Ty::I64, BinOp::Add, cp.sbase, base_off);
    let dst = bin(&mut g, f, Ty::I64, BinOp::Add, cp.dbase, base_off);
    let dend = bin(&mut g, f, Ty::I64, BinOp::Add, dst, len);
    let send = bin(&mut g, f, Ty::I64, BinOp::Add, src, len);
    let cmp = |g: &mut Vec<Inst>, f: &mut Func, op: CmpOp, ty: Ty, a: Operand, b: Operand| -> Operand {
        let d = f.new_value(Ty::I32, Def::Inst(guard, g.len() as u32));
        g.push(Inst::Cmp { dst: d, op, ty, a, b });
        Operand::Val(d)
    };
    // Pointers compare UNSIGNED; the trip test compares the way the loop did.
    let c1 = cmp(&mut g, f, CmpOp::Ule, Ty::I64, dend, src);
    let c2 = cmp(&mut g, f, CmpOp::Ule, Ty::I64, send, dst);
    let disjoint = bin(&mut g, f, Ty::I32, BinOp::Or, c1, c2);
    let runs = cmp(&mut g, f, CmpOp::Sgt, Ty::I64, cp.n, i0);
    let take = bin(&mut g, f, Ty::I32, BinOp::And, disjoint, runs);

    f.blocks[guard as usize].insts = g;
    f.blocks[guard as usize].term = Term::Br(
        take,
        Target { block: fast, args: Vec::new() },
        Target { block: cp.header, args: entry_args.clone() },
    );

    // ── the fast arm ───────────────────────────────────────────────────────
    let sig = Sig {
        params: vec![PTy::S(Ty::I64), PTy::S(Ty::I64), PTy::S(Ty::I64)],
        ret: Some(PTy::S(Ty::I64)),
        nfix: 3,
        variadic: false,
    };
    f.blocks[fast as usize].insts = vec![Inst::Call {
        dst: None,
        sig,
        callee: Callee::Direct("memcpy".to_string()),
        args: vec![dst, src, len],
        sret: None,
    }];
    f.blocks[fast as usize].term = Term::Jmp(cp.exit.clone());

    // ── the entry edge now reaches the guard ───────────────────────────────
    let mut term = f.blocks[cp.entry as usize].term.clone();
    for t in term.targets_mut() {
        if t.block == cp.header {
            t.block = guard;
            t.args.clear();
        }
    }
    f.blocks[cp.entry as usize].term = term;
    let _ = hi;
}
