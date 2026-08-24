// src/opt/loops.rs — loop & control-flow — cfg-simplify, LICM, strength-reduction, pointer-IV, if-convert, remat.
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

pub fn cfg_simplify(f: &mut IrFunc) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto: the CFG is incomplete → merge/reachability unsound
    }
    let mut changed = 0u32;
    // (1) straight-line merges to a fixpoint (recompute predecessors each step).
    loop {
        let preds = predecessors(f);
        let mut pair = None;
        for p in 0..f.blocks.len() as BlockId {
            if let Term::Jmp(s) = f.blocks[p as usize].term
                && s != 0 // never merge the entry away
                && s != p // not a self-loop
                && preds[s as usize].len() == 1
            {
                pair = Some((p, s)); // s's sole predecessor is p
                break;
            }
        }
        let (p, s) = match pair {
            Some(x) => x,
            None => break,
        };
        let mut succs = Vec::new();
        term_targets(&f.blocks[s as usize].term, &mut succs);
        for t in succs {
            if t != s {
                rename_phi_pred(&mut f.blocks[t as usize], s, p);
            }
        }
        let sb = std::mem::replace(
            &mut f.blocks[s as usize],
            Block { insts: Vec::new(), term: Term::Unreachable },
        );
        for inst in sb.insts {
            match inst {
                Inst::Phi(d, ty, arms) => {
                    let v = arms.iter().find(|(pp, _)| *pp == p).map(|(_, v)| *v).unwrap_or(Val::Imm(0));
                    f.blocks[p as usize].insts.push(Inst::Copy(d, ty, v));
                }
                other => f.blocks[p as usize].insts.push(other),
            }
        }
        f.blocks[p as usize].term = sb.term;
        changed += 1; // s is now an isolated Unreachable block; step (2) deletes it
    }
    // (2) unreachable-block elimination + renumber.
    let reach = reachable_blocks(f);
    if reach.iter().any(|&r| !r) {
        let mut map = vec![0u32; f.blocks.len()];
        let mut next = 0u32;
        for (b, &r) in reach.iter().enumerate() {
            if r {
                map[b] = next;
                next += 1;
            }
        }
        let old = std::mem::take(&mut f.blocks);
        for (b, mut blk) in old.into_iter().enumerate() {
            if !reach[b] {
                continue; // deleted (already counted as a merge, or genuinely dead)
            }
            remap_term(&mut blk.term, &map);
            for i in blk.insts.iter_mut() {
                if let Inst::Phi(_, _, arms) = i {
                    arms.retain(|(pp, _)| reach[*pp as usize]);
                    for (pp, _) in arms.iter_mut() {
                        *pp = map[*pp as usize];
                    }
                }
            }
            f.blocks.push(blk);
        }
        for (_, b) in f.labels.iter_mut() {
            *b = map[*b as usize];
        }
    }
    changed
}

// ─────────────────────────────────────────────────────────────────────────────
// LOOP INFRASTRUCTURE + LICM (Phase B) — LOOP-INVARIANT CODE MOTION.
//
// GOVERNING THEOREM (CbC): `⟦f⟧ = ⟦licm(f)⟧`, MEASURED by `equiv`. QBE deliberately
// skips loop passes; we admit ONE (LICM) because it has a clean commuting-square proof:
//
//   A PURE, TRAP-FREE instruction whose operands are all defined OUTSIDE the loop
//   computes the SAME value on every iteration. Moving its single (SSA) definition to
//   the loop's PREHEADER — a block that dominates the loop and lies on the only entry
//   edge — evaluates it ONCE instead of n times. Because it is pure and trap-free, the
//   preheader may compute it even when the loop body runs zero times (speculation is
//   safe): the result is simply unused, and no observable state changes. Every use of
//   the value is inside the loop, which the preheader dominates ⟹ def-before-use holds.
//   Hence ⟦·⟧ is preserved.
//
// SAFETY FENCES (why the "trap-free / pure" hypothesis actually holds):
//   • hoist only Bin(¬Div,¬Rem) / Un / Copy / Cast / Lea — pure and non-faulting
//     (integer +−×, bitwise, shifts, casts, frame-address computation). Div/Rem are the
//     only trapping arithmetic (÷0 is UB) and are NOT hoisted; Load is NOT hoisted (it may
//     fault or alias a store in the loop) — so speculation never introduces a fault.
//   • zcc's IR is only PARTIAL SSA — to_ssa promotes address-not-taken scalar locals, but
//     straight-lowering temps and non-promotable values stay MULTI-DEF (reassignable). So
//     single-assignment is NOT assumed; it is CHECKED: an instruction is hoisted only when
//     its DST is single-def and every operand is a constant or a single-def temp defined
//     outside the loop. Hoisting one def of a multi-def temp (e.g. a loop-condition temp
//     reassigned each iteration) would FREEZE the value and change control flow — the class
//     of bug the earlier version introduced (a terminating loop turned infinite). `equiv`
//     is BLIND to it (interp of an infinite loop → Err → skipped as UB), so the guard is a
//     construction-time obligation, re-checked by a direct-interp regression test.
//   • `cfg_complete`-guarded (computed goto ⟹ the CFG is incomplete → dominance unsound).
//   • a loop with no single out-of-loop entry (irreducible / entry-as-header) is skipped.
// This attacks matmul's invariant address arithmetic (`a + i*rowsize`, the base of
// `c[i][j]`), the biggest gap in the bench.
// ─────────────────────────────────────────────────────────────────────────────


/// Back-edges (tail → header): a CFG edge whose head DOMINATES its tail (Aho §9.6.2).
/// The head is a natural-loop header.
pub(crate) fn back_edges(f: &IrFunc, dom: &[HashSet<BlockId>]) -> Vec<(BlockId, BlockId)> {
    let succ = successors(f);
    let mut out = Vec::new();
    for u in 0..f.blocks.len() as BlockId {
        for &v in &succ[u as usize] {
            if dom[u as usize].contains(&v) {
                out.push((u, v));
            }
        }
    }
    out
}


/// The natural loop of back-edge (tail→header): {header} ∪ {nodes that reach `tail`
/// without passing through `header`} — backward reachability from tail (Aho Alg. 9.45).
// Returns a BTreeSet (sorted, DETERMINISTIC iteration) — LICM's hoist `'scan` picks the
// FIRST candidate in body-iteration order, so a hash-order body made the emitted coloring
// nondeterministic across runs (same input, different .s). Sorted-by-block-id iteration is
// a stable, reproducible order; membership tests (`.contains`) are order-agnostic already.
pub(crate) fn natural_loop(f: &IrFunc, tail: BlockId, header: BlockId) -> BTreeSet<BlockId> {
    let preds = predecessors(f);
    let mut body = BTreeSet::from([header]);
    let mut stack = Vec::new();
    if tail != header && body.insert(tail) {
        stack.push(tail);
    }
    while let Some(n) = stack.pop() {
        for &p in &preds[n as usize] {
            if body.insert(p) {
                stack.push(p);
            }
        }
    }
    body
}


/// Ensure a dedicated PREHEADER for `header`: a block OUTSIDE `body`, on the sole
/// out-of-loop entry edge, dominating the loop. Returns its BlockId, or None when the
/// loop has no single out-of-loop entry (0 = the function entry is itself the header;
/// >1 = multiple entries / irreducible → bail). Reuses an existing single-successor
/// `Jmp(header)` entry block; otherwise inserts a fresh empty block on the entry edge
/// (⟦·⟧-preserving, exactly like a critical-edge split) and moves the header's φ-arm
/// for the entry predecessor onto the preheader.
pub(crate) fn ensure_preheader(f: &mut IrFunc, header: BlockId, body: &BTreeSet<BlockId>) -> Option<BlockId> {
    let preds = predecessors(f);
    let entries: Vec<BlockId> =
        preds[header as usize].iter().copied().filter(|p| !body.contains(p)).collect();
    if entries.len() != 1 {
        return None;
    }
    let p = entries[0];
    let mut succ = Vec::new();
    term_targets(&f.blocks[p as usize].term, &mut succ);
    if succ.len() == 1 && matches!(f.blocks[p as usize].term, Term::Jmp(h) if h == header) {
        return Some(p); // already a dedicated preheader (also gives idempotency across runs)
    }
    let ph = f.blocks.len() as BlockId;
    f.blocks.push(Block { insts: Vec::new(), term: Term::Jmp(header) });
    retarget(&mut f.blocks[p as usize].term, header, ph);
    rename_phi_pred(&mut f.blocks[header as usize], p, ph);
    Some(ph)
}


