// src/opt/inline.rs — interprocedural — the inliner and dead-static-function elimination.
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

// A scalar type = one that lives in a single register/slot (int / float / pointer). A
// scalar param is seeded by exactly ONE `Store(pty, &slot, arg)` — the same instruction
// the prologue uses to spill the incoming arg register — so its β-substitution is
// provably the ABI. An AGGREGATE (struct/union/array) param is passed byval/indirect
// and copied by Memcpy, NOT a scalar store; seeding it with one Store miscompiles.
pub(crate) fn scalar_ty(tt: &TyTab, ty: TypeId) -> bool {
    tt.is_integer(ty) || tt.is_float(ty) || matches!(tt.tys[ty as usize], Ty::Ptr(_))
}


// May a callee be inlined? Whitelist the instruction kinds whose relocation is a pure
// temp/frame shift. Rejects va*, alloca/VLA, computed goto (and any callee carrying
// label symbols), inline asm, atomics, overflow builtins, and CallX (its sret slot is
// a second frame offset) — each would need identity-/state-aware fix-up we don't do.
// Also requires every PARAMETER to be scalar and the RETURN to be scalar-or-void: an
// aggregate param/return uses byval/sret ABI copying the scalar store-seed can't model
// (the struct-by-value miscompiles: gcc 20000706-1 / pr20621-1).
pub(crate) fn inline_ok(tt: &TyTab, callee: &IrFunc) -> bool {
    callee.labels.is_empty()
        && callee.params.iter().all(|&(_, pty)| scalar_ty(tt, pty))
        // A SUB-WORD (u8/u16/…) param must not be inlined: the splice turns it into a spliced
        // local written by Store(arg); to_ssa then promotes that local to a register, but the
        // sub-word wrap-on-back-edge canonicalization (the pr81913 case) is proven only for a
        // function's OWN sub-word params (to_ssa excludes them from promotion) — a spliced one
        // is not in the caller's params list, so it would promote and mis-wrap. Reject here.
        && callee.params.iter().all(|&(_, pty)| tt.size(pty) >= 4)
        && (callee.ret == VOID || scalar_ty(tt, callee.ret))
        && callee.blocks.iter().flat_map(|b| &b.insts).all(|i| {
            matches!(
                i,
                Inst::Bin(..)
                    | Inst::Un(..)
                    | Inst::Copy(..)
                    | Inst::Load(..)
                    | Inst::Store(..)
                    | Inst::Memcpy(..)
                    | Inst::Lea(..)
                    | Inst::Cast(..)
                    | Inst::Call(..)
                    | Inst::Zero(..)
                    | Inst::FunAddr(..)
            )
        })
}


pub(crate) fn inst_count(f: &IrFunc) -> usize {
    f.blocks.iter().map(|b| b.insts.len()).sum()
}


// Splice `callee` into `caller` at block `b`, instruction `k` (a direct Call). The
// call site block is split: [0..k] + param-seeding stores → jump to the clone entry;
// the clone's returns write the call's dst and jump to a continuation block holding
// [k+1..] + the block's original terminator.
pub(crate) fn splice(caller: &mut IrFunc, b: BlockId, k: usize, callee: &IrFunc) {
    let tb = caller.temps.len() as Tmp; // clone temps land here
    let fb = caller.frame; // clone frame appended below the caller's
    let bb = caller.blocks.len() as BlockId; // clone entry = first appended block
    let nk = callee.blocks.len() as BlockId;
    let cont = bb + nk; // continuation = appended just after the clone blocks

    let (dst, args) = match &caller.blocks[b as usize].insts[k] {
        Inst::Call(d, Callee::Sym(_), a, _) => (*d, a.clone()),
        _ => unreachable!("splice: not a direct call"),
    };
    let ret_ty = callee.ret;

    // 1. Grow the caller's Γ + frame by the callee's (the frame-append law above).
    caller.temps.extend_from_slice(&callee.temps);
    caller.frame += callee.frame;

    // 2. Clone + relocate the callee blocks; Ret v → (dst := v) ; goto cont.
    for blk in &callee.blocks {
        let mut nb = blk.clone();
        for i in nb.insts.iter_mut() {
            relocate_inst(i, tb, fb);
        }
        relocate_term(&mut nb.term, tb, bb);
        if let Term::Ret(v) = &nb.term {
            let v = *v; // Option<Val> is Copy; already relocated by relocate_term
            if let Some(d) = dst {
                // void-return path in a value fn is unreachable → Imm(0) keeps dst defined.
                let rv = v.unwrap_or(Val::Imm(0));
                nb.insts.push(Inst::Copy(d, ret_ty, rv));
            }
            nb.term = Term::Jmp(cont);
        }
        caller.blocks.push(nb);
    }

    // 3. Detach the tail (everything AFTER the call) + the block's old terminator.
    let tail = caller.blocks[b as usize].insts.split_off(k + 1);
    let old_term = std::mem::replace(&mut caller.blocks[b as usize].term, Term::Jmp(bb));
    caller.blocks[b as usize].insts.pop(); // drop the Call now at index k

    // 4. Seed the callee params: Store arg → the relocated param slot (off + fb).
    for (idx, &(poff, pty)) in callee.params.iter().enumerate() {
        let arg = args.get(idx).copied().unwrap_or(Val::Imm(0));
        let addr = caller.temps.len() as Tmp;
        caller.temps.push(ULONG); // a local-address temp
        caller.blocks[b as usize].insts.push(Inst::Lea(addr, Place::Local(poff + fb)));
        caller.blocks[b as usize].insts.push(Inst::Store(pty, Val::Tmp(addr), arg));
    }

    caller.blocks.push(Block { insts: tail, term: old_term });
}


