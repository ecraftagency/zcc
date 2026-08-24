// src/opt/regalloc.rs — register allocation — liveness, interference, coloring, ABI homing.
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;


// ─────────────────────────────────────────────────────────────────────────────
// Pass 5 — REGISTER ALLOCATION (graph coloring, Chaitin–Briggs).
//
// NP-complete (THEORY §C2 — graph coloring) ⟹ use a HEURISTIC simplify/spill, NOT
// demanding a strict optimum. But CORRECTNESS (a valid coloring) is verifiable in P.
//
// Correctness here DIFFERS from the four passes above: interp does NOT model
// registers, so ⟦before⟧=⟦after⟧ cannot be used. The correctness invariant is
// RENAMING BISIMULATION (THEORY §A7): the register-assigned program is bisimilar to
// the temporary program ⟺ two SIMULTANEOUSLY LIVE temporaries always occupy DIFFERENT
// locations (a live value is never overwritten). We check the INTERFERENCE INVARIANT
// mechanically:
//   ∀ edge (u,v) ∈ interference-graph, color[u] ≠ color[v]  (a spill = its own slot, never overwritten).
//
// Chain of theorems: liveness (monotone dataflow, Kleene fixpoint) → interference
// graph (u interferes with v ⟺ both live at some def) → coloring (simplify degree<k / spill) → verify.
// ─────────────────────────────────────────────────────────────────────────────


/// Flow-SENSITIVE liveness (backward dataflow, THEORY §B3 fixpoint over the lattice 2^Tmp).
/// Only live-OUT is consumed downstream (interference is built at defs, scanning tailward);
/// live-IN is the fixpoint's working set, not exported.
pub struct Liveness {
    pub live_out: Vec<Vec<bool>>,
}


pub fn liveness(f: &IrFunc) -> Liveness {
    let nb = f.blocks.len();
    let nt = f.temps.len();
    // gen (use before def within a block) + kill (def within a block)
    let mut useb = vec![vec![false; nt]; nb];
    let mut defb = vec![vec![false; nt]; nb];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut defined = vec![false; nt];
        for i in &b.insts {
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                if !defined[u as usize] {
                    useb[bi][u as usize] = true;
                }
            }
            if let Some(d) = inst_def(i) {
                defined[d as usize] = true;
                defb[bi][d as usize] = true;
            }
        }
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            if !defined[u as usize] {
                useb[bi][u as usize] = true;
            }
        }
    }
    let succ = successors(f);
    let mut live_in = vec![vec![false; nt]; nb];
    let mut live_out = vec![vec![false; nt]; nb];
    loop {
        let mut changed = false;
        for bi in (0..nb).rev() {
            let mut lo = vec![false; nt];
            for &s in &succ[bi] {
                for t in 0..nt {
                    if live_in[s as usize][t] {
                        lo[t] = true;
                    }
                }
            }
            let mut li = useb[bi].clone();
            for t in 0..nt {
                if lo[t] && !defb[bi][t] {
                    li[t] = true;
                }
            }
            if lo != live_out[bi] {
                live_out[bi] = lo;
                changed = true;
            }
            if li != live_in[bi] {
                live_in[bi] = li;
                changed = true;
            }
        }
        if !changed {
            break; // fixpoint (Kleene): no set grows any further
        }
    }
    let _ = live_in; // the fixpoint's working set; not exported (only live_out is consumed)
    Liveness { live_out }
}


/// Interference graph: u—v ⟺ u,v are both live at some definition point (they cannot share a register).
pub fn interference(f: &IrFunc, lv: &Liveness) -> Vec<HashSet<Tmp>> {
    let nt = f.temps.len();
    let mut adj: Vec<HashSet<Tmp>> = vec![HashSet::new(); nt];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        // `live` is the SPARSE set of currently-live temps (iterate only members, never a
        // full 0..nt scan): at a def we add an edge to each ALREADY-live temp, so the cost
        // is Σ live-set-size ≈ O(edges), not O(defs·nt). The full-nt bitvector scan was the
        // O(nt²) compile-time pathology — an SSA temp dies at its last use, so the live set
        // stays SMALL even in a fuzz-generated mega-block with hundreds of thousands of
        // temps (⟦·⟧ is unchanged: same edge set, sparser walk).
        let mut live: HashSet<Tmp> =
            lv.live_out[bi].iter().enumerate().filter_map(|(t, &a)| a.then_some(t as u32)).collect();
        // the terminator's operands are live at the block's tail
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live.insert(u);
        }
        for i in b.insts.iter().rev() {
            if let Some(d) = inst_def(i) {
                for &t in &live {
                    if t != d {
                        adj[d as usize].insert(t);
                        adj[t as usize].insert(d);
                    }
                }
                live.remove(&d);
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live.insert(u);
            }
        }
    }
    adj
}