/// LOOP-INVARIANT IMMEDIATE HOISTING (the const-bound lever). A large integer constant used
/// inside a loop — the archetype is a loop bound `j <= 20000000` — is materialized by codegen
/// with a multi-instruction `mov;movk` sequence on EVERY iteration, because it lives as a
/// `Val::Imm` operand, not as an instruction LICM could lift (the comparison reads the changing
/// induction variable, so the whole `Bin(Le, j, Imm(N))` is variant). This pass lifts the
/// invariant immediate itself into ONE preheader temp `Copy(t, Imm(N))` and rewrites the in-loop
/// comparison operands to read `t` — turning per-iteration `mov;movk;cmp` into a preheader
/// `mov;movk` plus a per-iteration register `cmp`. On the sieve's two O(n) linear scans (the
/// measured hot spots) this is the dominant win. ⟦f⟧ is preserved: reading a temp defined once as
/// `Copy(t, Imm(N))` in a dominating preheader is value-identical to reading `Imm(N)` in place (a
/// pure value-numbering identity). PRESSURE-SAFE without a guard — the dual of remat's argument:
/// if `t` spills, its reload is ONE `ldr` ≤ the two-instruction `mov;movk` rematerialization it
/// replaces, so C_M never increases even under register pressure. SCOPE: operands of COMPARISON
/// Bins only, and only "expensive" immediates (|N| beyond the cmp imm12 field). Restricting to
/// comparisons keeps the pass away from the address/arith immediate FOLDS that later codegen
/// passes (`try_fuse_*`, ext-fold) depend on seeing as literal `Imm`s. Runs LAST (after the SSA
/// fixpoint, out_of_ssa, optimize, remat) so no copy-prop/const-fold/remat afterward folds the
/// hoisted constant back into the loop.
pub fn hoist_loop_consts(f: &mut IrFunc) -> u32 {
    use std::collections::BTreeMap;
    if !cfg_complete(f) {
        return 0; // computed goto ⟹ incomplete CFG ⟹ back-edges/dominance unsound
    }
    let is_cmp = |op: Op| matches!(op, Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge);
    // "Expensive" ⟺ outside the ±imm12 window a cmp/cmn can encode inline (0..=4095, and its
    // <<12 form). A value codegen would emit in a single `cmp #imm` is left alone (hoisting it
    // would trade a free inline immediate for a preheader mov + a register — a size loss).
    let expensive = |n: i64| {
        let a = n.unsigned_abs();
        !(a <= 4095 || (a & 0xFFF == 0 && (a >> 12) <= 4095))
    };
    let dom = dominators(f);
    let backs = back_edges(f, &dom);
    let mut hoisted = 0u32;
    for (tail, header) in backs {
        let body = natural_loop(f, tail, header);
        // Distinct (value, type) of expensive imms appearing as a comparison operand in the body.
        let mut wanted: BTreeMap<(i64, TypeId), ()> = BTreeMap::new();
        for &bid in &body {
            for inst in &f.blocks[bid as usize].insts {
                if let Inst::Bin(_, op, ty, a, b) = inst
                    && is_cmp(*op)
                {
                    for v in [a, b] {
                        if let Val::Imm(n) = v
                            && expensive(*n)
                        {
                            wanted.insert((*n, *ty), ());
                        }
                    }
                }
            }
        }
        if wanted.is_empty() {
            continue;
        }
        let ph = match ensure_preheader(f, header, &body) {
            Some(p) => p,
            None => continue,
        };
        for (n, ty) in wanted.keys().copied().collect::<Vec<_>>() {
            let t = f.temps.len() as Tmp;
            f.temps.push(ty);
            f.blocks[ph as usize].insts.push(Inst::Copy(t, ty, Val::Imm(n)));
            for &bid in &body {
                for inst in &mut f.blocks[bid as usize].insts {
                    if let Inst::Bin(_, op, bty, a, b) = inst
                        && is_cmp(*op)
                        && *bty == ty
                    {
                        if matches!(a, Val::Imm(m) if *m == n) {
                            *a = Val::Tmp(t);
                        }
                        if matches!(b, Val::Imm(m) if *m == n) {
                            *b = Val::Tmp(t);
                        }
                    }
                }
            }
            hoisted += 1;
        }
    }
    hoisted
}


/// Hoistable ⟺ pure AND trap-free AND fault-free (see SAFETY FENCES). φ, Load, Store,
/// Div/Rem, and every impure exotic are excluded.
pub(crate) fn is_hoistable(i: &Inst) -> bool {
    match i {
        Inst::Bin(_, op, ..) => !matches!(op, Op::Div | Op::Rem),
        Inst::Un(..) | Inst::Copy(..) | Inst::Cast(..) | Inst::Lea(..) => true,
        _ => false,
    }
}


/// Every operand is either a compile-time constant or a temp AVAILABLE in the preheader
/// (a single-def source whose one definition lies outside the loop, incl. an already-hoisted one).
pub(crate) fn operands_avail(i: &Inst, avail: &[bool]) -> bool {
    let mut u = Vec::new();
    inst_uses(i, &mut u);
    u.iter().all(|&t| avail[t as usize])
}


/// Register-PRESSURE of a loop body = the max, over every program point in `body`, of the
/// number of simultaneously-live GP (non-float) temps. This is the SCARCE resource the
/// speed-positivity guard protects: the GP class has only `k` colours (AAPCS64 allocatable
/// set, a Side-II constant threaded from the backend budget), so a point with pressure > k
/// forces a spill. Computed by the standard backward live-set walk (the transfer function
/// abi_alloc's crossing scan uses), each block seeded with live-out ∪ terminator-uses.
pub(crate) fn loop_gp_pressure(tt: &TyTab, f: &IrFunc, lv: &Liveness, body: &BTreeSet<BlockId>) -> u32 {
    let nt = f.temps.len();
    let is_fp: Vec<bool> = f.temps.iter().map(|&ty| tt.is_float(ty)).collect();
    let gp_count = |live: &[bool]| (0..nt).filter(|&t| live[t] && !is_fp[t]).count() as u32;
    let mut maxp = 0u32;
    let mut buf = Vec::new();
    for &bid in body {
        let b = &f.blocks[bid as usize];
        let mut live = lv.live_out[bid as usize].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true; // at block exit the terminator operands are live
        }
        maxp = maxp.max(gp_count(&live));
        for i in b.insts.iter().rev() {
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
            maxp = maxp.max(gp_count(&live));
        }
    }
    maxp
}


/// `gp_k` = the GP colour budget (AAPCS64 allocatable count, the backend's `GP_BUDGET.k`),
/// threaded in so the SPEED-POSITIVITY guard shares the ONE Side-II constant rather than
/// duplicating it. Correctness (`⟦f⟧=⟦licm f⟧`) is independent of `gp_k`; it only caps how
/// many invariants may be hoisted per loop (see the guard note inline).
pub fn licm(tt: &TyTab, f: &mut IrFunc, gp_k: u32) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto ⟹ incomplete CFG ⟹ dominance/back-edges unsound
    }
    // zcc's IR is only PARTIAL SSA: to_ssa promotes address-not-taken scalar locals into
    // single-assignment temps, but straight-lowering temps and non-promotable values stay
    // MULTI-DEF (reassignable 3-address temps). LICM's theorem needs single-assignment, so
    // it is gated on it EXPLICITLY: def counts per temp + the (unique) defining block.
    let nt = f.temps.len();
    let mut defcnt = vec![0u32; nt];
    let mut def_block = vec![u32::MAX; nt];
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defcnt[d as usize] += 1;
                def_block[d as usize] = bi as BlockId;
            }
        }
    }
    let dom = dominators(f);
    let backs = back_edges(f, &dom);
    let mut hoisted = 0u32;
    for (tail, header) in backs {
        let body = natural_loop(f, tail, header); // against the CURRENT CFG (fresh preds)
        // A temp is an AVAILABLE invariant source ⟺ it is SINGLE-DEF and its one definition
        // lies OUTSIDE the loop body. A multi-def temp is never invariant (it may be
        // reassigned — the loop-condition bug), and a temp defined inside the body is not
        // yet available (until it is itself hoisted).
        let mut avail = vec![false; nt];
        for t in 0..nt {
            if defcnt[t] == 1 && def_block[t] != u32::MAX && !body.contains(&def_block[t]) {
                avail[t] = true;
            }
        }
        // Candidate ⟺ hoistable shape ∧ its DST is SINGLE-DEF (moving a multi-def
        // instruction would break the value that flows on another path) ∧ every operand
        // is available. Only disturb the CFG if such a candidate exists (no empty preheaders).
        let candidate = |inst: &Inst, avail: &[bool]| {
            is_hoistable(inst)
                && matches!(inst_def(inst), Some(d) if defcnt[d as usize] == 1)
                && operands_avail(inst, avail)
        };
        let has_candidate =
            body.iter().any(|&bid| f.blocks[bid as usize].insts.iter().any(|i| candidate(i, &avail)));
        if !has_candidate {
            continue;
        }
        // SPEED-POSITIVITY GUARD — the mathematical fence that makes LICM ship-safe on a
        // k-register machine. Correctness lives on one axis (the commuting square ⟦f⟧=⟦licm f⟧,
        // still an identity because a partial hoist is a SUBSET of the proven hoist set); COST
        // lives on the orthogonal axis this guard governs. Each hoist makes one value live
        // ACROSS the whole body, so it can raise the live-count at any point by at most 1 ⟹
        // post-hoist max GP pressure ≤ P + (#hoists). Capping #hoists at (k − P) keeps pressure
        // ≤ k = the GP colour budget ⟹ the k-colouring survives ⟹ the allocator introduces NO
        // new spill ⟹ each hoist strictly deletes (trip−1) dynamic ALU ops with ZERO added
        // memory traffic ⟹ C_M strictly decreases. P is MEASURED (liveness); k is a Side-II ABI
        // constant — no tuned weight anywhere. (Residual: P is SSA-pressure, a proxy for the
        // post-φ-destruction allocator's pressure; the box A/B closes that gap before the
        // default-ON flip — OPT.md §2 (why proof-faster meets reality-slower).)
        let lv = liveness(f);
        let headroom = gp_k.saturating_sub(loop_gp_pressure(tt, f, &lv, &body));
        if headroom == 0 {
            continue; // loop already at/over budget — ANY hoist would spill; refuse (no regress)
        }
        let ph = match ensure_preheader(f, header, &body) {
            Some(p) => p,
            None => continue,
        };
        // Fixpoint hoist, CAPPED at `headroom`: each round moves the first newly-available
        // invariant instruction and marks its (single) def available, so a dependent invariant
        // becomes hoistable next round — landing AFTER its producer (dependency order kept).
        let mut here = 0u32;
        loop {
            if here >= headroom {
                break; // GP-pressure budget for this loop exhausted
            }
            let mut found = None;
            'scan: for &bid in body.iter() {
                for (ix, inst) in f.blocks[bid as usize].insts.iter().enumerate() {
                    if candidate(inst, &avail) {
                        found = Some((bid, ix));
                        break 'scan;
                    }
                }
            }
            let (bid, ix) = match found {
                Some(x) => x,
                None => break,
            };
            let inst = f.blocks[bid as usize].insts.remove(ix);
            if let Some(d) = inst_def(&inst) {
                avail[d as usize] = true; // now defined in the preheader (outside the body)
            }
            f.blocks[ph as usize].insts.push(inst);
            hoisted += 1;
            here += 1;
        }
    }
    hoisted
}

