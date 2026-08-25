// purity — the interprocedural READ-ONLY predicate (REARCH §13c row 1).
//
// This is an ANALYSIS, not a transform: it adds no instruction and so carries no
// commuting square of its own. What it owes instead is SOUNDNESS of the
// predicate, because a pass that trusts it (`licm::hoist_calls`) turns a false
// positive into a miscompile. So the obligation is stated here and discharged
// instruction by instruction below.
//
// THE PREDICATE. `readonly(g)` means: a call to `g` performs no store to memory
// that outlives the call, performs no volatile access, and calls nothing that
// does. It is gcc's `__attribute__((pure))`, NOT `const`: `g` may READ memory,
// so its result depends on the memory state as well as its arguments, and a
// caller may only move a call across code it has proven memory-clean.
// Termination is NOT part of the predicate (gcc's `pure` does not promise it
// either) — the caller's ≥1-trip fence is what makes that irrelevant.
//
// WHY OPTIMISTIC INITIALIZATION IS SOUND. The fixpoint starts with every
// function in the module assumed read-only and removes a function the moment a
// witness to a write is found. For a recursive cycle this is the right
// direction: "performs a write" is an EXISTENTIAL over the instructions
// reachable from `g`, so if no member of the cycle contains a writing
// instruction, no execution of the cycle writes — however many times it recurs.
// (The dual, "terminates", is universal and would need pessimistic
// initialization; it is deliberately not claimed.)
//
// SCOPE. A function not defined in this module (an external `printf`, a call
// through a pointer) is never read-only: there is nothing to inspect. A function
// DEFINED here is the program's one external definition (C99 6.9p5), so its body
// is the whole truth about it. ELF symbol interposition would break that, and is
// a shared-library property zcc does not target (Article B: ELF executables).
use super::*;
use std::collections::HashSet;

/// The names of every function in `m` whose call writes no memory the caller can
/// observe. See the predicate above.
pub fn readonly_functions(m: &Module) -> HashSet<String> {
    // The address-taken/local map is a property of one function's own body and
    // does not depend on the assumption set, so it is computed once.
    let locals: Vec<Vec<bool>> = m.funcs.iter().map(local_addresses).collect();
    let mut ro: HashSet<String> = m.funcs.iter().map(|f| f.name.clone()).collect();
    loop {
        let mut dropped = false;
        for (i, f) in m.funcs.iter().enumerate() {
            if ro.contains(&f.name) && writes(f, &locals[i], &ro) {
                ro.remove(&f.name);
                dropped = true;
            }
        }
        if !dropped {
            return ro;
        }
    }
}

/// A witness that `f` writes memory the caller can observe, under the current
/// assumption `ro` about the rest of the module.
fn writes(f: &Func, local: &[bool], ro: &HashSet<String>) -> bool {
    let is_local = |o: &Operand| matches!(o, Operand::Val(v) if local[*v as usize]);
    for b in &f.blocks {
        for inst in &b.insts {
            let bad = match inst {
                // A volatile access is an observable event in itself (C99
                // 6.7.3p6), whatever it touches.
                Inst::Load { vol: true, .. } | Inst::Store { vol: true, .. } => true,
                // A store into this frame dies with the call.
                Inst::Store { addr, .. } => !is_local(addr),
                Inst::MemCpy { dst, .. } | Inst::MemSet { dst, .. } => !is_local(dst),
                // An `sret` call writes through an address the CALLEE was handed
                // (AAPCS64 §6.9, the x8 convention). Refused rather than
                // reasoned about: the write is real and its target is the
                // caller's business.
                Inst::Call { sret: Some(_), .. } => true,
                Inst::Call { callee: Callee::Direct(n), .. } => !ro.contains(n),
                // A call through a pointer names no body to inspect.
                Inst::Call { callee: Callee::Indirect(_), .. } => true,
                // `alloca` moves the stack pointer and the builtin surface is
                // opaque by construction (REARCH §3.1) — neither is inspectable,
                // so neither is read-only.
                Inst::Alloca { .. } | Inst::Intrinsic { .. } => true,
                _ => false,
            };
            if bad {
                return true;
            }
        }
    }
    false
}

/// Values that certainly point INSIDE this function's own frame.
///
/// C99 6.5.6p8 is what makes the arithmetic rule sound: pointer arithmetic is
/// defined only within the object pointed into, so adding an integer to a
/// frame-local address yields another frame-local address, and subtracting one
/// does the same. Adding two frame-local addresses is not a C operation at all,
/// which is why the rule is an exclusive-or rather than a disjunction — a sum of
/// two pointers can only arise from code that has already left the standard.
///
/// The lattice starts at "nothing is local" and only grows, so every value it
/// fails to recognize is treated as pointing at memory the caller can see. One
/// such case is recorded rather than hidden: a pointer carried around a loop as
/// a BLOCK PARAMETER (`for (p = buf; ...; p++)`) never becomes local, because
/// the parameter's own back-edge argument is derived from the parameter. Closing
/// it needs the optimistic-cycle treatment; it is a Law-4 category-(b) residual
/// of this analysis, not a limit of the theorem.
fn local_addresses(f: &Func) -> Vec<bool> {
    fn isloc(loc: &[bool], o: &Operand) -> bool {
        matches!(o, Operand::Val(v) if loc[*v as usize])
    }
    fn mark(loc: &mut [bool], grew: &mut bool, v: ValueId, to: bool) {
        if to && !loc[v as usize] {
            loc[v as usize] = true;
            *grew = true;
        }
    }
    let mut has_pred = vec![false; f.blocks.len()];
    for b in &f.blocks {
        for t in b.term.targets() {
            has_pred[t.block as usize] = true;
        }
    }
    let mut loc = vec![false; f.values.len()];
    loop {
        let mut grew = false;
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::SlotAddr { dst, .. } => mark(&mut loc, &mut grew, *dst, true),
                    Inst::Bin { dst, op: BinOp::Add, a, b: rhs, .. } => {
                        let t = isloc(&loc, a) ^ isloc(&loc, rhs);
                        mark(&mut loc, &mut grew, *dst, t);
                    }
                    Inst::Bin { dst, op: BinOp::Sub, a, b: rhs, .. } => {
                        let t = isloc(&loc, a) && !isloc(&loc, rhs);
                        mark(&mut loc, &mut grew, *dst, t);
                    }
                    Inst::Cvt { dst, op: CvtOp::Bitcast, a, .. } => {
                        let t = isloc(&loc, a);
                        mark(&mut loc, &mut grew, *dst, t);
                    }
                    Inst::Select { dst, a, b: rhs, .. } => {
                        let t = isloc(&loc, a) && isloc(&loc, rhs);
                        mark(&mut loc, &mut grew, *dst, t);
                    }
                    _ => {}
                }
            }
        }
        // A join parameter is local when EVERY edge into it carries a local
        // address, so the meet is taken over all incoming arguments at once.
        let mut join: Vec<Vec<bool>> = f
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| vec![has_pred[i]; b.params.len()])
            .collect();
        for b in &f.blocks {
            for t in b.term.targets() {
                for (k, a) in t.args.iter().enumerate() {
                    if let Some(slot) = join[t.block as usize].get_mut(k) {
                        *slot &= isloc(&loc, a);
                    }
                }
            }
        }
        for (i, b) in f.blocks.iter().enumerate() {
            for (k, p) in b.params.iter().enumerate() {
                let t = join[i][k];
                mark(&mut loc, &mut grew, *p, t);
            }
        }
        if !grew {
            return loc;
        }
    }
}