// ─────────────────────────────────────────────────────────────────────────────
// Stage 5b — ABI-AWARE register allocation (the backend consumes this coloring).
//
// Extends the interference-invariant bisimulation above with the ABI (THEORY §A7,
// §D2). A machine register belongs to one of two ABI classes (AAPCS64 §6.1.1): a
// CALLER-saved register is clobbered by every `bl`; a CALLEE-saved register is
// preserved across it. Hence the extra proof obligation of the plain interference
// invariant:
//   CALL-CLOBBER SET-DISJOINTNESS — a temp whose value is LIVE ACROSS a call must
//   receive a callee-saved color, else the `bl` overwrites it (⟦·⟧ broken).
// We model this by RESTRICTING such a temp's select-range to the callee-saved colors.
// Colors within a class are ordered [caller … | callee …]; `ncaller` marks the split.
// A non-crossing temp prefers the low (caller) colors (a callee-saved home costs a
// prologue save/restore); a crossing temp is confined to [ncaller, k).
pub struct ClassBudget {
    pub k: u32,       // total colors in the class
    pub ncaller: u32, // colors [0,ncaller) are caller-saved; [ncaller,k) callee-saved
    // The TOP `narg` caller colors [ncaller-narg, ncaller) map to ARGUMENT registers (x6,x7
    // in GP-WIDE). A PARAM temp must NOT be homed there: params arrive in the arg registers,
    // so a param homed at a DIFFERENT arg register makes Inst::Param delivery a register
    // permutation the sequential per-Param `mov` gets wrong (lost-copy). Excluding params from
    // these colors keeps homes disjoint from arg registers for params, so any delivery order is
    // safe. Non-param temps use them freely (crossing[] still confines call-crossers to callee).
    pub narg: u32,
}


