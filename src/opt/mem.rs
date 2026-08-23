// src/opt/mem.rs — memory — the alias oracle and load-after-store elimination.
// One optimization family per file (see opt/mod.rs). Semantics-preservation is
// proved in opt::tests via the commuting square; a pure code-move leaves the
// emitted .s byte-identical (determinism seal).

use super::*;

/// The base of an address value — the 4-point lattice (Side-I). `Sym` carries a kind
/// (0 = global, 1 = string literal) so a global and a string never share an identity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ABase {
    Loc(u32),      // stack slot @ frame offset
    Sym(u8, u32),  // (kind, interned id)
    Con,           // pure integer-constant address
    Unk(Tmp),      // unknown; base is the temp itself
}


#[derive(Clone, Copy, Debug)]
pub(crate) struct Alias {
    pub base: ABase,
    pub off: i64,
}


#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AliasR {
    No,
    Must,
    May,
}


/// The alias oracle for one function: a per-temp descriptor + the escaped-slot set.
pub(crate) struct AliasInfo {
    a: Vec<Alias>,
    escaped: HashSet<u32>,
}


/// getalias(Val): a temp reads its descriptor; a constant is a `Con` address at its
/// value; a float bit-pattern is never an address (a distinct Unknown that matches
/// nothing real).
pub(crate) fn getal(a: &[Alias], v: &Val) -> Alias {
    match v {
        Val::Tmp(t) => a[*t as usize],
        Val::Imm(x) => Alias { base: ABase::Con, off: *x },
        Val::FImm(_) => Alias { base: ABase::Unk(u32::MAX), off: 0 },
    }
}


impl AliasInfo {
    fn get(&self, v: Val) -> Alias {
        getal(&self.a, &v)
    }
    /// Is this base a PROVABLY-LOCAL stack slot (never leaked)? Such a slot cannot be
    /// reached through any unknown pointer.
    fn is_local(&self, b: ABase) -> bool {
        matches!(b, ABase::Loc(off) if !self.escaped.contains(&off))
    }
    /// Is a Load of `addr` FAULT-FREE, hence safe to SPECULATE past a branch (B4
    /// if-conversion)? A stack slot (`Loc`) and a symbol address (`Sym`, a global /
    /// string) are always MAPPED — reading them cannot trap. A pure integer-constant
    /// address (`Con`) or an unknown pointer (`Unk`) may be null/dangling ⟹ NOT safe.
    /// (Escape is irrelevant here: an escaped local is still a valid, mapped address.)
    pub(crate) fn fault_free(&self, addr: Val) -> bool {
        matches!(self.get(addr).base, ABase::Loc(_) | ABase::Sym(_, _))
    }
    /// The decidable alias relation. `sp`/`sq` are the byte widths of the two accesses.
    pub(crate) fn alias(&self, p: Val, sp: u32, q: Val, sq: u32) -> AliasR {
        use ABase::*;
        let (ap, aq) = (self.get(p), self.get(q));
        let ovlap = ap.off < aq.off + sq as i64 && aq.off < ap.off + sp as i64;
        let ov = |b| if b { AliasR::Must } else { AliasR::No };
        match (ap.base, aq.base) {
            // both stack: same slot ⟹ overlap decides; different slots are disjoint.
            (Loc(x), Loc(y)) => {
                if x == y {
                    ov(ovlap)
                } else {
                    AliasR::No
                }
            }
            // both symbolic: same symbol ⟹ overlap decides; different ⟹ conservatively May.
            (Sym(k1, i1), Sym(k2, i2)) => {
                if (k1, i1) != (k2, i2) {
                    AliasR::May
                } else {
                    ov(ovlap)
                }
            }
            (Con, Con) => ov(ovlap),
            (Unk(x), Unk(y)) if x == y => ov(ovlap),
            // one side unknown vs a non-provably-local base ⟹ May; otherwise the two
            // bases are disjoint memory regions (a non-escaped local, or two distinct
            // kinds among loc/sym/con) ⟹ No.
            _ => {
                if matches!(ap.base, Unk(_)) && !self.is_local(aq.base) {
                    AliasR::May
                } else if matches!(aq.base, Unk(_)) && !self.is_local(ap.base) {
                    AliasR::May
                } else {
                    AliasR::No
                }
            }
        }
    }
}


