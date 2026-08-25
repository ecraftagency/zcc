// load_elim / dse (REARCH §4 row 5) — the memory that SROA could not promote.
// THEORY A7b — optimization: this pass ships its commuting square
//
// mem2reg removes a local's memory entirely when the local is a private cell.
// What is left is genuinely memory: globals, anything reached through a pointer,
// and the objects whose address escaped. For those the question is not "can this
// become a value" but "does this access have to happen at all", and answering it
// needs an ALIAS ORACLE.
//
// THE ORACLE (REARCH §3.3 B1, the conservative base — TBAA is §16 ★1 and reads
// the `aclass` field this IR already carries). Two locations are DISJOINT when
// the C standard says they are different objects:
//   * two stack pieces whose byte ranges do not overlap (C99 6.2.4: distinct
//     objects with distinct storage);
//   * a stack object and a global — no object has both storage durations;
//   * two different linker symbols.
// Everything else MAY alias, including any pointer against anything.
//
// THE TRANSFORMS. Each is block-local in its reasoning; what R4.9 adds is not a
// new transform but a bigger BLOCK to reason over — see "ACROSS ONE EDGE" below.
// A block is where the oracle is exact enough to be worth the walk; the fully
// general versions (`-ftree-fre`, `-ftree-dse` over arbitrary control flow) need
// a memory SSA and stay a residual in REARCH §12 rather than half-built here.
//   * STORE→LOAD FORWARDING — a load of a location whose value was just stored
//     is that value. ⟦·⟧: memory is a function, and reading what was written
//     returns it.
//   * REDUNDANT LOAD ELIMINATION — a second load of a location no intervening
//     write may touch returns the first load's value.
//   * DEAD STORE ELIMINATION — a store overwritten by a later store to the same
//     location, with no read that may see it in between, is invisible.
// A volatile access, a call, an `alloca` and a `memcpy`/`memset` all clear the
// table: C99 6.7.3 forbids touching the first, and the rest may write anything.
//
// ACROSS ONE EDGE (R4.9 — gcc's `-ftree-fre`, in the one case that needs no
// dataflow). A block C whose ONLY predecessor is P is entered exactly once per
// execution of P, immediately after it, by no other route. So the memory state
// at C's entry IS the state at P's exit — the same statement the block-local
// walk already makes about two adjacent instructions, applied to two adjacent
// BLOCKS — and C's table may be seeded with P's. The three side conditions are
// what make it a proof rather than an analogy:
//   * `preds(C) = {P}`, so no other path can reach C with a different memory;
//   * P is visited before C (reverse postorder), so a BACK edge — where P has
//     not run yet in the walk, and where the loop body may have written since —
//     seeds nothing;
//   * a carried entry loses its `Some(at)` deletion candidacy. A store in P may
//     be forwarded to a load in C (memory is a function; reading what was
//     written returns it), but DELETING that store because C overwrites it would
//     need C to always follow P, and P may have other successors.
// The value a carried entry names is defined in P or above it, and P dominates
// C — its only predecessor — so it dominates every use in C.
//
// This is the case §13n row (h) measured on j5: `p[j]` is loaded in the loop's
// condition and loaded AGAIN in the body, which has that condition block as its
// only predecessor.
use super::*;

/// A memory location, as precisely as the oracle can name it.
#[derive(Clone, PartialEq)]
enum Loc {
    /// a byte range of a stack slot
    Slot(u32, i64),
    /// a linker symbol — the whole object, since the offset is not tracked
    Sym(Sym),
    /// through a pointer value, at a constant displacement
    Ptr(ValueId, i64),
}

fn disjoint(a: &(Loc, u32), b: &(Loc, u32)) -> bool {
    match (&a.0, &b.0) {
        (Loc::Slot(s1, o1), Loc::Slot(s2, o2)) => {
            s1 != s2 || o1 + a.1 as i64 <= *o2 || o2 + b.1 as i64 <= *o1
        }
        // no object has both automatic and static storage duration
        (Loc::Slot(..), Loc::Sym(_)) | (Loc::Sym(_), Loc::Slot(..)) => true,
        (Loc::Sym(x), Loc::Sym(y)) => x != y,
        (Loc::Ptr(p, o1), Loc::Ptr(q, o2)) => {
            // the same pointer at displacements that do not overlap
            p == q && (o1 + a.1 as i64 <= *o2 || o2 + b.1 as i64 <= *o1)
        }
        _ => false,
    }
}

fn same(a: &(Loc, u32), b: &(Loc, u32)) -> bool {
    a.1 == b.1 && a.0 == b.0
}

