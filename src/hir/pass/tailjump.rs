// tailjump — the dispatch a state machine re-enters, copied into the state it came from.
// THEORY A7b — optimization: this pass ships its commuting square
//
// WHAT IT IS FOR. A `switch` inside a loop — a parser's state machine, a bytecode
// interpreter's opcode dispatch — compiles to ONE dispatch that every arm jumps
// back to. Every arm therefore re-executes the whole sequence: reload the state,
// bound-check it, index the jump table, `br`. gcc duplicates that sequence into
// the tail of each arm, so an arm branches STRAIGHT to the next state's code:
//
//     zcc                                  gcc -O2
//     .arm_A: …; b .dispatch                .arm_A: …; ldrb; cmp; b.hi …; br  ← its own
//     .arm_B: …; b .dispatch                .arm_B: …; ldrb; cmp; b.hi …; br  ← its own
//     .dispatch: ldrb; cmp; …; br
//
// `MEASURED M52` priced it on `m2_http_parse`, an nginx-shaped request parser at
// 2.14x against `gcc -O2`, with hardware counters rather than a guess: zcc
// executes **1.95x the instructions** and 2.79x the branches there. The
// mispredicts are 21x worse in ratio but only ~7% of cycles, so the win is
// DELETED INSTRUCTIONS — the dispatch an arm never runs — and not, as the textbook
// framing suggests, a repaired branch predictor. That distinction is why this
// pass is bounded by instructions rather than by call-site counts.
//
// AND IT IS A LAW-0 ROW UNDER THE SECOND `≫`. It grows code by definition: a
// header copied into k arms costs (k−1) copies. `purity ≫ exec ≫ size` says that
// ships, and `M51` says to check the corpus before building — which was done:
// 7 candidate dispatch loops in the sqlite amalgamation (including
// `sqlite3VdbeExec`, 184 arms, whose header is THREE instructions), 84 in lua,
// 29 in the taxonomy suite, 0 in musl. This is an interpreter/parser shape, and
// the corpus that has interpreters has it in quantity.
//
// COMMUTING SQUARE. Copying a block into one of its predecessors is duplication,
// not motion: the copy executes exactly when that predecessor's edge is taken, on
// the same values, and the original still serves every other edge. ⟦f⟧ is
// unchanged because no instruction is added to or removed from any PATH — each
// path through the copy executes the same sequence it executed through the
// original. Nothing is speculated: the copy sits after the predecessor's own
// terminator position, so it runs only when that edge would have been taken.
//
// THE SSA OBLIGATION, and the condition that discharges it without a
// reconstruction. The copy defines fresh values, so a use of the ORIGINAL block's
// values must now choose between two definitions — a phi. In general that needs
// SSA reconstruction over the whole dominance frontier. This pass instead REFUSES
// any block whose defined values are used anywhere but an IMMEDIATE SUCCESSOR:
// there, the choice is exactly a block parameter, and the parameter is threaded by
// appending one argument to each edge. The refusal is not a weakening in practice
// — a dispatch's live-out is the loaded state byte, and the arm it dispatches to
// is where that byte is read.
use super::*;

/// `MEASURED M53` — the corpus census set this. Copying a header into `k` arms
/// costs `k−1` copies of it, so the block has to
/// be small for the trade to be one. Twelve instructions covers every dispatch in
/// the corpus census — `sqlite3VdbeExec`'s is three, `m2_http_parse`'s is two —
/// and refuses the loop bodies that merely happen to end in a switch.
const MAX_BLOCK: usize = 12;

/// `MEASURED M53` — 184 arms times a three-instruction header is 552, the largest
/// real case the census found. A ceiling on what one function may grow by, so a
/// pathological switch cannot
/// turn a function into its own jump table; a 12-instruction header times 184
/// would not be a dispatch and is refused by `MAX_BLOCK` first.
const MAX_GROWTH: usize = 800;

/// THEORY A7b  SQUARE tailjump_copies_the_dispatch_into_each_arm — the duplicated
/// dispatch
pub fn run(f: &mut Func) -> bool {
    if std::env::var_os("ZCC_NOTAILJUMP").is_some() {
        return false;
    }
    let mut grown = 0usize;
    let mut changed = false;
    // One sweep per shape, innermost loops first — an inner dispatch is the hot
    // one, and duplicating it first leaves the outer loop's shape unchanged.
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    let lf = dom::loops(&c, &dt);
    let mut order: Vec<usize> = (0..lf.loops.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(lf.loops[i].depth));
    let mut targets: Vec<BlockId> = Vec::new();
    for li in order {
        let h = lf.loops[li].header;
        if !matches!(f.blocks[h as usize].term, Term::Switch(..)) {
            continue;
        }
        // The dispatch, and the block the arms converge on before reaching it.
        // Duplicating the latch first is what turns "one latch, many arms" into
        // "many tails, each ending in a jump to the header" — without it the
        // header has a single predecessor inside the loop and there is nothing to
        // duplicate into.
        for &l in &lf.loops[li].latches {
            if l != h {
                targets.push(l);
            }
        }
        targets.push(h);
    }
    for b in targets {
        if grown >= MAX_GROWTH {
            break;
        }
        if let Some(n) = dup_into_preds(f, b, MAX_GROWTH - grown) {
            grown += n;
            changed = true;
        }
    }
    if changed {
        refresh_defs(f);
    }
    changed
}