/// Intern a symbol name → a small dense id (so equality is an integer compare).
pub(crate) fn intern(syms: &mut HashMap<String, u32>, name: &str) -> u32 {
    if let Some(&id) = syms.get(name) {
        id
    } else {
        let id = syms.len() as u32;
        syms.insert(name.to_string(), id);
        id
    }
}


/// Compute the alias oracle: ONE RPO pass filling per-temp descriptors and the
/// escaped-slot set. In SSA (and in straight lowering for address computations) a def
/// precedes its uses, so an operand's descriptor is ready when read; callers query
/// only AFTER this returns, so the escaped set is complete before any verdict.
pub(crate) fn alias_info(f: &IrFunc) -> AliasInfo {
    let n = f.temps.len();
    let mut a: Vec<Alias> = (0..n).map(|t| Alias { base: ABase::Unk(t as Tmp), off: 0 }).collect();
    let mut escaped: HashSet<u32> = HashSet::new();
    let mut syms: HashMap<String, u32> = HashMap::new();

    // Mark the stack slot behind a Val as escaped (leaked). Non-Loc values carry no slot.
    let esc = |escaped: &mut HashSet<u32>, a: &[Alias], v: &Val| {
        if let Val::Tmp(t) = v {
            if let ABase::Loc(off) = a[*t as usize].base {
                escaped.insert(off);
            }
        }
    };

    for b in rpo(f) {
        let blk = &f.blocks[b as usize];
        for i in &blk.insts {
            // 1. the descriptor of the destination (if any).
            match i {
                Inst::Lea(d, Place::Local(off)) => {
                    a[*d as usize] = Alias { base: ABase::Loc(*off), off: 0 }
                }
                Inst::Lea(d, Place::Global(name, off)) => {
                    let id = intern(&mut syms, name);
                    a[*d as usize] = Alias { base: ABase::Sym(0, id), off: *off }
                }
                Inst::Lea(d, Place::Str(s)) => {
                    a[*d as usize] = Alias { base: ABase::Sym(1, *s), off: 0 }
                }
                Inst::Copy(d, _, v) => a[*d as usize] = getal(&a, v), // propagate
                // pointer ± constant offset stays on the SAME base (a ring identity on
                // the address); pointer + variable / pointer + pointer is untrackable.
                Inst::Bin(d, Op::Add, _, x, y) => {
                    let (ax, ay) = (getal(&a, x), getal(&a, y));
                    a[*d as usize] = if ax.base == ABase::Con {
                        Alias { base: ay.base, off: ay.off.wrapping_add(ax.off) }
                    } else if ay.base == ABase::Con {
                        Alias { base: ax.base, off: ax.off.wrapping_add(ay.off) }
                    } else {
                        Alias { base: ABase::Unk(*d), off: 0 }
                    };
                }
                Inst::Bin(d, Op::Sub, _, x, y) => {
                    let (ax, ay) = (getal(&a, x), getal(&a, y));
                    a[*d as usize] = if ay.base == ABase::Con {
                        Alias { base: ax.base, off: ax.off.wrapping_sub(ay.off) }
                    } else {
                        Alias { base: ABase::Unk(*d), off: 0 }
                    };
                }
                _ => {
                    if let Some(d) = inst_def(i) {
                        a[d as usize] = Alias { base: ABase::Unk(d), off: 0 }
                    }
                }
            }
            // 2. escape: an instruction whose result is NOT a tracked base leaks its
            // pointer operands — EXCEPT a bare dereference (load/store through an
            // address does not leak the address; a store leaks the stored VALUE only).
            let leaks = match inst_def(i) {
                None => true,
                Some(d) => matches!(a[d as usize].base, ABase::Unk(_)),
            };
            if leaks {
                match i {
                    Inst::Load(..) => {} // addr dereferenced, not leaked
                    Inst::Store(_, _addr, val) => esc(&mut escaped, &a, val),
                    _ => {
                        let mut us = Vec::new();
                        inst_uses(i, &mut us);
                        for u in us {
                            esc(&mut escaped, &a, &Val::Tmp(u));
                        }
                    }
                }
            }
        }
        // a pointer flowing out through the terminator (return value / branch) leaks.
        let mut tu = Vec::new();
        term_uses(&blk.term, &mut tu);
        for u in tu {
            esc(&mut escaped, &a, &Val::Tmp(u));
        }
    }
    AliasInfo { a, escaped }
}

