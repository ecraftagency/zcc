// sroa + mem2reg (REARCH §4 row 2) — the pass the R1 ground metric points at.
//
// §13a measured `add` at 28.2% of sqlite's instructions and `ldr` at 19.2%, and
// named one cause: R0/R1 keep EVERY C local in the frame, so each access is a
// `SlotAddr` (an `add`) followed by a `ldr`/`str`. This pass is the half of the
// answer that removes the memory traffic itself; R3.1's addressing modes remove
// the `add` from what remains.
//
// THE OBSTACLE, AND WHY IT IS GONE. The parser reports `Node::Var(off)` — an
// offset, not an identity — so HIR sees one slot with accesses scattered through
// it. An offset alone cannot tell where a local ENDS, so an escaped `&x` had to
// be assumed to reach the whole frame, and nothing was promotable (REARCH §14).
// The fix is not an analysis but a FACT: `alloc_local` already computed each
// object's extent, and `ast::Func::objs` now exports it. With extents in hand,
// C99 6.5.6p8 — pointer arithmetic is defined only WITHIN an object — bounds an
// escaped address to its own object, and everything disjoint from it promotes.
//
// SROA falls out of the same machinery rather than needing a second pass: the
// unit of promotion is a (offset, type) PIECE, not a whole object, so a struct
// whose address is never taken has each field promoted independently, and an
// array indexed by a variable disqualifies only itself (its `SlotAddr` feeds an
// `add`, which is an escape).
//
// COMMUTING SQUARE. Three obligations:
//   (1) DISJOINTNESS — a promoted piece's bytes are touched by nothing else. The
//       analysis admits a piece only when every access to its range is a
//       non-volatile load/store at exactly its offset and width, and the range
//       overlaps no escaped object and no other piece. So its memory cell is
//       private, and a private cell is exactly a variable.
//   (2) REACHING DEFINITIONS — the value a load returns is the value the last
//       dominating store wrote. That is what SSA construction computes:
//       parameters are placed at the iterated dominance frontier of the stores
//       (Cytron et al. 1991 — the theorem is that this is exactly where two
//       different definitions can meet), and the in/out walk below is the
//       reaching-definition dataflow, exact because after placement every join
//       either has a parameter or has one reaching definition.
//   (3) INDETERMINATE READS — a load with no reaching store reads an object
//       whose value C99 6.7.8p10 leaves indeterminate. Any value is a refinement
//       of an indeterminate one, so `0` is chosen; nothing is promised about it.
use super::*;
use std::collections::HashMap;

/// One promotable memory cell: `slot[off .. off+ty.bytes)`, accessed only by
/// loads and stores of exactly `ty`.
struct Piece {
    slot: u32,
    off: i64,
    ty: Ty,
}

pub fn run(f: &mut Func) -> bool {
    let mut changed = canon_slot_addr(f);
    let pieces = analyze(f);
    if pieces.is_empty() {
        return changed;
    }
    changed |= promote(f, &pieces);
    changed
}