/// Chaitin simplify/select over ONE register class (`in_class` selects its temps;
/// interference edges to out-of-class temps are ignored — the two files are disjoint).
/// A `crossing` temp may only take a callee-saved color. Result: per-temp color, None = spill.
///
/// `bias` carries CONSERVATIVE register coalescing (Phase A): `bias[v]` lists the temps
/// move-related to v (a non-interfering `Copy` partner). At SELECT, v prefers a color
/// already held by such a partner, so the copy lowers to a same-register `mov` the
/// peephole elides. This is coalescing WITHOUT node-merge — it only picks among the
/// colors already free & legal for v, so it can never worsen k-colorability and NEVER
/// changes the coloring's validity: the interference invariant (hence the ⟦·⟧-preserving
/// rename-bisimulation) is identical with or without the bias. Correctness therefore
/// rests on the SAME `verify_abi` theorem as Stage 5b — no new proof obligation.
pub fn color_abi(
    adj: &[HashSet<Tmp>],
    in_class: &[bool],
    b: &ClassBudget,
    crossing: &[bool],
    bias: &[Vec<Tmp>],
    is_param: &[bool],
    target: &[Option<u32>],
    cost: &[u32],
) -> Vec<Option<u32>> {
    let nt = adj.len();
    let k = b.k as usize;
    // Spill-metric toggle (Article-E: a heuristic switch, not a spec constant). Default =
    // cost/degree (Briggs); ZCC_SPILL=degree restores the prior cost-blind max-degree for A/B.
    let spill_by_degree = std::env::var("ZCC_SPILL").ok().as_deref() == Some("degree");
    // class-local degree: count only in-class, not-yet-removed neighbors
    let mut degree: Vec<usize> = (0..nt)
        .map(|v| {
            if in_class[v] {
                adj[v].iter().filter(|&&u| in_class[u as usize]).count()
            } else {
                0
            }
        })
        .collect();
    let mut removed = vec![false; nt];
    let mut stack: Vec<Tmp> = Vec::new();
    // SIMPLIFY worklist. `low` holds every class-degree<k node ordered by index (Reverse ⇒
    // min-heap), so the removal order — hence the SELECT stack order, hence the emitted
    // coloring — is BYTE-IDENTICAL to the old lowest-index linear scan, but produced in
    // O(nt log nt) rather than O(nt²) (the per-step `(0..nt).find`/`max_by_key` was the
    // residual compile-time quadratic on a fuzz mega-block). Class degree only DECREASES
    // during simplify, so a node crosses below k AT MOST ONCE — push it exactly then; a
    // heap entry is retired only by being popped (guarded against a stale duplicate).
    use std::cmp::Reverse;
    let mut low: std::collections::BinaryHeap<Reverse<Tmp>> =
        (0..nt as u32).filter(|&v| in_class[v as usize] && degree[v as usize] < k).map(Reverse).collect();
    let mut left = in_class.iter().filter(|&&c| c).count();
    while left > 0 {
        let v = if let Some(Reverse(v)) = low.pop() {
            if removed[v as usize] {
                continue; // stale (already retired) — skip
            }
            v as usize
        } else {
            // no degree<k node remains → OPTIMISTIC SPILL. Chaitin-Briggs spill METRIC:
            // spill the node minimising cost/degree — cheapest reload traffic per unit of
            // simplify-unblocking. `cost[v]` = the static use+def count of the temps merged
            // into rep v (the number of reloads/spill-stores a real spill of v would emit);
            // dividing by class-degree favours a high-degree node (it unblocks the most
            // neighbours). This changes ONLY which temp lands in memory — `verify_abi`'s
            // interference invariant is agnostic to the choice, so no new proof obligation
            // (same theorem as Stage-5b). The prior heuristic (max class-degree, cost-blind)
            // is preserved under `ZCC_SPILL=degree` for A/B. Compare cost_v/deg_v < cost_u/deg_u
            // as cost_v*deg_u < cost_u*deg_v (integer, no float); ties → lowest index (in-order
            // fold), keeping the coloring deterministic.
            let cand = (0..nt).filter(|&v| in_class[v] && !removed[v]);
            let pick = if spill_by_degree {
                cand.max_by_key(|&v| degree[v])
            } else {
                cand.reduce(|a, v| {
                    // spill the lower cost/degree ratio; degree≥1 here (all remaining ≥k)
                    let (ca, da) = (cost[a] as u64, degree[a].max(1) as u64);
                    let (cv, dv) = (cost[v] as u64, degree[v].max(1) as u64);
                    if cv * da < ca * dv { v } else { a }
                })
            };
            match pick {
                Some(v) => v,
                None => break,
            }
        };
        removed[v] = true;
        left -= 1;
        stack.push(v as u32);
        for &nb in &adj[v] {
            let u = nb as usize;
            if in_class[u] && !removed[u] {
                if degree[u] == k {
                    low.push(Reverse(nb)); // crosses k → k-1 (<k) this step; enqueue once
                }
                degree[u] -= 1;
            }
        }
    }
    // SELECT: smallest free color in the temp's allowed range; out of range → spill.
    let mut colr = vec![None; nt];
    while let Some(v) = stack.pop() {
        let mut used = vec![false; k];
        for &nb in &adj[v as usize] {
            if in_class[nb as usize]
                && let Some(c) = colr[nb as usize]
            {
                used[c as usize] = true;
            }
        }
        let lo = if crossing[v as usize] { b.ncaller } else { 0 };
        // A param temp may not take an arg-register caller color [ncaller-narg, ncaller).
        let arg_lo = b.ncaller - b.narg;
        let forbid_arg = is_param[v as usize];
        let free =
            |c: u32| c >= lo && c < b.k && !used[c as usize] && !(forbid_arg && c >= arg_lo && c < b.ncaller);
        // ABI TARGETING (Phase 2): an outgoing arg temp #i prefers color i (⟹ x{i}, the arg
        // register), a call-result temp prefers color 0 (⟹ x0) — so marshal_call_args' `mov
        // x{i},x{i}` / the result-capture `mov home,x0` become self-moves the peephole drops.
        // Checked FIRST, but only among FREE & legal colors, so — exactly like `bias` below —
        // it never forces an illegal color, never changes k-colorability, and rests on the SAME
        // verify_abi theorem (no new proof obligation). Meaningful only in WIDE (caller color
        // i = x{i}); abi_alloc leaves target=None otherwise.
        let targeted = target[v as usize].filter(|&c| free(c));
        // biased coalescing: prefer a free, in-range color already held by a same-class
        // move partner (the copy becomes a self-move). Falls back to the smallest free
        // color — so the result is always a valid coloring regardless of the bias.
        let biased = bias[v as usize]
            .iter()
            .filter(|&&p| in_class[p as usize])
            .filter_map(|&p| colr[p as usize])
            .find(|&c| free(c));
        colr[v as usize] = targeted.or(biased).or_else(|| (lo..b.k).find(|&c| free(c)));
    }
    colr
}


