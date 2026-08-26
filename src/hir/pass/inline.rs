// inline (REARCH §4 row 7) — β-reduction on the call graph.
// THEORY A7b — optimization: this pass ships its commuting square
//
// POLICY, dated and cited rather than tuned. gcc -O1 enables
// `-finline-functions-called-once` and nothing else: a function with exactly one
// call site and internal linkage disappears into that site at no size cost, and
// that is the whole of inlining at this level. A second, narrower rule is added
// here — a body of at most `TINY` instructions — because the call sequence itself
// (argument marshalling, `bl`, the result copy) is already several instructions,
// so a body smaller than the call cannot make the program larger. Anything more
// aggressive is `-O2`'s `-finline-small-functions` and belongs on the §16 shelf.
//
// COMMUTING SQUARE. Substituting a call by the callee's body preserves ⟦f⟧
// because HIR is already SSA: the callee's parameters are VALUES, so binding them
// to the argument operands is capture-free renaming, and the callee's locals are
// its own stack slots, which are appended to the caller rather than merged. The
// return becomes a jump to the continuation block, whose parameter is the call's
// result — exactly the meaning ⟦hir⟧ gives `Term::Ret` inside a call frame.
//
// What is REFUSED, each for a reason that is a property of the IR and not a
// heuristic: a variadic callee (its `va_list` is built from a frame layout that
// only exists as a real call), a callee that takes a block address or performs a
// computed goto (block identity is observable), one with a VLA or `alloca` (the
// stack pointer moves inside the body), one that returns a composite (the sret
// convention is a property of a real call), and any function in a cycle of the
// call graph (β-reduction on a recursive term does not terminate).
use super::*;

use std::collections::{HashMap, HashSet};

/// The size of the CALL SEQUENCE a body would replace: one instruction to place
/// each argument, the `bl` itself, and one to take the result. A body no larger
/// than that cannot make the program bigger, which is why this bound is derived
/// from the ABI rather than picked — there is no threshold to tune.
fn call_cost(sig: &Sig) -> usize {
    sig.params.len() + 2
}

/// THEORY A7b — a fixpoint bound: termination insurance, not a policy
/// Rounds of inlining. A callee's own calls become the caller's after the first
/// round; two rounds cover the ordinary "wrapper around a wrapper" and stop the
/// growth from compounding.
const ROUNDS: u32 = 2;

/// Blocks of `f` that lie in some loop.
fn loop_blocks(f: &Func) -> Vec<bool> {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let mut v = vec![false; f.blocks.len()];
    for lp in &lf.loops {
        v[lp.header as usize] = true;
        for &b in &lp.body {
            v[b as usize] = true;
        }
    }
    v
}

/// Does `f` contain a loop of its own?
fn has_loop(f: &Func) -> bool {
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    !dom::loops(&c, &dt).loops.is_empty()
}