/// Copy `b` into every predecessor that reaches it by a plain `Jmp`. Returns the
/// number of instructions added, or `None` if the block is refused.
macro_rules! why { ($($t:tt)*) => { if std::env::var_os("ZCC_TJ_REPORT").is_some() { eprintln!($($t)*); } } }

fn dup_into_preds(f: &mut Func, b: BlockId, budget: usize) -> Option<usize> {
    let bi = b as usize;
    if f.blocks[bi].insts.len() > MAX_BLOCK || !f.blocks[bi].labels.is_empty() {
        why!("tj b{}: too big ({}) or labelled", b, f.blocks[bi].insts.len());
        return None;
    }
    // Nothing with a side effect is duplicated. A call would be executed the same
    // number of times, but `Effect::Call` is also what `alloca` and the bulk
    // memory operations carry, and duplicating a stack adjustment is not a
    // duplication of anything.
    if f.blocks[bi].insts.iter().any(|i| !matches!(i.effect(), Effect::Pure | Effect::Read)) {
        return None;
    }
    let c = dom::cfg(f);
    // Predecessors reached by an unconditional jump: those are arm tails, and the
    // copy replaces the jump exactly. A conditional edge would need the copy in a
    // new block, which buys the same thing and costs an extra branch.
    let preds: Vec<BlockId> = c.preds[bi]
        .iter()
        .copied()
        .filter(|&p| p != b && matches!(&f.blocks[p as usize].term, Term::Jmp(t) if t.block == b))
        .collect();
    if preds.len() < 2 {
        why!("tj b{}: only {} jmp-preds", b, preds.len());
        return None; // one predecessor is a merge, not a dispatch: nothing to gain
    }
    let cost = f.blocks[bi].insts.len() * (preds.len() - 1);
    if cost > budget {
        return None;
    }

    // THE SSA CONDITION. Every value `b` defines — its parameters included — may
    // be used only inside `b` or in an immediate successor of `b`, because that is
    // exactly where a block parameter can carry the choice between the original
    // and a copy.
    let succs: Vec<BlockId> = f.blocks[bi].term.targets().iter().map(|t| t.block).collect();
    let mut defined: Vec<ValueId> = f.blocks[bi].params.clone();
    defined.extend(f.blocks[bi].insts.iter().filter_map(|i| i.dst()));
    let mut liveout: Vec<ValueId> = Vec::new();
    for &d in &defined {
        let mut outside_ok = true;
        let mut escapes = false;
        for (ob, blk) in f.blocks.iter().enumerate() {
            if ob == bi {
                continue;
            }
            let mut hit = false;
            for i in &blk.insts {
                i.uses(|o| {
                    if o == Operand::Val(d) {
                        hit = true;
                    }
                });
            }
            blk.term.uses(|o| {
                if o == Operand::Val(d) {
                    hit = true;
                }
            });
            if hit {
                escapes = true;
                if !succs.contains(&(ob as BlockId)) {
                    outside_ok = false;
                }
            }
        }
        if !outside_ok {
            // THE ROW'S CEILING, and it is named rather than left as a mystery.
            // This refusal is what stops the pass at the one block worth
            // duplicating: `m2_http_parse`'s dispatch loads the byte its arms
            // read, and that byte is used PAST the arm's first block. Two ways
            // past it were considered. Real SSA reconstruction over the dominance
            // frontier is the general answer and is shared infrastructure —
            // `unroll.rs` names the same gap in its own comment. Copying the load
            // down into each arm that uses it ("remat, then duplicate") is the
            // cheap answer and is free on the clock, since every path still
            // executes exactly one load; it was built and REVERTED the same hour,
            // because the first cut deleted the load from the dispatch without
            // placing it correctly — `m2` went from one `ldrb` to none and hung.
            // The idea is sound and the implementation was not; it is a row, not a
            // footnote, and it is written here so the next attempt starts from the
            // failure rather than from the idea.
            why!("tj b{}: v{} used past an immediate successor", b, d);
            return None;
        }
        if escapes {
            liveout.push(d);
        }
    }
    // A successor that is `b` itself would need its own parameters rewritten while
    // they are being read; refuse the self-loop rather than order the two.
    if succs.contains(&b) {
        return None;
    }
    // THE OTHER HALF OF THE SSA OBLIGATION, and leaving it out is a MISCOMPILE
    // rather than a missed transform. A new parameter on a successor must be given
    // an argument on EVERY edge into it. This pass writes one on `b`'s edges and on
    // each copy's, and it has nothing to write on an edge from anywhere else —
    // the value is not defined there. So a successor with a predecessor outside
    // `{b}` is refused. `k1_dispatch` and `k2_live_pressure` are the two programs
    // that caught this: their switch arms converge, so an arm's block is reached
    // both from the dispatch and from a sibling arm, and the argument list came out
    // one short. A pure dispatch does not have that shape — each arm's only
    // predecessor IS the header — which is why 217 unit tests and 94 of 96 suite
    // programs did not notice.
    if !liveout.is_empty() {
        for &s in &succs {
            if c.preds[s as usize].iter().any(|&p| p != b) {
                return None;
            }
        }
    }

    // (1) Every successor gains one parameter per live-out value, and the
    //     ORIGINAL block's own edges hand over the original values.
    let mut newp: Vec<Vec<ValueId>> = Vec::new();
    for &s in &succs {
        let mut ps = Vec::new();
        for &d in &liveout {
            let ty = f.ty_of(d);
            let p = f.new_value(ty, Def::Param(s, f.blocks[s as usize].params.len() as u32));
            f.blocks[s as usize].params.push(p);
            ps.push(p);
        }
        newp.push(ps);
    }
    for t in f.blocks[bi].term.targets_mut() {
        for &d in &liveout {
            t.args.push(Operand::Val(d));
        }
    }
    // (2) Uses of a live-out value in a successor become that successor's new
    //     parameter. Done before the copies exist, so a copy's own terminator
    //     args — written below — are not rewritten by it.
    for (si, &s) in succs.iter().enumerate() {
        for (di, &d) in liveout.iter().enumerate() {
            let p = newp[si][di];
            for inst in f.blocks[s as usize].insts.iter_mut() {
                inst.uses_mut(|o| {
                    if *o == Operand::Val(d) {
                        *o = Operand::Val(p);
                    }
                });
            }
            f.blocks[s as usize].term.uses_mut(|o| {
                if *o == Operand::Val(d) {
                    *o = Operand::Val(p);
                }
            });
        }
    }

    // (3) The copies. Each predecessor's `Jmp` is replaced by `b`'s instructions
    //     and `b`'s terminator, with `b`'s parameters bound to that edge's
    //     arguments and every definition renamed.
    for &p in &preds {
        let args = match &f.blocks[p as usize].term {
            Term::Jmp(t) => t.args.clone(),
            _ => unreachable!("filtered above"),
        };
        let mut map: std::collections::HashMap<ValueId, Operand> =
            f.blocks[bi].params.iter().copied().zip(args).collect();
        let src = f.blocks[bi].insts.clone();
        let mut out: Vec<Inst> = Vec::with_capacity(src.len());
        for inst in src {
            let mut ni = inst;
            ni.uses_mut(|o| {
                if let Operand::Val(v) = *o {
                    if let Some(r) = map.get(&v) {
                        *o = *r;
                    }
                }
            });
            if let Some(d) = ni.dst() {
                let nd = f.new_value(f.ty_of(d), Def::Inst(p, 0));
                map.insert(d, Operand::Val(nd));
                set_dst(&mut ni, nd);
            }
            out.push(ni);
        }
        let mut term = f.blocks[bi].term.clone();
        term.uses_mut(|o| {
            if let Operand::Val(v) = *o {
                if let Some(r) = map.get(&v) {
                    *o = *r;
                }
            }
        });
        f.blocks[p as usize].insts.extend(out);
        f.blocks[p as usize].term = term;
    }
    Some(cost)
}

/// Write a new destination into an instruction. `Inst::dst` reads it; nothing in
/// the IR wrote it before, because every other pass builds instructions rather
/// than renaming them in place.
fn set_dst(i: &mut Inst, nd: ValueId) {
    match i {
        Inst::Bin { dst, .. }
        | Inst::Un { dst, .. }
        | Inst::Cmp { dst, .. }
        | Inst::Cvt { dst, .. }
        | Inst::Load { dst, .. }
        | Inst::SlotAddr { dst, .. }
        | Inst::SymAddr { dst, .. }
        | Inst::Select { dst, .. }
        | Inst::Alloca { dst, .. } => *dst = nd,
        Inst::Call { dst, .. } | Inst::Intrinsic { dst, .. } => *dst = Some(nd),
        Inst::Store { .. } | Inst::MemCpy { .. } | Inst::MemSet { .. } => {}
    }
}