/// A temp's HOME after ABI allocation: (is_fp, color-within-class), or None = spill (memory slot).
pub type AbiHome = Option<(bool, u32)>;


/// Stage 5b entry — partition temps by ABI file (GP int/ptr vs FP float), color each
/// against its budget, confining call-crossing temps to callee-saved. Falls back to
/// ALL-SPILL for a function containing inline asm (`Inst::Asm`): its operand pool grows
/// over x9../v16.. without bound and can clobber ANY allocatable register, defeating the
/// disjointness invariant — so no home is safe (the pre-Stage-5b memory model, verbatim).
pub fn abi_alloc(tt: &TyTab, f: &IrFunc, gp: &ClassBudget, fp: &ClassBudget, coalesce: bool) -> Vec<AbiHome> {
    let nt = f.temps.len();
    let mut home: Vec<AbiHome> = vec![None; nt];
    if f.blocks.iter().flat_map(|b| &b.insts).any(|i| matches!(i, Inst::Asm(..))) {
        return home; // conservative all-spill
    }
    // PERF BACKSTOP (Article-E policy constant, dated 2026-08 — a convenience ceiling, NOT
    // a spec number; one env-var from tuning, guarded by opt-parity + torture). The
    // allocator's dominant costs were made ~linear (sparse `interference`, worklist
    // `color_abi`); this cap bounds the RESIDUAL super-linear paths (liveness fixpoint over
    // a deep CFG; the optimistic-spill max-degree scan on a genuinely DENSE graph) so no
    // fuzz-generated mega-function can CTIMEOUT. 60000 is >6× any plausible real function's
    // temp count yet well below the fuzz tail; above it we return the ALL-SPILL baseline
    // (the proven pre-Stage-5b memory model, sound by construction — same as inline-asm).
    let max_temps: usize =
        std::env::var("ZCC_MAXTEMPS").ok().and_then(|v| v.parse().ok()).unwrap_or(60000);
    if nt > max_temps {
        return home; // conservative all-spill — pathological function, keep compile bounded
    }
    let lv = liveness(f);
    let adj = interference(f, &lv);
    // crossing[t]: t ∈ live-out(call) \ {def(call)} for some call ⟹ its value must survive the bl.
    let mut crossing = vec![false; nt];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &u in &buf {
            live[u as usize] = true;
        }
        for i in b.insts.iter().rev() {
            // `live` == live-OUT(i) at this point (before the backward transfer)
            if matches!(i, Inst::Call(..) | Inst::CallX(..)) {
                let d = inst_def(i);
                for (t, &alive) in live.iter().enumerate() {
                    if alive && Some(t as u32) != d {
                        crossing[t] = true;
                    }
                }
            }
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &u in &buf {
                live[u as usize] = true;
            }
        }
    }
    let is_fp: Vec<bool> = f.temps.iter().map(|&ty| tt.is_float(ty)).collect();
    // PROBE (env-gated, zero-effect): realized x0-x7 targeting ceiling. For each Call,
    // a GP arg at index i<8 that is a NON-CROSSING Tmp could be homed at x{i} (kills the
    // marshal mov); a non-crossing result could be homed at x0 (kills the capture mov).
    // Also counts args that are the temp's SOLE use (cleanly targetable, no home conflict).
    if std::env::var("ZCC_PROBE").is_ok() {
        let mut usec = vec![0u32; nt];
        let mut ub = Vec::new();
        for b in &f.blocks {
            for i in &b.insts {
                ub.clear();
                inst_uses(i, &mut ub);
                for &u in &ub {
                    usec[u as usize] += 1;
                }
            }
            ub.clear();
            term_uses(&b.term, &mut ub);
            for &u in &ub {
                usec[u as usize] += 1;
            }
        }
        let (mut arg_tot, mut arg_nc, mut arg_sole, mut res_tot, mut res_nc) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Call(d, _, args, _) = i {
                    let mut gpi = 0u32;
                    for a in args {
                        let fp = matches!(a, Val::Tmp(t) if is_fp[*t as usize]);
                        if !fp {
                            if gpi < 8 {
                                arg_tot += 1;
                                if let Val::Tmp(t) = a {
                                    if !crossing[*t as usize] {
                                        arg_nc += 1;
                                        if usec[*t as usize] == 1 {
                                            arg_sole += 1;
                                        }
                                    }
                                }
                            }
                            gpi += 1;
                        }
                    }
                    if let Some(t) = d {
                        if !is_fp[*t as usize] {
                            res_tot += 1;
                            if !crossing[*t as usize] {
                                res_nc += 1;
                            }
                        }
                    }
                }
            }
        }
        eprintln!("PROBE arg_tot={arg_tot} arg_nc={arg_nc} arg_sole={arg_sole} res_tot={res_tot} res_nc={res_nc}");
    }
    // ── Conservative (Briggs) register COALESCING ──────────────────────────────
    // A Copy(d,s) with disjoint live ranges (d,s do NOT interfere) may share ONE
    // register — the copy then lowers to a self-move the peephole elides (this is
    // the home←home residency lever: ~22k reg-reg mov on sqlite3.c). We MERGE such
    // move-related temps into a representative and color the QUOTIENT interference
    // graph, but ONLY when the merge stays colorable — Briggs' test: the merged
    // node has < k neighbours of significant degree (an over-merge would spill BOTH
    // temps, worse than one copy). This strictly dominates the old biased-coloring,
    // which could merely PREFER a partner's color when free but never force it.
    //
    // CORRECTNESS (no new proof obligation). Merging two NON-interfering temps is a
    // sound rename — they are never simultaneously live, so one register holds both
    // and every read sees the right value. Transitive safety: a merge is refused
    // whenever the combined neighbourhood already contains the other group (found via
    // the same union-find), so two originally-interfering temps can NEVER end up
    // sharing a home. `verify_abi` re-derives interference on the ORIGINAL graph and
    // confirms every simultaneously-live pair still holds a DISTINCT home — the SAME
    // theorem as plain Stage-5b coloring. `coalesce=false` ⟹ every temp is its own
    // representative ⟹ byte-identical to the un-coalesced coloring.
    fn find(rep: &mut [Tmp], x: Tmp) -> Tmp {
        let mut r = x;
        while rep[r as usize] != r {
            r = rep[r as usize];
        }
        let mut c = x; // path-compress
        while rep[c as usize] != r {
            let n = rep[c as usize];
            rep[c as usize] = r;
            c = n;
        }
        r
    }
    let mut rep: Vec<Tmp> = (0..nt as u32).collect();
    if coalesce {
        // `madj[root]` = the root's interference neighbours, stored as ids that were
        // roots when inserted; every READ resolves them through `find` so staleness
        // from later merges is corrected lazily (keeping merges ~O(edges), not O(V²)).
        let mut madj: Vec<HashSet<Tmp>> = adj.clone();
        let mut pairs: Vec<(Tmp, Tmp)> = Vec::new();
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Copy(d, _, Val::Tmp(s)) = i
                    && d != s
                    && is_fp[*d as usize] == is_fp[*s as usize]
                    && !adj[*d as usize].contains(s)
                {
                    pairs.push((*d, *s)); // program order ⟹ deterministic merge sequence
                }
            }
        }
        for (d, s) in pairs {
            let (rd, rs) = (find(&mut rep, d), find(&mut rep, s));
            if rd == rs {
                continue; // already one group
            }
            let nbrs_rd: Vec<Tmp> = madj[rd as usize].iter().copied().collect();
            if nbrs_rd.iter().any(|&n| find(&mut rep, n) == rs) {
                continue; // a prior merge made the two groups interfere — never coalesce
            }
            // effective colour count: a value crossing a call is confined to the
            // callee-saved band, so its usable palette shrinks by ncaller.
            let crosses = crossing[rd as usize] || crossing[rs as usize];
            let (kk, ncall) = if is_fp[rd as usize] {
                (fp.k as usize, fp.ncaller as usize)
            } else {
                (gp.k as usize, gp.ncaller as usize)
            };
            let eff_k = kk - if crosses { ncall } else { 0 };
            // Briggs: merged neighbourhood (resolved to roots), count high-degree ones
            let nbrs_rs: Vec<Tmp> = madj[rs as usize].iter().copied().collect();
            let mut nb: HashSet<Tmp> = HashSet::new();
            for &n in nbrs_rd.iter().chain(nbrs_rs.iter()) {
                let rn = find(&mut rep, n);
                if rn != rd && rn != rs {
                    nb.insert(rn);
                }
            }
            let signif = nb.iter().filter(|&&n| madj[n as usize].len() >= eff_k).count();
            if eff_k == 0 || signif >= eff_k {
                continue; // merge risks an extra spill — keep the copy, safer
            }
            // commit: fold rs into rd, union adjacency, propagate the crossing flag
            rep[rs as usize] = rd;
            if crosses {
                crossing[rd as usize] = true;
            }
            for &n in &nbrs_rs {
                let rn = find(&mut rep, n);
                if rn != rd {
                    madj[rd as usize].insert(rn);
                    madj[rn as usize].insert(rd);
                }
            }
        }
    }
    // Quotient interference graph over representatives (color reps only).
    let mut qadj: Vec<HashSet<Tmp>> = vec![HashSet::new(); nt];
    for u in 0..nt {
        let ru = find(&mut rep, u as Tmp);
        let nbrs: Vec<Tmp> = adj[u].iter().copied().collect();
        for v in nbrs {
            let rv = find(&mut rep, v);
            if ru != rv {
                qadj[ru as usize].insert(rv);
            }
        }
    }
    // Residual move-bias among representatives: a Copy whose merge Briggs REFUSED can
    // still be nudged onto a shared colour when one is free (the old Phase-A behaviour).
    let mut move_adj: Vec<Vec<Tmp>> = vec![Vec::new(); nt];
    if coalesce {
        for b in &f.blocks {
            for i in &b.insts {
                if let Inst::Copy(d, _, Val::Tmp(s)) = i {
                    let (rd, rs) = (find(&mut rep, *d), find(&mut rep, *s));
                    if rd != rs && !qadj[rd as usize].contains(&rs) {
                        move_adj[rd as usize].push(rs);
                        move_adj[rs as usize].push(rd);
                    }
                }
            }
        }
    }
    // A PARAM temp (defined by Inst::Param) must avoid arg-register colors (see ClassBudget.narg).
    // Flag the coalescing REPRESENTATIVE: if any temp merged into a rep is a param, the rep's
    // home applies to it, so the rep inherits the exclusion.
    let mut is_param = vec![false; nt];
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Param(d, _) = i {
                let r = find(&mut rep, *d) as usize;
                is_param[r] = true;
            }
        }
    }
    // ABI TARGETING hints (Phase 2), per coalescing REP. Meaningful only when the caller
    // color i maps to arg register x{i} — i.e. WIDE GP (gp.ncaller>0); NARROW leaves it empty
    // (caller colors are x19–x28, no arg-register match) so its coloring stays byte-identical.
    // A GP arg at position i<ncaller → prefer color i; a GP call result → prefer color 0. Only
    // for NON-crossing (caller-band-eligible), NON-param (arg-register-barred) temps; first-set
    // wins for determinism. CallX arg positions depend on a struct/stack plan, so only its
    // RESULT (always x0) is targeted here; simple-Call args get full position targeting.
    let mut target: Vec<Option<u32>> = vec![None; nt];
    if gp.ncaller > 0 {
        let mut set_tgt = |t: Tmp, c: u32, rep: &mut [Tmp]| {
            if !is_fp[t as usize] && !crossing[t as usize] {
                let r = find(rep, t) as usize;
                if !is_param[r] && target[r].is_none() {
                    target[r] = Some(c);
                }
            }
        };
        for b in &f.blocks {
            for i in &b.insts {
                match i {
                    Inst::Call(d, _, args, _) => {
                        let mut gpi = 0u32;
                        for a in args {
                            let fparg = match a {
                                Val::FImm(_) => true,
                                Val::Imm(_) => false,
                                Val::Tmp(t) => is_fp[*t as usize],
                            };
                            if !fparg {
                                if gpi < gp.ncaller && let Val::Tmp(t) = a {
                                    set_tgt(*t, gpi, &mut rep);
                                }
                                gpi += 1;
                            }
                        }
                        if let Some(t) = d {
                            set_tgt(*t, 0, &mut rep);
                        }
                    }
                    Inst::CallX(Some(t), _, _, _, _) => set_tgt(*t, 0, &mut rep),
                    _ => {}
                }
            }
        }
    }
    // SPILL COST per coloring rep = static use+def count of every temp in its group. This is
    // the reload/spill-store traffic a real spill of the rep would emit; color_abi minimises
    // cost/degree at the optimistic-spill step so the cheapest temps go to memory. Loop-depth
    // weighting (dynamic hotness) is a separate SPEED sub-lever; this unweighted count is the
    // exact static-`.s` reload proxy (SIZE arm). A def counts once, each use once.
    let mut cost = vec![0u32; nt];
    let mut cb = Vec::new();
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                cost[find(&mut rep, d) as usize] += 1;
            }
            cb.clear();
            inst_uses(i, &mut cb);
            for &u in &cb {
                cost[find(&mut rep, u) as usize] += 1;
            }
        }
        cb.clear();
        term_uses(&b.term, &mut cb);
        for &u in &cb {
            cost[find(&mut rep, u) as usize] += 1;
        }
    }
    let gp_in: Vec<bool> = (0..nt).map(|t| !is_fp[t] && find(&mut rep, t as Tmp) == t as Tmp).collect();
    let fp_in: Vec<bool> = (0..nt).map(|t| is_fp[t] && find(&mut rep, t as Tmp) == t as Tmp).collect();
    let gc = color_abi(&qadj, &gp_in, gp, &crossing, &move_adj, &is_param, &target, &cost);
    let fc = color_abi(&qadj, &fp_in, fp, &crossing, &move_adj, &is_param, &target, &cost);
    for t in 0..nt {
        let r = find(&mut rep, t as Tmp) as usize;
        home[t] = if is_fp[t] { fc[r] } else { gc[r] }.map(|c| (is_fp[t], c));
    }
    home
}