// ─────────────────────────────────────────────────────────────────────────────
// STRENGTH REDUCTION (Phase B.5) — induction-variable based, default-OFF.
//
// THE CLASSIC OPTIMIZATION (Cocke–Kennedy; Aho §9.6): inside a loop, a MULTIPLY by a
// constant that rides an induction variable is replaced by an ADD accumulator. The
// canonical target is address arithmetic: `a + i*elemsize` in a matmul inner loop
// recomputes a multiply every iteration; strength reduction turns it into one add per
// step (the accumulator marches `elemsize` per iteration). This is *the* textbook loop
// optimization — the pedagogical heart of Phase B.
//
// GOVERNING THEOREM (CbC): `⟦f⟧ = ⟦strength_reduce(f)⟧`, MEASURED by `equiv`. The proof
// is an INDUCTION on the loop trip count. Let the loop carry a BASIC INDUCTION VARIABLE
// (BIV) as an SSA header φ:
//
//        i₁ = φ(preheader: i₀, latch: i₂)        i₂ = i₁ + c        (c constant)
//
// so at the head of iteration k, i₁ = i₀ + k·c. A DERIVED induction variable is a body
// expression j = i₁ · d (d constant). We introduce a parallel accumulator φ:
//
//        j₁ = φ(preheader: i₀·d, latch: j₂)      j₂ = j₁ + c·d
//
// and replace `j = i₁·d` with `j = j₁`. CLAIM: j₁ = i₁·d at the head of every iteration.
//   • BASE (k=0): j₁ = i₀·d = i₁·d, since i₁ = i₀ on entry.  ✓
//   • STEP: assume j₁ = i₁·d. After the latch, j₂ = j₁ + c·d = i₁·d + c·d = (i₁+c)·d =
//     i₂·d, which becomes the next head value of both φ's.  ✓
// Hence every observation of j is unchanged ⟹ ⟦·⟧ preserved. The constant c·d is folded
// at build time. Distribution (i₁+c)·d = i₁·d + c·d holds EXACTLY in ℤ/2ⁿ (two's-complement
// wrapping), so no overflow/UB gap opens at the IR level — the reduction is faithful to
// the fixed-width integer semantics interp implements.
//
// SAFETY FENCES (why the hypotheses actually hold on this partial-SSA IR):
//   • INTEGER only — float × does not distribute exactly (non-associative), so `tt.is_float`
//     types are skipped for both the BIV step and the derived multiply.
//   • CONSTANT step c and CONSTANT multiplier d ⟹ c·d is a compile-time constant; no
//     invariant-multiply needs inserting, keeping the inserted latch op a pure ADD.
//   • SINGLE-DEF everywhere (i₁, i₂, j) — SSA guarantees it for promoted temps, but this IR
//     is only PARTIAL SSA, so it is CHECKED, never assumed (same discipline as LICM).
//   • REDUCIBLE single-latch loop — the header φ must have exactly two arms (one external,
//     one from the unique back-edge tail); multi-latch / irreducible loops are skipped.
//   • `cfg_complete`-guarded (computed goto ⟹ unsound dominance).
//
// MEASURED, not asserted: like LICM, on the memory-bound naive-slot backend the new φ adds
// spill slots and edge copies that can outweigh the mul→add saving — so this ships behind
// the `Passes` toggle, default-OFF, and its value here is the PROOF and the teaching, with
// the perf win latent for a register-resident backend.
// ─────────────────────────────────────────────────────────────────────────────


/// Match `Bin(_, _, _, a, b)` operands as (Tmp(want), Imm k) in either order → k.
pub(crate) fn tmp_times_imm(a: &Val, b: &Val, want: Tmp) -> Option<i64> {
    match (a, b) {
        (Val::Tmp(t), Val::Imm(k)) | (Val::Imm(k), Val::Tmp(t)) if *t == want => Some(*k),
        _ => None,
    }
}


pub fn strength_reduce(tt: &TyTab, f: &mut IrFunc, gp_k: u32) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto ⟹ dominance/back-edges unsound (as LICM/GVN/SCCP)
    }
    let dom = dominators(f);
    let backs = back_edges(f, &dom);
    let mut changed = 0u32;
    for &(tail, header) in &backs {
        // REDUCIBLE single-latch only: a unique back-edge into `header` ⟹ the header φ has
        // exactly one latch arm, so the accumulator φ we build is well-formed.
        if backs.iter().filter(|(_, h)| *h == header).count() != 1 {
            continue;
        }
        // Def-counts are recomputed PER back-edge, against the CURRENT function: a previous
        // loop's rewrite may have appended fresh temps (jbase/jnext/jphi), so a stale snapshot
        // would both under-size the array (a nested-loop panic) and miss the new single-defs.
        let mut defcnt = vec![0u32; f.temps.len()];
        for b in &f.blocks {
            for i in &b.insts {
                if let Some(d) = inst_def(i) {
                    defcnt[d as usize] += 1;
                }
            }
        }
        let body = natural_loop(f, tail, header);
        // SPEED-POSITIVITY GUARD (same axis-split as LICM). The reduction trades a per-iter
        // `mul` for a per-iter `add` (a win in the count model) but INSERTS an accumulator that
        // is live across the loop: an added φ (`j₁`, live header→latch) + its latch step (`j₂`)
        // ⟹ up to +2 GP values live across the body. If the body is already within 2 colours of
        // the budget, that accumulator spills and the reload-per-iteration outweighs the
        // mul→add saving (the exact 2026 regression). Refuse unless P + 2 ≤ k. Correctness is
        // unaffected (⟦f⟧=⟦sr f⟧ holds whether or not a given loop is transformed).
        {
            let lv = liveness(f);
            if loop_gp_pressure(tt, f, &lv, &body) + 2 > gp_k {
                continue; // no register headroom for the accumulator φ ⟹ would spill
            }
        }
        // 1. BASIC INDUCTION VARIABLES from the header φ region: i₁ = φ(ext: i₀, tail: i₂)
        //    with i₂ = i₁ + c (constant c), integer, all single-def.
        let mut bivs: Vec<(Tmp, TypeId, Val, i64)> = Vec::new(); // (i₁, ty, i₀, step c)
        for inst in &f.blocks[header as usize].insts {
            let Inst::Phi(i1, ty, arms) = inst else { continue };
            if tt.is_float(*ty) || arms.len() != 2 || defcnt[*i1 as usize] != 1 {
                continue;
            }
            let latch = arms.iter().find(|(p, _)| *p == tail);
            let ext = arms.iter().find(|(p, _)| *p != tail);
            let (Some((_, Val::Tmp(i2))), Some((_, i0))) = (latch, ext) else { continue };
            if defcnt[*i2 as usize] != 1 {
                continue;
            }
            // find i₂'s definition inside the body: Bin(i₂, Add, ty, i₁, Imm c) (either order).
            let mut step = None;
            for &bid in &body {
                for di in &f.blocks[bid as usize].insts {
                    if let Inst::Bin(d, Op::Add, dty, a, b) = di {
                        if *d == *i2 && *dty == *ty {
                            step = tmp_times_imm(a, b, *i1);
                        }
                    }
                }
            }
            if let Some(c) = step {
                bivs.push((*i1, *ty, *i0, c));
            }
        }
        if bivs.is_empty() {
            continue;
        }
        // 2. DERIVED IVs: Bin(j, Mul, ty, i₁, Imm d) in the body, j single-def, integer.
        let mut derived: Vec<(BlockId, Tmp, TypeId, Val, i64, i64)> = Vec::new(); // (blk, j, ty, i₀, c, d)
        for &bid in &body {
            for inst in &f.blocks[bid as usize].insts {
                let Inst::Bin(j, Op::Mul, ty, a, b) = inst else { continue };
                if tt.is_float(*ty) || defcnt[*j as usize] != 1 {
                    continue;
                }
                for &(i1, bty, i0, c) in &bivs {
                    if bty == *ty {
                        if let Some(d) = tmp_times_imm(a, b, i1) {
                            derived.push((bid, *j, *ty, i0, c, d));
                            break;
                        }
                    }
                }
            }
        }
        if derived.is_empty() {
            continue;
        }
        // 3. Materialize the preheader ONLY now (no empty preheaders). It may rename the
        //    header φ's external arm predecessor to `ph` (the value i₀ is unchanged).
        let ph = match ensure_preheader(f, header, &body) {
            Some(p) => p,
            None => continue,
        };
        // 4. Rewrite each derived IV into an accumulator φ.
        for (bid, j, ty, i0, c, d) in derived {
            let jbase = f.temps.len() as Tmp; // j₀ = i₀·d, computed ONCE in the preheader
            f.temps.push(ty);
            f.blocks[ph as usize].insts.push(Inst::Bin(jbase, Op::Mul, ty, i0, Val::Imm(d)));
            let jnext = f.temps.len() as Tmp; // j₂ = j₁ + c·d, at the latch
            f.temps.push(ty);
            let jphi = f.temps.len() as Tmp; // j₁ = φ(ph: j₀, tail: j₂), the accumulator
            f.temps.push(ty);
            let pos = f.blocks[header as usize]
                .insts
                .iter()
                .position(|i| !matches!(i, Inst::Phi(..)))
                .unwrap_or(0);
            f.blocks[header as usize].insts.insert(
                pos,
                Inst::Phi(jphi, ty, vec![(ph, Val::Tmp(jbase)), (tail, Val::Tmp(jnext))]),
            );
            let step = c.wrapping_mul(d); // c·d folded now (interp canon's to `ty` width)
            f.blocks[tail as usize].insts.push(Inst::Bin(jnext, Op::Add, ty, Val::Tmp(jphi), Val::Imm(step)));
            // Replace the derived multiply `j = i₁·d` with `j = j₁`. Indices shifted (φ
            // insert / latch append), so RE-LOCATE by j's unique (single-def) defining site.
            if let Some(cur) = f.blocks[bid as usize].insts.iter().position(|i| inst_def(i) == Some(j)) {
                f.blocks[bid as usize].insts[cur] = Inst::Copy(j, ty, Val::Tmp(jphi));
            }
            changed += 1;
        }
    }
    changed
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass — POINTER-IV STRENGTH REDUCTION + LFTR (the gcc-O1 loop-nest recipe).
//
// THE MEASURED LEVER (OPT.md §4): the accumulator-φ `strength_reduce` above turns a
// derived `i·d` into a marching add but LEAVES the invariant base add and the loop
// counter — matmul's inner loop keeps a per-iter `mul`, a `base+index` add and a
// counter compare (2.92× even fully un-starved). gcc-O1 collapses the same loop to
// SIX instructions by (1) folding the invariant base INTO the induction, producing a
// MARCHING POINTER, and (2) LFTR — replacing the counter test with a pointer/limit
// compare so the counter dies. This pass does both. Unlike the accumulator SR it is
// PRESSURE-REDUCING (a pointer replaces {counter, base, index-mul, base-add}).
//
// GOVERNING THEOREM (CbC): `⟦f⟧ = ⟦pointer_iv(f)⟧`, MEASURED by `equiv`. Two legs:
//   • BASE-FOLD leg. For an address `a = base + iv·s` (base loop-invariant, iv a basic
//     induction φ `i₁=φ(i₀,i₂)`, `i₂=i₁+c`), introduce `p₁=φ(ph: base+i₀·s, tail: p₂)`,
//     `p₂=p₁+c·s`, and replace `a` with `p₁`. CLAIM p₁ = base+i₁·s at every head:
//       BASE k=0: p₁ = base+i₀·s = base+i₁·s (i₁=i₀ on entry). STEP: p₂ = p₁+c·s =
//       base+i₁·s+c·s = base+(i₁+c)·s = base+i₂·s. ✓ (distribution exact in ℤ/2ⁿ.)
//     Value-preserving casts between iv and the multiply are transparent (a widening in
//     the same signedness class keeps the integer value), so `Cast(i₁)·d` reduces too —
//     the exact form the accumulator SR missed on matmul.
//   • LFTR leg. When after base-fold the counter i₁ feeds ONLY its increment and a header
//     test `i₁ < N` (N invariant, step c>0, stride s>0), replace the test with `p₁ < L`,
//     L = base+N·s (preheader). The map i₁↦p₁=base+i₁·s is monotone over the values i₁ takes
//     here NOT by abstract modular arithmetic (which would wrap) but because these pₖ are
//     addresses the source already forms: C99 6.5.6 bounds a valid IV-addressed access to
//     within the object (≤ one-past-end), so no pₖ wraps the address space and order is
//     preserved ⟹ `i₁<N` ⇔ `p₁<L` on every iteration ⟹ the branch outcome is identical.
//     (This is exactly why the pass fires only on address IVs, not arbitrary integer IVs.)
//     The now-
//     dead counter φ/increment are reclaimed by DCE (next fixpoint round).
//
// SAFETY FENCES: integer IV only (float × non-associative); constant step c and stride s
// (c·s folds to a constant ⟹ the latch op is a pure add); EXACTLY ONE iv-linear term in
// the address (all other Add leaves loop-invariant); single-def i₁,i₂,a; reducible
// single-latch loop; `cfg_complete`-guarded. The invariant base is RECOMPUTED into the
// preheader (a self-contained mini-LICM, so the pass does not depend on LICM running or
// being pressure-unblocked first). Ships behind the `Passes` toggle; measured on the box
// before any default-ON flip (OPT.md §4).
// ─────────────────────────────────────────────────────────────────────────────