/// `add(slot_addr(s, o), k)` IS `slot_addr(s, o + k)` — the identity that makes a
/// STRUCTURE promotable field by field. The frontend lowers `p.y` as the address
/// of `p` plus the field offset, and an address that feeds an `add` looks like an
/// escape to the analysis below; folding the offset into the address instead
/// turns each field into an ordinary named cell. It is also the first bite out of
/// the `add` share the R1 metric measured, and it is what lets isel later fold
/// the offset into the load itself (R3.1).
fn canon_slot_addr(f: &mut Func) -> bool {
    let mut changed = false;
    // A chain (`&p.a.b`) folds one link per sweep; the bound is the longest
    // chain, and a sweep that changes nothing stops it.
    loop {
        let mut base: Vec<Option<(u32, i64)>> = vec![None; f.values.len()];
        for b in &f.blocks {
            for inst in &b.insts {
                if let Inst::SlotAddr { dst, slot, off } = inst {
                    base[*dst as usize] = Some((*slot, *off));
                }
            }
        }
        let mut hit = false;
        for b in f.blocks.iter_mut() {
            for inst in b.insts.iter_mut() {
                let new = match inst {
                    Inst::Bin { dst, op: BinOp::Add, ty: Ty::I64, a, b } => {
                        match (*a, *b) {
                            (Operand::Val(v), Operand::Imm(k)) | (Operand::Imm(k), Operand::Val(v)) => {
                                base[v as usize].map(|(s, o)| Inst::SlotAddr {
                                    dst: *dst,
                                    slot: s,
                                    off: o + k,
                                })
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some(n) = new {
                    *inst = n;
                    hit = true;
                }
            }
        }
        changed |= hit;
        if !hit {
            return changed;
        }
    }
}

// ── analysis ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Access {
    /// only ever the address of a load/store of this type
    Mem(Ty),
    /// used as anything else: the address is observable
    Escape,
}

fn analyze(f: &Func) -> Vec<Piece> {
    // (a) every `SlotAddr` value, by the cell it names
    let mut cell: HashMap<ValueId, (u32, i64)> = HashMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            if let Inst::SlotAddr { dst, slot, off } = inst {
                cell.insert(*dst, (*slot, *off));
            }
        }
    }
    if cell.is_empty() {
        return Vec::new();
    }
    // (b) classify each cell by how its address is used. A `SlotAddr` operand in
    //     any position other than `Load.addr` / `Store.addr` — an argument, a
    //     stored VALUE, an operand of an `add`, a `memcpy` end — means the
    //     address left our sight.
    let mut acc: HashMap<(u32, i64), Access> = HashMap::new();
    let mut mark = |acc: &mut HashMap<(u32, i64), Access>, k: (u32, i64), a: Access| {
        match acc.get(&k) {
            None => {
                acc.insert(k, a);
            }
            Some(Access::Escape) => {}
            Some(prev) if *prev == a => {}
            // two different widths at one offset: a type pun, never a variable
            Some(_) => {
                acc.insert(k, Access::Escape);
            }
        }
    };
    for b in &f.blocks {
        for inst in &b.insts {
            match inst {
                Inst::Load { ty, addr: Operand::Val(v), vol: false, .. } => {
                    if let Some(&k) = cell.get(v) {
                        mark(&mut acc, k, Access::Mem(*ty));
                        continue;
                    }
                }
                Inst::Store { ty, addr: Operand::Val(v), val, vol: false, .. } => {
                    if let Some(&k) = cell.get(v) {
                        if *val != Operand::Val(*v) {
                            mark(&mut acc, k, Access::Mem(*ty));
                            // the stored VALUE still has to be examined below
                            if let Operand::Val(w) = val {
                                if let Some(&kw) = cell.get(w) {
                                    mark(&mut acc, kw, Access::Escape);
                                }
                            }
                            continue;
                        }
                    }
                }
                Inst::SlotAddr { .. } => continue,
                _ => {}
            }
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    if let Some(&k) = cell.get(&v) {
                        mark(&mut acc, k, Access::Escape);
                    }
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                if let Some(&k) = cell.get(&v) {
                    mark(&mut acc, k, Access::Escape);
                }
            }
        });
    }

    // (c) turn each escape into the byte range it may reach: its own object
    //     (C99 6.5.6p8). An escaped address inside no known object is not
    //     bounded at all, and disqualifies its whole slot.
    let mut blocked: Vec<(u32, i64, i64)> = Vec::new();
    let mut dead_slots: Vec<u32> = Vec::new();
    for (&(slot, off), a) in acc.iter() {
        if *a != Access::Escape {
            continue;
        }
        match object_of(f, slot, off) {
            Some((lo, hi)) => blocked.push((slot, lo, hi)),
            None => dead_slots.push(slot),
        }
    }
    // (d) the surviving cells, each claiming its own bytes
    let mut cand: Vec<Piece> = Vec::new();
    for (&(slot, off), a) in acc.iter() {
        if let Access::Mem(ty) = a {
            if !dead_slots.contains(&slot) {
                cand.push(Piece { slot, off, ty: *ty });
            }
        }
    }
    cand.sort_by_key(|p| (p.slot, p.off));
    // (e) reject anything that overlaps an escaped object or another piece
    let mut out: Vec<Piece> = Vec::new();
    for (i, p) in cand.iter().enumerate() {
        let (lo, hi) = (p.off, p.off + p.ty.bytes() as i64);
        if blocked.iter().any(|&(s, blo, bhi)| s == p.slot && lo < bhi && blo < hi) {
            continue;
        }
        let clash = cand.iter().enumerate().any(|(j, q)| {
            j != i && q.slot == p.slot && lo < q.off + q.ty.bytes() as i64 && q.off < hi
        });
        if clash {
            continue;
        }
        out.push(Piece { slot: p.slot, off: p.off, ty: p.ty });
    }
    out
}