/// Mechanically CHECK the Stage-5b obligations (the P-verify): (1) the interference
/// invariant per class — two same-class simultaneously-live temps get distinct homes;
/// (2) call-clobber — no call-crossing temp received a caller-saved color.
/// Test-only: the theorem is checked over a corpus in `tests`, not on every compile.
#[cfg(test)]
pub fn verify_abi(
    tt: &TyTab,
    f: &IrFunc,
    home: &[AbiHome],
    gp: &ClassBudget,
    fp: &ClassBudget,
) -> Result<(), String> {
    let lv = liveness(f);
    let adj = interference(f, &lv);
    for u in 0..adj.len() {
        if let Some((fu, cu)) = home[u] {
            for &v in &adj[u] {
                if let Some((fv, cv)) = home[v as usize]
                    && fu == fv
                    && cu == cv
                {
                    return Err(format!("interference (t{u},t{v}) share {}-reg {cu}", if fu { "fp" } else { "gp" }));
                }
            }
        }
    }
    // recompute crossing and check no caller-saved home for a crossing temp
    let mut crossing = vec![false; f.temps.len()];
    let mut buf = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut live = lv.live_out[bi].clone();
        buf.clear();
        term_uses(&b.term, &mut buf);
        for &x in &buf {
            live[x as usize] = true;
        }
        for i in b.insts.iter().rev() {
            if matches!(i, Inst::Call(..) | Inst::CallX(..)) {
                let d = inst_def(i);
                for (t, &al) in live.iter().enumerate() {
                    if al && Some(t as u32) != d {
                        crossing[t] = true;
                    }
                }
            }
            if let Some(d) = inst_def(i) {
                live[d as usize] = false;
            }
            buf.clear();
            inst_uses(i, &mut buf);
            for &x in &buf {
                live[x as usize] = true;
            }
        }
    }
    let _ = tt;
    for (t, &h) in home.iter().enumerate() {
        if let Some((is_fp, c)) = h {
            let ncaller = if is_fp { fp.ncaller } else { gp.ncaller };
            if crossing[t] && c < ncaller {
                return Err(format!("call-crossing t{t} got caller-saved {}-color {c}", if is_fp { "fp" } else { "gp" }));
            }
        }
    }
    Ok(())
}

