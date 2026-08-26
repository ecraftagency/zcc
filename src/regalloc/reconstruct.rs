// SSA reconstruction — putting a value back together where several definitions
// of it reach one point (REARCH allocator-splitting spec §4.1).
// THEORY A7 — register allocation ON SSA
//
// Braun, Buchwald, Hack, Leißa, Mallon, Zwinkau, "Simple and Efficient
// Construction of SSA Form", CC 2013.
//
// WHY THIS MODULE HAS TO EXIST. The spiller's carry (`spill.rs`) keeps a value
// in a register across an edge only where EVERY predecessor is holding it under
// the SAME name. That condition is not a heuristic, it is a PROOF: a name has
// exactly one definition, so "every predecessor holds it" says every path from
// the entry to the use runs through that definition — dominance — and SSA holds
// with nothing else done. It is also the reason the carry stops where it does.
// A diamond whose two arms compute the value differently (a register on one, a
// reload out of the frame slot on the other) has TWO reaching definitions at the
// join, no single name spans both, and no amount of care with names fixes it.
// A loop header is the same shape with one arm running backwards: the preheader
// reaches it with the initial definition, the latch with the carried one.
//
// WHAT BRAUN 2013 ANSWERS. Give the join a fresh name defined AT ITS HEAD, and
// let every predecessor say which of its own values that name stands for on its
// own edge. That is a phi; in this IR a phi is spelled as a BLOCK PARAMETER plus
// one argument per incoming edge, which is the same object with the arguments
// stored at the edges rather than in a list beside the node.
//
// THE COMMUTING SQUARE, and why it is structural rather than interpreted. The
// parameter is defined at the head of the block, so it dominates every point in
// the block and everything the block dominates — every use the caller renames to
// it. Each predecessor passes the value that reaches the join along that edge, so
// on every path the parameter holds what the original name held: the two programs
// compute the same thing, which is `⟦f⟧ = ⟦insert_phi f⟧` under the renaming.
// Downstream this is not taken on trust — `destruct` lowers each parameter to a
// parallel copy on the edge and `mir::verify` re-derives one-definition-per-name
// and use-dominated-by-definition from the graph itself, so a phi built wrongly
// here is caught at the layer below rather than as a wrong answer in a suite
// (Law 3).
//
// WHAT IS DELIBERATELY NOT HERE. Choosing WHERE a phi is worth building, and
// renaming the uses that should read it, belong to the caller: this module knows
// how to build one correctly and nothing about when. Braun's minimal-SSA pruning
// (do not build a parameter whose arguments are all the same value) is that same
// caller's decision — building it anyway is correct, merely wasteful, and
// `destruct` coalesces the identity copies away.
use crate::mir::*;

/// Materialize a phi for one value at `block`: a fresh block parameter of
/// `class`/`width`, fed on each listed predecessor's edge into `block` by the
/// register that predecessor reaches it with. Returns the new name; renaming the
/// uses to it is the caller's half.
///
/// `args` must name each predecessor edge into `block` exactly once. A
/// predecessor that reaches `block` on TWO edges (a `cbz` whose arms both land
/// there, a switch with a repeated case) therefore appears twice, and the two
/// entries fill the two edges in order — which is why an edge is filled by
/// ARGUMENT COUNT rather than by "the first target that matches": a block's
/// arguments and its successor's parameters are positional, so the edge still
/// short of this parameter is exactly the one still to fill.
pub fn insert_phi(
    f: &mut MFunc,
    block: MBlockId,
    class: Class,
    width: Width,
    args: &[(MBlockId, Reg)],
) -> VReg {
    // `Class::Flags` is a class of one register and is never spilled or carried
    // (it is rematerialized instead — THEORY A7, Briggs 1992), so a phi of flags
    // would be a value the rest of the allocator has no way to honour.
    debug_assert!(
        class == width.class() && class != Class::Flags,
        "a phi of {:?}/{:?} is not a value this allocator can pass on an edge",
        class,
        width
    );
    let p = f.new_vreg(width);
    f.blocks[block as usize].params.push(p);
    // the position this parameter takes in the block's parameter list, which is
    // the position its argument takes on every incoming edge
    let pos = f.blocks[block as usize].params.len() - 1;
    for &(pred, r) in args {
        let mut filled = false;
        for t in f.blocks[pred as usize].term.targets_mut() {
            if t.block == block && t.args.len() == pos {
                t.args.push(r);
                filled = true;
                break;
            }
        }
        debug_assert!(
            filled,
            "b{} was named as a predecessor of b{} but has no edge into it still short of \
             argument {} — the phi would be fed on some paths and not others",
            pred, block, pos
        );
    }
    p.vreg().expect("new_vreg returns a virtual register")
}