/// The extent of the object containing `off`, in slot coordinates. Only slot 0
/// has an object table (it is the parser's frame); any other slot is a single
/// object of its own declared size.
fn object_of(f: &Func, slot: u32, off: i64) -> Option<(i64, i64)> {
    if slot != 0 {
        let s = f.slots[slot as usize].size as i64;
        return if (0..s).contains(&off) { Some((0, s)) } else { None };
    }
    f.objs
        .iter()
        .find(|&&(o, s)| off >= o && off < o + s as i64)
        .map(|&(o, s)| (o, o + s as i64))
}

// ── SSA construction (Cytron placement + a reaching-definition walk) ───────

fn promote(f: &mut Func, pieces: &[Piece]) -> bool {
    let n = f.blocks.len();
    let c = dom::cfg(f);
    let dt = dom::domtree(f, &c);
    // which variable, if any, a `SlotAddr` value names
    let mut var_of_addr: HashMap<ValueId, usize> = HashMap::new();
    for b in &f.blocks {
        for inst in &b.insts {
            if let Inst::SlotAddr { dst, slot, off } = inst {
                if let Some(k) = pieces.iter().position(|p| p.slot == *slot && p.off == *off) {
                    var_of_addr.insert(*dst, k);
                }
            }
        }
    }
    if var_of_addr.is_empty() {
        return false;
    }

    // (1) dominance frontiers (Cytron et al. 1991 §4.2, the "runner" formulation)
    let mut df: Vec<Vec<BlockId>> = vec![Vec::new(); n];
    for b in 0..n {
        if c.preds[b].len() < 2 || !c.reachable(b as BlockId) {
            continue;
        }
        let idom_b = dt.idom[b];
        for &p in &c.preds[b] {
            let mut r = p;
            while r != idom_b && r != u32::MAX && c.reachable(r) {
                if !df[r as usize].contains(&(b as BlockId)) {
                    df[r as usize].push(b as BlockId);
                }
                r = dt.idom[r as usize];
            }
        }
    }

    // (2) where each variable is stored, then its iterated dominance frontier
    let nv = pieces.len();
    let mut stores_in: Vec<Vec<BlockId>> = vec![Vec::new(); nv];
    for b in 0..n {
        for inst in &f.blocks[b].insts {
            if let Inst::Store { addr: Operand::Val(a), .. } = inst {
                if let Some(&k) = var_of_addr.get(a) {
                    if !stores_in[k].contains(&(b as BlockId)) {
                        stores_in[k].push(b as BlockId);
                    }
                }
            }
        }
    }
    // `added[b]` = the variables that gained a parameter at b, in the order the
    // parameters were appended — the same order every incoming edge appends its
    // arguments in.
    // Blocks an ARGUMENT-LESS edge reaches. EXT(gcc) `goto *e` names its
    // successors without passing anything, so a parameter placed on one of them
    // would never be given a value on that edge — and neither the verifier nor
    // ⟦·⟧ can see the hole, because both read arguments through `Term::targets`,
    // which a computed goto has none of. A variable whose frontier touches such
    // a block is therefore not promoted at all. Same for the entry block, which
    // takes its values from the ABI and may hold no parameters at all.
    let mut argless: Vec<bool> = vec![false; n];
    argless[f.entry as usize] = true;
    for b in &f.blocks {
        if let Term::GotoPtr(_, bs) = &b.term {
            for &t in bs {
                argless[t as usize] = true;
            }
        }
    }
    let mut added: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut placed = vec![vec![false; nv]; n];
    let mut promoted: Vec<bool> = vec![true; nv];
    // `ever`/`seen` are membership bitmaps, not Vecs: on a 7,266-block function
    // (yarpgen `init`) with 1,643 pieces, an `ever.contains` linear scan per
    // worklist step made the iterated-frontier walk O(blocks²) PER piece —
    // minutes of compile. Reused across pieces by clearing only the bits touched.
    let mut ever = vec![false; n];
    let mut seen = vec![false; n];
    for k in 0..nv {
        let mut work = stores_in[k].clone();
        let mut touched: Vec<BlockId> = Vec::new();
        for &b in &work {
            ever[b as usize] = true;
            touched.push(b);
        }
        let mut sites: Vec<BlockId> = Vec::new();
        while let Some(b) = work.pop() {
            for &y in &df[b as usize] {
                if seen[y as usize] {
                    continue;
                }
                seen[y as usize] = true;
                touched.push(y);
                sites.push(y);
                if !ever[y as usize] {
                    ever[y as usize] = true;
                    work.push(y);
                }
            }
        }
        for &b in &touched {
            ever[b as usize] = false;
            seen[b as usize] = false;
        }
        if sites.iter().any(|&y| argless[y as usize]) {
            promoted[k] = false;
            continue;
        }
        for y in sites {
            placed[y as usize][k] = true;
            added[y as usize].push(k);
        }
    }
    var_of_addr.retain(|_, k| promoted[*k]);
    if var_of_addr.is_empty() {
        return false;
    }
    // materialize the parameters
    let mut param_of: HashMap<(BlockId, usize), ValueId> = HashMap::new();
    for b in 0..n {
        for i in 0..added[b].len() {
            let k = added[b][i];
            let v = f.new_value(pieces[k].ty, Def::Param(b as BlockId, 0));
            f.blocks[b].params.push(v);
            param_of.insert((b as BlockId, k), v);
        }
    }

    // (3) the reaching-definition walk, in reverse postorder
    let undef = |ty: Ty| if ty.is_float() { Operand::Fimm(0) } else { Operand::Imm(0) };
    let mut out_val: Vec<Vec<Operand>> =
        vec![pieces.iter().map(|p| undef(p.ty)).collect(); n];
    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    let mut dead: Vec<(usize, usize)> = Vec::new();
    for bi in 0..c.rpo.len() {
        let b = c.rpo[bi] as usize;
        // entry state
        let mut cur: Vec<Operand> = (0..nv)
            .map(|k| match param_of.get(&(b as BlockId, k)) {
                Some(&v) => Operand::Val(v),
                None => {
                    // no parameter here ⟹ every predecessor carries the same
                    // reaching definition (Cytron's theorem), so any already
                    // computed predecessor answers for all of them
                    match c.preds[b].iter().find(|&&p| c.rpo_num[p as usize] < bi as u32) {
                        Some(&p) => out_val[p as usize][k],
                        None => undef(pieces[k].ty),
                    }
                }
            })
            .collect();
        for (i, inst) in f.blocks[b].insts.iter().enumerate() {
            match inst {
                Inst::Load { dst, addr: Operand::Val(a), .. } => {
                    if let Some(&k) = var_of_addr.get(a) {
                        map[*dst as usize] = Some(cur[k]);
                        dead.push((b, i));
                    }
                }
                Inst::Store { addr: Operand::Val(a), val, .. } => {
                    if let Some(&k) = var_of_addr.get(a) {
                        cur[k] = *val;
                        dead.push((b, i));
                    }
                }
                _ => {}
            }
        }
        out_val[b] = cur;
    }

    // (4) hand each edge the definitions its successor's new parameters expect
    for b in 0..n {
        let mut term = f.blocks[b].term.clone();
        for t in term.targets_mut() {
            for &k in &added[t.block as usize] {
                t.args.push(out_val[b][k]);
            }
        }
        f.blocks[b].term = term;
    }

    // (5) drop the promoted loads and stores; the `SlotAddr` values they used go
    //     with the next DCE round
    rewrite_values(f, &map);
    dead.sort_unstable();
    for &(b, i) in dead.iter().rev() {
        f.blocks[b].insts.remove(i);
    }
    refresh_defs(f);
    true
}