pub fn run_module(m: &mut Module, pinned: &HashSet<String>) -> bool {
    let mut any = false;
    for _ in 0..ROUNDS {
        let mut changed = false;
        let cyclic = cyclic_functions(m);
        let counts = call_counts(m);
        let by_name: HashMap<String, usize> =
            m.funcs.iter().enumerate().map(|(i, f)| (f.name.clone(), i)).collect();
        for ci in 0..m.funcs.len() {
            loop {
                let inloop = loop_blocks(&m.funcs[ci]);
                let site = m.funcs[ci].blocks.iter().enumerate().find_map(|(b, blk)| {
                    blk.insts.iter().enumerate().find_map(|(i, inst)| match inst {
                        Inst::Call { callee: Callee::Direct(n), sret: None, args, .. } => {
                            let gi = *by_name.get(n)?;
                            if !args_match(&m.funcs[ci], &m.funcs[gi], args) {
                                return None;
                            }
                            let g = &m.funcs[gi];
                            // `is_static` is NOT a conflation with the dead-body
                            // sweep — it is the whole economics of the rule, and
                            // R4.14 (3) proposed dropping it on a premise the
                            // measurement refuted. A callee with EXTERNAL linkage
                            // can be called from another translation unit, so its
                            // out-of-line body can never be deleted: inlining it
                            // duplicates the body permanently. Measured over the
                            // 35-program suite, dropping the requirement bought
                            // EXEC 1.1627 → 1.0841 and cost INSN 1.1493 → 1.3326
                            // — a 16% size regression for a 7% speed one, which
                            // fails THE ULTIMATUM's "both axes" outright. gcc's
                            // own `-finline-functions-called-once` says "all
                            // STATIC functions called once" for exactly this
                            // reason; §13n read e2's inlined `mix` as evidence of
                            // a rule gcc does not have.
                            // THE EXTERNAL HALF, narrowed twice until it paid.
                            //
                            // Dropping `is_static` outright is refuted above: 16%
                            // size for 7% speed. Narrowing it to call sites
                            // INSIDE A LOOP fixed the size (sqlite moved +2
                            // instructions) and then lost on SPEED anyway —
                            // h2_revbits went 43ms to 61ms, because `revbits`
                            // CONTAINS A LOOP and splicing one loop into another
                            // changes what the allocator must hold across the
                            // inner one. So the fence is the shape that
                            // distinguishes the two cases: a STRAIGHT-LINE body.
                            // `mix` is ten multiplies and adds with no control
                            // flow, and e2_many_args pays its ten argument moves
                            // and two stack stores four million times.
                            //
                            // NO SIZE CAP, deliberately. The first cut carried a
                            // `HOT_EXTERNAL_BODY = 48` and `provenance.sh`
                            // rejected it — correctly, since 48 was a number this
                            // author picked, and Article E asks of every constant
                            // whether it is the spec's or the author's. The
                            // fences here are all STRUCTURAL — called once, from
                            // inside a loop, and no loop of its own — so the rule
                            // needs no threshold to tune and none is invented.
                            // "Called once and loop-free" already bounds what can
                            // be duplicated: sqlite moves by 2 instructions.
                            let once = counts.get(n).copied().unwrap_or(0) == 1;
                            let called_once = once
                                && (g.is_static
                                    || (inloop.get(b).copied().unwrap_or(false)
                                        && !has_loop(g)));
                            let want = called_once || body_size(g) <= call_cost(&g.sig);
                            if gi != ci && want && !cyclic.contains(&gi) && inlinable(g) {
                                Some((b, i, gi))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    })
                });
                match site {
                    Some((b, i, gi)) => {
                        let g = m.funcs[gi].clone();
                        splice(&mut m.funcs[ci], b, i, &g);
                        changed = true;
                    }
                    None => break,
                }
            }
        }
        any |= changed;
        if !changed {
            break;
        }
    }
    // A static function whose every call site was substituted is now DEAD, and
    // leaving it emitted turns "inline the callee that is called once" from a
    // size-neutral rewrite into a pure size loss — sqlite grew 25% before this
    // existed. `pinned` names the functions a global initializer refers to,
    // which HIR cannot see and the linker very much can.
    any |= drop_unreferenced(m, pinned);
    any
}

fn drop_unreferenced(m: &mut Module, pinned: &HashSet<String>) -> bool {
    let mut changed = true;
    let mut any = false;
    while changed {
        changed = false;
        let counts = call_counts(m);
        let dead: Vec<usize> = m
            .funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.is_static
                    && f.name != "main"
                    && !pinned.contains(&f.name)
                    && counts.get(&f.name).copied().unwrap_or(0) == 0
            })
            .map(|(i, _)| i)
            .collect();
        for i in dead.into_iter().rev() {
            m.funcs.remove(i);
            changed = true;
            any = true;
        }
    }
    any
}

