// if_convert (MECHANISM.md §G4 row 10) — a side-effect-free diamond becomes `select`.
// THEORY A7b — optimization: this pass ships its commuting square
//
// The shape is the one mem2reg produces from every `x = c ? a : b` and every
// small `if`: a block branching on `c` into two arms that join, where the join's
// PARAMETER is the only thing that differs between them. Replacing the branch by
// a `select` removes two edges, two blocks and — on A64 — one `cmp` plus two
// branches, in exchange for one `csel` (DDI 0487 C6.2.69). It also removes a
// MISPREDICTION, which is why gcc does it at -O1 despite the instruction count
// being roughly a wash.
//
// COMMUTING SQUARE. ⟦select c a b⟧ is `if c ≠ 0 then a else b` — literally the
// meaning of the branch it replaces — so the only obligation is that nothing
// ELSE the arms did becomes visible on the path that used to skip it. That is
// enforced by SPECULATION SAFETY: an arm may hold only `Effect::Pure`
// instructions, and only those that cannot fault. A division whose divisor is not
// a known non-zero literal is refused (C99 6.5.5p5), and so is a load: refining
// ⊥ is legal only when the ORIGINAL program was already ⊥, and on the path that
// skipped the arm it was not.
use super::*;

/// MEASURED M8 — the if-conversion arm bound
/// How many instructions an arm may hold and still be speculated. Both arms are
/// executed unconditionally afterwards, so the bound is the branch's own cost:
/// a compare, a taken branch and the pipeline bubble a misprediction costs. Two
/// is the conservative reading of that, and the shape this pass exists for —
/// a join parameter and nothing else — needs none at all.
const ARM_LIMIT: usize = 2;

/// THEORY A7b  SQUARE ifconv_turns_a_diamond_into_a_select — a side-effect-free diamond
pub fn run(f: &mut Func, a: &mut Analyses) -> bool {
    let mut changed = false;
    // WHAT IS PINNED DOES NOT CHANGE HERE, so it is asked once. A block is pinned
    // by being the entry, by having its address taken, or by being a computed
    // goto's target. `convert` MOVES instructions rather than deleting them, so
    // no `SymAddr(Label)` disappears; it only ever replaces an arm's terminator,
    // and `diamond` accepts an arm only when that terminator is a `Jmp`, so no
    // `GotoPtr` disappears either; and it adds no blocks. Rebuilding it per
    // converted diamond was a full walk of every instruction in the function.
    let pin = pinned(f);
    loop {
        // Each conversion rewrites terminators, so the handle is invalidated at
        // the top of every turn — the DECLARATION this pass owes the layer.
        a.invalidate();
        let c = a.cfg(f);
        let mut hit = None;
        for b in 0..f.blocks.len() {
            if !c.reachable(b as BlockId) {
                continue;
            }
            if let Some(d) = diamond(f, &c, &pin, b as BlockId) {
                hit = Some(d);
                break;
            }
        }
        match hit {
            Some(d) => {
                convert(f, d);
                changed = true;
            }
            None => return changed,
        }
    }
}

struct Diamond {
    head: BlockId,
    cond: Operand,
    /// the arm taken when the condition is true, if it is a block of its own
    t: Option<BlockId>,
    f: Option<BlockId>,
    join: BlockId,
    /// the arguments the join's parameters receive on each side
    targs: Vec<Operand>,
    fargs: Vec<Operand>,
}