// ─────────────────────────────────────────────────────────────────────────────
// B2 — LOAD ELIMINATION / STORE→LOAD FORWARDING (gated by the B1 alias oracle).
// [Side-I theorem — OPT.md §5 (B2), ported from QBE `load.c`.]
//
// THEOREM. Within a straight-line block, a `Load` from `a` (width sa) whose value is
// already available in memory — because a preceding `Store` wrote a MUST-alias
// address with the same width, with NO intervening MAY-alias store — equals that
// stored value ⟹ forward it (Load → Copy). A load whose value was produced by an
// earlier same-width must-alias load is likewise redundant. The alias oracle also
// lets an available value SURVIVE a store it provably does-not-alias (block-local
// `cse` conservatively kills every load at any store; the oracle keeps NoAlias ones).
//
// This is BLOCK-LOCAL: within one block the instruction sequence is executed in order
// with no incoming edges mid-block, so "preceding" = "dominating on the only path" and
// no dominance machinery is needed. Floats are excluded (a `float`(4) store rounds to
// f32 on the way to memory, but `Copy` does not round — forwarding would skip the
// rounding). Any opaque memory writer (call / memcpy / zero / exotic) conservatively
// clears the available set.
//
// PROOF OBLIGATION (Law 3):
//   • correctness — the commuting square `⟦f⟧=⟦load-elim(f)⟧` via `equiv`: interp's
//     per-frame `mem` models intra-function store→load, so a forwarded local load is
//     machine-validated; global/unknown loads are unmodeled → `equiv` SKIP, correct
//     by the B1 soundness theorem (the oracle proved must/no-alias) + the box torture.
//   • improvement — the emitted `.s` has strictly FEWER `ldr` (each forwarded load
//     becomes a register copy the peephole then folds). Statically visible, no race.
// ─────────────────────────────────────────────────────────────────────────────


/// Is the stored value `val` guaranteed to be in BACKEND-CANONICAL form for a `ty`
/// access — i.e. does forwarding it (a register `Copy`) reproduce what the memory
/// round-trip (`str`+`ldr`, which sign/zero-extends to `ty`) would yield?
///
/// [Side-II fact — the arm64 backend's canonicalization timing.] A temp's REGISTER
/// value is canonical to its type only for width ≥ 4 (`ir_bin_r` ext-s width-4 results;
/// loads ext all widths — but width-1/2 arithmetic results are left wide, canonicalized
/// lazily at the `strb`/`ldrb` boundary). A store→load forward that assumes canonicity
/// on a width-1/2 value would skip that truncation (GCC torture pr81913: `u8 d--`
/// forwarded as a full-width `-1` instead of the wrapped `255`). interp canonicalizes
/// EAGERLY at every `Bin`, so `equiv` is blind to this — the box torture is the oracle.
/// Constants forward when they already fit `ty` (the backend materializes the same bits).
pub(crate) fn fwd_canonical(tt: &TyTab, ty: TypeId, val: Val) -> bool {
    if tt.is_float(ty) || matches!(tt.tys[ty as usize], Ty::Bitfield(..)) {
        return false;
    }
    match val {
        Val::FImm(_) => false,
        Val::Imm(x) => canon(tt, ty, x) == x, // an in-range constant round-trips identically
        Val::Tmp(_) => tt.size(ty) >= 4,      // width-4/8 def is register-canonical; width-1/2 is not
    }
}


