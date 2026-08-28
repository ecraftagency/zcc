// auto_inc (MECHANISM.md §G8, gcc's `-fauto-inc-dec`) — fold a pointer
// THEORY A6b — MIR; THEORY A7b — optimization, proven pass by pass
// post-increment into the load that reads through it.
//
// A loop walking an array compiles to `ldr r, [p]` followed by `add p2, p, #k`,
// and A64 has `ldr r, [p], #k` doing both in one instruction: the access at `p`,
// then the writeback `p2 = p + k` (DDI 0487 C6.2, the post-indexed form; the
// offset is an UNSCALED signed 9-bit immediate, −256..255).
//
// LOADS ONLY. A store post-index would risk the stored register aliasing the
// base (`STR Xt, [Xn], #imm` with t == n is CONSTRAINED UNPREDICTABLE, DDI 0487
// C6.2). A load cannot hit that: its transfer register is live past the load —
// it is what the loop body consumes — so it interferes with the writeback
// register, which the allocator ties to the base (`color.rs`), and the two get
// distinct colours by construction. Restricting to loads buys provable safety
// with no t != n side condition.
//
// COMMUTING SQUARE (SEMANTICS.md §5.5). `ldr r,[p]; add p2,p,#k` and
// `ldr r,[p],#k` denote the same run: the load reads mem[p] either way, and
// p2 = p + k either way. Three side conditions are CHECKED, not assumed:
//   * the base's ONLY two uses are this load and the add (so nothing observes
//     the pointer between the access and the increment, and moving the
//     definition of p2 back to the load cannot cross a reader of the old p);
//   * k fits the imm9 range;
//   * the add FOLLOWS the load in the same block (post-index, not pre-index).
// Moving p2's definition earlier keeps def-before-use: p2 was defined by the add
// and read only after it, and nothing between the load and the add touches p2
// (SSA — it did not exist yet).
use crate::mir::*;

/// THEORY A6b  SQUARE auto_inc_fires_and_preserves_meaning — the post-index writeback
pub fn run(f: &mut MFunc) {
    // Cheap gate first: no `[base,#0]` load means nothing to fold, so the
    // liveness recompute below is skipped entirely — the pass costs ~0 on a
    // function with no pointer walk (most of them).
    let has_candidate = f.blocks.iter().any(|b| {
        b.insts.iter().any(|i| {
            matches!(
                i,
                MInst::Load { mem: AddrMode::BaseImm { base: Reg::V(_), off: 0 }, vol: false, .. }
            )
        })
    });
    if !has_candidate {
        return;
    }

    // The tie the allocator will make — `wb` shares the base's physical register
    // (`color.rs`) — means that shared register is live across the WHOLE span
    // base.def‥wb.last-use. If a call falls in it the register must be
    // callee-saved, but the allocator decides base's class from base's OWN
    // crossing (base dies at the load). So the fold is refused when it would make
    // `wb` span a call the base did not: `wb` already crosses one (pre-fold), or a
    // `Call` sits between the load and the add (folding moves `wb`'s definition
    // back across it). Loops without calls — the common array walk — are unaffected.
    let cfg = crate::mir::verify::cfg(f);
    let lv = crate::regalloc::live::compute(f, &cfg);

    // Uses per virtual register across the whole function, DEFS excluded and edge
    // arguments included (a base passed on an edge has a use the terminator
    // carries, so its count exceeds two and it is left alone).
    let mut uses = vec![0u32; f.vregs.len()];
    let mut bump = |r: Reg, uses: &mut [u32]| {
        if let Reg::V(v) = r {
            uses[v as usize] += 1;
        }
    };
    for b in &f.blocks {
        for inst in &b.insts {
            inst.visit(&mut |r, c| {
                if matches!(c, Constraint::Use | Constraint::UseFixed(_)) {
                    bump(r, &mut uses);
                }
            });
        }
        b.term.visit(&mut |r, _| bump(r, &mut uses));
    }

    for b in 0..f.blocks.len() {
        let mut remove: Vec<usize> = Vec::new();
        let n = f.blocks[b].insts.len();
        for i in 0..n {
            let base = match &f.blocks[b].insts[i] {
                MInst::Load {
                    mem: AddrMode::BaseImm { base: Reg::V(p), off: 0 },
                    vol: false,
                    ..
                } => *p,
                _ => continue,
            };
            // exactly the load and its bump; nothing else reads the pointer
            if uses[base as usize] != 2 {
                continue;
            }
            // the matching `add p2, p, #k` (or `sub`, folded as −k) later in the
            // block. Nothing between it and the load reads the base (count == 2),
            // so any distance is safe.
            let mut hit = None;
            for j in (i + 1)..f.blocks[b].insts.len() {
                if let MInst::Alu {
                    op,
                    dst: Reg::V(p2),
                    a: Reg::V(a),
                    b: Rhs::Imm(k),
                    flags: None,
                    ..
                } = &f.blocks[b].insts[j]
                {
                    if *a == base && matches!(op, AluOp::Add | AluOp::Sub) {
                        let off = if *op == AluOp::Sub { -*k } else { *k };
                        if (-256..=255).contains(&off) && !lv.crosses_call[*p2 as usize] {
                            hit = Some((j, *p2, off as i32));
                            break;
                        }
                    }
                }
            }
            let Some((j, wb, off)) = hit else { continue };
            // folding moves wb's definition back to the load; refuse if that drags
            // it across a call the base did not already span
            if f.blocks[b].insts[i + 1..j]
                .iter()
                .any(|x| matches!(x, MInst::Call { .. }))
            {
                continue;
            }
            if let MInst::Load { mem, .. } = &mut f.blocks[b].insts[i] {
                *mem = AddrMode::PostIdx { base: Reg::V(base), wb: Reg::V(wb), off };
            }
            remove.push(j);
        }
        remove.sort_unstable();
        for &j in remove.iter().rev() {
            f.blocks[b].insts.remove(j);
        }
    }
}