fn diamond(f: &Func, c: &dom::Cfg, pin: &[bool], b: BlockId) -> Option<Diamond> {
    let (cond, tt, ft) = match &f.blocks[b as usize].term {
        Term::Br(cond, x, y) => (*cond, x.clone(), y.clone()),
        _ => return None,
    };
    if cond.val().is_none() {
        return None; // a literal condition is cfg_simplify's business
    }
    // One side may be the join itself (a triangle); the other must be an arm
    // with this block as its only predecessor.
    let arm = |t: &Target| -> Option<BlockId> {
        let x = t.block;
        if pin[x as usize]
            || !f.blocks[x as usize].labels.is_empty()
            || c.preds[x as usize].len() != 1
            || !t.args.is_empty()
        {
            return None;
        }
        match &f.blocks[x as usize].term {
            Term::Jmp(_) if speculatable(f, x) => Some(x),
            _ => None,
        }
    };
    let jmp_of = |x: BlockId| match &f.blocks[x as usize].term {
        Term::Jmp(t) => Some(t.clone()),
        _ => None,
    };
    let (ta, tj) = match arm(&tt) {
        Some(x) => (Some(x), jmp_of(x)?),
        None => (None, tt.clone()),
    };
    let (fa, fj) = match arm(&ft) {
        Some(x) => (Some(x), jmp_of(x)?),
        None => (None, ft.clone()),
    };
    if tj.block != fj.block || tj.block == b {
        return None;
    }
    let join = tj.block;
    // THE JOIN MAY HAVE OTHER PREDECESSORS, and requiring exactly two refused
    // the commonest shape there is. A small `if` inside a `switch` arm joins at
    // the arm's `break` — which is the LOOP LATCH, shared by every arm — so the
    // join has one predecessor per arm and this test rejected all of them.
    // `convert` never reads the count: it moves the arms into the head and
    // redirects ONE edge carrying the selects, leaving every other predecessor's
    // edge and arguments exactly as they were.
    //
    // Measured on `m1_resp_parse`, a redis RESP parser, where the refused shape
    // was `if (--want == 0) st = S_CR;` in the arm that runs for every payload
    // byte: hand-converting that one branch to a `csel` in the emitted `.s` took
    // the program from 91,328us to 76,465us — 16% of the whole program for one
    // branch, because it is data-dependent and mispredicts.
    if pin[join as usize] || c.preds[join as usize].len() < 2 {
        return None;
    }
    // Both sides must already have the PARAMETER's type. `build` sometimes hands
    // an edge a value wider than the parameter it feeds (a promoted `char`), and
    // that is tolerable only while the parameter still exists to narrow it — a
    // `select` would inherit the mismatch and hand it to whatever reads the
    // parameter after `cfg_simplify` merges the join away.
    let ps = &f.blocks[join as usize].params;
    for (k, a) in tj.args.iter().enumerate() {
        let b = fj.args.get(k)?;
        if a == b {
            continue;
        }
        let want = f.ty_of(*ps.get(k)?);
        for o in [a, b] {
            if operand_ty(f, *o).is_some_and(|t| t != want) {
                return None;
            }
        }
    }
    // A FLOATING-POINT select is `fcsel`, a different instruction on a different
    // register file, and MIR has no form for it yet. Refusing the diamond keeps
    // the branch rather than emitting an ill-typed `csel` — recorded as a
    // residual, not silently dropped.
    if f.blocks[join as usize]
        .params
        .iter()
        .enumerate()
        .any(|(k, p)| f.ty_of(*p).is_float() && tj.args.get(k) != fj.args.get(k))
    {
        return None;
    }
    // a triangle's non-arm side reaches the join directly, so the head is one of
    // the join's two predecessors — that is still exactly two edges
    Some(Diamond {
        head: b,
        cond,
        t: ta,
        f: fa,
        join,
        targs: tj.args,
        fargs: fj.args,
    })
}

/// The type an operand carries in its own right; a literal has none (it takes
/// the type of the instruction that reads it).
fn operand_ty(f: &Func, o: Operand) -> Option<Ty> {
    o.val().map(|v| f.ty_of(v))
}

/// Every instruction in the block may be executed unconditionally.
fn speculatable(f: &Func, b: BlockId) -> bool {
    let blk = &f.blocks[b as usize];
    if blk.insts.len() > ARM_LIMIT || !blk.params.is_empty() {
        return false;
    }
    blk.insts.iter().all(|i| {
        if i.effect() != Effect::Pure {
            return false;
        }
        match i {
            Inst::Bin { op, b, .. } => {
                !matches!(op, BinOp::SDiv | BinOp::UDiv | BinOp::SRem | BinOp::URem)
                    || matches!(b, Operand::Imm(k) if *k != 0)
            }
            _ => true,
        }
    })
}

fn convert(f: &mut Func, d: Diamond) {
    // (1) the arms' instructions move into the head, in their original order
    for arm in [d.t, d.f].into_iter().flatten() {
        let insts = std::mem::take(&mut f.blocks[arm as usize].insts);
        f.blocks[d.head as usize].insts.extend(insts);
        f.blocks[arm as usize].term = Term::Unreachable;
    }
    // (2) one `select` per join parameter whose two arguments differ
    let params = f.blocks[d.join as usize].params.clone();
    let mut args = Vec::with_capacity(params.len());
    for (k, p) in params.iter().enumerate() {
        let (a, b) = (d.targs[k], d.fargs[k]);
        if a == b {
            args.push(a);
            continue;
        }
        // `diamond` has already established that both sides carry the
        // parameter's own type.
        let ty = f.ty_of(*p);
        let at = f.blocks[d.head as usize].insts.len() as u32;
        let dst = f.new_value(ty, Def::Inst(d.head, at));
        f.blocks[d.head as usize].insts.push(Inst::Select {
            dst,
            ty,
            c: d.cond,
            a,
            b,
        });
        args.push(Operand::Val(dst));
    }
    f.blocks[d.head as usize].term = Term::Jmp(Target { block: d.join, args });
    // ONE BLOCK CHANGED. The arms' instructions are now the head's, at new
    // positions, and the selects were stamped as they were pushed; the arms are
    // empty and the join is untouched. The whole-function version costs O(values)
    // per converted diamond, inside a loop that converts one per pass — the same
    // defect licm already had, and fixed the same way.
    super::refresh_block_defs(f, d.head);
}