/// The call site must agree with the callee's declaration EXACTLY, because
/// binding a parameter to an argument is a renaming and a renaming performs no
/// conversion. C99 6.5.2.2p6 leaves a call through a mismatched declaration
/// undefined, but "undefined" is not "may be miscompiled into ill-typed IR" —
/// the verifier rejects an I64 value in an I32 operand, and it is right to.
/// (torture `loop-2d`, `pr36321`.)
fn args_match(caller: &Func, g: &Func, args: &[Operand]) -> bool {
    if args.len() != g.sig.params.len() {
        return false;
    }
    args.iter().zip(&g.sig.params).all(|(a, p)| match (a, p) {
        (Operand::Val(v), PTy::S(t)) => caller.ty_of(*v) == *t,
        (Operand::Val(_), PTy::LDouble) => true,
        (Operand::Imm(_), PTy::S(t)) => !t.is_float(),
        (Operand::Fimm(_), PTy::S(t)) => t.is_float(),
        _ => false,
    })
}

fn body_size(f: &Func) -> usize {
    f.blocks.iter().map(|b| b.insts.len()).sum()
}

fn call_counts(m: &Module) -> HashMap<String, usize> {
    let mut c: HashMap<String, usize> = HashMap::new();
    for f in &m.funcs {
        for b in &f.blocks {
            for inst in &b.insts {
                // A function whose ADDRESS is taken may be called from anywhere,
                // so the count is only a bound when every reference is a direct
                // call. `SymAddr(Func)` therefore counts as a use too.
                match inst {
                    Inst::Call { callee: Callee::Direct(n), .. } => {
                        *c.entry(n.clone()).or_insert(0) += 1;
                    }
                    Inst::SymAddr { sym: Sym::Func(n), .. } => {
                        *c.entry(n.clone()).or_insert(0) += 2;
                    }
                    _ => {}
                }
            }
        }
    }
    c
}

/// Functions that lie on a cycle of the direct call graph, including self-calls.
/// β-reduction on those does not terminate, so they are simply not candidates.
fn cyclic_functions(m: &Module) -> HashSet<usize> {
    let idx: HashMap<&str, usize> =
        m.funcs.iter().enumerate().map(|(i, f)| (f.name.as_str(), i)).collect();
    let n = m.funcs.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, f) in m.funcs.iter().enumerate() {
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::Call { callee: Callee::Direct(name), .. } = inst {
                    if let Some(&j) = idx.get(name.as_str()) {
                        adj[i].push(j);
                    }
                }
            }
        }
    }
    // reachability closure: i is cyclic iff i reaches i
    let mut out = HashSet::new();
    for s in 0..n {
        let mut seen = vec![false; n];
        let mut work = adj[s].clone();
        while let Some(x) = work.pop() {
            if x == s {
                out.insert(s);
                break;
            }
            if !seen[x] {
                seen[x] = true;
                work.extend(adj[x].iter().copied());
            }
        }
    }
    out
}