/// A cast that PRESERVES the integer value of an induction variable: an integer widening
/// in the same signedness class (sign/zero-extension keeps the value). Narrowing may wrap
/// ⟹ rejected; float casts never qualify.
pub(crate) fn iv_preserving_cast(tt: &TyTab, from: TypeId, to: TypeId) -> bool {
    !tt.is_float(from)
        && !tt.is_float(to)
        && tt.size(to) >= tt.size(from)
        && tt.is_unsigned(from) == tt.is_unsigned(to)
}


/// Recognize a temp whose loop value is `biv·stride`, through value-preserving casts and
/// constant multiplies/shifts. Returns (biv i₁, stride). Read-only (defs via `def_of`).
pub(crate) fn iv_linear(
    f: &IrFunc,
    tt: &TyTab,
    def_of: &[Option<(BlockId, usize)>],
    t: Tmp,
    biv_of: &HashMap<Tmp, (Val, i64)>,
) -> Option<(Tmp, i64)> {
    if biv_of.contains_key(&t) {
        return Some((t, 1));
    }
    let (b, i) = def_of[t as usize]?;
    match &f.blocks[b as usize].insts[i] {
        Inst::Cast(_, from, to, Val::Tmp(s)) if iv_preserving_cast(tt, *from, *to) => {
            iv_linear(f, tt, def_of, *s, biv_of)
        }
        // A Copy preserves the value exactly ⟹ same (biv, stride). Copy-prop residue (a
        // widened index copied for a second address) must not block that address's reduction.
        Inst::Copy(_, _, Val::Tmp(s)) => iv_linear(f, tt, def_of, *s, biv_of),
        Inst::Bin(_, Op::Mul, _, a, c) => match (a, c) {
            (Val::Tmp(s), Val::Imm(m)) | (Val::Imm(m), Val::Tmp(s)) => {
                iv_linear(f, tt, def_of, *s, biv_of).map(|(bi, st)| (bi, st.wrapping_mul(*m)))
            }
            _ => None,
        },
        Inst::Bin(_, Op::Shl, _, Val::Tmp(s), Val::Imm(sh)) if *sh >= 0 && *sh < 63 => {
            iv_linear(f, tt, def_of, *s, biv_of).map(|(bi, st)| (bi, st.wrapping_shl(*sh as u32)))
        }
        _ => None,
    }
}


/// Walk the Add-tree of address value `v`, sorting each leaf into an invariant `base`
/// term or THE (single) iv-linear term. Returns false if any leaf is neither.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decompose_addr(
    f: &IrFunc,
    tt: &TyTab,
    def_of: &[Option<(BlockId, usize)>],
    v: &Val,
    body: &BTreeSet<BlockId>,
    biv_of: &HashMap<Tmp, (Val, i64)>,
    inv: &[bool],
    base: &mut Vec<Val>,
    lin: &mut Vec<(Tmp, i64)>,
) -> bool {
    let t = match v {
        Val::Tmp(t) => *t,
        _ => {
            base.push(v.clone()); // a constant addend is loop-invariant
            return true;
        }
    };
    if inv[t as usize] {
        base.push(Val::Tmp(t));
        return true;
    }
    if let Some(ls) = iv_linear(f, tt, def_of, t, biv_of) {
        lin.push(ls);
        return true;
    }
    // Not invariant, not a bare linear term ⟹ must be an in-body Add to recurse through.
    if let Some((b, i)) = def_of[t as usize] {
        if body.contains(&b) {
            if let Inst::Bin(_, Op::Add, _, a, c) = &f.blocks[b as usize].insts[i] {
                return decompose_addr(f, tt, def_of, a, body, biv_of, inv, base, lin)
                    && decompose_addr(f, tt, def_of, c, body, biv_of, inv, base, lin);
            }
        }
    }
    false
}


/// Clone the invariant computation of temp `t` (defined inside `body`) into preheader
/// `ph`, with fresh temps, operands first (a targeted hoist). A temp defined OUTSIDE the
/// body is already available ⟹ returned unchanged. Memoized.
pub(crate) fn clone_inv_to_ph(
    f: &mut IrFunc,
    ph: BlockId,
    body: &BTreeSet<BlockId>,
    t: Tmp,
    def_of: &[Option<(BlockId, usize)>],
    memo: &mut HashMap<Tmp, Tmp>,
) -> Tmp {
    if let Some(&nt) = memo.get(&t) {
        return nt;
    }
    let (b, i) = match def_of[t as usize] {
        Some(loc) if body.contains(&loc.0) => loc,
        _ => return t, // available in the preheader already
    };
    // Operands first (so the memo is populated before we remap this inst's uses).
    let mut us = Vec::new();
    inst_uses(&f.blocks[b as usize].insts[i], &mut us);
    for u in us {
        clone_inv_to_ph(f, ph, body, u, def_of, memo);
    }
    let mut inst = f.blocks[b as usize].insts[i].clone();
    each_use_mut(&mut inst, |val| {
        if let Val::Tmp(x) = val {
            if let Some(&nu) = memo.get(x) {
                *x = nu;
            }
        }
    });
    let ty = f.temps[t as usize];
    let nt = f.temps.len() as Tmp;
    f.temps.push(ty);
    set_dst(&mut inst, nt);
    f.blocks[ph as usize].insts.push(inst);
    memo.insert(t, nt);
    nt
}


