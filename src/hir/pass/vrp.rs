// vrp — VALUE RANGE PROPAGATION (REARCH §16 ★2; Patterson 1995).
// THEORY A7b — optimization: this pass ships its commuting square
//
// `sccp` answers "is this value THE constant k?". Most of what a program knows
// about a value is weaker than that and still decisive: an index masked with
// `& 0xff` is in `[0, 255]` whatever it started as, a value guarded by
// `if (n < 16)` is below sixteen for the whole arm, and a comparison against a
// bound outside the range is not a branch at all. That is the fact this pass
// computes, and it is the same lattice shape as `sccp` one step up: a constant
// is the interval `[k, k]`, so the two are not rivals — `sccp` is the point case
// solved exactly, and this is the interval case solved with widening.
//
// THE LATTICE. Every integer value carries `[lo, hi]`, bounds on its
// SIGN-EXTENDED value at its own width, held in `i128` so no transfer function
// can overflow the arithmetic it is reasoning about. `Full` for a width is the
// whole of it, and every value starts there except a constant. Meet is interval
// union, which is what a block parameter takes over its incoming arguments.
//
// TERMINATION is Cousot & Cousot's widening and nothing cleverer: a value whose
// range grows on a back edge is sent straight to `Full` rather than crawled
// upward one iteration at a time. The lattice has finite height under that rule
// — a range either stays, narrows once, or goes to `Full` and stops — so the
// fixpoint is reached in a bounded number of rounds.
//
// GUARDS, and this is where the useful facts come from. A range computed from
// definitions alone knows nothing about `if (n < 16) { ... }`; the fact lives on
// the EDGE, not in the definition. The general machinery for that is a
// per-edge lattice; the cheap 90% is the observation `mem.rs` already makes for
// memory — a block whose ONLY predecessor is a conditional branch is entered
// exactly when that branch went its way, so the guard holds throughout it. The
// constraint map is built once along the dominator tree, each block inheriting
// its immediate dominator's and adding its own edge's, so a query is a lookup
// and not a walk.
//
// WHAT IT DOES WITH THE FACT, and both are proven by the same argument — an
// expression is replaced by one that has the same value ON THE RANGE THE VALUE
// IS PROVEN TO HAVE, so the two agree on every executable run:
//
//   * BRANCH FOLDING — a comparison every pair in the two ranges decides the
//     same way is that constant. The branch it feeds then falls to `sccp`, which
//     already deletes an arm no run reaches; this pass does not touch the
//     terminator itself, so the two rows keep their separate squares.
//   * SIGNED DIVISION BY A POWER OF TWO — `x / 2^k` and `x % 2^k` are a
//     three-instruction dance in two's complement because the quotient rounds
//     toward zero and the dividend may be negative. On a dividend PROVEN
//     non-negative the correction is dead: the quotient is `x >> k` logically
//     and the remainder is `x & (2^k - 1)`, exactly as for an unsigned value.
//     C99 6.5.5p6 fixes the rounding, so this is a rewrite of the operation and
//     not of the language.
//
// A residual, and it is a category-(b) truncation rather than a limit (Law 4):
// the ranges of NARROW loads are `Full` at their width, because HIR's `Ty`
// carries no signedness and a `Load` at `I8` may be a `char` or an
// `unsigned char`. The fact that would fix it lives in the frontend, beside the
// alias class R5.2 stamps.
use super::*;

/// R5.5's A/B SEAM (`ZCC_VRP`). Off, the pass does not run and the ladder is the
/// pre-R5.5 one, which the byte-identical gate checks. A thread-local overlay
/// over the environment, as `hir::tbaa_wanted` and `hir::freq::weights_wanted`.
pub fn wanted() -> bool {
    VRP.with(|c| c.get()).unwrap_or_else(|| {
        static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENV.get_or_init(|| std::env::var_os("ZCC_VRP").is_some())
    })
}