/// Does this instruction potentially WRITE memory (⟹ clear the available set)? The
/// allowlist is the pure, memory-read-or-less set; anything else (incl. a future
/// exotic) kills by default — never silently retaining a value across an unknown write.
pub(crate) fn pure_mem(i: &Inst) -> bool {
    matches!(
        i,
        Inst::Bin(..)
            | Inst::Un(..)
            | Inst::Copy(..)
            | Inst::Load(..)
            | Inst::Lea(..)
            | Inst::Cast(..)
            | Inst::FunAddr(..)
            | Inst::LabelAddr(..)
            | Inst::VaArea(..)
            | Inst::Param(..)
            | Inst::Phi(..)
    )
}


pub fn load_elim(tt: &TyTab, f: &mut IrFunc) -> u32 {
    let ai = alias_info(f);
    let mut n = 0u32;
    for b in f.blocks.iter_mut() {
        // available memory contents, in program order: (address, byte width, type, value).
        let mut avail: Vec<(Val, u32, TypeId, Val)> = Vec::new();
        for i in b.insts.iter_mut() {
            // C99 6.7.3 — a volatile access is a barrier: never FORWARD a volatile store's
            // value to a later load (that would elide the load), never forward/cache a volatile
            // load (each read must execute). Clear the available set and leave the access intact.
            if is_volatile_access(tt, i) {
                avail.clear();
                continue;
            }
            match i {
                Inst::Store(ty, addr, v) => {
                    let sz = tt.size(*ty);
                    // a store INVALIDATES every value it may-alias (Must or May); keep
                    // only the provably-disjoint (NoAlias) entries.
                    let (addr, v, ty) = (*addr, *v, *ty);
                    avail.retain(|(a2, s2, _, _)| ai.alias(addr, sz, *a2, *s2) == AliasR::No);
                    if !tt.is_float(ty) {
                        avail.push((addr, sz, ty, v)); // this exact address now holds `v`
                    }
                }
                Inst::Load(d, ty, addr) => {
                    let (d, ty, addr) = (*d, *ty, *addr);
                    let sz = tt.size(ty);
                    // forward a must-alias, same-width, same-type available value —
                    // only when its register form is canonical for `ty` (see fwd_canonical).
                    let hit = avail.iter().find(|(a2, s2, t2, v)| {
                        *t2 == ty
                            && *s2 == sz
                            && fwd_canonical(tt, ty, *v)
                            && ai.alias(addr, sz, *a2, *s2) == AliasR::Must
                    });
                    if let Some(&(_, _, _, val)) = hit {
                        *i = Inst::Copy(d, ty, val);
                        n += 1;
                    } else if !tt.is_float(ty) {
                        avail.push((addr, sz, ty, Val::Tmp(d))); // this load's value is now cached
                    }
                }
                other if !pure_mem(other) => avail.clear(), // opaque memory writer
                _ => {}                                      // pure non-memory op
            }
        }
    }
    n
}

// ─────────────────────────────────────────────────────────────────────────────
// Pass 5 — REGISTER ALLOCATION (graph coloring, Chaitin–Briggs).
//
// NP-complete (THEORY §C2 — graph coloring) ⟹ use a HEURISTIC simplify/spill, NOT
// demanding a strict optimum. But CORRECTNESS (a valid coloring) is verifiable in P.
//
// Correctness here DIFFERS from the four passes above: interp does NOT model
// registers, so ⟦before⟧=⟦after⟧ cannot be used. The correctness invariant is
// RENAMING BISIMULATION (THEORY §A7): the register-assigned program is bisimilar to
// the temporary program ⟺ two SIMULTANEOUSLY LIVE temporaries always occupy DIFFERENT
// locations (a live value is never overwritten). We check the INTERFERENCE INVARIANT
// mechanically:
//   ∀ edge (u,v) ∈ interference-graph, color[u] ≠ color[v]  (a spill = its own slot, never overwritten).
//
// Chain of theorems: liveness (monotone dataflow, Kleene fixpoint) → interference
// graph (u interferes with v ⟺ both live at some def) → coloring (simplify degree<k / spill) → verify.
// ─────────────────────────────────────────────────────────────────────────────

