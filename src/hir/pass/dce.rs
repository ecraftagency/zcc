// dce — dead-code elimination by the effect table (MECHANISM.md §G4 row 6).
// THEORY A7b — optimization: this pass ships its commuting square
//
// The whole pass is one rule, and the rule is a TABLE LOOKUP, not a hand-written
// opcode list: `Inst::effect()` classifies every instruction, and an instruction
// whose class is `Pure` or `Read` and whose result nothing needs is deleted.
// That is the point of carrying `Effect` in the IR (MECHANISM.md §G3.1) — adding an
// opcode can never silently make this pass unsound, because a new opcode must
// declare its class before it compiles.
//
// Commuting square. Deleting `i` changes ⟦f⟧ only if some run observes `i`.
// A `Pure` instruction is observable only through its result; a non-volatile
// `Read` likewise (it may FAULT, but a fault is ⊥ and refining ⊥ is legal —
// C99 makes the access undefined in exactly the cases where it can fault).
// Liveness is the least fixpoint of "used by a live instruction, a terminator,
// or an instruction with an effect", so an instruction outside it is observed by
// nothing.
//
// Dead BLOCK PARAMETERS go with them: a parameter no live instruction reads is
// removed together with the argument every incoming edge supplied, which in turn
// can kill the value that computed the argument. That is the block-parameter
// form of dead-φ elimination, and it is what lets the loop-counter of an empty
// loop disappear.
use super::*;

/// THEORY A7b  SQUARE dce_removes_an_unused_computation_but_not_a_call — the effect table
pub fn run(f: &mut Func) -> bool {
    let nv = f.values.len();
    let n = f.blocks.len();
    // incoming edges, addressed as `Term::targets()` orders them
    let mut in_edges: Vec<Vec<(BlockId, usize)>> = vec![Vec::new(); n];
    for b in 0..n {
        for (i, t) in f.blocks[b].term.targets().iter().enumerate() {
            in_edges[t.block as usize].push((b as BlockId, i));
        }
    }

    let mut live = vec![false; nv];
    let mut work: Vec<ValueId> = Vec::new();
    let mut mark = |live: &mut Vec<bool>, work: &mut Vec<ValueId>, o: Operand| {
        if let Operand::Val(v) = o {
            if !live[v as usize] {
                live[v as usize] = true;
                work.push(v);
            }
        }
    };
    for b in 0..n {
        for inst in &f.blocks[b].insts {
            if matches!(inst.effect(), Effect::Write | Effect::Call) {
                inst.uses(|o| mark(&mut live, &mut work, o));
            }
        }
        f.blocks[b].term.uses(|o| mark(&mut live, &mut work, o));
    }
    while let Some(v) = work.pop() {
        match f.values[v as usize].def {
            Def::FuncParam(_) => {}
            Def::Inst(b, i) => {
                if let Some(inst) = f.blocks[b as usize].insts.get(i as usize) {
                    inst.uses(|o| mark(&mut live, &mut work, o));
                }
            }
            Def::Param(b, k) => {
                for &(pb, ti) in &in_edges[b as usize] {
                    let a = f.blocks[pb as usize].term.targets()[ti].args[k as usize];
                    mark(&mut live, &mut work, a);
                }
            }
        }
    }

    let mut changed = false;
    // (1) instructions
    for b in 0..n {
        let before = f.blocks[b].insts.len();
        f.blocks[b].insts.retain(|inst| match inst.effect() {
            Effect::Write | Effect::Call => true,
            _ => match inst.dst() {
                Some(d) => live[d as usize],
                None => true,
            },
        });
        changed |= f.blocks[b].insts.len() != before;
    }
    // (2) block parameters, and the arguments that fed them
    for b in 0..n {
        if f.blocks[b].params.iter().all(|p| live[*p as usize]) {
            continue;
        }
        let keep: Vec<bool> = f.blocks[b].params.iter().map(|p| live[*p as usize]).collect();
        let mut k = 0;
        f.blocks[b].params.retain(|_| {
            k += 1;
            keep[k - 1]
        });
        for &(pb, ti) in &in_edges[b] {
            let mut ts = f.blocks[pb as usize].term.targets_mut();
            let mut k = 0;
            ts[ti].args.retain(|_| {
                k += 1;
                keep[k - 1]
            });
        }
        changed = true;
    }
    if changed {
        refresh_defs(f);
    }
    changed
}
