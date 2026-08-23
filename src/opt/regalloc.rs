// src/opt/regalloc.rs — register allocation — liveness, interference, coloring, ABI homing.
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

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
) -> Vec<Option<u32>> {
    let nt = adj.len();
    let k = b.k as usize;
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
            // no degree<k node remains → OPTIMISTIC SPILL: pick the max class-degree node
            // (the same fallback as before; rare on the sparse graphs that stress compile
            // time, so its O(remaining) scan does not reintroduce the quadratic there).
            match (0..nt).filter(|&v| in_class[v] && !removed[v]).max_by_key(|&v| degree[v]) {
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
        let free = |c: u32| c >= lo && c < b.k && !used[c as usize];
        // biased coalescing: prefer a free, in-range color already held by a same-class
        // move partner (the copy becomes a self-move). Falls back to the smallest free
        // color — so the result is always a valid coloring regardless of the bias.
        let biased = bias[v as usize]
            .iter()
            .filter(|&&p| in_class[p as usize])
            .filter_map(|&p| colr[p as usize])
            .find(|&c| free(c));
        colr[v as usize] = biased.or_else(|| (lo..b.k).find(|&c| free(c)));
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
    let gp_in: Vec<bool> = (0..nt).map(|t| !is_fp[t] && find(&mut rep, t as Tmp) == t as Tmp).collect();
    let fp_in: Vec<bool> = (0..nt).map(|t| is_fp[t] && find(&mut rep, t as Tmp) == t as Tmp).collect();
    let gc = color_abi(&qadj, &gp_in, gp, &crossing, &move_adj);
    let fc = color_abi(&qadj, &fp_in, fp, &crossing, &move_adj);
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