fn inlinable(g: &Func) -> bool {
    if g.sig.variadic || g.has_vla {
        return false;
    }
    if matches!(g.sig.ret, Some(PTy::Agg { .. })) {
        return false;
    }
    for b in &g.blocks {
        if !b.labels.is_empty() || matches!(b.term, Term::GotoPtr(..)) {
            return false;
        }
        for inst in &b.insts {
            match inst {
                Inst::SymAddr { sym: Sym::Label(_), .. } => return false,
                Inst::Alloca { .. } => return false,
                Inst::Intrinsic { kind, .. } => {
                    if matches!(
                        kind,
                        IntrinKind::VaStart
                            | IntrinKind::VaArg(_)
                            | IntrinKind::VaArea
                            | IntrinKind::Asm { .. }
                    ) {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// Replace `f.blocks[b].insts[i]` — a call to `g` — by `g`'s body.
fn splice(f: &mut Func, b: usize, i: usize, g: &Func) {
    let (args, dst) = match &f.blocks[b].insts[i] {
        Inst::Call { args, dst, .. } => (args.clone(), *dst),
        _ => unreachable!("splice called on a non-call"),
    };
    // (1) the continuation: everything after the call, plus the block's own
    //     terminator, moves into a fresh block whose parameter is the result
    let cont = f.new_block();
    f.blocks[cont as usize].weight = f.blocks[b].weight;
    let rest: Vec<Inst> = f.blocks[b].insts.split_off(i + 1);
    f.blocks[cont as usize].insts = rest;
    f.blocks[cont as usize].term = std::mem::replace(&mut f.blocks[b].term, Term::Unreachable);
    f.blocks[cont as usize].labels = std::mem::take(&mut f.blocks[b].labels);
    f.blocks[b].insts.pop(); // the call itself
    let ret_param = dst.map(|d| {
        let ty = f.ty_of(d);
        let p = f.new_value(ty, Def::Param(cont, 0));
        f.blocks[cont as usize].params.insert(0, p);
        (d, p)
    });

    // (2) clone the callee's slots and values
    let slot0 = f.slots.len() as u32;
    f.slots.extend(g.slots.iter().copied());
    let mut vmap: Vec<Operand> = Vec::with_capacity(g.values.len());
    for (v, info) in g.values.iter().enumerate() {
        match info.def {
            Def::FuncParam(k) => vmap.push(args[k as usize]),
            _ => {
                let n = f.new_value(info.ty, Def::Inst(0, 0));
                let _ = v;
                vmap.push(Operand::Val(n));
            }
        }
    }
    let blk0 = f.blocks.len() as BlockId;
    let sub = |o: Operand| -> Operand {
        match o {
            Operand::Val(v) => vmap[v as usize],
            k => k,
        }
    };
    let newv = |o: Operand| -> ValueId {
        match o {
            Operand::Val(v) => v,
            _ => unreachable!("a callee definition must map to a value"),
        }
    };

    // (3) clone the blocks
    for gb in &g.blocks {
        let nb = f.new_block();
        f.blocks[nb as usize].weight = gb.weight;
        let mut insts = Vec::with_capacity(gb.insts.len());
        for inst in &gb.insts {
            let mut c = inst.clone();
            c.uses_mut(|o| *o = sub(*o));
            match &mut c {
                Inst::Bin { dst, .. }
                | Inst::Un { dst, .. }
                | Inst::Cmp { dst, .. }
                | Inst::Cvt { dst, .. }
                | Inst::Load { dst, .. }
                | Inst::SlotAddr { dst, .. }
                | Inst::SymAddr { dst, .. }
                | Inst::Select { dst, .. }
                | Inst::Alloca { dst, .. } => *dst = newv(vmap[*dst as usize]),
                Inst::Call { dst, .. } | Inst::Intrinsic { dst, .. } => {
                    if let Some(d) = dst {
                        *d = newv(vmap[*d as usize]);
                    }
                }
                Inst::Store { .. } | Inst::MemCpy { .. } | Inst::MemSet { .. } => {}
            }
            if let Inst::SlotAddr { slot, .. } = &mut c {
                *slot += slot0;
            }
            insts.push(c);
        }
        f.blocks[nb as usize].insts = insts;
        let mut params = Vec::with_capacity(gb.params.len());
        for p in &gb.params {
            params.push(newv(vmap[*p as usize]));
        }
        f.blocks[nb as usize].params = params;
        let mut term = gb.term.clone();
        match &mut term {
            Term::Br(c, ..) | Term::Switch(c, ..) => *c = sub(*c),
            Term::Ret(Some(v)) => *v = sub(*v),
            _ => {}
        }
        for t in term.targets_mut() {
            t.block += blk0;
            for a in t.args.iter_mut() {
                *a = sub(*a);
            }
        }
        // a return leaves the inlined body through the continuation
        term = match term {
            Term::Ret(v) => Term::Jmp(Target {
                block: cont,
                args: match (v, &ret_param) {
                    (Some(x), Some(_)) => vec![x],
                    (None, Some((_, _))) => vec![Operand::Imm(0)],
                    _ => Vec::new(),
                },
            }),
            other => other,
        };
        f.blocks[nb as usize].term = term;
    }

    // (4) enter the body, and rewire the call's result to the continuation's
    //     parameter
    f.blocks[b].term = Term::Jmp(Target {
        block: blk0 + g.entry,
        args: Vec::new(),
    });
    if let Some((d, p)) = ret_param {
        let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
        map[d as usize] = Some(Operand::Val(p));
        rewrite_values(f, &map);
    }
    refresh_defs(f);
}