/// Whole-program inlining (Tier-1 #5). Depth-1: expands only the call sites present in
/// each caller's ORIGINAL blocks, against a SNAPSHOT taken before any mutation — so a
/// non-recursive callee is substituted once and a self-recursive callee is unrolled
/// exactly one level (its clone's recursive calls survive). Terminates trivially.
///
/// `caller_ok[ci]` = may func ci be inlined INTO (false for variadic / VLA callers): the
/// splice appends the callee frame into [caller.frame, caller.frame+cf), which for a
/// variadic caller is exactly the AAPCS64 reg-save area and for a VLA caller confuses the
/// dynamic-SP reset base — both need va/VLA-aware placement this pass does not do. Callee
/// eligibility (scalar-only, no va/alloca/asm/…) is separately gated by `inline_ok`.
/// `caller_ok[ci]` gates who may be inlined INTO; `callee_ok[gi]` gates who may be inlined
/// (splicing a volatile callee's body into an optimized caller would subject its volatile
/// accesses to opt — C99 6.7.3 — so a volatile function is never a callee here).
/// Inlining policy (thresholds). POLICY is separated from MECHANISM: the pass reads a cfg;
/// the caller (emit_ir) supplies `from_env()`, tests supply `exercise_all()` to exize every
/// edge. Article-E convenience thresholds (NOT spec constants — policy, dated 2026-08).
#[derive(Clone, Copy)]
pub struct InlineCfg {
    pub leaf: usize,   // max body of a MULTI-call non-recursive callee to inline
    pub self_: usize,  // max body of a self-recursive callee (depth-1 unroll per round)
    pub single: usize, // max body of a SOLE-call-site callee (inline + DFE — pure win)
    pub rounds: usize, // fixpoint rounds: a call chain A→B→C inlines one edge per round
}

impl InlineCfg {
    // MEASURED on sqlite (the O1 size gap): the two edges behave OPPOSITELY for SIZE —
    //  • SINGLE-call callees are a pure win — the body is not duplicated (one site), and DFE
    //    then deletes the standalone copy, so inlining reclaims prologue+epilogue+bl+marshalling
    //    for free. Inline these regardless of size (single large). ✓ −18k on sqlite.
    //  • MULTI-call inlining currently REGRESSES size (each site duplicates the body, and the
    //    copies carry the eager-sxtw int-value-contract bloat — ~21k sxtw on sqlite — that the
    //    post-inline SSA cleanup cannot reclaim). So leaf defaults 0 (off). This flips to a net
    //    win — as it is for gcc -O1 — once the int value contract stops re-extending after every
    //    op (OPT.md: the sxtw lever); revisit leaf then.
    // All guarded by opt-parity + torture, so a wrong value costs SIZE, never correctness.
    pub fn from_env() -> Self {
        let e = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        InlineCfg { leaf: e("ZCC_LEAF", 0), self_: e("ZCC_SELF", 0), single: e("ZCC_SINGLE", 400), rounds: e("ZCC_ROUNDS", 4) }
    }
    /// Test config — non-zero on every edge so the commuting-square tests exercise leaf,
    /// self-recursive, and single-call inlining regardless of the shipped size policy.
    #[cfg(test)]
    pub fn exercise_all() -> Self {
        InlineCfg { leaf: 40, self_: 40, single: 400, rounds: 4 }
    }
}