thread_local! {
    // THEORY A7b — instrument half: the switch a test flips to measure that the
    // ranges decided something (the non-vacuity obligation).
    static VRP: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Force the pass on or off for the CURRENT THREAD, or hand it back to the
/// environment with `None`.
#[cfg(test)]
pub fn set_vrp(on: Option<bool>) {
    VRP.with(|c| c.set(on));
}

/// A closed interval on the sign-extended value, or nothing known.
#[derive(Clone, Copy, PartialEq, Debug)]
struct R {
    lo: i128,
    hi: i128,
}

impl R {
    fn point(k: i128) -> R {
        R { lo: k, hi: k }
    }
    /// The whole of a width — what a load, a call or a parameter is worth.
    fn full(ty: Ty) -> R {
        let b = ty.bits() as u32;
        R { lo: -(1i128 << (b - 1)), hi: (1i128 << (b - 1)) - 1 }
    }
    fn union(self, o: R) -> R {
        R { lo: self.lo.min(o.lo), hi: self.hi.max(o.hi) }
    }
    fn meet(self, o: R) -> R {
        // an intersection that would be empty means the two facts cannot both
        // hold, which on a reachable path they do — so keep the weaker one
        // rather than inventing an empty range no transfer function expects
        let r = R { lo: self.lo.max(o.lo), hi: self.hi.min(o.hi) };
        if r.lo > r.hi {
            self
        } else {
            r
        }
    }
    /// The interval, unless it has left the width — a value that overflowed is
    /// not the mathematical result, so the fact is dropped rather than kept.
    fn fit(self, ty: Ty) -> R {
        let f = R::full(ty);
        if self.lo < f.lo || self.hi > f.hi {
            f
        } else {
            self
        }
    }
    fn nonneg(self) -> bool {
        self.lo >= 0
    }
}

/// THEORY A7b  SQUARE vrp_replaces_an_expression_by_one_equal_on_its_range
pub fn run(f: &mut Func) -> bool {
    if !wanted() {
        return false;
    }
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let rng = solve(f, &c);
    let guards = guard_map(f, &c, &dt);
    let mut changed = false;
    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    for b in 0..f.blocks.len() {
        if !c.reachable(b as BlockId) {
            continue;
        }
        for i in 0..f.blocks[b].insts.len() {
            let at = |o: Operand, ty: Ty| -> R {
                let base = range_of(&rng, o, ty);
                match o {
                    Operand::Val(v) => match guards[b].iter().find(|(g, _)| *g == v) {
                        Some((_, r)) => base.meet(*r),
                        None => base,
                    },
                    _ => base,
                }
            };
            match f.blocks[b].insts[i].clone() {
                Inst::Cmp { dst, op, ty, a, b: bb } => {
                    if ty.is_float() {
                        continue;
                    }
                    if let Some(k) = decide(op, at(a, ty), at(bb, ty)) {
                        map[dst as usize] = Some(Operand::Imm(k));
                        changed = true;
                    }
                }
                Inst::Bin { dst, op, ty, a, b: bo } => {
                    let k = match bo {
                        Operand::Imm(k) if k > 0 && (k as u64).is_power_of_two() => k,
                        _ => continue,
                    };
                    if !at(a, ty).nonneg() {
                        continue;
                    }
                    let sh = (k as u64).trailing_zeros() as i64;
                    let new = match op {
                        BinOp::SDiv => Inst::Bin {
                            dst,
                            op: BinOp::LShr,
                            ty,
                            a,
                            b: Operand::Imm(sh),
                        },
                        BinOp::SRem => Inst::Bin {
                            dst,
                            op: BinOp::And,
                            ty,
                            a,
                            b: Operand::Imm(k - 1),
                        },
                        _ => continue,
                    };
                    f.blocks[b].insts[i] = new;
                    changed = true;
                }
                _ => {}
            }
        }
    }
    if map.iter().any(|x| x.is_some()) {
        rewrite_values(f, &map);
    }
    changed
}

fn range_of(rng: &[R], o: Operand, ty: Ty) -> R {
    match o {
        Operand::Val(v) => rng[v as usize],
        Operand::Imm(k) => R::point(k as i128),
        // a float operand has no integer range, and no caller asks for one
        Operand::Fimm(_) => R::full(ty),
    }
}

/// The fixpoint: every value's range, over the whole function.
///
/// Reverse postorder with a bounded number of sweeps rather than a worklist —
/// the widening below is what bounds it, and a sweep is a pass over the
/// instructions the pass is already reading.
fn solve(f: &Func, c: &dom::Cfg) -> Vec<R> {
    let mut rng: Vec<R> = f.values.iter().map(|v| R::full(v.ty)).collect();
    // ROUNDS is the ladder's own constant: the same bound, for the same reason —
    // a fixpoint that has not settled by then is one this pass will not settle.
    for round in 0..super::ROUNDS {
        let mut moved = false;
        for &blk in &c.rpo {
            let b = blk as usize;
            for k in 0..f.blocks[b].params.len() {
                let p = f.blocks[b].params[k];
                let ty = f.values[p as usize].ty;
                let mut m: Option<R> = None;
                let mut back = false;
                for &pb in &c.preds[b] {
                    // a predecessor this sweep has not reached yet is a BACK
                    // edge; its argument's range may still grow
                    if c.rpo_num[pb as usize] >= c.rpo_num[b] {
                        back = true;
                    }
                    for t in f.blocks[pb as usize].term.targets() {
                        if t.block != blk {
                            continue;
                        }
                        let a = match t.args.get(k) {
                            Some(a) => *a,
                            None => continue,
                        };
                        let r = range_of(&rng, a, ty);
                        m = Some(match m {
                            Some(x) => x.union(r),
                            None => r,
                        });
                    }
                }
                // WIDENING: a parameter that a back edge feeds is given one
                // round to be a constant and then, if it moved, the whole width.
                // Crawling upward one iteration at a time is what would not
                // terminate; `Full` is the sound answer and the loop stops.
                let m = match m {
                    Some(x) if back && round > 0 && x != rng[p as usize] => R::full(ty),
                    Some(x) => x,
                    None => R::full(ty),
                };
                if m != rng[p as usize] {
                    rng[p as usize] = m;
                    moved = true;
                }
            }
            for inst in &f.blocks[b].insts {
                let d = match inst.dst() {
                    Some(d) => d,
                    None => continue,
                };
                let ty = f.values[d as usize].ty;
                let r = transfer(inst, &rng, ty);
                if r != rng[d as usize] {
                    rng[d as usize] = r;
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
    rng
}

/// The range an instruction's result is in, given its operands'.
///
/// Every arm is an interval identity that holds over ℤ, followed by `fit`, which
/// drops the fact if the true result could have left the type's width — a value
/// that wrapped is not the mathematical one, and no arm may claim it is.
fn transfer(inst: &Inst, rng: &[R], ty: Ty) -> R {
    if ty.is_float() {
        return R::full(ty);
    }
    let get = |o: Operand| range_of(rng, o, ty);
    match inst {
        Inst::Cmp { .. } => R { lo: 0, hi: 1 },
        Inst::Bin { op, ty: bt, a, b, .. } if !bt.is_float() => {
            let (x, y) = (get(*a), get(*b));
            match op {
                BinOp::Add => R { lo: x.lo + y.lo, hi: x.hi + y.hi }.fit(ty),
                BinOp::Sub => R { lo: x.lo - y.hi, hi: x.hi - y.lo }.fit(ty),
                BinOp::Mul => {
                    let p = [x.lo * y.lo, x.lo * y.hi, x.hi * y.lo, x.hi * y.hi];
                    R {
                        lo: *p.iter().min().unwrap(),
                        hi: *p.iter().max().unwrap(),
                    }
                    .fit(ty)
                }
                // AND with a non-negative mask is bounded BY that mask, whatever
                // the other side is: no bit above the mask's highest survives,
                // and the sign bit is one of them.
                BinOp::And if y.nonneg() => R { lo: 0, hi: y.hi }.fit(ty),
                BinOp::And if x.nonneg() => R { lo: 0, hi: x.hi }.fit(ty),
                // a logical shift right of a non-negative value divides it
                BinOp::LShr | BinOp::AShr if x.nonneg() && y.lo == y.hi && y.lo >= 0 => {
                    R { lo: 0, hi: x.hi >> y.lo.min(127) }.fit(ty)
                }
                BinOp::Shl if y.lo == y.hi && y.lo >= 0 && y.lo < 127 => R {
                    lo: x.lo << y.lo,
                    hi: x.hi << y.lo,
                }
                .fit(ty),
                // C99 6.5.5p6: the quotient of non-negative operands is bounded
                // by the dividend, and the remainder by the divisor
                BinOp::SDiv | BinOp::UDiv if x.nonneg() && y.lo > 0 => {
                    R { lo: 0, hi: x.hi }.fit(ty)
                }
                BinOp::SRem | BinOp::URem if x.nonneg() && y.lo > 0 => {
                    R { lo: 0, hi: y.hi - 1 }.fit(ty)
                }
                _ => R::full(ty),
            }
        }
        // A widening conversion carries the source's range; `ZExt` of a narrower
        // type is non-negative by construction, which is the fact the frontend
        // could not express in `Ty`.
        Inst::Cvt { op, from, a, .. } => match op {
            CvtOp::Sext => get(*a).fit(ty),
            CvtOp::Zext => R { lo: 0, hi: (1i128 << from.bits()) - 1 }.fit(ty),
            _ => R::full(ty),
        },
        _ => R::full(ty),
    }
}

/// `Some(0)` / `Some(1)` when every pair drawn from the two ranges compares the
/// same way; `None` when the ranges overlap enough to leave the answer open.
///
/// An UNSIGNED comparison is answered only when both ranges are non-negative,
/// where the unsigned and signed orders agree. Below zero they do not, and a
/// range that straddles it says nothing about the unsigned order at all.
fn decide(op: CmpOp, a: R, b: R) -> Option<i64> {
    let both = a.nonneg() && b.nonneg();
    let (lt, le, gt, ge) = (a.hi < b.lo, a.hi <= b.lo, a.lo > b.hi, a.lo >= b.hi);
    let yes = |c: bool| Some(c as i64);
    match op {
        CmpOp::Eq if a.lo == a.hi && a == b => yes(true),
        CmpOp::Eq if lt || gt => yes(false),
        CmpOp::Ne if a.lo == a.hi && a == b => yes(false),
        CmpOp::Ne if lt || gt => yes(true),
        CmpOp::Slt if lt => yes(true),
        CmpOp::Slt if ge => yes(false),
        CmpOp::Sle if le => yes(true),
        CmpOp::Sle if a.lo > b.hi => yes(false),
        CmpOp::Sgt if gt => yes(true),
        CmpOp::Sgt if le => yes(false),
        CmpOp::Sge if ge => yes(true),
        CmpOp::Sge if a.hi < b.lo => yes(false),
        CmpOp::Ult if both && lt => yes(true),
        CmpOp::Ult if both && ge => yes(false),
        CmpOp::Ule if both && le => yes(true),
        CmpOp::Ule if both && a.lo > b.hi => yes(false),
        CmpOp::Ugt if both && gt => yes(true),
        CmpOp::Ugt if both && le => yes(false),
        CmpOp::Uge if both && ge => yes(true),
        CmpOp::Uge if both && a.hi < b.lo => yes(false),
        _ => None,
    }
}

/// What each block's position in the CFG proves about a value, inherited down
/// the dominator tree.
///
/// A block whose ONLY predecessor ends in `br c, t, e` is entered exactly when
/// that branch went one way, so the comparison behind `c` holds for the whole
/// block and everything it dominates. The single-predecessor fence is what makes
/// it a proof rather than an analogy: with two predecessors the other path may
/// arrive with the guard false.
///
/// The list is small by construction — one entry per enclosing guard — so a
/// linear scan over it is a scan over a handful, not the defect class `678e700`
/// fixed.
fn guard_map(f: &Func, c: &dom::Cfg, dt: &dom::DomTree) -> Vec<Vec<(ValueId, R)>> {
    let mut out: Vec<Vec<(ValueId, R)>> = vec![Vec::new(); f.blocks.len()];
    // where each value is defined, so a guard on a comparison can be read back
    let mut def: Vec<Option<(usize, usize)>> = vec![None; f.values.len()];
    for b in 0..f.blocks.len() {
        for (i, inst) in f.blocks[b].insts.iter().enumerate() {
            if let Some(d) = inst.dst() {
                def[d as usize] = Some((b, i));
            }
        }
    }
    for &blk in &dt.preorder {
        let b = blk as usize;
        let idom = dt.idom[b];
        if idom != blk && (idom as usize) < out.len() {
            out[b] = out[idom as usize].clone();
        }
        let p = match c.preds[b].as_slice() {
            [p] => *p,
            _ => continue,
        };
        let (cond, taken) = match &f.blocks[p as usize].term {
            Term::Br(cv, t, e) if t.block == blk && e.block != blk => (*cv, true),
            Term::Br(cv, t, e) if e.block == blk && t.block != blk => (*cv, false),
            _ => continue,
        };
        let (op, ty, a, bb) = match cond {
            Operand::Val(v) => match def[v as usize].and_then(|(db, di)| {
                match &f.blocks[db].insts[di] {
                    Inst::Cmp { op, ty, a, b: bb, .. } => Some((*op, *ty, *a, *bb)),
                    _ => None,
                }
            }) {
                Some(x) => x,
                None => continue,
            },
            _ => continue,
        };
        if ty.is_float() {
            continue;
        }
        let op = if taken { op } else { invert(op) };
        if let (Operand::Val(v), Operand::Imm(k)) = (a, bb) {
            if let Some(r) = bound(op, k as i128, ty) {
                add(&mut out[b], v, r);
            }
        }
        if let (Operand::Imm(k), Operand::Val(v)) = (a, bb) {
            if let Some(r) = bound(swap(op), k as i128, ty) {
                add(&mut out[b], v, r);
            }
        }
    }
    out
}

fn add(list: &mut Vec<(ValueId, R)>, v: ValueId, r: R) {
    match list.iter_mut().find(|(x, _)| *x == v) {
        Some(e) => e.1 = e.1.meet(r),
        None => list.push((v, r)),
    }
}

/// What `v OP k` proves about `v`, when it is known to hold.
///
/// The unsigned forms bound only from BELOW at zero and above at `k`: an
/// unsigned comparison against a non-negative constant says the value, read as
/// unsigned, is under `k` — which for a sign-extended view means it is in
/// `[0, k]`, since anything negative is a huge unsigned number.
fn bound(op: CmpOp, k: i128, ty: Ty) -> Option<R> {
    let f = R::full(ty);
    Some(match op {
        CmpOp::Eq => R::point(k),
        CmpOp::Slt => R { lo: f.lo, hi: k - 1 },
        CmpOp::Sle => R { lo: f.lo, hi: k },
        CmpOp::Sgt => R { lo: k + 1, hi: f.hi },
        CmpOp::Sge => R { lo: k, hi: f.hi },
        CmpOp::Ult if k >= 0 => R { lo: 0, hi: k - 1 },
        CmpOp::Ule if k >= 0 => R { lo: 0, hi: k },
        _ => return None,
    }
    .fit(ty))
}

/// The comparison that holds when `op` does not.
fn invert(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Eq => CmpOp::Ne,
        CmpOp::Ne => CmpOp::Eq,
        CmpOp::Slt => CmpOp::Sge,
        CmpOp::Sle => CmpOp::Sgt,
        CmpOp::Sgt => CmpOp::Sle,
        CmpOp::Sge => CmpOp::Slt,
        CmpOp::Ult => CmpOp::Uge,
        CmpOp::Ule => CmpOp::Ugt,
        CmpOp::Ugt => CmpOp::Ule,
        CmpOp::Uge => CmpOp::Ult,
        other => other,
    }
}

/// The comparison with its operands exchanged — `k < v` is `v > k`.
fn swap(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Slt => CmpOp::Sgt,
        CmpOp::Sle => CmpOp::Sge,
        CmpOp::Sgt => CmpOp::Slt,
        CmpOp::Sge => CmpOp::Sle,
        CmpOp::Ult => CmpOp::Ugt,
        CmpOp::Ule => CmpOp::Uge,
        CmpOp::Ugt => CmpOp::Ult,
        CmpOp::Uge => CmpOp::Ule,
        other => other,
    }
}