// `_gp_k` is intentionally unused (kept only for call-site signature-uniformity with
// licm/strength_reduce): pointer_iv is *pressure-REDUCING*, not pressure-increasing. It
// replaces `base + i·stride` (a live index + a multiply per use) with a single marching
// pointer advanced by an add, and LFTR drops the original index φ entirely — so it can only
// LOWER the loop's GP live-count. licm/SR hoist values INTO the loop and thus need the `gp_k`
// cap to bite; this transform has no such cap because there is no pressure to cap.
pub fn pointer_iv(tt: &TyTab, f: &mut IrFunc, _gp_k: u32) -> u32 {
    if !cfg_complete(f) {
        return 0; // computed goto ⟹ dominance/back-edges unsound (as LICM/SR)
    }
    let dom = dominators(f);
    let backs = back_edges(f, &dom);
    let mut changed = 0u32;
    for &(tail, header) in &backs {
        if backs.iter().filter(|(_, h)| *h == header).count() != 1 {
            continue; // reducible single-latch only
        }
        let nt = f.temps.len();
        // def location of every temp, against the CURRENT function (prior loops appended temps).
        let mut def_of: Vec<Option<(BlockId, usize)>> = vec![None; nt];
        let mut defcnt = vec![0u32; nt];
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, inst) in b.insts.iter().enumerate() {
                if let Some(d) = inst_def(inst) {
                    def_of[d as usize] = Some((bi as BlockId, ii));
                    defcnt[d as usize] += 1;
                }
            }
        }
        let body = natural_loop(f, tail, header);
        // BASIC INDUCTION VARIABLES: header φ i₁=φ(ext: i₀, tail: i₂) with i₂=i₁+c (const c),
        // integer, single-def — the same recognition as the accumulator SR.
        let mut biv_of: HashMap<Tmp, (Val, i64)> = HashMap::new();
        for inst in &f.blocks[header as usize].insts {
            let Inst::Phi(i1, ty, arms) = inst else { continue };
            if tt.is_float(*ty) || arms.len() != 2 || defcnt[*i1 as usize] != 1 {
                continue;
            }
            let (Some((_, Val::Tmp(i2))), Some((_, i0))) =
                (arms.iter().find(|(p, _)| *p == tail), arms.iter().find(|(p, _)| *p != tail))
            else {
                continue;
            };
            if defcnt[*i2 as usize] != 1 {
                continue;
            }
            let mut step = None;
            for &bid in &body {
                for di in &f.blocks[bid as usize].insts {
                    if let Inst::Bin(d, Op::Add, dty, a, b) = di {
                        if *d == *i2 && *dty == *ty {
                            step = tmp_times_imm(a, b, *i1);
                        }
                    }
                }
            }
            if let Some(c) = step {
                biv_of.insert(*i1, (i0.clone(), c));
            }
        }
        if biv_of.is_empty() {
            continue;
        }
        // LOOP-INVARIANT set over the body: const / defined outside body / (in-body, pure,
        // all operands invariant). Fixpoint. φ is never invariant (excludes the BIVs).
        let mut inv = vec![false; nt];
        for t in 0..nt {
            if let Some((b, _)) = def_of[t] {
                if !body.contains(&b) {
                    inv[t] = true;
                }
            }
        }
        loop {
            let mut grew = false;
            for &bid in &body {
                for inst in &f.blocks[bid as usize].insts {
                    let Some(d) = inst_def(inst) else { continue };
                    if inv[d as usize] || !is_hoistable(inst) {
                        continue;
                    }
                    let mut us = Vec::new();
                    inst_uses(inst, &mut us);
                    if us.iter().all(|&u| inv[u as usize]) {
                        inv[d as usize] = true;
                        grew = true;
                    }
                }
            }
            if !grew {
                break;
            }
        }
        // RECOGNIZE reducible addresses (Load/Store whose address = base + iv·stride).
        struct Reduction {
            addr: Tmp,
            ty: TypeId,
            base: Vec<Val>,
            biv: Tmp,
            stride: i64,
        }
        let mut reds: Vec<Reduction> = Vec::new();
        let mut seen: HashSet<Tmp> = HashSet::new();
        for &bid in &body {
            for inst in &f.blocks[bid as usize].insts {
                let addr = match inst {
                    Inst::Load(_, _, Val::Tmp(a)) => *a,
                    Inst::Store(_, Val::Tmp(a), _) => *a,
                    _ => continue,
                };
                if !seen.insert(addr) || defcnt[addr as usize] != 1 {
                    continue;
                }
                match def_of[addr as usize] {
                    Some((b, _)) if body.contains(&b) => {}
                    _ => continue, // address already invariant / outside loop ⟹ nothing to march
                }
                let (mut base, mut lin) = (Vec::new(), Vec::new());
                if decompose_addr(
                    f,
                    tt,
                    &def_of,
                    &Val::Tmp(addr),
                    &body,
                    &biv_of,
                    &inv,
                    &mut base,
                    &mut lin,
                ) && lin.len() == 1
                    && lin[0].1 != 0
                    // THEOREM PRECONDITION (opt.rs:2625): the transform reduces `base + iv·stride`
                    // where `base` is a loop-invariant address to be FOLDED into the pointer init.
                    // `base = ∅` ⟹ the "address" is a bare induction variable (incl. a marching
                    // pointer this pass already produced) — nothing to fold, the theorem does not
                    // apply. Omitting this let the pass reduce its own output every fixpoint round.
                    && !base.is_empty()
                {
                    reds.push(Reduction {
                        addr,
                        ty: f.temps[addr as usize],
                        base,
                        biv: lin[0].0,
                        stride: lin[0].1,
                    });
                }
            }
        }
        if reds.is_empty() {
            continue;
        }
        // MATERIALIZE. Preheader first (no empty preheaders).
        let ph = match ensure_preheader(f, header, &body) {
            Some(p) => p,
            None => continue,
        };
        let mut memo: HashMap<Tmp, Tmp> = HashMap::new();
        let mut lftr: Option<(Tmp /*pphi*/, Tmp /*pbase sum*/, i64 /*stride*/, Tmp /*biv*/)> = None;
        for r in &reds {
            // The previous reduction's header-φ insert / temp pushes made the outer `def_of`
            // stale. Rebuild it fresh (covers the temps materialized so far) so every locator
            // clone_inv_to_ph / the addr-replacement below reads is valid — root fix for s0272.
            let def_of = def_locations(f);
            // pbase = Σ base terms, recomputed once in the preheader.
            let mut acc: Option<Val> = None;
            for bt in &r.base {
                let v = match bt {
                    Val::Tmp(t) => Val::Tmp(clone_inv_to_ph(f, ph, &body, *t, &def_of, &mut memo)),
                    other => other.clone(),
                };
                acc = Some(match acc {
                    None => v,
                    Some(prev) => {
                        let d = f.temps.len() as Tmp;
                        f.temps.push(r.ty);
                        f.blocks[ph as usize].insts.push(Inst::Bin(d, Op::Add, r.ty, prev, v));
                        Val::Tmp(d)
                    }
                });
            }
            let pbase = acc.unwrap_or(Val::Imm(0));
            let pbase_t = match pbase {
                Val::Tmp(t) => t,
                imm => {
                    let d = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Copy(d, r.ty, imm));
                    d
                }
            };
            // pinit = pbase + i₀·stride  (folded; i₀ is the φ external arm, available in ph).
            let (i0, c) = biv_of[&r.biv].clone();
            let pinit = match i0 {
                Val::Imm(0) => Val::Tmp(pbase_t),
                Val::Imm(v) => {
                    let d = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Bin(
                        d,
                        Op::Add,
                        r.ty,
                        Val::Tmp(pbase_t),
                        Val::Imm(v.wrapping_mul(r.stride)),
                    ));
                    Val::Tmp(d)
                }
                Val::Tmp(t) => {
                    let t = clone_inv_to_ph(f, ph, &body, t, &def_of, &mut memo);
                    let off = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Bin(
                        off,
                        Op::Mul,
                        r.ty,
                        Val::Tmp(t),
                        Val::Imm(r.stride),
                    ));
                    let d = f.temps.len() as Tmp;
                    f.temps.push(r.ty);
                    f.blocks[ph as usize].insts.push(Inst::Bin(
                        d,
                        Op::Add,
                        r.ty,
                        Val::Tmp(pbase_t),
                        Val::Tmp(off),
                    ));
                    Val::Tmp(d)
                }
                Val::FImm(_) => continue,
            };
            // p₁ = φ(ph: pinit, tail: p₂); p₂ = p₁ + c·stride.
            let p2 = f.temps.len() as Tmp;
            f.temps.push(r.ty);
            let pphi = f.temps.len() as Tmp;
            f.temps.push(r.ty);
            let pos = f.blocks[header as usize]
                .insts
                .iter()
                .position(|i| !matches!(i, Inst::Phi(..)))
                .unwrap_or(0);
            f.blocks[header as usize].insts.insert(
                pos,
                Inst::Phi(pphi, r.ty, vec![(ph, pinit), (tail, Val::Tmp(p2))]),
            );
            f.blocks[tail as usize].insts.push(Inst::Bin(
                p2,
                Op::Add,
                r.ty,
                Val::Tmp(pphi),
                Val::Imm(c.wrapping_mul(r.stride)),
            ));
            // Replace the address def with `a = p₁` (uses unchanged; the old base+index
            // computation is left for DCE to reclaim).
            if let Some(cur) =
                f.blocks[def_of[r.addr as usize].unwrap().0 as usize]
                    .insts
                    .iter()
                    .position(|i| inst_def(i) == Some(r.addr))
            {
                let blk = def_of[r.addr as usize].unwrap().0 as usize;
                f.blocks[blk].insts[cur] = Inst::Copy(r.addr, r.ty, Val::Tmp(pphi));
            }
            if lftr.is_none() && c > 0 && r.stride > 0 {
                lftr = Some((pphi, pbase_t, r.stride, r.biv));
            }
            changed += 1;
        }
        // LFTR: replace the header counter test `i₁ < N` with `p₁ < L`, L = pbase + N·stride,
        // so the counter chain dies (DCE reclaims it next round). Conservative: exact shape.
        if let Some((pphi, pbase_t, stride, biv)) = lftr {
            // The reds loop mutated every block; refresh the locators before LFTR's own clone.
            def_of = def_locations(f);
            // The counter must now feed ONLY its increment and the test. The address
            // reduction left the old index arithmetic (`biv·stride`, casts) ORPHANED —
            // still present but dead until DCE runs. Counting raw uses would see those and
            // block LFTR forever, so count against a DCE-accurate liveness (fixpoint: a use
            // inside a dead-defined pure instruction does not keep `biv` alive).
            let live = {
                let mut live = vec![false; f.temps.len()];
                loop {
                    let mut ch = false;
                    let mark = |v: &[Tmp], live: &mut Vec<bool>, ch: &mut bool| {
                        for &u in v {
                            if !live[u as usize] {
                                live[u as usize] = true;
                                *ch = true;
                            }
                        }
                    };
                    for b in &f.blocks {
                        for inst in &b.insts {
                            let keep = !is_pure(inst)
                                || inst_def(inst).map_or(true, |d| live[d as usize]);
                            if keep {
                                let mut us = Vec::new();
                                inst_uses(inst, &mut us);
                                mark(&us, &mut live, &mut ch);
                            }
                        }
                        let mut us = Vec::new();
                        term_uses(&b.term, &mut us);
                        mark(&us, &mut live, &mut ch);
                    }
                    if !ch {
                        break;
                    }
                }
                live
            };
            let mut uses = 0u32;
            for b in &f.blocks {
                for inst in &b.insts {
                    let keep =
                        !is_pure(inst) || inst_def(inst).map_or(true, |d| live[d as usize]);
                    if !keep {
                        continue;
                    }
                    let mut us = Vec::new();
                    inst_uses(inst, &mut us);
                    uses += us.iter().filter(|&&u| u == biv).count() as u32;
                }
                let mut us = Vec::new();
                term_uses(&b.term, &mut us);
                uses += us.iter().filter(|&&u| u == biv).count() as u32;
            }
            // find the header test: Bin(cmp, Lt, biv, bound), bound invariant, feeding Br.
            let mut applied = false;
            let hdr = header as usize;
            let test = f.blocks[hdr].insts.iter().position(|i| {
                matches!(i, Inst::Bin(_, Op::Lt, _, Val::Tmp(x), bound)
                    if *x == biv && matches!(bound, Val::Imm(_) | Val::Tmp(_)))
            });
            // biv used exactly twice (increment + this test) ⟹ removing the test-use frees it.
            if uses == 2 {
                if let Some(ti) = test {
                    if let Inst::Bin(_, _, cty, _, bound) = f.blocks[hdr].insts[ti].clone() {
                        let bound_ok = match &bound {
                            Val::Imm(_) => true,
                            Val::Tmp(t) => inv[*t as usize],
                            _ => false,
                        };
                        if bound_ok {
                            // L = pbase + bound·stride  (in the preheader)
                            let boundv = match bound {
                                Val::Tmp(t) => {
                                    Val::Tmp(clone_inv_to_ph(f, ph, &body, t, &def_of, &mut memo))
                                }
                                other => other,
                            };
                            let scaled = f.temps.len() as Tmp;
                            f.temps.push(f.temps[pbase_t as usize]);
                            f.blocks[ph as usize].insts.push(Inst::Bin(
                                scaled,
                                Op::Mul,
                                f.temps[pbase_t as usize],
                                boundv,
                                Val::Imm(stride),
                            ));
                            let limit = f.temps.len() as Tmp;
                            f.temps.push(f.temps[pbase_t as usize]);
                            f.blocks[ph as usize].insts.push(Inst::Bin(
                                limit,
                                Op::Add,
                                f.temps[pbase_t as usize],
                                Val::Tmp(pbase_t),
                                Val::Tmp(scaled),
                            ));
                            // rewrite the test to compare the pointer against the limit
                            if let Inst::Bin(_, _, ty, a, b) = &mut f.blocks[hdr].insts[ti] {
                                *ty = f.temps[pphi as usize];
                                *a = Val::Tmp(pphi);
                                *b = Val::Tmp(limit);
                            }
                            let _ = cty;
                            applied = true;
                            // Clear the orphaned index arithmetic (old `biv·stride`, casts,
                            // copies) the address reduction left behind — it is non-cyclic
                            // dead code that still NAMES biv, so it must go before biv's def
                            // can be removed (else a dead use dangles). DCE cannot touch the
                            // counter's φ↔increment cycle itself (each keeps the other live).
                            dce(tt, f);
                            // The counter is now a dead self-cycle: biv (φ result) feeds
                            // only its increment i₂, and i₂ feeds only the φ. Break it here —
                            // find the header φ(biv), take its tail arm i₂, and if i₂ is used
                            // nowhere but that φ, delete both the increment and the φ.
                            let i2 = f.blocks[hdr].insts.iter().find_map(|i| match i {
                                Inst::Phi(d, _, arms) if *d == biv => arms
                                    .iter()
                                    .find(|(p, _)| *p == tail)
                                    .and_then(|(_, v)| match v {
                                        Val::Tmp(t) => Some(*t),
                                        _ => None,
                                    }),
                                _ => None,
                            });
                            if let Some(i2) = i2 {
                                let i2_uses: u32 = f
                                    .blocks
                                    .iter()
                                    .flat_map(|b| b.insts.iter())
                                    .map(|i| {
                                        let mut u = Vec::new();
                                        inst_uses(i, &mut u);
                                        u.iter().filter(|&&x| x == i2).count() as u32
                                    })
                                    .sum();
                                // exactly one use of i₂ = the φ's tail arm ⟹ cycle is dead.
                                if i2_uses == 1 {
                                    for b in f.blocks.iter_mut() {
                                        b.insts.retain(|i| {
                                            !matches!(i, Inst::Phi(d, ..) if *d == biv)
                                                && inst_def(i) != Some(i2)
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let _ = applied;
        }
    }
    changed
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass — FUNCTION INLINING (Tier-1 #5; β-reduction across the call graph).
//
// Theorem (call-substitution / β-reduction): in any context,
//   ⟦Call f(args)⟧ = ⟦ body_f with params bound to args, each Ret v → (dst := v; goto cont) ⟧.
// Proof obligation = the standard commuting square ⟦prog⟧ = ⟦inline(prog)⟧ on the
// ENTRY; `equiv` recurses through the residual Call(Sym), so before/after are compared
// whole-program (the self-recursive case included: a depth-1 unroll returns the same
// value, only fewer frames).
//
// The one non-obvious Side-II fact is the FRAME MODEL (ir.rs interp + arm64_elf
// lea_local): a local at offset `off` lives at index (frame − off) / address (x29 −
// off) — ONE offset law shared by the interpreter and the emitter. To merge a callee's
// frame into the caller's WITHOUT disturbing the caller's own offsets we APPEND the
// callee frame below: a callee local off_k relocates to off_k + frame_base (frame_base
// = the caller frame at splice time) and the merged frame grows by the callee frame.
// Then caller and every clone occupy DISJOINT regions under both (frame − off) and
// (x29 − off), so the single relocation is correct for the semantics AND the backend.
//
// Bound: at most ONE level — only call sites in the caller's ORIGINAL blocks are
// expanded; a clone's own calls stay calls — so recursion is finite and self-recursion
// is a depth-1 unroll (the fib lever: halves prologue/`bl` traffic, no unbounded
// growth). Cost model = a static instruction-count ceiling; only a SMALL callee pays.
// ─────────────────────────────────────────────────────────────────────────────


/// Is `i` safe to hoist out of a conditional arm (pure AND cannot trap)? `al` is the
/// function's alias oracle, consulted for Load fault-safety.
pub(crate) fn speculatable(tt: &TyTab, al: &AliasInfo, i: &Inst) -> bool {
    match i {
        Inst::Bin(_, Op::Div | Op::Rem, ty, _, _) => tt.is_float(*ty), // int /0 traps
        Inst::Bin(..) | Inst::Un(..) | Inst::Copy(..) | Inst::Cast(..) | Inst::Lea(..) => true,
        // A volatile Load is an observable side effect (C99 6.7.3): it must execute exactly
        // on the paths the source dictates — never speculated out of a conditional arm.
        Inst::Load(_, ty, addr) => !tt.is_volatile(*ty) && al.fault_free(*addr),
        _ => false, // Store/Call/Phi/… — a side effect or a control artifact
    }
}


pub fn if_convert(tt: &TyTab, f: &mut IrFunc) -> u32 {
    let al = alias_info(f);
    let preds = predecessors(f);
    // Blocks that lie inside SOME natural loop — the only place triangle if-conversion fires.
    // A triangle (`if(c) stmt;`) trades a predicted-branch skip for unconditional arm execution
    // plus a csel: profitable in HOT code (a per-iteration branch removed) but a size/speed loss
    // on the many one-off conditional assignments outside loops (measured: +472 on sqlite when
    // unrestricted). "In a loop" is the static profitability proxy gcc gets from branch weights.
    let in_loop: Vec<bool> = if cfg_complete(f) {
        let dom = dominators(f);
        let mut m = vec![false; f.blocks.len()];
        for (tail, header) in back_edges(f, &dom) {
            for b in natural_loop(f, tail, header) {
                m[b as usize] = true;
            }
        }
        m
    } else {
        vec![false; f.blocks.len()]
    };
    // Defining instruction of each temp (single-def in SSA) — used to classify a branch
    // condition as DATA-DEPENDENT (derives from a memory Load, hence unpredictable) vs derived
    // only from loop-structural values (predictable). Triangle if-conversion pays exactly when
    // the branch is unpredictable — the sieve's `if(is[i])` (a load) — and loses on the many
    // predictable in-loop conditionals in sqlite. This is the static proxy for gcc's branch
    // probability. Shallow trace (≤2 levels through cmp/Un/Cast) is enough for the load ⟶ cmp ⟶ br
    // idiom without a full backward slice.
    // Per-block flag: the block ends in `Br(cond,..)` whose condition is DATA-DEPENDENT — it
    // derives from a memory Load, hence is unpredictable (the sieve's `if(is[i])`), as opposed
    // to a condition over loop-structural values (predictable). Triangle if-conversion pays
    // exactly on the unpredictable branch and loses on the predictable in-loop conditionals that
    // fill sqlite; this is the static proxy for gcc's branch probability. Computed BEFORE the
    // mutation loop (all borrows immutable here), then just indexed. Shallow trace (≤2 def-hops
    // through a cmp/Un/Cast) covers the load ⟶ cmp ⟶ br idiom without a full backward slice.
    let cond_dd: Vec<bool> = {
        let mut def_of: Vec<Option<usize>> = vec![None; f.temps.len()];
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, inst) in b.insts.iter().enumerate() {
                if let Some(d) = inst_def(inst) {
                    def_of[d as usize] = Some(bi << 16 | ii);
                }
            }
        }
        let inst_at = |code: usize| -> &Inst { &f.blocks[code >> 16].insts[code & 0xFFFF] };
        let is_load = |v: &Val| matches!(v, Val::Tmp(t) if def_of[*t as usize].is_some_and(|c| matches!(inst_at(c), Inst::Load(..))));
        let load_derived = |v0: &Val| -> bool {
            let mut cur = *v0;
            for _ in 0..2 {
                let Val::Tmp(c) = cur else { return false };
                let Some(code) = def_of[c as usize] else { return false };
                match inst_at(code) {
                    Inst::Load(..) => return true,
                    Inst::Bin(_, _, _, a, b) => cur = if is_load(a) { *a } else { *b },
                    Inst::Un(_, _, _, a) | Inst::Cast(_, _, _, a) => cur = *a,
                    _ => return false,
                }
            }
            false
        };
        f.blocks.iter().map(|b| matches!(&b.term, Term::Br(c, ..) if load_derived(c))).collect()
    };
    let mut n = 0u32;
    // One linear scan over heads; each rewrite is local (h, T, E, M) and never creates a
    // new diamond, so a single pass suffices (nested ternaries fold bottom-up across the
    // optimize_ssa fixpoint's earlier cfg_simplify — here we take one layer).
    for h in 0..f.blocks.len() {
        let Term::Br(cond, t_id, e_id) = f.blocks[h].term.clone() else { continue };
        let (t, e) = (t_id as usize, e_id as usize);
        if t == h || e == h || t == e {
            continue;
        }
        // TRIANGLE if-conversion (`if(c) stmt;` — one arm IS the merge, no else). Shape:
        // `Br(cond, t, e)` where exactly one arm A (single-pred = h, pure body) jumps straight
        // to the OTHER target, which is then the merge M; M's preds are {h, A}. The h→M edge is
        // the "empty else" path. Convert: hoist A's pure body into h, turn each φ of M — whose
        // arms are exactly {h, A} — into `Select(cond, then_val, else_val)`, rewire h→M. `arm_true`
        // = whether the arm lies on the cond-TRUE path (A == t): it decides the Select operand
        // order. ⟦f⟧ preserved — the φ already encodes value-per-edge; Select is the same choice
        // made data-flow. Checked BEFORE the diamond path; a matched-but-unconvertible triangle is
        // never a diamond, so it falls through to the shared `continue` below.
        let tri = if matches!(f.blocks[t].term, Term::Jmp(j) if j == e_id) {
            Some((t_id, t, e_id, e, true)) // arm = t (cond-true), merge = e
        } else if matches!(f.blocks[e].term, Term::Jmp(j) if j == t_id) {
            Some((e_id, e, t_id, t, false)) // arm = e (cond-false), merge = t
        } else {
            None
        };
        if let Some((arm_id, arm, mrg_id, mrg, arm_true)) = tri {
            // arm private to this branch; merge reached ONLY from {h, arm}.
            let mut mp = preds[mrg].clone();
            mp.sort_unstable();
            let mut want = [h as BlockId, arm_id];
            want.sort_unstable();
            // PROFITABILITY (size-safety): only convert a SHORT arm. A triangle replaces a
            // predicted-branch skip with UNCONDITIONAL execution of the arm plus a csel; that is
            // a win only when the arm is tiny (the count++/flag/min-max sweet spot csel exists
            // for). Speculating a large arm bloats and de-optimizes. Cap = ≤2 compute insts —
            // enough for `x = x ± k` / a single reconciled value, the pattern that regressed
            // sqlite size when unbounded. (The diamond path below is already balanced by both
            // arms hoisting; a triangle hoists one arm against an EMPTY else, so it needs the cap.)
            let arm_len = f.blocks[arm].insts.len();
            let arm_pure = in_loop[h]
                && arm_len <= 2
                && cond_dd[h]
                && f.blocks[arm].insts.iter().all(|i| speculatable(tt, &al, i));
            // Every leading φ of M is a plain 2-arm (h, arm) φ, non-float (backend csel); and there
            // is at least one (the merged value the Select will carry — else nothing to convert).
            let phi_iter = || f.blocks[mrg].insts.iter().take_while(|i| matches!(i, Inst::Phi(..)));
            let has_phi = phi_iter().next().is_some();
            let phis_ok = has_phi && phi_iter().all(|i| {
                let Inst::Phi(_, ty, arms) = i else { return false };
                !tt.is_float(*ty)
                    && arms.len() == 2
                    && arms.iter().any(|(b, _)| *b == h as BlockId)
                    && arms.iter().any(|(b, _)| *b == arm_id)
            });
            if arm != h && mrg != h && arm != mrg && preds[arm] == [h as BlockId] && mp == want && arm_pure && phis_ok {
                // Hoist the arm's (pure, arm-private) computation into h; it now runs
                // unconditionally, its defs dominate M.
                let body = std::mem::take(&mut f.blocks[arm].insts);
                f.blocks[h].insts.extend(body);
                // Each φ of M → Select. `then`/`else` = the φ arm on the cond-true / cond-false
                // edge. The arm edge carries cond=arm_true; the direct h edge carries cond=!arm_true.
                for i in f.blocks[mrg].insts.iter_mut() {
                    let Inst::Phi(d, ty, arms) = i else { break };
                    let v_arm = arms.iter().find(|(b, _)| *b == arm_id).unwrap().1;
                    let v_h = arms.iter().find(|(b, _)| *b == h as BlockId).unwrap().1;
                    let (vt, ve) = if arm_true { (v_arm, v_h) } else { (v_h, v_arm) };
                    *i = Inst::Select(*d, *ty, cond, vt, ve);
                    n += 1;
                }
                f.blocks[h].term = Term::Jmp(mrg_id);
                // arm is now unreachable+empty; cfg_simplify reclaims it.
                continue;
            }
            continue; // triangle-shaped but not convertible ⟹ not a diamond either
        }
        // Arms must be private to this diamond: single predecessor = h.
        if preds[t] != [h as BlockId] || preds[e] != [h as BlockId] {
            continue;
        }
        // Both arms converge on the same merge M by an unconditional Jmp.
        let (Term::Jmp(mt), Term::Jmp(me)) = (f.blocks[t].term.clone(), f.blocks[e].term.clone()) else { continue };
        if mt != me {
            continue;
        }
        let m_id = mt;
        let m = mt as usize;
        if m == t || m == e || m == h {
            continue;
        }
        // M's ONLY predecessors are the two arms (else its φs have arms we cannot fold).
        let mut mp = preds[m].clone();
        mp.sort_unstable();
        let mut want = [t_id, e_id];
        want.sort_unstable();
        if mp != want {
            continue;
        }
        // The MERGE temps: this IR keeps a diamond's result as a register temp assigned
        // in BOTH arms (to_ssa φ-ifies only promoted memory), reconciled after the join —
        // NOT a φ. A merge temp is one defined in both T and E; that pair of defs is what
        // becomes a Select. `defs(blk)` = the set of temps the block defines.
        let defs = |blk: usize| -> HashSet<Tmp> {
            f.blocks[blk].insts.iter().filter_map(inst_def).collect()
        };
        let (dt, de) = (defs(t), defs(e));
        let merge: Vec<Tmp> = { let mut v: Vec<Tmp> = dt.intersection(&de).copied().collect(); v.sort_unstable(); v };
        if merge.is_empty() {
            continue; // no value to select — a pure diamond with no reconciled result
        }
        // Every merge temp must be NON-FLOAT (backend csel) and reconciled by a plain
        // Copy in EACH arm (so its per-arm contribution is a Val we can drop into Select).
        // `src(blk, r)` = the Val copied into r in that arm, or None if r is not Copy-defined there.
        let src = |blk: usize, r: Tmp| -> Option<Val> {
            f.blocks[blk].insts.iter().find_map(|i| match i {
                Inst::Copy(d, _, v) if *d == r => Some(*v),
                _ => None,
            })
        };
        let merge_ok = merge.iter().all(|&r| {
            !tt.is_float(f.temps[r as usize]) && src(t, r).is_some() && src(e, r).is_some()
        });
        if !merge_ok {
            continue;
        }
        // Every NON-merge instruction of each arm must be speculatable (pure + non-faulting)
        // — it will run unconditionally. (The merge Copies are dropped, not hoisted.)
        let arm_pure = |blk: usize| {
            f.blocks[blk].insts.iter().all(|i| match inst_def(i) {
                Some(d) if merge.contains(&d) => matches!(i, Inst::Copy(..)), // the merge reconciler
                _ => speculatable(tt, &al, i),
            })
        };
        if !arm_pure(t) || !arm_pure(e) {
            continue;
        }
        // M may ALSO hold φ nodes (a promoted-memory merge, e.g. a ternary over an
        // address-taken local): each such φ has arms exactly {T,E} (M's only preds).
        // Rewiring h→M would leave those φs with no arm for h — so we convert them TOO,
        // in place. Refuse the diamond if any leading φ is float (backend csel is
        // integer) or (defensively) not a plain 2-arm (T,E) φ.
        let phis_ok = f.blocks[m].insts.iter().take_while(|i| matches!(i, Inst::Phi(..))).all(|i| {
            let Inst::Phi(_, ty, arms) = i else { return false };
            !tt.is_float(*ty)
                && arms.len() == 2
                && arms.iter().any(|(b, _)| *b == t_id)
                && arms.iter().any(|(b, _)| *b == e_id)
        });
        if !phis_ok {
            continue;
        }
        // ---- COMMIT the rewrite (all guards passed) ----
        // Extract each merge temp's per-arm source BEFORE mutating the blocks.
        let sels: Vec<(Tmp, Val, Val)> =
            merge.iter().map(|&r| (r, src(t, r).unwrap(), src(e, r).unwrap())).collect();
        // 1. Hoist every NON-merge-Copy instruction of both arms into h, in order (their
        //    defs are arm-private, distinct, and — once unconditional — dominate M). The
        //    merge Copies are dropped; their role is taken by the Selects below.
        let hoist = |blk: &mut Block, merge: &[Tmp]| -> Vec<Inst> {
            std::mem::take(&mut blk.insts).into_iter()
                .filter(|i| !matches!(inst_def(i), Some(d) if merge.contains(&d)))
                .collect()
        };
        let ht = hoist(&mut f.blocks[t], &merge);
        let he = hoist(&mut f.blocks[e], &merge);
        let hb = &mut f.blocks[h];
        hb.insts.extend(ht);
        hb.insts.extend(he);
        // 2. One Select per merge temp: r = (cond ≠ 0) ? src_T : src_E. Appended AFTER the
        //    hoisted computations, so every referenced value is already in scope.
        for (r, vt, ve) in sels {
            hb.insts.push(Inst::Select(r, f.temps[r as usize], cond, vt, ve));
            n += 1;
        }
        hb.term = Term::Jmp(m_id);
        // 3. Convert every leading φ of M into a Select (in place). φ arms reference values
        //    live on the T / E edges — defined in the arms (now hoisted into h) or above —
        //    so they dominate M. φs never reference sibling-φ dsts, so in-place rewrite is sound.
        for i in f.blocks[m].insts.iter_mut() {
            let Inst::Phi(d, ty, arms) = i else { break };
            let vt = arms.iter().find(|(b, _)| *b == t_id).unwrap().1;
            let ve = arms.iter().find(|(b, _)| *b == e_id).unwrap().1;
            *i = Inst::Select(*d, *ty, cond, vt, ve);
            n += 1;
        }
        // The arms T,E are now unreachable (no preds) and empty; cfg_simplify drops them.
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass — REMATERIALIZATION (Tier-5 #26; the CbC-PURE slice of register allocation).
//
// The QBE cross-check named register allocation as zcc's real QBE-class gap, but its
// Belady-spill core rides a tuned loop-weight ⟹ below the CbC-purity line (rejected). Remat
// is the slice that IS a total theorem: a value whose definition is PURE and OPERAND-FREE —
// a constant (`Copy(Imm)`), a frame/symbol/string address (`Lea`), a function/label address
// (`FunAddr`/`LabelAddr`) — can be RECOMPUTED at any program point unconditionally, because
// it depends on nothing (its availability is the whole-program constant `true`). So instead
// of keeping it live across a high-pressure region — where the allocator must SPILL it
// (store at def + reload per use, memory traffic) — we clone the 1–2-instruction recompute
// right before each use and shorten its live range to ~zero.
//
// THEOREM (CbC): `⟦f⟧ = ⟦remat(f)⟧`. A pure operand-free instruction `D` computes a value
// that is a function of NOTHING (no temp, no memory) ⟹ its result is identical at every
// program point ⟹ replacing a use of `t=D` by a use of a fresh `t'=D` cloned immediately
// before it is a ⟦·⟧-identity (Law-1 Side-I: ⟦·⟧ is a function of the operands, here empty).
// The original def, now useless, is dropped (mark-sweep in one shot). Validated by `equiv`
// over the structural corpus (commuting square) — see `remat_*` tests.
//
// SPEED (the orthogonal axis, Law 3 — MEASURED, not asserted): remat fires ONLY on a temp
// live at some point of GP pressure ≥ k (exactly the temps the k-colouring must spill). It
// trades {store + N reloads} for {N recomputes of a 1–2-inst value} and frees a register
// across the span ⟹ statically fewer memory ops. Default-OFF until the box A/B confirms the
// win on the real allocator (the SSA/backend-pressure proxy gap, same residual as LICM).
// ─────────────────────────────────────────────────────────────────────────────


/// A def that can be recomputed anywhere: PURE and OPERAND-FREE (no temp/memory input, so
/// available unconditionally). `Place` is `Local|Global|Str` — all operand-free — so every
/// `Lea` qualifies; `inst_uses`-empty is asserted belt-and-suspenders.
pub(crate) fn rematerializable(i: &Inst) -> bool {
    let mut u = Vec::new();
    inst_uses(i, &mut u);
    u.is_empty()
        && matches!(
            i,
            Inst::Copy(_, _, Val::Imm(_)) | Inst::Lea(..) | Inst::FunAddr(..) | Inst::LabelAddr(..)
        )
}


pub fn remat(tt: &TyTab, f: &mut IrFunc, gp_k: u32) -> u32 {
    let nt = f.temps.len();
    // 1. Per temp: def count + (if rematerializable) a clone of its defining instruction.
    let mut defcnt = vec![0u32; nt];
    let mut def_inst: Vec<Option<Inst>> = vec![None; nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                defcnt[d as usize] += 1;
                if rematerializable(i) {
                    def_inst[d as usize] = Some(i.clone());
                }
            }
        }
    }
    // 2. Under-pressure set: temps live at any point with GP pressure ≥ k (the spill victims).
    //    Backward live-set walk; at each point of pressure ≥ k, mark every live GP temp.
    let lv = liveness(f);
    let is_fp: Vec<bool> = f.temps.iter().map(|&ty| tt.is_float(ty)).collect();
    let gp = |live: &[bool]| (0..nt).filter(|&t| live[t] && !is_fp[t]).count() as u32;
    let mut pressured = vec![false; nt];
    let mark = |live: &[bool], pr: &mut [bool]| {
        if gp(live) >= gp_k {
            for t in 0..nt {
                if live[t] && !is_fp[t] {
                    pr[t] = true;
                }
            }
        }
    };
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true;
        }
        mark(&live, &mut pressured);
        for i in b.insts.iter().rev() {
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
            mark(&live, &mut pressured);
        }
    }
    // 3. Targets = single-def ∧ rematerializable ∧ under pressure.
    let target: Vec<bool> =
        (0..nt).map(|t| defcnt[t] == 1 && def_inst[t].is_some() && pressured[t]).collect();
    if !target.iter().any(|&b| b) {
        return 0;
    }
    // 4. Clone the def before each USE (fresh temp), redirect that use, drop the dead original.
    //    A target's own def is operand-free ⟹ dropping it never orphans another use.
    let mut count = 0u32;
    for bi in 0..f.blocks.len() {
        let old = std::mem::take(&mut f.blocks[bi].insts);
        let mut out: Vec<Inst> = Vec::with_capacity(old.len());
        for mut inst in old {
            let fresh = clone_remats_before(f, &mut inst, &def_inst, &target, &mut out);
            count += fresh;
            if let Some(d) = inst_def(&inst) {
                if target[d as usize] {
                    continue; // the original def is now dead — drop it
                }
            }
            out.push(inst);
        }
        // The terminator may also use a target (e.g. Ret(Tmp)): materialize before it.
        let mut term = std::mem::replace(&mut f.blocks[bi].term, Term::Ret(None));
        count += clone_remats_before_term(f, &mut term, &def_inst, &target, &mut out);
        f.blocks[bi].insts = out;
        f.blocks[bi].term = term;
    }
    count
}


/// For each target temp USED by `inst`, push a fresh clone of its def to `out` and rewrite
/// the use to the clone (a temp used twice reuses one clone). Returns the clone count.
pub(crate) fn clone_remats_before(
    f: &mut IrFunc,
    inst: &mut Inst,
    def_inst: &[Option<Inst>],
    target: &[bool],
    out: &mut Vec<Inst>,
) -> u32 {
    let mut uses = Vec::new();
    inst_uses(inst, &mut uses);
    let mut map: Vec<(Tmp, Tmp)> = Vec::new();
    for &t in &uses {
        if !target[t as usize] || map.iter().any(|&(o, _)| o == t) {
            continue;
        }
        let ty = f.temps[t as usize];
        let fresh = f.temps.len() as Tmp;
        f.temps.push(ty);
        let mut clone = def_inst[t as usize].clone().unwrap();
        each_def_mut(&mut clone, |d| *d = fresh);
        out.push(clone);
        map.push((t, fresh));
    }
    each_use_mut(inst, |v| {
        if let Val::Tmp(t) = v
            && let Some(&(_, fr)) = map.iter().find(|&&(o, _)| o == *t)
        {
            *t = fr;
        }
    });
    map.len() as u32
}


/// Terminator variant of `clone_remats_before`.
pub(crate) fn clone_remats_before_term(
    f: &mut IrFunc,
    term: &mut Term,
    def_inst: &[Option<Inst>],
    target: &[bool],
    out: &mut Vec<Inst>,
) -> u32 {
    let mut uses = Vec::new();
    term_uses(term, &mut uses);
    let mut map: Vec<(Tmp, Tmp)> = Vec::new();
    for &t in &uses {
        if !target[t as usize] || map.iter().any(|&(o, _)| o == t) {
            continue;
        }
        let ty = f.temps[t as usize];
        let fresh = f.temps.len() as Tmp;
        f.temps.push(ty);
        let mut clone = def_inst[t as usize].clone().unwrap();
        each_def_mut(&mut clone, |d| *d = fresh);
        out.push(clone);
        map.push((t, fresh));
    }
    each_use_term_mut(term, |v| {
        if let Val::Tmp(t) = v
            && let Some(&(_, fr)) = map.iter().find(|&&(o, _)| o == *t)
        {
            *t = fr;
        }
    });
    map.len() as u32
}