pub fn inline(tt: &TyTab, funcs: &mut [IrFunc], caller_ok: &[bool], callee_ok: &[bool], cfg: &InlineCfg) -> u32 {
    let (leaf_max, self_max, single_max, max_rounds) = (cfg.leaf, cfg.self_, cfg.single, cfg.rounds);
    let mut total = 0u32;
    for _round in 0..max_rounds {
        let snapshot: Vec<IrFunc> = funcs.to_vec();
        let by_name: HashMap<&str, usize> =
            snapshot.iter().enumerate().map(|(i, f)| (f.name.as_str(), i)).collect();
        // Global direct-call count per callee (on this round's state): a callee reached by
        // exactly one Call site is a duplication-free inline (DFE reclaims its body).
        let mut callcnt: Vec<u32> = vec![0; snapshot.len()];
        for f in &snapshot {
            for b in &f.blocks {
                for inst in &b.insts {
                    if let Inst::Call(_, Callee::Sym(name), ..) = inst {
                        if let Some(&gi) = by_name.get(name.as_str()) {
                            callcnt[gi] += 1;
                        }
                    }
                }
            }
        }
        let mut round_splices = 0u32;
        for ci in 0..funcs.len() {
            if !caller_ok.get(ci).copied().unwrap_or(true) {
                continue;
            }
            let start_nc = funcs[ci].blocks.len();
            let mut sites: Vec<(BlockId, usize, usize)> = Vec::new();
            for b in 0..start_nc {
                for (k, inst) in funcs[ci].blocks[b].insts.iter().enumerate() {
                    if let Inst::Call(_, Callee::Sym(name), _, _) = inst {
                        let Some(&gi) = by_name.get(name.as_str()) else { continue };
                        if !callee_ok.get(gi).copied().unwrap_or(true) {
                            continue; // volatile callee — never spliced into optimized code
                        }
                        let g = &snapshot[gi];
                        if !inline_ok(tt, g) {
                            continue;
                        }
                        let n = inst_count(g);
                        let ok = if gi == ci {
                            n <= self_max
                        } else if callcnt[gi] == 1 {
                            n <= single_max // sole call site → inline + DFE, no duplication
                        } else {
                            n <= leaf_max
                        };
                        if ok {
                            sites.push((b as BlockId, k, gi));
                        }
                    }
                }
            }
            // Per block, splice HIGHER indices first: lower call sites keep their (b,k)
            // valid (split_off only detaches the tail above k), so the whole original
            // worklist can be applied without recomputation.
            sites.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
            for (b, k, gi) in sites {
                splice(&mut funcs[ci], b, k, &snapshot[gi]);
                total += 1;
                round_splices += 1;
            }
        }
        if round_splices == 0 {
            break;
        }
    }
    total
}


/// Dead-function elimination (interprocedural, runs AFTER inline). A function with
/// INTERNAL linkage (`is_static`) that no longer appears as a call target or has its
/// address taken anywhere reachable is unreferenced ⟹ its standalone body is dead code
/// and need not be emitted (this is exactly gcc's remove-unused-static, the other half
/// of why -O1 emits fewer functions than we do). Reachability roots: every EXTERNALLY
/// visible function (non-static — a translation unit outside can call it) plus every
/// symbol NAMED by a global initializer (a function-pointer table — sqlite's vtab/opcode
/// methods). From the roots we follow Call/CallX `Callee::Sym` and `FunAddr` edges to a
/// fixpoint; any static function not reached is dead. Correctness: a deleted function is
/// provably unreachable from any entry the linker can see, so ⟦program⟧ is unchanged.
pub fn dead_static_fns(funcs: &[IrFunc], is_static: &[bool], root_syms: &HashSet<String>) -> Vec<bool> {
    let idx: HashMap<&str, usize> =
        funcs.iter().enumerate().map(|(i, f)| (f.name.as_str(), i)).collect();
    let mut reach = vec![false; funcs.len()];
    let mut work: Vec<usize> = Vec::new();
    for (i, f) in funcs.iter().enumerate() {
        if !is_static.get(i).copied().unwrap_or(true) || root_syms.contains(&f.name) {
            if !reach[i] {
                reach[i] = true;
                work.push(i);
            }
        }
    }
    let mark = |name: &str, reach: &mut Vec<bool>, work: &mut Vec<usize>| {
        if let Some(&j) = idx.get(name) {
            if !reach[j] {
                reach[j] = true;
                work.push(j);
            }
        }
    };
    while let Some(i) = work.pop() {
        for b in &funcs[i].blocks {
            for inst in &b.insts {
                match inst {
                    Inst::Call(_, Callee::Sym(n), ..) | Inst::CallX(_, Callee::Sym(n), ..) => {
                        mark(n, &mut reach, &mut work)
                    }
                    Inst::FunAddr(_, n) => mark(n, &mut reach, &mut work),
                    _ => {}
                }
            }
        }
    }
    (0..funcs.len()).map(|i| is_static.get(i).copied().unwrap_or(false) && !reach[i]).collect()
}