/// What memory is known to hold: the location, the type it was accessed at, the
/// value, and — for a store — its instruction index, so a later store that makes
/// it invisible can delete it.
type Avail = Vec<((Loc, u32), Ty, Operand, Option<usize>)>;

/// THEORY A7b  SQUARE a_second_read_of_the_same_place_is_the_first — the alias oracle
pub fn run(f: &mut Func) -> bool {
    // where each value's address comes from
    let mut addr: Vec<Option<Loc>> = vec![None; f.values.len()];
    for b in &f.blocks {
        for inst in &b.insts {
            match inst {
                Inst::SlotAddr { dst, slot, off } => addr[*dst as usize] = Some(Loc::Slot(*slot, *off)),
                Inst::SymAddr { dst, sym } => addr[*dst as usize] = Some(Loc::Sym(sym.clone())),
                _ => {}
            }
        }
    }
    let loc_of = |o: Operand, ty: Ty| -> Option<(Loc, u32)> {
        match o {
            Operand::Val(v) => Some((
                addr[v as usize].clone().unwrap_or(Loc::Ptr(v, 0)),
                ty.bytes(),
            )),
            _ => None,
        }
    };

    let mut map: Vec<Option<Operand>> = vec![None; f.values.len()];
    let mut dead: Vec<(usize, usize)> = Vec::new();
    // R4.9: the table each block LEAVES, so a single-predecessor successor can
    // start from it instead of from nothing. Reverse postorder is what makes the
    // predecessor's entry already present when its successor is reached.
    let cfg = dom::cfg(f);
    let mut exit: Vec<Option<Avail>> = vec![None; f.blocks.len()];
    for &blk in &cfg.rpo {
        let b = blk as usize;
        // what each location is known to hold, and — for a STORE — where the
        // instruction that put it there sits, so it can be deleted if a later
        // store makes it invisible
        let mut avail: Avail = match cfg.preds[b].as_slice() {
            [p] => exit[*p as usize]
                .clone()
                .map(|t| {
                    // carried in, a store is no longer a deletion candidate: C
                    // does not always follow P
                    t.into_iter().map(|(k, ty, v, _)| (k, ty, v, None)).collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for i in 0..f.blocks[b].insts.len() {
            match f.blocks[b].insts[i].clone() {
                Inst::Load { dst, ty, addr: a, vol: false, .. } => {
                    let key = match loc_of(a, ty) {
                        Some(k) => k,
                        // an address the oracle cannot name may be anything
                        None => {
                            avail.clear();
                            continue;
                        }
                    };
                    // A read the load does NOT get forwarded is a read of
                    // memory, and it makes every store it may alias OBSERVABLE —
                    // so those stores stop being candidates for deletion. This is
                    // the type-punning case: `*(double*)p = x; l = *(long*)p;`
                    // reads the store through a different type, and deleting the
                    // store because a later one overwrites it loses `l`.
                    let mut hit = None;
                    for e in avail.iter_mut() {
                        if same(&e.0, &key) && e.1 == ty && hit.is_none() {
                            hit = Some(e.2);
                        } else if !disjoint(&e.0, &key) {
                            e.3 = None;
                        }
                    }
                    match hit {
                        Some(val) => {
                            map[dst as usize] = Some(val);
                            dead.push((b, i));
                        }
                        None => avail.push((key, ty, Operand::Val(dst), None)),
                    }
                }
                Inst::Store { ty, addr: a, val, vol: false, .. } => {
                    let key = match loc_of(a, ty) {
                        Some(k) => k,
                        None => {
                            avail.clear();
                            continue;
                        }
                    };
                    // the store this one makes invisible, if any
                    if let Some(pos) = avail
                        .iter()
                        .position(|(k, t, _, at)| at.is_some() && same(k, &key) && *t == ty)
                    {
                        if let Some(at) = avail[pos].3 {
                            dead.push((b, at));
                        }
                        avail.remove(pos);
                    }
                    avail.retain(|(k, ..)| disjoint(k, &key));
                    avail.push((key, ty, val, Some(i)));
                }
                // A PURE instruction touches no memory and must not disturb the
                // table — the address computation for the next access is itself
                // an instruction, and clearing on it would defeat the pass.
                other if other.effect() == Effect::Pure => {}
                // Anything opaque may read or write any object: an intervening
                // read also makes an earlier store visible, so the table cannot
                // simply be filtered — it goes.
                _ => avail.clear(),
            }
        }
        exit[b] = Some(avail);
    }
    if dead.is_empty() && map.iter().all(|x| x.is_none()) {
        return false;
    }
    rewrite_values(f, &map);
    dead.sort_unstable();
    dead.dedup();
    for &(b, i) in dead.iter().rev() {
        f.blocks[b].insts.remove(i);
    }
    refresh_defs(f);
    true
}
