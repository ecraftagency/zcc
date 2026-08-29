// HIR → MIR instruction selection (MECHANISM.md §G6, §12 R0.6).
// THEORY A5 — instruction selection; THEORY II-5 — the A64 encodings it targets
//
// R0 shipped the BASE COVER: one HIR instruction becomes one canonical machine
// sequence, with no munching of operand trees. That was not a shortcut to be
// grown out of by patching — it is the base case, proven correct, on top of
// which R3.1 added the MUNCH TABLE (addressing modes, cmp-branch fusion,
// madd/msub, bfx, extend folding). The rows live in `munch` (below) and their
// `lower` arms, NOT in a separate `pattern.rs` — each with its own
// `⟦hir-tree⟧ = ⟦mir-seq⟧` battery entry in `tests.rs`. `munch` is one pre-pass:
// the producer is emitted before the consumer, so which producers a consumer
// absorbs must be decided ahead of emission, not at it.
//
// The one non-obvious invariant, established in `hir::build`: HIR never performs
// arithmetic or comparison at I8/I16 (C99 6.3.1.1 promotes first). Narrow types
// appear only in `load`, `store` and `cvt`, which is exactly where A64 has a
// dedicated form — so no re-extension is ever needed around an ALU op.
use super::abi::{self, Loc};
use super::imm;
use crate::hir::{self, BinOp, CmpOp, CvtOp, Inst, Operand, Term, UnOp, ValueId};
use crate::mir::*;

/// The A/B seam for the commutative-immediate swap in `binop`. ON by default;
/// `ZCC_NOCOMMUTE=1` turns it off, which is the only way to take the paired
/// EXEC reading the ±0.007 session spread demands (`MECHANISM.md` Part E §6).
/// The A/B seam for the switch-arm ordering (`MEASURED M31`). ON by default;
/// `ZCC_NOARMORD=1` turns it off.
/// The A/B seam for the default's trampoline (`MEASURED M34`). ON by default;
/// `ZCC_NOJTDFLT=1` restores the old refusal.
fn jt_default() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_NOJTDFLT").is_err())
}

fn armord() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_NOARMORD").is_err())
}

fn commute() -> bool {
    static W: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *W.get_or_init(|| std::env::var("ZCC_NOCOMMUTE").is_err())
}

/// MEASURED M4 — the jump-table/compare-tree crossover, UNSETTLED
/// The jump-table density threshold (MECHANISM.md Part F R4.14 (2)) — the case count at
/// which a table beats a compare tree on THIS machine, taken on the clock.
/// The arm count at which a JUMP TABLE beats a linear chain of equality tests.
///
/// MEASURED, not chosen (`MEASURED M4`, re-taken 2026-08-26). R3.3 used 4 by
/// taste and d1_switch paid for it: eight arms went through an indirect branch
/// at 1.500x gcc -O1 where the chain it replaced runs at 1.20-1.30x. A sweep at
/// 16/20/24/28/32 arms, with a pseudorandom index AND with a repeating one, puts
/// the crossover between 20 and 24 — the chain is better or equal to 20 on both,
/// the table wins from 24 on both. 24 is the first measured size where the table
/// actually wins; 21..23 were not measured and the constant does not pretend
/// otherwise.
///
/// A BALANCED SEARCH TREE was built and REFUTED here: it lost at every size from
/// 4 to 64 (at 16 arms: chain 62 ms, table 65, tree 84). It asks fewer questions
/// and takes more time, because the chain's tests FALL THROUGH while the tree
/// spends a taken branch per level and scatters the arms. See `ARM64.md`.
const MIN_CASES: usize = 24;

/// MEASURED M40 — the inline small-copy bound, in BYTES (M14 set this to 32 by
/// minimizing sqlite's STATIC instruction count; M40 re-derives it on the TIME
/// axis, where a `bl memcpy` costs a constant ~12 dynamic instructions the
/// static count cannot see, and the open-coded form wins at every size measured
/// out to 512). 128 is where sqlite's static cost stops moving: no
/// compiler-generated copy in the amalgamation exceeds it.
const INLINE_COPY_MAX: usize = 128;

/// The measurement seam for the bound above — `ZCC_ICM=<bytes>` overrides it so
/// the threshold can be re-swept without a rebuild per point, exactly as
/// `ZCC_NOHOIST` and `ZCC_LDSTP` are the seams for their own rows.
fn inline_copy_max() -> usize {
    use std::sync::OnceLock;
    static V: OnceLock<usize> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("ZCC_ICM")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(INLINE_COPY_MAX)
    })
}

pub fn lower(m: &hir::Module) -> MModule {
    MModule {
        funcs: m.funcs.iter().map(lower_func).collect(),
    }
}

fn wid(t: hir::Ty) -> Width {
    match t {
        hir::Ty::I8 | hir::Ty::I16 | hir::Ty::I32 => Width::W32,
        hir::Ty::I64 => Width::W64,
        hir::Ty::F32 => Width::S,
        hir::Ty::F64 => Width::D,
    }
}

/// The A64 access form for a load/store of this HIR type. A narrow LOAD is
/// zero-extending, which matches ⟦hir⟧: `Inst::Load` yields the raw bytes, and
/// any sign extension is a separate `Cvt` the C front end asked for.
fn memop(t: hir::Ty) -> MemOp {
    match t {
        hir::Ty::I8 => MemOp::B,
        hir::Ty::I16 => MemOp::H,
        hir::Ty::I32 => MemOp::W,
        hir::Ty::I64 => MemOp::X,
        hir::Ty::F32 => MemOp::S,
        hir::Ty::F64 => MemOp::D,
    }
}

fn cc_of(op: CmpOp) -> CC {
    match op {
        // integer
        CmpOp::Eq => CC::Eq,
        CmpOp::Ne => CC::Ne,
        CmpOp::Slt => CC::Lt,
        CmpOp::Sle => CC::Le,
        CmpOp::Sgt => CC::Gt,
        CmpOp::Sge => CC::Ge,
        CmpOp::Ult => CC::Lo,
        CmpOp::Ule => CC::Ls,
        CmpOp::Ugt => CC::Hi,
        CmpOp::Uge => CC::Hs,
        // floating (DDI 0487 C6.2.65: unordered sets C and V, clears N and Z).
        // Each ORDERED predicate has a single condition that is false when
        // unordered; C's `!=` is the UNORDERED one and is plain `ne`.
        CmpOp::FOeq => CC::Eq,
        CmpOp::FOlt => CC::Mi,
        CmpOp::FOle => CC::Ls,
        CmpOp::FOgt => CC::Gt,
        CmpOp::FOge => CC::Ge,
        CmpOp::FUne => CC::Ne,
        CmpOp::FUno => CC::Vs,
        // ordered-not-equal is the one predicate with no single A64 condition
        // (it is `mi || gt`); C never produces it — `!=` is FUne.
        CmpOp::FOne => unreachable!("ordered != has no single condition code"),
    }
}

/// The same relation with its operands exchanged. DDI 0487 C1.2.4's condition
/// table is symmetric under this, so `k < x` and `x > k` denote one comparison —
/// which lets a constant that C wrote on the LEFT reach A64's immediate field,
/// which exists only on the second source operand. Float predicates are left
/// alone: exchanging them also exchanges which side an unordered result favours.
fn swap_cmp(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Slt => CmpOp::Sgt,
        CmpOp::Sgt => CmpOp::Slt,
        CmpOp::Sle => CmpOp::Sge,
        CmpOp::Sge => CmpOp::Sle,
        CmpOp::Ult => CmpOp::Ugt,
        CmpOp::Ugt => CmpOp::Ult,
        CmpOp::Ule => CmpOp::Uge,
        CmpOp::Uge => CmpOp::Ule,
        _ => op,
    }
}

/// One end of a composite move: an address in a register, or a fixed offset in
/// the outgoing-argument area (which has no base register — it is always sp).
#[derive(Clone, Copy)]
enum Place {
    At(Reg),
    Out(u32),
    Slot(SlotId),
}

/// An address the memory operand can express by itself (MECHANISM.md §G6, the
/// addressing-mode rows of the munch table). Folding it is what removes the
/// `add` the R1 ground metric measured at 28.2% of sqlite's instructions.
#[derive(Clone, Copy)]
enum Folded {
    /// a frame object at a constant displacement
    Slot(u32, i32),
    /// a register base at a constant displacement
    Base(ValueId, i32),
    /// `[base, idx, ext #shift]` — an array subscript, addressed directly.
    /// `src` is the operand of the ADD that became the index, kept because the
    /// dead-marking below has to know WHICH side that was — guessing it wrote
    /// `use of undefined` into a yarpgen case (§13j).
    Indexed {
        src: ValueId,
        base: ValueId,
        idx: ValueId,
        ext: Option<ExtKind>,
        shift: u8,
    },
}

/// An instruction whose SECOND operand is produced by another instruction A64
/// can perform as part of this one: a shift, a 32→64 extension, or a whole
/// multiply (`madd`/`msub`).
#[derive(Clone, Copy)]
enum AluFold {
    /// `ubfx`/`sbfx`: a shift-and-mask pair that is one instruction
    Bfx(bool, ValueId, u8, u8),
    /// `a op (b <shift> amt)`
    Shifted(Operand, ValueId, ShiftKind, u8),
    /// `a op (b <ext>)`
    Extended(Operand, ValueId, ExtKind),
    /// `a*b + c` / `c − a*b`. The multiply's operands are OPERANDS, not values:
    /// a multiply by a literal has to materialize that literal into a register
    /// before `mul` can read it, so the register exists either way and `madd`
    /// absorbs the `add` for free.
    Mul3(Alu3Op, Operand, Operand, Operand),
}

/// A compare whose single use is a branch or a select: `(op, compared type,
/// lhs, rhs)`. It is not emitted where it stands — it is RE-EMITTED at its
/// consumer, so the flags live for one instruction and can never collide with
/// another compare (NZCV is a register class of size one).
type CmpSrc = (hir::CmpOp, hir::Ty, Operand, Operand);

#[derive(Default)]
struct Munch {
    addr: std::collections::HashMap<ValueId, Folded>,
    alu: std::collections::HashMap<ValueId, AluFold>,
    /// a block whose branch tests a single-use compare directly
    br: std::collections::HashMap<hir::BlockId, CmpSrc>,
    /// a select whose condition is a single-use compare
    sel: std::collections::HashMap<ValueId, CmpSrc>,
    /// a narrow load whose single consumer is a `sext`: the load performs the
    /// extension itself (`ldrsb`/`ldrsh`/`ldrsw`) and writes the EXTENSION's
    /// register, so the `Cvt` is never emitted
    ldext: std::collections::HashMap<ValueId, (MemOp, ValueId)>,
    /// a block whose branch is a single-bit test: `(value, type, bit, branch
    /// when the bit is SET)`
    tb: std::collections::HashMap<hir::BlockId, (ValueId, hir::Ty, u8, bool)>,
    /// a select whose false arm is a negation, complement or increment of a
    /// value the true arm already names: `(form, kept operand, the negated /
    /// complemented / incremented source, invert the condition)`
    csop: std::collections::HashMap<ValueId, (CSelOp, Operand, ValueId, bool)>,
    /// producers whose every use folded, so they are never emitted
    dead: std::collections::HashSet<ValueId>,
}

/// The munch table's single pass: decide, for every consumer, which producers it
/// absorbs. It has to be a PASS rather than a decision taken at emission time
/// because the producer is emitted first — the consumer's choice has to be known
/// before the producer's turn comes.
///
/// Two different licences appear here and they are not interchangeable:
///   * an ADDRESS folds when EVERY use of it is a memory operand that can hold
///     it — folding into some uses while still computing it for others would
///     only duplicate work;
///   * an ALU operand folds when it has exactly ONE use, since the shift or
///     extension is performed inside the consumer and is not available to
///     anyone else.
fn munch(h: &hir::Func) -> Munch {
    use std::collections::{HashMap, HashSet};
    let n = h.values.len();
    let mut uses = vec![0u32; n];
    let mut def: Vec<Option<&hir::Inst>> = vec![None; n];
    // Which block defines each value. A fold that absorbs a producer from
    // ANOTHER block moves that producer's work to wherever the consumer runs —
    // for a shift or an extension that is free (it rides inside the consumer's
    // encoding), but a MULTIPLY is a multiply, and pulling one that LICM has
    // just hoisted back into the loop body is a de-optimization dressed as a
    // munch row (d2_nested_loops: `madd` every inner iteration).
    let mut blk = vec![u32::MAX; n];
    for (bi, b) in h.blocks.iter().enumerate() {
        for inst in &b.insts {
            if let Some(d) = inst.dst() {
                blk[d as usize] = bi as u32;
            }
        }
    }
    for b in &h.blocks {
        for inst in &b.insts {
            if let Some(d) = inst.dst() {
                def[d as usize] = Some(inst);
            }
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    uses[v as usize] += 1;
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                uses[v as usize] += 1;
            }
        });
    }
    let bin = |v: ValueId| -> Option<(hir::BinOp, hir::Ty, Operand, Operand)> {
        match def.get(v as usize)?.as_ref()? {
            hir::Inst::Bin { op, ty, a, b, .. } => Some((*op, *ty, *a, *b)),
            _ => None,
        }
    };
    let cvt = |v: ValueId| -> Option<(hir::CvtOp, hir::Ty, hir::Ty, Operand)> {
        match def.get(v as usize)?.as_ref()? {
            hir::Inst::Cvt { op, from, to, a, .. } => Some((*op, *from, *to, *a)),
            _ => None,
        }
    };
    // ── extending loads (§17 row "extending loads", R4.7) ──────────────────
    // DDI 0487 C6.2: `ldrsb`/`ldrsh`/`ldrsw` sign-extend INSIDE the load, so a
    // `sext` whose only source is a narrow load is performed by the load and
    // needs no instruction of its own. TWO things fall out, and the second is
    // the reason this row sits first in R4's order:
    //   * the `sxtb`/`sxth` disappears — one instruction (sqlite: 687 `sxth`
    //     against gcc's 90, 0 `ldrsh` against gcc's 492);
    //   * the value arrives ALREADY WIDE, so a consumer that would otherwise
    //     have absorbed the extension as an ALU OPERAND takes the plain form.
    //     `add x1,x1,w5,sxtw` is an extended-register ALU op at 2 cycles where
    //     `ldrsw x5,[…]` + `add x1,x1,x5` is 1 — IDENTICAL instruction count,
    //     half the latency on a loop-carried recurrence. `cost = |MIR|` cannot
    //     see that, which is the cost-model caveat R4.7 records (j3, 1.94×).
    // The extension REPLACES the load's own destination register: the load has
    // exactly one use (checked), so nothing reads the raw narrow value, and
    // writing the wide register at the load's own position moves nothing.
    let mut ldext: HashMap<ValueId, (MemOp, ValueId)> = HashMap::new();
    let mut in_load: HashSet<ValueId> = HashSet::new();
    for b in &h.blocks {
        for inst in &b.insts {
            let hir::Inst::Cvt { dst, op: hir::CvtOp::Sext, from, to, a: Operand::Val(v) } = inst
            else {
                continue;
            };
            if uses[*v as usize] != 1 {
                continue;
            }
            match def.get(*v as usize).and_then(|d| *d) {
                Some(hir::Inst::Load { ty, .. }) if ty == from => {}
                _ => continue,
            }
            let op = match (from, to) {
                (hir::Ty::I8, hir::Ty::I32) => MemOp::SB,
                (hir::Ty::I8, hir::Ty::I64) => MemOp::SBX,
                (hir::Ty::I16, hir::Ty::I32) => MemOp::SH,
                (hir::Ty::I16, hir::Ty::I64) => MemOp::SHX,
                (hir::Ty::I32, hir::Ty::I64) => MemOp::SW,
                _ => continue,
            };
            ldext.insert(*v, (op, *dst));
            in_load.insert(*dst);
        }
    }

    // `sext`/`zext` from I32 to I64 — the two extensions the operand field holds
    let ext_of = |v: ValueId| -> Option<(ValueId, ExtKind)> {
        // …unless the LOAD already performed it: then there is no extension
        // left to absorb, and the value is a full-width register.
        if in_load.contains(&v) {
            return None;
        }
        let (op, from, to, a) = cvt(v)?;
        let src = a.val()?;
        if from != hir::Ty::I32 || to != hir::Ty::I64 {
            return None;
        }
        match op {
            hir::CvtOp::Sext => Some((src, ExtKind::Sxtw)),
            hir::CvtOp::Zext => Some((src, ExtKind::Uxtw)),
            _ => None,
        }
    };
    // `x << k`, possibly of an extended value
    let scaled = |v: ValueId| -> Option<(ValueId, Option<ExtKind>, u8)> {
        match bin(v) {
            Some((hir::BinOp::Shl, hir::Ty::I64, a, Operand::Imm(k))) if (0..=4).contains(&k) => {
                let src = a.val()?;
                match ext_of(src) {
                    Some((s, e)) if uses[src as usize] == 1 => Some((s, Some(e), k as u8)),
                    _ => Some((src, None, k as u8)),
                }
            }
            _ => match ext_of(v) {
                Some((s, e)) => Some((s, Some(e), 0)),
                None => None,
            },
        }
    };

    let mut m = Munch::default();
    for d in &in_load {
        m.dead.insert(*d);
    }
    m.ldext = ldext;
    // ── compares that feed exactly one consumer ────────────────────────────
    // A `cmp` followed by `cset` followed by `cbnz` is three instructions where
    // the machine wants one compare and one conditional branch; sqlite paid
    // 3,859 `cset` against gcc's 374 for it.
    let cmp_of = |v: ValueId| -> Option<CmpSrc> {
        if uses[v as usize] != 1 {
            return None;
        }
        match def.get(v as usize)?.as_ref()? {
            hir::Inst::Cmp { op, ty, a, b, .. } => Some((*op, *ty, *a, *b)),
            _ => None,
        }
    };
    for (bi, b) in h.blocks.iter().enumerate() {
        if let Term::Br(Operand::Val(c), ..) = &b.term {
            if let Some(src) = cmp_of(*c) {
                m.br.insert(bi as hir::BlockId, src);
                m.dead.insert(*c);
            }
        }
        for inst in &b.insts {
            if let hir::Inst::Select { dst, c: Operand::Val(c), .. } = inst {
                if let Some(src) = cmp_of(*c) {
                    m.sel.insert(*dst, src);
                    m.dead.insert(*c);
                }
            }
        }
    }
    // ── single-bit tests (§17 "compare-and-branch", R4.7) ──────────────────
    // `if (x & (1<<k))` is `and` + `cbz` here and ONE `tbz`/`tbnz` on the
    // machine (DDI 0487 C6.2.375). The sign-bit case is already handled at the
    // terminator; this is the general bit, which sqlite pays 1,721-against-326
    // for. The mask must be a single bit — a wider mask has no `tb` form, and
    // `tst` + `b.cc` is two instructions exactly like `and` + `cbz`, so there
    // is nothing to win there (recorded as category (a) for the `tst` row).
    // `v = x & (1<<k)` used once as a truth value, `set` = branch when the bit is 1.
    let one_bit = |v: ValueId| -> Option<(ValueId, hir::Ty, u8)> {
        if uses[v as usize] != 1 {
            return None;
        }
        let (hir::BinOp::And, ty, x, y) = bin(v)? else { return None };
        let (src, mask) = match (x, y) {
            (Operand::Val(s), Operand::Imm(k)) | (Operand::Imm(k), Operand::Val(s)) => (s, k),
            _ => return None,
        };
        let m = mask as u64;
        if mask <= 0 || !m.is_power_of_two() || m.trailing_zeros() >= ty.bits() {
            return None;
        }
        Some((src, ty, m.trailing_zeros() as u8))
    };
    let mut tb: HashMap<hir::BlockId, (ValueId, hir::Ty, u8, bool)> = HashMap::new();
    let mut tb_dead: Vec<ValueId> = Vec::new();
    for (bi, b) in h.blocks.iter().enumerate() {
        let blk = bi as hir::BlockId;
        // Two spellings reach the same branch: `if (x & 8)`, which C tests
        // against zero directly and HIR carries as `br(value)`, and
        // `if ((x & 8) != 0)`, which arrives as a fused compare.
        let (masked, cty, set) = match (m.br.get(&blk).copied(), &b.term) {
            (Some((op, ty, a, c)), _)
                if matches!(op, hir::CmpOp::Eq | hir::CmpOp::Ne) && !ty.is_float() =>
            {
                match (a, c) {
                    (Operand::Val(v), Operand::Imm(0)) | (Operand::Imm(0), Operand::Val(v)) => {
                        (v, Some(ty), op == hir::CmpOp::Ne)
                    }
                    _ => continue,
                }
            }
            (None, Term::Br(Operand::Val(c), ..)) => (*c, None, true),
            _ => continue,
        };
        let Some((src, ty, bit)) = one_bit(masked) else { continue };
        if cty.is_some_and(|t| t != ty) {
            continue;
        }
        tb.insert(blk, (src, ty, bit, set));
        tb_dead.push(masked);
    }
    for v in tb_dead {
        m.dead.insert(v);
    }
    m.tb = tb;

    // ── the conditional-select family (§17, R4.7) ──────────────────────────
    // DDI 0487 C6.2.83-86: `csinc`/`csinv`/`csneg` apply an increment, a
    // complement or a negation to the SECOND source as part of the select. C
    // writes `c ? x : -x` and `c ? x+1 : x`, and each is two instructions here
    // — the arithmetic, then the select — where the machine has one. sqlite
    // emits none of the three against gcc's 94.
    //
    // COMMUTING SQUARE: `csneg d,x,x,cc` denotes `cc ? x : -x` by definition of
    // the instruction, which is the select's own denotation; the arithmetic
    // moves INTO the select and is otherwise unobserved (single use, checked).
    // When the transformed arm is the TRUE one the condition is inverted, which
    // is exact — `CC::invert` is the ISA's own pairing.
    let mut csop: HashMap<ValueId, (CSelOp, Operand, ValueId, bool)> = HashMap::new();
    for b in &h.blocks {
        for inst in &b.insts {
            let hir::Inst::Select { dst, ty, a, b: fb, .. } = inst else { continue };
            if ty.is_float() {
                continue;
            }
            // `v` performs `op` on `x`; the other arm must name `x` itself.
            let form = |v: ValueId| -> Option<(CSelOp, ValueId)> {
                if uses[v as usize] != 1 {
                    return None;
                }
                match def.get(v as usize).and_then(|d| *d) {
                    Some(hir::Inst::Un { op: hir::UnOp::Neg, ty: t, a, .. }) if t == ty => {
                        Some((CSelOp::Csneg, a.val()?))
                    }
                    Some(hir::Inst::Bin { op: hir::BinOp::Xor, ty: t, a, b, .. })
                        if t == ty && (*a == Operand::Imm(-1) || *b == Operand::Imm(-1)) =>
                    {
                        Some((CSelOp::Csinv, a.val().or_else(|| b.val())?))
                    }
                    Some(hir::Inst::Bin { op: hir::BinOp::Add, ty: t, a, b, .. })
                        if t == ty && (*a == Operand::Imm(1) || *b == Operand::Imm(1)) =>
                    {
                        Some((CSelOp::Csinc, a.val().or_else(|| b.val())?))
                    }
                    _ => None,
                }
            };
            let plan = fb
                .val()
                .and_then(form)
                .filter(|(_, x)| *a == Operand::Val(*x))
                .map(|(k, x)| (k, *a, x, false, fb.val().unwrap()))
                .or_else(|| {
                    let v = a.val()?;
                    let (k, x) = form(v)?;
                    (*fb == Operand::Val(x)).then_some((k, *fb, x, true, v))
                });
            if let Some((k, keep, x, inv, folded)) = plan {
                csop.insert(*dst, (k, keep, x, inv));
                m.dead.insert(folded);
            }
        }
    }
    m.csop = csop;

    // ── addresses ──────────────────────────────────────────────────────────
    let mut cand: HashMap<ValueId, Folded> = HashMap::new();
    for b in &h.blocks {
        for inst in &b.insts {
            match inst {
                hir::Inst::SlotAddr { dst, slot, off } => {
                    if let Ok(o) = i32::try_from(*off) {
                        cand.insert(*dst, Folded::Slot(*slot, o));
                    }
                }
                hir::Inst::Bin { dst, op: hir::BinOp::Add, ty: hir::Ty::I64, a, b } => {
                    let plan = match (*a, *b) {
                        (Operand::Val(v), Operand::Imm(k)) | (Operand::Imm(k), Operand::Val(v)) => {
                            i32::try_from(k).ok().map(|o| Folded::Base(v, o))
                        }
                        (Operand::Val(x), Operand::Val(y)) => {
                            // `base + index*scale`: whichever side is the scaled
                            // one is the index
                            let pick = |i: ValueId, base: ValueId| -> Option<Folded> {
                                // An extension the LOAD performed is already a
                                // full 64-bit register, so the address needs no
                                // extension in its index — and `src == idx` is
                                // safe to mark dead here because the value is
                                // defined by the load, not by the `Cvt`.
                                if in_load.contains(&i) {
                                    return Some(Folded::Indexed {
                                        src: i,
                                        base,
                                        idx: i,
                                        ext: None,
                                        shift: 0,
                                    });
                                }
                                if uses[i as usize] == 1 {
                                    let (idx, ext, shift) = scaled(i)?;
                                    return Some(Folded::Indexed { src: i, base, idx, ext, shift });
                                }
                                // A MULTIPLY-USED index cannot be PEELED — its
                                // `sext` or shift has other readers and must
                                // still be materialized — but the ADD folds
                                // anyway, as the plain 64-bit register-offset
                                // form `[base, idx]`. Requiring single use for
                                // the whole thing conflated the two and cost an
                                // `add` per access in the commonest loop there
                                // is: `d[i] = s[i]`, where one index feeds two
                                // addresses, folded for NEITHER (§13j). Both
                                // operands are I64 here, so reading the index as
                                // a 64-bit register is exact; and `[a, b]` is
                                // symmetric, so it does not matter which side of
                                // a multi-use pair is called the index.
                                Some(Folded::Indexed { src: i, base, idx: i, ext: None, shift: 0 })
                            };
                            pick(y, x).or_else(|| pick(x, y))
                        }
                        _ => None,
                    };
                    if let Some(p) = plan {
                        cand.insert(*dst, p);
                    }
                }
                _ => {}
            }
        }
    }
    let mut bad: Vec<ValueId> = Vec::new();
    let mut seen: HashSet<ValueId> = HashSet::new();
    for b in &h.blocks {
        for inst in &b.insts {
            let mem = match inst {
                hir::Inst::Load { ty, addr: Operand::Val(v), .. } => Some((*v, ty.bytes())),
                hir::Inst::Store { ty, addr: Operand::Val(v), .. } => Some((*v, ty.bytes())),
                _ => None,
            };
            if let Some((v, size)) = mem {
                seen.insert(v);
                match cand.get(&v) {
                    // DDI 0487 C3.2 bounds the displacement by the access size;
                    // `legalize` rescues a FRAME offset after layout, but a
                    // register base has no such second chance.
                    Some(Folded::Base(_, off)) if !isa::mem_off_ok(*off, size) => bad.push(v),
                    // C6.2: the register-offset form scales by exactly log2 of
                    // the access size, or not at all.
                    Some(Folded::Indexed { shift, .. })
                        if *shift != 0 && 1u32 << *shift != size =>
                    {
                        bad.push(v)
                    }
                    _ => {}
                }
            }
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    let is_addr = match (inst, mem) {
                        (hir::Inst::Load { addr, .. }, Some(_)) => *addr == Operand::Val(v),
                        (hir::Inst::Store { addr, val, .. }, Some(_)) => {
                            *addr == Operand::Val(v) && *val != Operand::Val(v)
                        }
                        _ => false,
                    };
                    if !is_addr {
                        bad.push(v);
                    }
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                bad.push(v);
            }
        });
    }
    for v in bad {
        cand.remove(&v);
    }
    cand.retain(|v, _| seen.contains(v));
    for (v, p) in &cand {
        m.dead.insert(*v);
        // The scale computation belongs to the address too — but ONLY when it was
        // peeled into the addressing mode, which is exactly when the operand
        // that became the index had a single use. A multiply-used index is read
        // as a register and must still be emitted.
        //
        // This used to guess which side of the add was the index by testing each
        // for `scaled`, and it guessed wrong: an `add` whose BASE happened to be
        // a single-use sign-extension had the base marked dead, and `dead` means
        // "not emitted", so its register was never defined. yarpgen `s0096`
        // printed `use of undefined v2659` (§13j). `src` records the choice
        // instead of re-deriving it.
        if let Folded::Indexed { src, .. } = p {
            if uses[*src as usize] == 1 {
                m.dead.insert(*src);
                if let Some((hir::BinOp::Shl, _, Operand::Val(inner), _)) = bin(*src) {
                    if uses[inner as usize] == 1 && ext_of(inner).is_some() {
                        m.dead.insert(inner);
                    }
                }
            }
        }
    }
    m.addr = cand;

    // ── ALU operands ───────────────────────────────────────────────────────
    // A producer may be absorbed only if it is single-use AND has not itself
    // already absorbed something: folding a consumer into a further consumer
    // would leave the value IT swallowed defined nowhere. (`(x << k) >> s` folded
    // into a `bfx`, then that `bfx` folded into an `and`, and `x << k` was gone.)
    let foldable = |v: ValueId, m: &Munch| {
        uses[v as usize] == 1 && !m.dead.contains(&v) && !m.alu.contains_key(&v)
    };
    let mut m2 = m;
    for (bi, b) in h.blocks.iter().enumerate() {
        for inst in &b.insts {
            let (dst, op, ty, a, bb) = match inst {
                hir::Inst::Bin { dst, op, ty, a, b } if !ty.is_float() => (*dst, *op, *ty, *a, *b),
                _ => continue,
            };
            // An instruction that is already DEAD — folded into an addressing
            // mode above — is never emitted, so nothing may be absorbed INTO it:
            // the absorbed value would be marked dead too and then defined
            // nowhere. yarpgen `s0096` printed `use of undefined v2659` for
            // exactly this once address folding began reaching the `add` that
            // an index feeds (§13j).
            if m2.dead.contains(&dst) {
                continue;
            }
            // A BITFIELD read: C spells it as a shift and a mask, or as a pair of
            // shifts, and A64 has one instruction for each (DDI 0487 C6.2.398).
            let bits = ty.bits();
            let bfx = match (op, a, bb) {
                // `(x >> s) & ((1 << w) - 1)`
                (hir::BinOp::And, Operand::Val(v), Operand::Imm(m))
                | (hir::BinOp::And, Operand::Imm(m), Operand::Val(v))
                    if m > 0 && (m as u64 + 1).is_power_of_two() =>
                {
                    let width = (m as u64 + 1).trailing_zeros() as u8;
                    match bin(v) {
                        Some((hir::BinOp::LShr, t, Operand::Val(x), Operand::Imm(sh)))
                            if t == ty
                                && foldable(v, &m2)
                                && sh >= 0
                                && sh as u32 + width as u32 <= bits =>
                        {
                            Some((v, false, x, sh as u8, width))
                        }
                        _ => None,
                    }
                }
                // `(x << k1) >> k2`, arithmetic or logical
                (hir::BinOp::AShr | hir::BinOp::LShr, Operand::Val(v), Operand::Imm(k2))
                    if k2 >= 0 && (k2 as u32) < bits =>
                {
                    match bin(v) {
                        Some((hir::BinOp::Shl, t, Operand::Val(x), Operand::Imm(k1)))
                            if t == ty && foldable(v, &m2) && k1 >= 0 && k2 >= k1 =>
                        {
                            let width = (bits - k2 as u32) as u8;
                            let lsb = (k2 - k1) as u8;
                            if width >= 1 && lsb as u32 + width as u32 <= bits {
                                Some((v, op == hir::BinOp::AShr, x, lsb, width))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((v, signed, x, lsb, width)) = bfx {
                m2.dead.insert(v);
                m2.alu.insert(dst, AluFold::Bfx(signed, x, lsb, width));
                continue;
            }
            // `a*b + c` and `c − a*b`: one instruction instead of two, and the
            // product's register disappears with it.
            // The multiply's operands are read as OPERANDS. Refusing the row when
            // the multiplier is a literal was a category-(b) truncation of §17
            // row 23: `a = a*1103515245 + 12345` had to materialize the literal
            // for the `mul` regardless, so the register was already there and the
            // `add` left behind was one the ISA never asked for (loops.c, where
            // `a` is the loop-carried recurrence every other value hangs off).
            let mul_of = |v: Operand| -> Option<(ValueId, Operand, Operand)> {
                let p = v.val()?;
                if !foldable(p, &m2) || blk[p as usize] != bi as u32 {
                    return None;
                }
                match bin(p)? {
                    (hir::BinOp::Mul, t, x, y) if t == ty => Some((p, x, y)),
                    _ => None,
                }
            };
            let mul3 = match op {
                hir::BinOp::Add => mul_of(bb)
                    .map(|p| (p, a))
                    .or_else(|| mul_of(a).map(|p| (p, bb))),
                hir::BinOp::Sub => mul_of(bb).map(|p| (p, a)),
                _ => None,
            };
            if let Some(((p, x, y), c)) = mul3 {
                let k = if op == hir::BinOp::Add { Alu3Op::Madd } else { Alu3Op::Msub };
                m2.dead.insert(p);
                m2.alu.insert(dst, AluFold::Mul3(k, x, y, c));
                continue;
            }
            if !matches!(
                op,
                hir::BinOp::Add
                    | hir::BinOp::Sub
                    | hir::BinOp::And
                    | hir::BinOp::Or
                    | hir::BinOp::Xor
            ) {
                continue;
            }
            // `op a, b, sxtw` / `op a, b, lsl #k`.
            //
            // A64 performs the shift/extension in the SECOND source operand
            // only. C writes the shifted side wherever it likes, so for a
            // COMMUTATIVE operation the two sides are tried in both orders:
            // `t = x << 1; y | t` is `orr w0, w3, w0, lsl #1` and not a
            // separate `lsl` (h2_revbits, sqlite's `orr` excess). Subtraction
            // is not commutative and keeps the single order.
            let commutes = matches!(
                op,
                hir::BinOp::Add | hir::BinOp::And | hir::BinOp::Or | hir::BinOp::Xor
            );
            let mut order = [(a, bb), (bb, a)];
            if !commutes {
                order[1] = order[0];
            }
            for (base, shifted) in order {
                let Some(v) = shifted.val().filter(|v| foldable(*v, &m2)) else { continue };
                // DDI 0487 C6.2: only ADD/SUB (and their flag-setting forms)
                // take an EXTENDED register operand. The logical instructions
                // take a shifted one and nothing else — `orr x0, x1, w3, uxtw`
                // is not an instruction, and the assembler says so.
                if ty == hir::Ty::I64 && matches!(op, hir::BinOp::Add | hir::BinOp::Sub) {
                    if let Some((src, e)) = ext_of(v) {
                        m2.dead.insert(v);
                        m2.alu.insert(dst, AluFold::Extended(base, src, e));
                        break;
                    }
                }
                if let Some((sop, st, sa, Operand::Imm(k))) = bin(v) {
                    let kind = match sop {
                        hir::BinOp::Shl => Some(ShiftKind::Lsl),
                        hir::BinOp::LShr => Some(ShiftKind::Lsr),
                        hir::BinOp::AShr => Some(ShiftKind::Asr),
                        _ => None,
                    };
                    if let (Some(kind), Some(src)) = (kind, sa.val()) {
                        if st == ty && k >= 0 && (k as u32) < ty.bits() {
                            m2.dead.insert(v);
                            m2.alu.insert(dst, AluFold::Shifted(base, src, kind, k as u8));
                            break;
                        }
                    }
                }
            }
        }
    }
    m2
}

struct L<'a> {
    h: &'a hir::Func,
    f: MFunc,
    /// HIR value → the virtual register holding it
    vmap: Vec<Reg>,
    cur: MBlockId,
    /// this function's own AAPCS64 assignment (its parameters and result)
    asn: abi::Assign,
    /// §6.9: the caller's x8, saved at entry because a call would clobber it
    sret_ptr: Option<Reg>,
    in_args_slot: Option<SlotId>,
    va_slot: Option<SlotId>,
    /// Addresses that are FOLDED into the memory operand of every access that
    /// uses them, so the `add` that would have computed them is never emitted.
    fold: std::collections::HashMap<ValueId, Folded>,
    /// Instructions whose second operand is performed inside them: a shift, a
    /// 32→64 extension, or a whole multiply.
    alu: std::collections::HashMap<ValueId, AluFold>,
    /// Producers every use of which folded, so they are never emitted at all.
    dead: std::collections::HashSet<ValueId>,
    /// per block: a compare whose only use is that block's branch
    br: std::collections::HashMap<hir::BlockId, CmpSrc>,
    /// per select: a compare that is its only condition
    sel: std::collections::HashMap<ValueId, CmpSrc>,
    /// narrow loads that sign-extend in the load and write the extension's reg
    ldext: std::collections::HashMap<ValueId, (MemOp, ValueId)>,
    /// per block: a branch that is one `tbz`/`tbnz`
    tb: std::collections::HashMap<hir::BlockId, (ValueId, hir::Ty, u8, bool)>,
    /// per select: the `csinc`/`csinv`/`csneg` form it collapses into
    csop: std::collections::HashMap<ValueId, (CSelOp, Operand, ValueId, bool)>,
    /// the current block's fused compare, taken by `terminator`
    fuse: Option<CmpSrc>,
}

impl<'a> L<'a> {
    fn push(&mut self, i: MInst) {
        self.f.blocks[self.cur as usize].insts.push(i);
    }
    fn tmp(&mut self, w: Width) -> Reg {
        self.f.new_vreg(w)
    }

    /// The register holding an HIR operand, materializing a constant if needed.
    /// MEASURED M14 — the inline small-copy bound
    ///
    /// A `MemCpy` this size or smaller becomes loads and stores here instead of
    /// a call to libc. The threshold is a size/speed trade with a measured
    /// crossover, not a taste: see `MECHANISM.md` Part F M14.
    ///
    /// WHY IT MATTERS AT ALL, and the case that found it. C says a by-value
    /// parameter IS a local object, so the frontend homes one by copying the
    /// incoming registers into the local's storage — a `MemCpy` of, for a
    /// four-int struct, SIXTEEN BYTES. Lowering that to `bl memcpy` costs the
    /// call itself, makes a leaf function non-leaf (so it saves x30 and builds
    /// a frame), and clobbers the caller-saved half at a point where the
    /// argument registers are still live. Measured on `e3_struct_byval`: 2.630x
    /// gcc -O1 on the clock, the worst program in the whole taxonomy suite on
    /// both axes, for a copy gcc does not perform at all.
    ///
    /// The shape emitted is two loads then two stores per sixteen bytes, not
    /// load-store-load-store, because `mir/pass/ldstp.rs` fuses ADJACENT
    /// accesses of the same kind — so this hands that pass exactly the pattern
    /// it already knows how to turn into one `ldp` and one `stp`.
    ///
    /// COMMUTING SQUARE. `memcpy` is defined on non-overlapping objects, so the
    /// order bytes move in is not observable; every byte of `[dst, dst+len)`
    /// receives the byte at the same offset of `[src, src+len)` and nothing
    /// else is written. The chunking is a partition of `[0, len)` into
    /// 16/8/4/2/1-byte pieces, so each byte is copied exactly once. Unaligned
    /// forms are legal: A64 permits unaligned `ldr`/`str`/`ldp`/`stp` to Normal
    /// memory (DDI 0487 B2.5.2), which is what a C object is.
    fn copy_inline(&mut self, d: Reg, s: Reg, len: i32) {
        let mut off = 0i32;
        while len - off >= 16 {
            let (a, b) = (self.tmp(Width::W64), self.tmp(Width::W64));
            self.push(MInst::Load {
                op: MemOp::X,
                dst: a,
                mem: AddrMode::BaseImm { base: s, off },
                vol: false,
            });
            self.push(MInst::Load {
                op: MemOp::X,
                dst: b,
                mem: AddrMode::BaseImm { base: s, off: off + 8 },
                vol: false,
            });
            self.push(MInst::Store {
                op: MemOp::X,
                src: a,
                mem: AddrMode::BaseImm { base: d, off },
                vol: false,
            });
            self.push(MInst::Store {
                op: MemOp::X,
                src: b,
                mem: AddrMode::BaseImm { base: d, off: off + 8 },
                vol: false,
            });
            off += 16;
        }
        for (bytes, op, w) in [
            (8i32, MemOp::X, Width::W64),
            (4, MemOp::W, Width::W32),
            (2, MemOp::H, Width::W32),
            (1, MemOp::B, Width::W32),
        ] {
            while len - off >= bytes {
                let t = self.tmp(w);
                self.push(MInst::Load {
                    op,
                    dst: t,
                    mem: AddrMode::BaseImm { base: s, off },
                    vol: false,
                });
                self.push(MInst::Store {
                    op,
                    src: t,
                    mem: AddrMode::BaseImm { base: d, off },
                    vol: false,
                });
                off += bytes;
            }
        }
        debug_assert_eq!(off, len, "small-copy expansion did not partition the length");
    }

    fn reg(&mut self, o: Operand, t: hir::Ty) -> Reg {
        let w = wid(t);
        match o {
            Operand::Val(v) => self.vmap[v as usize],
            // xzr/wzr IS the constant zero: no instruction, no live range.
            Operand::Imm(0) => Reg::P(isa::ZR),
            Operand::Imm(k) => {
                let d = self.tmp(w);
                let i = imm::materialize(d, k, w);
                self.push(i);
                d
            }
            Operand::Fimm(bits) => {
                let d = self.tmp(w);
                if bits == 0 {
                    // `fmov d, xzr` — the zero bit pattern, no literal needed
                    self.push(MInst::FMov {
                        dw: w,
                        sw: if w == Width::S { Width::W32 } else { Width::W64 },
                        dst: d,
                        src: Reg::P(isa::ZR),
                    });
                } else if imm::fp_is_imm8(bits, w) {
                    // DDI 0487 C7 `VFPExpandImm` covers it, so the constant needs
                    // neither a GPR nor a crossing between the register files
                    // (MECHANISM.md M37).
                    self.push(MInst::FMovImm { w, dst: d, bits });
                } else {
                    let gw = if w == Width::S { Width::W32 } else { Width::W64 };
                    let g = self.tmp(gw);
                    let i = imm::materialize(g, bits as i64, gw);
                    self.push(i);
                    self.push(MInst::FMov {
                        dw: w,
                        sw: gw,
                        dst: d,
                        src: g,
                    });
                }
                d
            }
        }
    }

    /// The register holding an operand, forbidding the zero register.
    ///
    /// DDI 0487 C6.2.4: in the ADD/SUB (immediate) form — and therefore in
    /// `cmp`/`cmn` too — register 31 encodes SP, not ZR. `add w0, wzr, #5` is
    /// not an instruction. Every other form that takes an immediate (the
    /// logical-immediate and shift aliases) does read 31 as ZR, so this applies
    /// exactly where the encoding says it does.
    fn reg_nonzr(&mut self, o: Operand, t: hir::Ty) -> Reg {
        match o {
            Operand::Imm(0) => {
                let w = wid(t);
                let d = self.tmp(w);
                self.push(imm::materialize(d, 0, w));
                d
            }
            _ => self.reg(o, t),
        }
    }

    /// The BASE register of a memory access. DDI 0487 C1.2.5: in the load/store
    /// addressing forms, register 31 in the Rn field decodes as SP, not ZR — so a
    /// null address folded to `Imm(0)` must be materialized into a real register
    /// rather than ridden for free in the zero register the way a data operand is.
    /// (Found by torture `930719-1`, whose `*(char *)0 = 0` only becomes a literal
    /// address once the HIR ladder folds the cast.)
    fn base(&mut self, o: Operand) -> Reg {
        self.reg_nonzr(o, hir::Ty::I64)
    }

    /// The memory operand for an access at this address: the folded form when
    /// `fold_addrs` proved every use of the address is a memory operand, and the
    /// plain register base otherwise.
    fn addr_mode(&mut self, o: Operand) -> AddrMode {
        if let Operand::Val(v) = o {
            match self.fold.get(&v).copied() {
                Some(Folded::Slot(slot, off)) => return AddrMode::Slot { slot, off },
                Some(Folded::Base(b, off)) => {
                    let base = self.base(Operand::Val(b));
                    return AddrMode::BaseImm { base, off };
                }
                Some(Folded::Indexed { base, idx, ext, shift, .. }) => {
                    let b = self.base(Operand::Val(base));
                    let t = if ext.is_some() { hir::Ty::I32 } else { hir::Ty::I64 };
                    let i = self.reg(Operand::Val(idx), t);
                    return AddrMode::BaseReg { base: b, idx: i, ext, shift };
                }
                None => {}
            }
        }
        let base = self.base(o);
        AddrMode::BaseImm { base, off: 0 }
    }

    /// An operand that may ride in an immediate field of `op`.
    fn rhs(&mut self, o: Operand, t: hir::Ty, op: AluOp) -> Rhs {
        if let Operand::Imm(k) = o {
            if let Some(r) = imm::as_rhs(op, k, wid(t)) {
                return r;
            }
        }
        Rhs::Reg(self.reg(o, t))
    }

    /// `MEASURED M31` — put the arms that STAY in the state ahead of the rest,
    /// keeping source order within each group.
    ///
    /// The switch operand `v` is the state. It is a PARAMETER of some block `H`
    /// — the loop header, since a state machine's state is exactly the value the
    /// loop carries — and an arm keeps the machine in its own state precisely
    /// when the arm's region hands `v` itself back to `H` at `v`'s own parameter
    /// index. Every other arm computes a new state and hands that back instead.
    ///
    /// The walk is bounded and forward-only: from the arm's target, follow
    /// successors until `H` is reached or the budget runs out. A budget rather
    /// than a fixpoint because this decides an ORDER and nothing else — being
    /// wrong costs a compare, never a value — and because an arm whose return to
    /// the header is eight blocks away is not the arm consuming a run of bytes.
    fn order_switch_arms(
        &self,
        c: Operand,
        arms: &[(i64, hir::Target)],
    ) -> Vec<(i64, hir::Target)> {
        let mut out: Vec<(i64, hir::Target)> = arms.to_vec();
        if !armord() {
            return out;
        }
        let v = match c {
            Operand::Val(v) => v,
            _ => return out,
        };
        // where `v` is a parameter, and at which index
        let mut home: Option<(hir::BlockId, usize)> = None;
        for (bi, b) in self.h.blocks.iter().enumerate() {
            if let Some(i) = b.params.iter().position(|&p| p == v) {
                home = Some((bi as hir::BlockId, i));
                break;
            }
        }
        let (hblk, idx) = match home {
            Some(x) => x,
            None => return out,
        };
        // THE QUESTION IS ABOUT THIS ARM, NOT ABOUT THE JOIN. Two earlier cuts
        // asked what value reaches the loop header and both fired on nothing:
        // every arm of a state machine merges into the SAME join before the back
        // edge, so the argument the header receives is one value for all of them
        // and the test cannot tell the arms apart. What distinguishes the arm
        // that STAYS is what it contributes to that join — it hands the OLD
        // state along, where an arm that transitions hands a fresh one.
        //
        // So: does this arm's own region carry `v` forward on any edge? `case
        // S_BULK: if (--want == 0) st = S_CR;` does, on the path where the
        // delimiter has not arrived. `case S_LF: st = want > 0 ? S_BULK :
        // S_TYPE;` never does. The walk stays inside the arm — it stops at the
        // header — and is bounded, because being wrong costs a compare and never
        // a value.
        /// MEASURED M31 — how far inside an arm to look for the edge that
        /// carries the old state on. It is a SEARCH bound, not a resource
        /// constant: the rule decides an ORDER, so a budget that stops early
        /// costs one `cmp` on a cold path and never a value. Eight covers the
        /// arms of both parsers that motivated the row (`m1_resp_parse` and
        /// `m2_http_parse`, four and six blocks deep); an arm whose return to
        /// the header is further away than that is not the arm consuming a run
        /// of bytes.
        const BUDGET: usize = 8;
        //
        // AND `v` DOES NOT ONLY TRAVEL ON EDGES. `if (--want == 0) st = S_CR;`
        // is exactly the shape `ifconv` collapses, so on the common path the old
        // state is not an edge ARGUMENT at all — it is the false arm of a
        // `Select`, an OPERAND. The first shipped cut looked at edges alone and
        // therefore caught `S_COUNT`/`S_BULKLEN`, whose bodies branch, while
        // missing `S_BULK` and `S_HVALUE` — the two arms that actually consume
        // the runs, and the whole point of the rule. A value kept by a select is
        // kept.
        let keeps_v = |o: &Operand| *o == Operand::Val(v);
        let stays = |t: &hir::Target| -> bool {
            if t.args.iter().any(keeps_v) {
                return true;
            }
            let mut seen: Vec<hir::BlockId> = Vec::new();
            let mut wave: Vec<hir::BlockId> = vec![t.block];
            for _ in 0..BUDGET {
                let mut next: Vec<hir::BlockId> = Vec::new();
                for b in wave {
                    if b == hblk || seen.contains(&b) {
                        continue;
                    }
                    seen.push(b);
                    let blk = &self.h.blocks[b as usize];
                    for i in &blk.insts {
                        if let hir::Inst::Select { a, b: bb, .. } = i {
                            if keeps_v(a) || keeps_v(bb) {
                                return true;
                            }
                        }
                    }
                    for e in blk.term.targets() {
                        if e.args.iter().any(keeps_v) {
                            return true;
                        }
                        next.push(e.block);
                    }
                }
                if next.is_empty() {
                    break;
                }
                wave = next;
            }
            false
        };
        let _ = idx;
        // STABLE partition: the staying arms first, source order kept in both
        // halves, so a program with no such arm is compiled exactly as before.
        let (a, b): (Vec<_>, Vec<_>) = out.drain(..).partition(|(_, t)| stays(t));
        if std::env::var("ZCC_ARMDBG").is_ok() {
            eprintln!(
                "ARMORD {} v{} stay={:?} go={:?}",
                self.h.name,
                v,
                a.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
                b.iter().map(|(k, _)| *k).collect::<Vec<_>>()
            );
        }
        a.into_iter().chain(b).collect()
    }

    fn dst_of(&self, v: ValueId) -> Reg {
        self.vmap[v as usize]
    }

    fn inst(&mut self, i: &Inst) {
        match i {
            Inst::Bin { dst, op, ty, a, b } => self.binop(*dst, *op, *ty, *a, *b),
            Inst::Un { dst, op, ty, a } => {
                let d = self.dst_of(*dst);
                let w = wid(*ty);
                match op {
                    // C99 6.5.3.3: -x is 0 - x, and xzr costs nothing
                    UnOp::Neg => {
                        let x = self.reg(*a, *ty);
                        self.push(MInst::Alu {
                            op: AluOp::Sub,
                            w,
                            dst: d,
                            a: Reg::P(isa::ZR),
                            b: Rhs::Reg(x),
                            flags: None,
                        });
                    }
                    UnOp::Not => {
                        let x = self.reg(*a, *ty);
                        self.push(MInst::Alu {
                            op: AluOp::Orn,
                            w,
                            dst: d,
                            a: Reg::P(isa::ZR),
                            b: Rhs::Reg(x),
                            flags: None,
                        });
                    }
                    UnOp::FNeg => {
                        let x = self.reg(*a, *ty);
                        self.push(MInst::FpUn {
                            op: FpUnOp::Fneg,
                            w,
                            dst: d,
                            src: x,
                            sw: w,
                        });
                    }
                }
            }
            Inst::Cmp { dst, op, ty, a, b } => {
                let d = self.dst_of(*dst);
                let (fl, cc) = self.compare(*op, *ty, *a, *b);
                self.push(MInst::CSet {
                    w: Width::W32,
                    dst: d,
                    cc,
                    flags: fl,
                });
            }
            Inst::Cvt {
                dst,
                op,
                from,
                to,
                a,
            } => self.convert(*dst, *op, *from, *to, *a),
            Inst::Load {
                dst,
                ty,
                addr,
                vol,
                ..
            } => {
                let mem = self.addr_mode(*addr);
                // The munch table's extending-load row: the `sext` that is this
                // load's only consumer is performed BY the load, into that
                // extension's own register (`ldrsw x5,[…]` for `ldr w5` +
                // `sxtw x5,w5`). Nothing moves — the load stays where it is.
                let (op, d) = match self.ldext.get(dst).copied() {
                    Some((op, ext)) => (op, self.vmap[ext as usize]),
                    None => (memop(*ty), self.dst_of(*dst)),
                };
                self.push(MInst::Load {
                    op,
                    dst: d,
                    mem,
                    vol: *vol,
                });
            }
            Inst::Store {
                ty,
                addr,
                val,
                vol,
                ..
            } => {
                let mem = self.addr_mode(*addr);
                let v = self.reg(*val, *ty);
                self.push(MInst::Store {
                    op: memop(*ty),
                    src: v,
                    mem,
                    vol: *vol,
                });
            }
            Inst::SlotAddr { dst, slot, off } => {
                let d = self.dst_of(*dst);
                self.push(MInst::SlotAddr {
                    dst: d,
                    slot: *slot,
                    off: *off as i32,
                });
            }
            // THEORY II-4: local-exec TLS — thread pointer, then the two halves
            // of the tprel offset. No GOT, no call.
            Inst::SymAddr {
                dst,
                sym: sym @ hir::Sym::Tls(_),
            } => {
                let d = self.dst_of(*dst);
                let t = self.tmp(Width::W64);
                self.push(MInst::Mrs { dst: t });
                let u = self.tmp(Width::W64);
                self.push(MInst::AddTprel {
                    dst: u,
                    base: t,
                    sym: sym.clone(),
                    hi: true,
                });
                self.push(MInst::AddTprel {
                    dst: d,
                    base: u,
                    sym: sym.clone(),
                    hi: false,
                });
            }
            Inst::SymAddr { dst, sym } => {
                let d = self.dst_of(*dst);
                let page = self.tmp(Width::W64);
                self.push(MInst::Adrp {
                    dst: page,
                    sym: sym.clone(),
                    got: false,
                });
                self.push(MInst::AddLo12 {
                    dst: d,
                    base: page,
                    sym: sym.clone(),
                    got: false,
                });
            }
            Inst::Select { dst, ty, c, a, b } => {
                let d = self.dst_of(*dst);
                let (fl, cc) = match self.sel.get(dst).copied() {
                    Some((op, cty, x, y)) => self.compare(op, cty, x, y),
                    None => (self.test(*c), CC::Ne),
                };
                // `c ? 1 : 0` is `cset` — one instruction (DDI 0487 C6.2.87,
                // the `csinc d,zr,zr,invert(cc)` alias), where materializing
                // the 1 and selecting it is two. C reaches this shape through
                // every `&&`/`||` that is not directly a branch condition.
                if let (Operand::Imm(1), Operand::Imm(0)) | (Operand::Imm(0), Operand::Imm(1)) =
                    (*a, *b)
                {
                    self.push(MInst::CSet {
                        w: wid(*ty),
                        dst: d,
                        cc: if *a == Operand::Imm(1) { cc } else { cc.invert() },
                        flags: fl,
                    });
                    return;
                }
                // The `csinc`/`csinv`/`csneg` row: the arithmetic on the other
                // arm happens INSIDE the select.
                if let Some((k, keep, src, inv)) = self.csop.get(dst).copied() {
                    let x = self.reg(keep, *ty);
                    let y = self.reg(Operand::Val(src), *ty);
                    self.push(MInst::CSel {
                        op: k,
                        w: wid(*ty),
                        dst: d,
                        a: x,
                        b: y,
                        cc: if inv { cc.invert() } else { cc },
                        flags: fl,
                    });
                    return;
                }
                let (x, y) = (self.reg(*a, *ty), self.reg(*b, *ty));
                self.push(MInst::CSel {
                    op: CSelOp::Csel,
                    w: wid(*ty),
                    dst: d,
                    a: x,
                    b: y,
                    cc,
                    flags: fl,
                });
            }
            Inst::Call {
                dst,
                sig,
                callee,
                args,
                sret,
            } => self.call(*dst, sig, callee, args, *sret),
            Inst::MemCpy { dst, src, len } => {
                let (d, s) = (self.reg(*dst, hir::Ty::I64), self.reg(*src, hir::Ty::I64));
                if (*len as usize) <= inline_copy_max() {
                    self.copy_inline(d, s, *len as i32);
                } else {
                    let n = self.reg(Operand::Imm(*len as i64), hir::Ty::I64);
                    self.libcall("memcpy", &[d, s, n]);
                }
            }
            Inst::MemSet { dst, byte, len } => {
                let d = self.reg(*dst, hir::Ty::I64);
                let b = self.reg(*byte, hir::Ty::I32);
                let n = self.reg(Operand::Imm(*len as i64), hir::Ty::I64);
                self.libcall("memset", &[d, b, n]);
            }
            // C99 6.7.5.2: one instruction, because the sp move and the address
            // it produces are a single indivisible step — splitting them would
            // let a pass schedule something between sp's two meanings.
            Inst::Alloca { dst, size, .. } => {
                let n = self.reg_nonzr(*size, hir::Ty::I64);
                let d = self.dst_of(*dst);
                self.push(MInst::StackAlloc { dst: d, size: n });
                self.f.dyn_stack = true;
            }
            Inst::Intrinsic { dst, kind, args } => self.intrinsic(*dst, kind, args),
        }
    }

    /// `cmp`/`fcmp`, returning the flag value it defines.
    /// The flags this relation sets, and the condition code that reads them —
    /// which is not always `cc_of(op)`: the operands may be exchanged to reach
    /// the immediate field, and the relation goes with them.
    fn compare(&mut self, op: CmpOp, ty: hir::Ty, a: Operand, b: Operand) -> (Reg, CC) {
        let fl = self.f.new_flags();
        if ty.is_float() {
            let x = self.reg(a, ty);
            let zero = matches!(b, Operand::Fimm(0));
            let y = if zero { x } else { self.reg(b, ty) };
            self.push(MInst::FpCmp {
                w: wid(ty),
                a: x,
                b: y,
                zero,
                flags: fl,
            });
            return (fl, cc_of(op));
        }
        // A constant C wrote on the left has no immediate field to ride in and
        // would be materialized for nothing; exchanging both operands and the
        // relation is exact and costs no instruction. `Imm(0)` is left where it
        // is — the zero register serves either side for free.
        let (op, a, b) = match (a, b) {
            (Operand::Imm(k), Operand::Val(_)) if k != 0 => (swap_cmp(op), b, a),
            _ => (op, a, b),
        };
        // `cmp x, #-k` subtracts a negative. `cmn x, #k` ADDS the same magnitude
        // — bit for bit the same arithmetic, so bit for bit the same NZCV — and
        // its immediate field is the one that can hold `k` (DDI 0487 C6.2.62;
        // the add/sub imm12 field is unsigned, so the negative form has none).
        let (kind, b) = match b {
            Operand::Imm(k) => match k.checked_neg().filter(|n| *n > 0 && isa::add_imm(*n).is_some())
            {
                Some(n) => (CmpKind::Cmn, Operand::Imm(n)),
                None => (CmpKind::Cmp, b),
            },
            _ => (CmpKind::Cmp, b),
        };
        // `cmp` is `subs`: the same register-31-is-SP rule applies
        let y = self.rhs(b, ty, AluOp::Sub);
        let x = match &y {
            Rhs::Imm(_) => self.reg_nonzr(a, ty),
            _ => self.reg(a, ty),
        };
        self.push(MInst::Cmp {
            kind,
            w: wid(ty),
            a: x,
            b: y,
            flags: fl,
        });
        (fl, cc_of(op))
    }

    /// `cmp c, #0` for a value used as a truth value.
    fn test(&mut self, c: Operand) -> Reg {
        let fl = self.f.new_flags();
        let x = self.reg_nonzr(c, hir::Ty::I32);
        self.push(MInst::Cmp {
            kind: CmpKind::Cmp,
            w: Width::W32,
            a: x,
            b: Rhs::Imm(0),
            flags: fl,
        });
        fl
    }

    fn binop(&mut self, dst: ValueId, op: BinOp, ty: hir::Ty, a: Operand, b: Operand) {
        let d = self.dst_of(dst);
        let w = wid(ty);
        // A64 PUTS THE IMMEDIATE ON THE RIGHT, and only there: `add wd, wn, #k`
        // has no mirror form. Everything below offers `b` to `imm::as_rhs` and
        // `a` to a register, so a COMMUTATIVE operation written with its
        // constant on the left — `'a' + i % 26`, which is how C source usually
        // says it — materializes that constant into a register and then adds two
        // registers. Two instructions where the ISA has one, inside whatever
        // loop the expression sits in.
        //
        // `MEASURED M30`'s worklist found it: `m1_resp_parse`'s two hottest blocks
        // are 58% of that program's weighted cost and each holds
        // `movz w13, #97 ; add w12, w13, w12` where gcc -O1 writes
        // `add w0, w0, 97`.
        //
        // Commutativity is the whole justification and it is exact for these
        // five over the integers modulo 2^n (ISO 9899 6.5: `+`, `*`, `&`, `^`,
        // `|` are all commutative, and unsigned wrap-around is associative and
        // commutative too). `Sub`, the divisions and the shifts are NOT, and are
        // not listed. The floating-point adds and multiplies are not listed
        // either: IEEE-754 addition commutes, but this is a LOWERING and the FP
        // path below takes both operands in registers regardless, so swapping
        // would buy nothing and would put a NaN-payload question in the way of a
        // reader for no reason.
        let (a, b) = match (op, a, b) {
            (
                BinOp::Add | BinOp::Mul | BinOp::And | BinOp::Or | BinOp::Xor,
                Operand::Imm(_),
                Operand::Val(_),
            ) if commute() => (b, a),
            _ => (a, b),
        };
        if let Some(fop) = match op {
            BinOp::FAdd => Some(FpOp::Fadd),
            BinOp::FSub => Some(FpOp::Fsub),
            BinOp::FMul => Some(FpOp::Fmul),
            BinOp::FDiv => Some(FpOp::Fdiv),
            _ => None,
        } {
            let (x, y) = (self.reg(a, ty), self.reg(b, ty));
            self.push(MInst::FpAlu {
                op: fop,
                w,
                dst: d,
                a: x,
                b: y,
            });
            return;
        }
        // C99 6.5.5: a % b is a - (a / b) * b, which A64 spells sdiv + msub —
        // there is no remainder instruction.
        if matches!(op, BinOp::SRem | BinOp::URem) {
            let (x, y) = (self.reg(a, ty), self.reg(b, ty));
            let q = self.tmp(w);
            self.push(MInst::Alu {
                op: if op == BinOp::SRem {
                    AluOp::SDiv
                } else {
                    AluOp::UDiv
                },
                w,
                dst: q,
                a: x,
                b: Rhs::Reg(y),
                flags: None,
            });
            self.push(MInst::Alu3 {
                op: Alu3Op::Msub,
                w,
                dst: d,
                a: q,
                b: y,
                c: x,
            });
            return;
        }
        let aop = match op {
            BinOp::Add => AluOp::Add,
            BinOp::Sub => AluOp::Sub,
            BinOp::Mul => AluOp::Mul,
            BinOp::SMulHi => AluOp::SMulH,
            BinOp::UMulHi => AluOp::UMulH,
            BinOp::SDiv => AluOp::SDiv,
            BinOp::UDiv => AluOp::UDiv,
            BinOp::And => AluOp::And,
            BinOp::Or => AluOp::Orr,
            BinOp::Xor => AluOp::Eor,
            BinOp::Shl => AluOp::Lsl,
            BinOp::LShr => AluOp::Lsr,
            BinOp::AShr => AluOp::Asr,
            _ => unreachable!(),
        };
        // The munch table's ALU rows: a shift, a 32→64 extension, or a whole
        // multiply performed inside this instruction rather than before it.
        if let Some(plan) = self.alu.get(&dst).copied() {
            match plan {
                AluFold::Bfx(signed, x, lsb, width) => {
                    let sr = self.reg_nonzr(Operand::Val(x), ty);
                    self.push(MInst::Bfx { signed, w, dst: d, src: sr, lsb, width });
                    return;
                }
                AluFold::Mul3(k, x, y, c) => {
                    let (xr, yr) = (self.reg(x, ty), self.reg(y, ty));
                    let cr = self.reg(c, ty);
                    self.push(MInst::Alu3 { op: k, w, dst: d, a: xr, b: yr, c: cr });
                    return;
                }
                AluFold::Extended(base, src, e) => {
                    let br = self.reg_nonzr(base, ty);
                    let sr = self.reg(Operand::Val(src), hir::Ty::I32);
                    self.push(MInst::Alu {
                        op: aop,
                        w,
                        dst: d,
                        a: br,
                        b: Rhs::Extended(sr, e, 0),
                        flags: None,
                    });
                    return;
                }
                AluFold::Shifted(base, src, k, amt) => {
                    let br = self.reg(base, ty);
                    let sr = self.reg(Operand::Val(src), ty);
                    self.push(MInst::Alu {
                        op: aop,
                        w,
                        dst: d,
                        a: br,
                        b: Rhs::Shifted(sr, k, amt),
                        flags: None,
                    });
                    return;
                }
            }
        }
        let y = self.rhs(b, ty, aop);
        let x = match (aop, &y) {
            (AluOp::Add | AluOp::Sub, Rhs::Imm(_)) => self.reg_nonzr(a, ty),
            _ => self.reg(a, ty),
        };
        self.push(MInst::Alu {
            op: aop,
            w,
            dst: d,
            a: x,
            b: y,
            flags: None,
        });
    }

    fn convert(&mut self, dst: ValueId, op: CvtOp, from: hir::Ty, to: hir::Ty, a: Operand) {
        let d = self.dst_of(dst);
        let (fw, tw) = (wid(from), wid(to));
        let x = self.reg(a, from);
        match op {
            CvtOp::Sext | CvtOp::Zext => {
                let e = match (from, op) {
                    (hir::Ty::I8, CvtOp::Sext) => Some(ExtOp::Sxtb),
                    (hir::Ty::I16, CvtOp::Sext) => Some(ExtOp::Sxth),
                    (hir::Ty::I32, CvtOp::Sext) => Some(ExtOp::Sxtw),
                    (hir::Ty::I8, _) => Some(ExtOp::Uxtb),
                    (hir::Ty::I16, _) => Some(ExtOp::Uxth),
                    // Zero-extending w→x emits a `w`-form move, which is what
                    // makes it free (DDI 0487 B1.2.1) — but it is named as the
                    // extension it is, so that no rule about redundant copies
                    // can delete the thing the freedom depends on.
                    (hir::Ty::I32, _) => Some(ExtOp::Uxtw),
                    _ => unreachable!(),
                };
                match e {
                    Some(op) => self.push(MInst::Ext {
                        op,
                        w: tw,
                        dst: d,
                        src: x,
                    }),
                    None => self.push(MInst::Copy {
                        w: Width::W32,
                        dst: d,
                        src: x,
                    }),
                }
            }
            // Narrowing keeps the low bits where they are; every consumer of a
            // narrow value reads only those bits (`strb`, `uxtb`, a promoted
            // compare), so no instruction is required beyond the width change.
            CvtOp::Trunc => self.push(MInst::Copy {
                w: tw,
                dst: d,
                src: x,
            }),
            CvtOp::SiToFp | CvtOp::UiToFp => {
                // scvtf/ucvtf read a whole w or x register, so a narrow source
                // must be extended first — its upper bits are not defined by C.
                let signed = op == CvtOp::SiToFp;
                let src = match from {
                    hir::Ty::I8 | hir::Ty::I16 => {
                        let e = match (from, signed) {
                            (hir::Ty::I8, true) => ExtOp::Sxtb,
                            (hir::Ty::I8, false) => ExtOp::Uxtb,
                            (_, true) => ExtOp::Sxth,
                            (_, false) => ExtOp::Uxth,
                        };
                        let t = self.tmp(Width::W32);
                        self.push(MInst::Ext {
                            op: e,
                            w: Width::W32,
                            dst: t,
                            src: x,
                        });
                        t
                    }
                    _ => x,
                };
                self.push(MInst::FpCvt {
                    op: if signed { CvtOp2::Scvtf } else { CvtOp2::Ucvtf },
                    dw: tw,
                    sw: if fw == Width::W64 {
                        Width::W64
                    } else {
                        Width::W32
                    },
                    dst: d,
                    src,
                });
            }
            CvtOp::FpToSi | CvtOp::FpToUi => self.push(MInst::FpCvt {
                op: if op == CvtOp::FpToSi {
                    CvtOp2::Fcvtzs
                } else {
                    CvtOp2::Fcvtzu
                },
                dw: if tw == Width::W64 {
                    Width::W64
                } else {
                    Width::W32
                },
                sw: fw,
                dst: d,
                src: x,
            }),
            CvtOp::FpExt | CvtOp::FpTrunc => self.push(MInst::FpUn {
                op: FpUnOp::Fcvt,
                w: tw,
                dst: d,
                src: x,
                sw: fw,
            }),
            CvtOp::Bitcast => self.push(MInst::FMov {
                dw: tw,
                sw: fw,
                dst: d,
                src: x,
            }),
        }
    }

    // ── composites (AAPCS64 §6.8.2) ────────────────────────────────────────
    // A composite travels as an ADDRESS in HIR, so every rule below is a
    // load/store between that address and the registers or stack slots the ABI
    // names. The chunking never reads or writes outside the object: a struct may
    // sit at the end of a page, and an 8-byte access to a 5-byte object would
    // fault where the C program does not.
    fn chunk_addr(&self, p: Place, off: i32) -> AddrMode {
        match p {
            Place::At(base) => AddrMode::BaseImm { base, off },
            Place::Out(o) => AddrMode::SpArg {
                off: (o as i32 + off) as u32,
            },
            Place::Slot(slot) => AddrMode::Slot { slot, off },
        }
    }

    /// The AAPCS64 register save area of a variadic function: 192 bytes laid out
    /// as §B.6 requires — v0–v7 in 16-byte slots at [0,128), x0–x7 at [128,192).
    /// `__vr_top` is therefore base+128 and `__gr_top` base+192, and the
    /// negative `*_offs` count back from those two points.
    fn va_area(&mut self) -> SlotId {
        if let Some(s) = self.va_slot {
            return s;
        }
        let s = self.f.new_slot(192, 16, SlotKind::Local);
        self.va_slot = Some(s);
        s
    }

    fn intrinsic(&mut self, dst: Option<ValueId>, kind: &hir::IntrinKind, args: &[Operand]) {
        match kind {
            hir::IntrinKind::VaArea => {
                let off = match args[0] {
                    Operand::Imm(k) => k as i32,
                    _ => unreachable!("__va_area__ offset is a constant"),
                };
                let d = self.dst_of(dst.expect("__va_area__ has a value"));
                let slot = self.in_args();
                self.push(MInst::SlotAddr { dst: d, slot, off });
            }
            // §B.6: the five fields, from the three counters the ABI walk left.
            hir::IntrinKind::VaStart => {
                let ap = self.reg(args[0], hir::Ty::I64);
                let (ngrn, nsrn, nsaa) = (self.asn.ngrn, self.asn.nsrn, self.asn.nsaa);
                let va = self.va_area();
                let ia = self.in_args();
                let mut field = |l: &mut Self, off: i32, slot: SlotId, at: i32| {
                    let t = l.tmp(Width::W64);
                    l.push(MInst::SlotAddr { dst: t, slot, off: at });
                    l.push(MInst::Store {
                        op: MemOp::X,
                        src: t,
                        mem: AddrMode::BaseImm { base: ap, off },
                        vol: false,
                    });
                };
                field(self, 0, ia, nsaa as i32);
                field(self, 8, va, 192);
                field(self, 16, va, 128);
                for (off, k) in [
                    (24, -8 * (8 - ngrn as i64)),
                    (28, -16 * (8 - nsrn as i64)),
                ] {
                    let t = self.tmp(Width::W32);
                    self.push(MInst::MovImm {
                        w: Width::W32,
                        dst: t,
                        imm: k,
                    });
                    self.push(MInst::Store {
                        op: MemOp::W,
                        src: t,
                        mem: AddrMode::BaseImm { base: ap, off },
                        vol: false,
                    });
                }
            }
            // THEORY II-2: memory holds binary128, a register holds the
            // canonical f64. libgcc's soft-float pair is the bridge, and a quad
            // never stays live across another call (AAPCS64 §6.1.2 preserves
            // only the low half of v8–v15, so a live quad has no home).
            hir::IntrinKind::LdLoad => {
                let a = self.base(args[0]);
                let q = self.tmp(Width::Q);
                self.push(MInst::Load {
                    op: MemOp::Q,
                    dst: q,
                    mem: AddrMode::BaseImm { base: a, off: 0 },
                    vol: false,
                });
                self.push(MInst::ParallelCopy(vec![(Reg::P(PReg::fpr(0)), q, Width::Q)]));
                self.fp_libcall("__trunctfdf2");
                let d = self.dst_of(dst.expect("long double load has a value"));
                self.push(MInst::FMov {
                    dw: Width::D,
                    sw: Width::D,
                    dst: d,
                    src: Reg::P(PReg::fpr(0)),
                });
            }
            hir::IntrinKind::LdStore => {
                let a = self.base(args[0]);
                let v = self.reg(args[1], hir::Ty::F64);
                self.push(MInst::ParallelCopy(vec![(Reg::P(PReg::fpr(0)), v, Width::D)]));
                self.fp_libcall("__extenddftf2");
                self.push(MInst::Store {
                    op: MemOp::Q,
                    src: Reg::P(PReg::fpr(0)),
                    mem: AddrMode::BaseImm { base: a, off: 0 },
                    vol: false,
                });
            }
            // ARM DDI 0487 B2.9 — the three exclusive-access primitives. The
            // retry loop around them was built in HIR, so each is 1:1 here.
            hir::IntrinKind::LdAxr(t) => {
                let a = self.reg(args[0], hir::Ty::I64);
                let d = self.dst_of(dst.expect("ldaxr has a value"));
                self.push(MInst::LdAxr {
                    w: wid(*t),
                    dst: d,
                    addr: a,
                });
            }
            hir::IntrinKind::StlXr(t) => {
                let a = self.reg(args[0], hir::Ty::I64);
                let v = self.reg(args[1], *t);
                let d = self.dst_of(dst.expect("stlxr reports its status"));
                self.push(MInst::StlXr {
                    w: wid(*t),
                    status: d,
                    src: v,
                    addr: a,
                });
            }
            hir::IntrinKind::Stlr(t) => {
                let a = self.reg(args[0], hir::Ty::I64);
                let v = self.reg(args[1], *t);
                self.push(MInst::Stlr {
                    w: wid(*t),
                    src: v,
                    addr: a,
                });
            }
            hir::IntrinKind::Dmb => self.push(MInst::Dmb),
            hir::IntrinKind::Asm { tmpl, ops } => self.asm(tmpl, ops, args),
            _ => todo!("R1.3: EXT intrinsics"),
        }
    }

    /// EXT(gcc) inline asm. Operands are PINNED to a reserved pool — x9–x15 and
    /// v16–v22, all caller-saved and none of them the parallel-copy scratch —
    /// rather than left to the allocator, because a `"+r"` operand needs its
    /// input and output in ONE register and the SSA constraint model has no way
    /// to say that. The pool is the whole surface real code (musl, xxhash) uses.
    fn asm(&mut self, tmpl: &str, ops: &[hir::AsmOperand], args: &[Operand]) {
        /// THEORY II-3 — AAPCS64 temporaries usable by inline asm
        const GP_POOL: [u8; 7] = [9, 10, 11, 12, 13, 14, 15];
        /// THEORY II-3 — AAPCS64 temporaries usable by inline asm
        const FP_POOL: [u8; 7] = [16, 17, 18, 19, 20, 21, 22];
        let (mut ngp, mut nfp) = (0usize, 0usize);
        let mut slots: Vec<AsmSlot> = Vec::with_capacity(ops.len());
        let mut ins: Vec<(Reg, Reg, Width)> = Vec::new();
        let mut outs: Vec<(PReg, Reg, hir::Ty)> = Vec::new();
        let mut at = 0usize;
        for (i, o) in ops.iter().enumerate() {
            let fp = o.fp || (!o.mem && o.ty.is_float());
            let reg = match (o.pin, o.tied) {
                (Some(n), _) => PReg::gpr(n),
                (None, Some(k)) => slots[k as usize].reg,
                (None, None) if fp => {
                    let p = PReg::fpr(FP_POOL[nfp.min(FP_POOL.len() - 1)]);
                    nfp += 1;
                    p
                }
                (None, None) => {
                    let p = PReg::gpr(GP_POOL[ngp.min(GP_POOL.len() - 1)]);
                    ngp += 1;
                    p
                }
            };
            let w = if o.mem {
                Width::W64
            } else {
                wid(o.ty)
            };
            let read = !o.out || o.rw || o.mem;
            slots.push(AsmSlot {
                reg,
                out: o.out && !o.mem,
                read,
                mem: o.mem,
                w,
            });
            // the argument list runs in operand order (see `IntrinKind::Asm`)
            if o.mem {
                let a = self.base(args[at]);
                at += 1;
                ins.push((Reg::P(reg), a, Width::W64));
            } else if o.out {
                let a = self.base(args[at]);
                at += 1;
                if o.rw {
                    let v = self.reg(args[at], o.ty);
                    at += 1;
                    ins.push((Reg::P(reg), v, w));
                }
                outs.push((reg, a, o.ty));
            } else {
                let v = self.reg(args[at], o.ty);
                at += 1;
                ins.push((Reg::P(reg), v, w));
            }
            let _ = i;
        }
        if !ins.is_empty() {
            self.push(MInst::ParallelCopy(ins));
        }
        self.push(MInst::Asm {
            tmpl: tmpl.to_string(),
            ops: slots,
        });
        // an output leaves through memory, which is where the C lvalue is
        for (p, a, t) in outs {
            self.push(MInst::Store {
                op: memop(t),
                src: Reg::P(p),
                mem: AddrMode::BaseImm { base: a, off: 0 },
                vol: false,
            });
        }
    }

    /// A libgcc soft-float call: argument and result both in v0, which is where
    /// the quad already is — so there is nothing to marshal, only the call.
    fn fp_libcall(&mut self, name: &str) {
        self.push(MInst::Call {
            callee: CallTarget::Direct(name.to_string()),
            uses: vec![(Reg::P(PReg::fpr(0)), PReg::fpr(0))],
            defs: vec![(Reg::P(PReg::fpr(0)), PReg::fpr(0))],
            clobbers: isa::caller_saved(),
            stack_bytes: 0,
            tail: false,
        });
    }

    /// The binary128 image of a double, parked in a 16-byte stack temporary.
    fn ld_extend(&mut self, v: Reg) -> SlotId {
        let slot = self.f.new_slot(16, 16, SlotKind::Local);
        self.push(MInst::ParallelCopy(vec![(Reg::P(PReg::fpr(0)), v, Width::D)]));
        self.fp_libcall("__extenddftf2");
        self.push(MInst::Store {
            op: MemOp::Q,
            src: Reg::P(PReg::fpr(0)),
            mem: AddrMode::Slot { slot, off: 0 },
            vol: false,
        });
        slot
    }

    /// The marker slot standing for the caller's argument area (`SlotKind::InArgs`,
    /// whose offset `pass/frame.rs` fixes at the end of this frame).
    fn in_args(&mut self) -> SlotId {
        if let Some(s) = self.in_args_slot {
            return s;
        }
        let s = self.f.new_slot(0, 8, SlotKind::InArgs);
        self.in_args_slot = Some(s);
        s
    }

    /// The `n ≤ 8` bytes at `p + off`, assembled into one 64-bit register.
    fn load_chunk(&mut self, p: Place, off: i32, n: u32) -> Reg {
        let mut acc: Option<Reg> = None;
        let (mut at, mut rem) = (0u32, n);
        while rem > 0 {
            let step = if rem >= 8 {
                8
            } else if rem >= 4 {
                4
            } else if rem >= 2 {
                2
            } else {
                1
            };
            let d = self.tmp(Width::W64);
            let mem = self.chunk_addr(p, off + at as i32);
            self.push(MInst::Load {
                op: match step {
                    8 => MemOp::X,
                    4 => MemOp::W,
                    2 => MemOp::H,
                    _ => MemOp::B,
                },
                dst: d,
                mem,
                vol: false,
            });
            // a narrow load zero-extends, so the pieces simply OR together
            let piece = if at == 0 {
                d
            } else {
                let sh = self.tmp(Width::W64);
                self.push(MInst::Alu {
                    op: AluOp::Lsl,
                    w: Width::W64,
                    dst: sh,
                    a: d,
                    b: Rhs::Imm((at * 8) as i64),
                    flags: None,
                });
                sh
            };
            acc = Some(match acc {
                None => piece,
                Some(a) => {
                    let o = self.tmp(Width::W64);
                    self.push(MInst::Alu {
                        op: AluOp::Orr,
                        w: Width::W64,
                        dst: o,
                        a,
                        b: Rhs::Reg(piece),
                        flags: None,
                    });
                    o
                }
            });
            at += step;
            rem -= step;
        }
        acc.unwrap()
    }

    /// The dual: write the low `n ≤ 8` bytes of `src` to `p + off`.
    fn store_chunk(&mut self, p: Place, off: i32, n: u32, src: Reg) {
        let (mut at, mut rem) = (0u32, n);
        let mut cur = src;
        while rem > 0 {
            let step = if rem >= 8 {
                8
            } else if rem >= 4 {
                4
            } else if rem >= 2 {
                2
            } else {
                1
            };
            if at > 0 {
                let sh = self.tmp(Width::W64);
                self.push(MInst::Alu {
                    op: AluOp::Lsr,
                    w: Width::W64,
                    dst: sh,
                    a: src,
                    b: Rhs::Imm((at * 8) as i64),
                    flags: None,
                });
                cur = sh;
            }
            let mem = self.chunk_addr(p, off + at as i32);
            self.push(MInst::Store {
                op: match step {
                    8 => MemOp::X,
                    4 => MemOp::W,
                    2 => MemOp::H,
                    _ => MemOp::B,
                },
                src: cur,
                mem,
                vol: false,
            });
            at += step;
            rem -= step;
        }
    }

    /// Copy a whole composite between two places, 8 bytes at a time.
    fn copy_agg(&mut self, dst: Place, src: Place, size: u32) {
        let mut at = 0u32;
        while at < size {
            let n = (size - at).min(8);
            let v = self.load_chunk(src, at as i32, n);
            self.store_chunk(dst, at as i32, n, v);
            at += n;
        }
    }

    /// Move one composite between its address and the `n` consecutive registers
    /// AAPCS64 assigns it (§6.8.2 for x-registers, §5.9.5 for an HFA's v-registers).
    fn agg_regs(
        &mut self,
        base: Reg,
        first: PReg,
        n: u32,
        esz: u32,
        size: u32,
        out: bool,
    ) -> Vec<(Reg, Reg, Width)> {
        let mut pairs = Vec::with_capacity(n as usize);
        for i in 0..n {
            let off = (i * esz) as i32;
            let p = PReg {
                class: first.class,
                num: first.num + i as u8,
            };
            if first.class == Class::Fpr {
                let (w, op) = if esz == 8 {
                    (Width::D, MemOp::D)
                } else {
                    (Width::S, MemOp::S)
                };
                if out {
                    let d = self.tmp(w);
                    self.push(MInst::Load {
                        op,
                        dst: d,
                        mem: AddrMode::BaseImm { base, off },
                        vol: false,
                    });
                    pairs.push((Reg::P(p), d, w));
                } else {
                    self.push(MInst::Store {
                        op,
                        src: Reg::P(p),
                        mem: AddrMode::BaseImm { base, off },
                        vol: false,
                    });
                }
            } else {
                let rem = size.saturating_sub(i * esz).min(esz).max(1);
                if out {
                    let d = self.load_chunk(Place::At(base), off, rem);
                    pairs.push((Reg::P(p), d, Width::W64));
                } else {
                    self.store_chunk(Place::At(base), off, rem, Reg::P(p));
                }
            }
        }
        pairs
    }

    fn libcall(&mut self, name: &str, args: &[Reg]) {
        let pairs: Vec<(Reg, Reg, Width)> = args
            .iter()
            .enumerate()
            .map(|(i, r)| (Reg::P(PReg::gpr(i as u8)), *r, Width::W64))
            .collect();
        self.push(MInst::ParallelCopy(pairs));
        let uses = (0..args.len())
            .map(|i| (Reg::P(PReg::gpr(i as u8)), PReg::gpr(i as u8)))
            .collect();
        self.push(MInst::Call {
            callee: CallTarget::Direct(name.to_string()),
            uses,
            defs: vec![],
            clobbers: isa::caller_saved(),
            stack_bytes: 0,
            tail: false,
        });
    }

    /// A DENSE switch becomes a table lookup: `sub` the smallest label, one
    /// unsigned compare against the span (which also rejects everything below
    /// the smallest — DDI 0487 C6.2.24's `b.hi` reads the subtraction as
    /// unsigned), then an indexed branch. `layout` and `emit` turn the
    /// terminator into `adr`/`ldr`/`br` over a `.rodata` table of offsets.
    ///
    /// The density rule is gcc's: at least four cases, and at least half the
    /// span occupied. Below that the table is mostly padding and a compare chain
    /// is both smaller and — for a handful of cases — no slower.
    /// The case count at which a jump table beats a compare tree. MEASURED, not
    /// chosen: see `jump_table` and MECHANISM.md Part F R4.14 (2).
    fn jump_table(
        &mut self,
        x: Reg,
        ty: hir::Ty,
        arms: &[(i64, hir::Target)],
        dflt: &MTarget,
    ) -> Option<MTerm> {
        // Article E, "the spec's number or my convenience's number?". R3.3 chose
        // 4 by taste; gcc -O1 builds a `cmp`/`tbnz`/`csel` compare TREE for
        // d1_switch's 8 cases and wins 1.33× on it, because an indirect branch
        // through a table is unpredictable while a tree of two or three
        // predictable compares is not. So the number is one to MEASURE, and
        // `ZCC_JT` sets it while the crossover is being taken.
        let min_cases: usize = std::env::var("ZCC_JT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(MIN_CASES);
        let why = |r: &str, span: i64| {
            if std::env::var("ZCC_JTDBG").is_ok() {
                eprintln!("JT {} arms={} span={} {}", self.h.name, arms.len(), span, r);
            }
        };
        if arms.len() < min_cases {
            why("refused: too-few-arms", 0);
            return None;
        }
        let lo = arms.iter().map(|(k, _)| *k).min()?;
        let hi = arms.iter().map(|(k, _)| *k).max()?;
        let span_check = hi.checked_sub(lo)?.checked_add(1)?;
        if span_check > (arms.len() as i64).checked_mul(2)? || span_check > 4096 {
            why("refused: span", span_check);
            return None;
        }
        let span = hi.checked_sub(lo)?.checked_add(1)?;
        if span > (arms.len() as i64).checked_mul(2)? || span > 4096 {
            return None;
        }
        // AN ARM THAT CARRIES EDGE ARGUMENTS GETS A TRAMPOLINE.
        //
        // A table entry is an address, so it has nowhere to put an edge's
        // copies, and this used to refuse the whole switch on that ground. What
        // it refused, measured: sqlite's `sqlite3VdbeExec` dispatches 196
        // opcodes, EVERY arm carries edge arguments because values are live
        // across the switch, so the table never fired and the dispatch became a
        // LINEAR COMPARE CHAIN — 183 `cmp`/`b.eq` pairs over 187 distinct
        // opcode constants, walked about ninety deep on average, roughly 1.4
        // million times in a 100,000-row INSERT. gcc spends one indirect branch.
        // That function carries 85% of sqlite's runtime gap (`MEASURED M16`).
        //
        // The fix is the block the comment already named: give the arm its own
        // block, put the edge there, and let the table point at THAT. The
        // trampoline is one `b` on the taken path — against ninety compares —
        // and the copies it carries are the ones the edge always had.
        // …AND SO DOES THE DEFAULT, for exactly the same reason.
        //
        // The arms got their trampoline; the default did not, and the refusal
        // above it turned every switch whose DEFAULT edge carries copies back
        // into a linear compare chain — the very shape the paragraph above says
        // costs `sqlite3VdbeExec` 85% of sqlite's gap. `k1_dispatch` is that
        // case in miniature: forty dense arms, well past `MIN_CASES`, span equal
        // to the arm count, and no table, because one edge out of forty-one had
        // arguments. gcc spends one indirect branch there; zcc spent
        // forty-one `cmp`/`b.eq` pairs (`MEASURED M34`).
        //
        // The default appears in two places and only one of them is a problem.
        // The out-of-range `Bcc` below is an ordinary MIR edge and carries its
        // copies as any edge does. The TABLE cannot: an entry is an address, and
        // both the holes in the range and the `default` field are filled with
        // it. So the table gets a trampoline and the `Bcc` keeps the real edge.
        if !dflt.args.is_empty() && !jt_default() {
            why("refused: default-args", span);
            return None;
        }
        why("ACCEPTED", span);
        let dflt_t = if dflt.args.is_empty() {
            dflt.clone()
        } else {
            let tb = self.f.new_block();
            self.f.blocks[tb as usize].term = MTerm::B(dflt.clone());
            MTarget { block: tb, args: vec![] }
        };
        let arms: Vec<(i64, MTarget)> = arms
            .iter()
            .map(|(k, t)| {
                let mt = self.target(t);
                if mt.args.is_empty() {
                    (*k, mt)
                } else {
                    let tb = self.f.new_block();
                    self.f.blocks[tb as usize].term = MTerm::B(mt);
                    (*k, MTarget { block: tb, args: vec![] })
                }
            })
            .collect();
        let w = wid(ty);
        // `v - lo`, then the unsigned range test against the span. A 32-bit `sub`
        // zeroes bits 63:32 (DDI 0487 B1.2.1), so its result is already the
        // 64-bit index the table wants and no extension instruction is needed —
        // but a table starting at zero needs no subtraction at all, and then the
        // 32-bit value has to be widened after all.
        let idx = if lo == 0 && w == Width::W64 {
            x
        } else {
            let d = self.tmp(w);
            let rhs = match imm::as_rhs(AluOp::Sub, lo, w) {
                Some(r) => r,
                None => {
                    let g = self.tmp(w);
                    self.push(imm::materialize(g, lo, w));
                    Rhs::Reg(g)
                }
            };
            self.push(MInst::Alu { op: AluOp::Sub, w, dst: d, a: x, b: rhs, flags: None });
            d
        };
        let fl = self.f.new_flags();
        let bound = match imm::as_rhs(AluOp::Sub, span - 1, w) {
            Some(r) => r,
            None => {
                let g = self.tmp(w);
                self.push(imm::materialize(g, span - 1, w));
                Rhs::Reg(g)
            }
        };
        self.push(MInst::Cmp { kind: CmpKind::Cmp, w, a: idx, b: bound, flags: fl });
        let body = self.f.new_block();
        let out = MTerm::Bcc(
            CC::Hi,
            fl,
            dflt.clone(),
            MTarget { block: body, args: vec![] },
        );
        let mut table: Vec<MTarget> = vec![dflt_t.clone(); span as usize];
        for (k, t) in &arms {
            table[(k - lo) as usize] = t.clone();
        }
        self.f.blocks[body as usize].term = MTerm::Switch {
            idx,
            table,
            default: dflt_t,
        };
        Some(out)
    }

    fn call(
        &mut self,
        dst: Option<ValueId>,
        sig: &hir::Sig,
        callee: &hir::Callee,
        args: &[Operand],
        sret: Option<Operand>,
    ) {
        let asn = abi::classify(sig);
        self.f.outgoing = self.f.outgoing.max(asn.stack_bytes);
        // Hack 2007 §4: a constrained instruction is preceded by ONE parallel
        // copy that puts every operand where the ABI wants it. The allocator
        // then sees no fixed constraint at all — the argument registers are
        // ordinary physical registers with very short live ranges, and a cycle
        // among them (f(b, a) where a is in x1 and b in x0) is resolved by the
        // same windmill sequentialization every block edge uses.
        let mut pairs = Vec::with_capacity(args.len());
        // long double arguments are converted FIRST (each conversion is itself a
        // call) and only loaded into their v register once every conversion is
        // done — see `ld_extend`.
        let mut quads: Vec<(Loc, SlotId)> = Vec::new();
        for ((o, p), loc) in args.iter().zip(&sig.params).zip(&asn.args) {
            match (p, loc) {
                (hir::PTy::S(t), Loc::Reg(pr, w)) => {
                    let r = self.reg(*o, *t);
                    pairs.push((Reg::P(*pr), r, *w));
                }
                (hir::PTy::S(t), Loc::Stack(off, _)) => {
                    let r = self.reg(*o, *t);
                    self.push(MInst::Store {
                        op: memop(*t),
                        src: r,
                        mem: AddrMode::SpArg { off: *off },
                        vol: false,
                    });
                }
                // §6.8.2 B.4: a composite over 16 bytes is already a pointer
                (hir::PTy::Agg { .. }, Loc::Reg(pr, w)) => {
                    let r = self.reg(*o, hir::Ty::I64);
                    pairs.push((Reg::P(*pr), r, *w));
                }
                (hir::PTy::Agg { .. }, Loc::Stack(off, _)) => {
                    let r = self.reg(*o, hir::Ty::I64);
                    self.push(MInst::Store {
                        op: MemOp::X,
                        src: r,
                        mem: AddrMode::SpArg { off: *off },
                        vol: false,
                    });
                }
                (
                    hir::PTy::Agg { size, .. },
                    Loc::Regs {
                        first, n, esz, ..
                    },
                ) => {
                    let a = self.base(*o);
                    let mut ps = self.agg_regs(a, *first, *n, *esz, *size, true);
                    pairs.append(&mut ps);
                }
                (hir::PTy::Agg { size, .. }, Loc::StackAgg { off, .. }) => {
                    let a = self.base(*o);
                    self.copy_agg(Place::Out(*off), Place::At(a), *size);
                }
                (hir::PTy::LDouble, _) => {
                    let v = self.reg(*o, hir::Ty::F64);
                    let slot = self.ld_extend(v);
                    quads.push((*loc, slot));
                }
                (hir::PTy::S(_), Loc::Regs { .. } | Loc::StackAgg { .. }) => {
                    unreachable!("scalar in a composite location")
                }
            }
        }
        for (loc, slot) in quads {
            let q = self.tmp(Width::Q);
            self.push(MInst::Load {
                op: MemOp::Q,
                dst: q,
                mem: AddrMode::Slot { slot, off: 0 },
                vol: false,
            });
            match loc {
                Loc::Reg(pr, _) => pairs.push((Reg::P(pr), q, Width::Q)),
                Loc::Stack(off, _) => self.push(MInst::Store {
                    op: MemOp::Q,
                    src: q,
                    mem: AddrMode::SpArg { off },
                    vol: false,
                }),
                _ => unreachable!("binary128 in a composite location"),
            }
        }
        // §6.9: the indirect result register is an ordinary fixed argument.
        if asn.sret {
            let a = self.reg(sret.expect("composite return without a destination"), hir::Ty::I64);
            pairs.push((Reg::P(PReg::gpr(8)), a, Width::W64));
        }
        let target = match callee {
            hir::Callee::Direct(n) => CallTarget::Direct(n.clone()),
            hir::Callee::Indirect(o) => CallTarget::Indirect(self.reg(*o, hir::Ty::I64)),
        };
        if !pairs.is_empty() {
            self.push(MInst::ParallelCopy(pairs));
        }
        let mut uses: Vec<(Reg, PReg)> = asn
            .args
            .iter()
            .flat_map(|l| match l {
                Loc::Reg(p, _) => vec![(Reg::P(*p), *p)],
                Loc::Regs { first, n, .. } => (0..*n)
                    .map(|i| {
                        let p = PReg {
                            class: first.class,
                            num: first.num + i as u8,
                        };
                        (Reg::P(p), p)
                    })
                    .collect(),
                Loc::Stack(..) | Loc::StackAgg { .. } => vec![],
            })
            .collect();
        if asn.sret {
            uses.push((Reg::P(PReg::gpr(8)), PReg::gpr(8)));
        }
        let defs = match asn.ret {
            Some(Loc::Reg(p, _)) if dst.is_some() => vec![(Reg::P(p), p)],
            Some(Loc::Regs { first, n, .. }) if sret.is_some() => (0..n)
                .map(|i| {
                    let p = PReg {
                        class: first.class,
                        num: first.num + i as u8,
                    };
                    (Reg::P(p), p)
                })
                .collect(),
            _ => vec![],
        };
        self.push(MInst::Call {
            callee: target,
            uses,
            defs,
            clobbers: isa::caller_saved(),
            stack_bytes: asn.stack_bytes,
            tail: false,
        });
        // and one copy out of the result register
        if let (Some(v), Some(hir::PTy::LDouble)) = (dst, sig.ret.as_ref()) {
            // the quad is already in v0, which is where __trunctfdf2 wants it
            self.fp_libcall("__trunctfdf2");
            let d = self.dst_of(v);
            self.push(MInst::FMov {
                dw: Width::D,
                sw: Width::D,
                dst: d,
                src: Reg::P(PReg::fpr(0)),
            });
            return;
        }
        match (dst, sret, asn.ret) {
            (Some(v), _, Some(Loc::Reg(p, w))) => {
                let d = self.dst_of(v);
                if p.class == Class::Fpr {
                    self.push(MInst::FMov {
                        dw: w,
                        sw: w,
                        dst: d,
                        src: Reg::P(p),
                    });
                } else {
                    self.push(MInst::Copy { w, dst: d, src: Reg::P(p) });
                }
            }
            // §6.9: a composite of 16 bytes or fewer comes back in registers and
            // is written to the destination the caller reserved.
            (
                _,
                Some(o),
                Some(Loc::Regs {
                    first,
                    n,
                    esz,
                    size,
                }),
            ) => {
                let a = self.reg(o, hir::Ty::I64);
                self.agg_regs(a, first, n, esz, size, false);
            }
            _ => {}
        }
    }

    fn target(&mut self, t: &hir::Target) -> MTarget {
        // An edge argument takes the type of the block PARAMETER it feeds, so a
        // constant is materialized at the parameter's width.
        let ptys: Vec<hir::Ty> = self.h.blocks[t.block as usize]
            .params
            .iter()
            .map(|p| self.h.ty_of(*p))
            .collect();
        let args = t
            .args
            .iter()
            .zip(&ptys)
            .map(|(a, ty)| self.reg(*a, *ty))
            .collect();
        MTarget {
            block: t.block,
            args,
        }
    }

    fn terminator(&mut self, t: &Term) {
        let term = match t {
            Term::Jmp(x) => {
                let x = self.target(x);
                MTerm::B(x)
            }
            // The condition is an I32 that C says is "nonzero = true": `cbnz`
            // tests it directly, with no compare instruction at all.
            // `if (x & (1<<k))` — one instruction, no mask and no compare
            // (DDI 0487 C6.2.375). Decided in `munch`, where the use counts are.
            Term::Br(_, x, y) if self.tb.contains_key(&(self.cur as hir::BlockId)) => {
                self.fuse = None;
                let (v, ty, bit, set) = self.tb[&(self.cur as hir::BlockId)];
                let r = self.reg_nonzr(Operand::Val(v), ty);
                let (x, y) = (self.target(x), self.target(y));
                MTerm::Tb { w: wid(ty), reg: r, bit, set, t: x, f: y }
            }
            Term::Br(c, x, y) => match self.fuse.take() {
                // `x == 0` / `x != 0` needs no compare at all: `cbz`/`cbnz` test
                // the register and branch in one instruction (DDI 0487 C6.2.42).
                // `x < 0` and `x >= 0` are a single bit: `tbnz`/`tbz` on the
                // sign bit, with no compare at all (DDI 0487 C6.2.375).
                Some((op, ty, a, Operand::Imm(0)))
                    if matches!(op, hir::CmpOp::Slt | hir::CmpOp::Sge) && !ty.is_float() =>
                {
                    let r = self.reg_nonzr(a, ty);
                    let (x, y) = (self.target(x), self.target(y));
                    MTerm::Tb {
                        w: wid(ty),
                        reg: r,
                        bit: (ty.bits() - 1) as u8,
                        set: op == hir::CmpOp::Slt,
                        t: x,
                        f: y,
                    }
                }
                Some((op, ty, a, b))
                    if matches!(op, hir::CmpOp::Eq | hir::CmpOp::Ne)
                        && !ty.is_float()
                        && (a == Operand::Imm(0) || b == Operand::Imm(0)) =>
                {
                    let v = if a == Operand::Imm(0) { b } else { a };
                    let r = self.reg_nonzr(v, ty);
                    let (x, y) = (self.target(x), self.target(y));
                    MTerm::Cbz {
                        w: wid(ty),
                        reg: r,
                        zero: op == hir::CmpOp::Eq,
                        t: x,
                        f: y,
                    }
                }
                Some((op, ty, a, b)) => {
                    let (fl, cc) = self.compare(op, ty, a, b);
                    let (x, y) = (self.target(x), self.target(y));
                    MTerm::Bcc(cc, fl, x, y)
                }
                None => {
                    let r = self.reg(*c, hir::Ty::I32);
                    let (x, y) = (self.target(x), self.target(y));
                    MTerm::Cbz {
                        w: Width::W32,
                        reg: r,
                        zero: false,
                        t: x,
                        f: y,
                    }
                }
            },
            Term::Ret(v) => {
                let rt = self.h.sig.ret.clone();
                match (rt, v) {
                    (Some(hir::PTy::Agg { size, .. }), Some(o)) => {
                        let a = self.reg(*o, hir::Ty::I64);
                        match self.asn.ret {
                            // §6.9: ≤16 bytes (or an HFA) go back in registers
                            Some(Loc::Regs { first, n, esz, size }) => {
                                let ps = self.agg_regs(a, first, n, esz, size, true);
                                self.push(MInst::ParallelCopy(ps));
                            }
                            // …anything larger through the caller's x8, whose
                            // value AAPCS64 also returns in x0
                            _ => {
                                let p = self.sret_ptr.expect("indirect result без x8");
                                let n = self.reg(Operand::Imm(size as i64), hir::Ty::I64);
                                self.libcall("memcpy", &[p, a, n]);
                                self.push(MInst::Copy {
                                    w: Width::W64,
                                    dst: Reg::P(PReg::gpr(0)),
                                    src: p,
                                });
                            }
                        }
                    }
                    (Some(hir::PTy::LDouble), Some(o)) => {
                        let v = self.reg(*o, hir::Ty::F64);
                        self.push(MInst::ParallelCopy(vec![(
                            Reg::P(PReg::fpr(0)),
                            v,
                            Width::D,
                        )]));
                        self.fp_libcall("__extenddftf2");
                    }
                    (Some(hir::PTy::S(ty)), Some(o)) => {
                        let r = self.reg(*o, ty);
                        if ty.is_float() {
                            self.push(MInst::FMov {
                                dw: wid(ty),
                                sw: wid(ty),
                                dst: Reg::P(PReg::fpr(0)),
                                src: r,
                            });
                        } else {
                            self.push(MInst::Copy {
                                w: wid(ty),
                                dst: Reg::P(PReg::gpr(0)),
                                src: r,
                            });
                        }
                    }
                    // a value returned by a void function, or none at all
                    (None, Some(o)) => {
                        let _ = self.reg(*o, hir::Ty::I64);
                    }
                    (_, None) => {}
                }
                MTerm::Ret
            }
            Term::Unreachable => MTerm::Unreachable,
            Term::GotoPtr(o, bs) => {
                let r = self.reg(*o, hir::Ty::I64);
                MTerm::BrReg(r, bs.clone())
            }
            // R0 lowers a switch to a compare chain; R3.3 adds the jump table
            // (the density threshold is a dated policy constant there).
            Term::Switch(c, ty, arms, d) => {
                // every arm compares against an immediate, so the switch value
                // may not be the zero register (see `reg_nonzr`)
                let x = self.reg_nonzr(*c, *ty);
                let dflt = self.target(d);
                if let Some(t) = self.jump_table(x, *ty, arms, &dflt) {
                    self.f.blocks[self.cur as usize].term = t;
                    return;
                }
                let mut next = dflt;
                // THE ARM THAT STAYS GOES FIRST (`MEASURED M31`).
                //
                // A linear chain tests its arms in order, so an arm at position
                // `i` costs `i` `cmp`+`b.eq` pairs on every byte that lands in
                // it. Source order is therefore a policy, and it is the wrong
                // one for the shape that dominates protocol parsing: a `switch`
                // on a state inside a read loop, where ONE state consumes runs —
                // a payload, a header value — and re-enters itself until a
                // delimiter arrives.
                //
                // That state is identifiable without a profile. Its arm's edge
                // back to the loop header passes the switch's OWN operand as the
                // state parameter, unchanged; every other arm passes a different
                // value. `stays_in_state` asks exactly that, and the partition
                // below is STABLE, so arms that tie keep source order.
                //
                // Measured by hand-editing the `.s`, output identical and
                // instruction count unchanged: `m2_http_parse` 0.8566 (1.318 →
                // 1.13) with this rule, 0.7754 with the ideal order — the gap is
                // the Law-4 residual, since ranking WITHIN the staying arms needs
                // something a static rule does not have. A balanced binary search
                // over the same arms measured 1.0741, which is `MEASURED M4`
                // arriving from the other side: the split costs an unconditional
                // branch on every path and buys nothing once the hot arm is
                // first.
                let arms = self.order_switch_arms(*c, arms);
                let arms: Vec<(i64, MTarget)> = arms
                    .iter()
                    .map(|(k, t)| (*k, self.target(t)))
                    .collect();
                // build the chain backwards so each test falls through to the next
                for (k, t) in arms.into_iter().rev() {
                    let test = self.f.new_block();
                    let fl = self.f.new_flags();
                    let w = wid(*ty);
                    let rhs = match imm::as_rhs(AluOp::Sub, k, w) {
                        Some(r) => r,
                        None => {
                            let g = self.tmp(w);
                            let seq = imm::materialize(g, k, w);
                            self.f.blocks[test as usize].insts.push(seq);
                            Rhs::Reg(g)
                        }
                    };
                    self.f.blocks[test as usize].insts.push(MInst::Cmp {
                        kind: CmpKind::Cmp,
                        w,
                        a: x,
                        b: rhs,
                        flags: fl,
                    });
                    self.f.blocks[test as usize].term = MTerm::Bcc(CC::Eq, fl, t, next);
                    next = MTarget {
                        block: test,
                        args: vec![],
                    };
                }
                MTerm::B(next)
            }
        };
        self.f.blocks[self.cur as usize].term = term;
    }
}

/// `mir::CvtOp` (integer↔float) under a distinct name: `hir::CvtOp` is a
/// different, larger set and both are in scope here.
use crate::mir::CvtOp as CvtOp2;

fn lower_func(h: &hir::Func) -> MFunc {
    let mut f = MFunc {
        name: h.name.clone(),
        blocks: Vec::new(),
        vregs: Vec::new(),
        slots: Vec::new(),
        entry: h.entry,
        is_static: h.is_static,
        is_weak: h.is_weak,
        order: Vec::new(),
        laid_out: false,
        frame_size: 0,
        saved: RegSet::default(),
        dyn_stack: false,
        has_vla: h.has_vla,
        outgoing: 0,
        fp_slot: 0,
        cs_saves: Vec::new(),
        physical: false,
    };
    for s in &h.slots {
        f.new_slot(s.size, s.align, SlotKind::Local);
    }
    for _ in 0..h.blocks.len() {
        f.new_block();
    }
    // one virtual register per HIR value: HIR is already SSA, so this mapping is
    // a bijection and needs no renaming
    let mut vmap = Vec::with_capacity(h.values.len());
    for v in &h.values {
        vmap.push(f.new_vreg(wid(v.ty)));
    }
    for (bi, b) in h.blocks.iter().enumerate() {
        f.blocks[bi].weight = b.weight;
        f.blocks[bi].labels = b.labels.clone();
        f.blocks[bi].params = b.params.iter().map(|p| vmap[*p as usize]).collect();
    }
    // AAPCS64 entry: each parameter arrives where the ABI names it, and the
    // entry block moves it into its virtual register. Everything that READS an
    // incoming physical register does so inside ONE parallel copy — otherwise a
    // temporary the allocator happens to colour x1 could destroy the second
    // argument before it has been read.
    let asn = abi::classify(&h.sig);
    let m = munch(h);
    let mut l = L {
        h,
        f,
        vmap,
        cur: h.entry,
        asn,
        sret_ptr: None,
        in_args_slot: None,
        va_slot: None,
        fold: m.addr,
        alu: m.alu,
        br: m.br,
        sel: m.sel,
        ldext: m.ldext,
        tb: m.tb,
        csop: m.csop,
        dead: m.dead,
        fuse: None,
    };
    let mut pairs = Vec::new();
    // (destination value, the registers holding the composite, its shape)
    let mut agg_regs: Vec<(Reg, Vec<(Reg, Width)>, u32, u32, u32)> = Vec::new();
    // (destination value, the in-argument-area offset, load width) for scalars
    let mut from_stack: Vec<(Reg, u32, hir::Ty)> = Vec::new();
    let mut agg_stack: Vec<(Reg, u32)> = Vec::new();
    // (destination F64 value, the quad's register if it came in one, else its
    // offset in the caller's argument area)
    let mut ld_params: Vec<(Reg, Option<Reg>, u32)> = Vec::new();
    if l.asn.sret {
        let p = l.f.new_vreg(Width::W64);
        pairs.push((p, Reg::P(PReg::gpr(8)), Width::W64));
        l.sret_ptr = Some(p);
    }
    for (i, vi) in h.values.iter().enumerate() {
        let hir::Def::FuncParam(k) = vi.def else {
            continue;
        };
        let d = l.vmap[i];
        let (p, loc) = match (h.sig.params.get(k as usize), l.asn.args.get(k as usize)) {
            (Some(p), Some(loc)) => (p.clone(), *loc),
            _ => continue,
        };
        match (&p, loc) {
            (hir::PTy::S(_) | hir::PTy::Agg { .. }, Loc::Reg(pr, w)) => {
                pairs.push((d, Reg::P(pr), w))
            }
            (hir::PTy::S(t), Loc::Stack(off, _)) => from_stack.push((d, off, *t)),
            (hir::PTy::Agg { .. }, Loc::Stack(off, _)) => {
                from_stack.push((d, off, hir::Ty::I64))
            }
            (hir::PTy::Agg { size, align, .. }, Loc::Regs { first, n, esz, .. }) => {
                let mut rs = Vec::with_capacity(n as usize);
                for j in 0..n {
                    let pr = PReg {
                        class: first.class,
                        num: first.num + j as u8,
                    };
                    let w = match (first.class, esz) {
                        (Class::Fpr, 8) => Width::D,
                        (Class::Fpr, _) => Width::S,
                        _ => Width::W64,
                    };
                    let v = l.f.new_vreg(w);
                    pairs.push((v, Reg::P(pr), w));
                    rs.push((v, w));
                }
                agg_regs.push((d, rs, esz, *size, *align));
            }
            (hir::PTy::Agg { .. }, Loc::StackAgg { off, .. }) => agg_stack.push((d, off)),
            // A quad parameter arrives as binary128 but the body reads an F64
            // value (THEORY II-2), so the bridge runs once, at entry.
            (hir::PTy::LDouble, Loc::Reg(pr, _)) => {
                let v = l.f.new_vreg(Width::Q);
                pairs.push((v, Reg::P(pr), Width::Q));
                ld_params.push((d, Some(v), 0));
            }
            (hir::PTy::LDouble, Loc::Stack(off, _)) => ld_params.push((d, None, off)),
            (hir::PTy::LDouble, _) => unreachable!("binary128 in a composite location"),
            (hir::PTy::S(_), Loc::Regs { .. } | Loc::StackAgg { .. }) => {
                unreachable!("scalar in a composite location")
            }
        }
    }
    // AAPCS64 §B.6: a variadic function preserves the argument registers the
    // named parameters did NOT consume, so `va_arg` can read them back. They are
    // taken in the same parallel copy as the parameters — a temporary the
    // allocator happened to colour x3 would otherwise destroy one first.
    let mut va_save: Vec<(Reg, u32, MemOp)> = Vec::new();
    if h.sig.variadic {
        let (ngrn, nsrn) = (l.asn.ngrn, l.asn.nsrn);
        let va = l.va_area();
        for i in ngrn..8 {
            let v = l.f.new_vreg(Width::W64);
            pairs.push((v, Reg::P(PReg::gpr(i as u8)), Width::W64));
            va_save.push((v, 128 + 8 * i, MemOp::X));
        }
        for i in nsrn..8 {
            let v = l.f.new_vreg(Width::Q);
            pairs.push((v, Reg::P(PReg::fpr(i as u8)), Width::Q));
            va_save.push((v, 16 * i, MemOp::Q));
        }
        let _ = va;
    }
    l.cur = h.entry;
    if !pairs.is_empty() {
        l.push(MInst::ParallelCopy(pairs));
    }
    for (v, off, op) in va_save {
        let slot = l.va_area();
        l.push(MInst::Store {
            op,
            src: v,
            mem: AddrMode::Slot {
                slot,
                off: off as i32,
            },
            vol: false,
        });
    }
    // THEORY II-2: convert each incoming binary128 to the canonical f64 the
    // body computes with. Done AFTER the parallel copy, so the conversion call
    // cannot destroy an argument register that has not been read yet.
    // Every incoming quad is PARKED IN MEMORY first, before any conversion call
    // runs. AAPCS64 §6.1.2 preserves only the LOW half of v8–v15, so a
    // 128-bit value has no register that survives a call — a quad must never be
    // live across one, and the parking is what guarantees it.
    let parked: Vec<(Reg, SlotId, u32)> = ld_params
        .iter()
        .map(|&(d, q, off)| match q {
            Some(q) => {
                let slot = l.f.new_slot(16, 16, SlotKind::Local);
                l.push(MInst::Store {
                    op: MemOp::Q,
                    src: q,
                    mem: AddrMode::Slot { slot, off: 0 },
                    vol: false,
                });
                (d, slot, 0)
            }
            None => (d, l.in_args(), off),
        })
        .collect();
    for (d, slot, off) in parked {
        let t = l.tmp(Width::Q);
        l.push(MInst::Load {
            op: MemOp::Q,
            dst: t,
            mem: AddrMode::Slot {
                slot,
                off: off as i32,
            },
            vol: false,
        });
        l.push(MInst::ParallelCopy(vec![(Reg::P(PReg::fpr(0)), t, Width::Q)]));
        l.fp_libcall("__trunctfdf2");
        l.push(MInst::FMov {
            dw: Width::D,
            sw: Width::D,
            dst: d,
            src: Reg::P(PReg::fpr(0)),
        });
    }
    // A composite delivered in registers has no address of its own, so one is
    // made: the registers are written to a scratch object and the parameter's
    // "incoming address" is that object. R2.2's SROA is what removes the copy.
    for (d, rs, esz, size, align) in agg_regs {
        let slot = l.f.new_slot(size.max(1), align.max(8), SlotKind::Local);
        for (j, (v, w)) in rs.iter().enumerate() {
            let off = (j as u32 * esz) as i32;
            if w.class() == Class::Fpr {
                let op = match w {
                    Width::Q => MemOp::Q,
                    Width::D => MemOp::D,
                    _ => MemOp::S,
                };
                l.push(MInst::Store {
                    op,
                    src: *v,
                    mem: AddrMode::Slot { slot, off },
                    vol: false,
                });
            } else {
                let n = size.saturating_sub(j as u32 * esz).min(esz).max(1);
                l.store_chunk(Place::Slot(slot), off, n, *v);
            }
        }
        l.push(MInst::SlotAddr { dst: d, slot, off: 0 });
    }
    // A composite the caller left on the stack is already an object: its address
    // is simply a point in the caller's argument area — no copy at all.
    for (d, off) in agg_stack {
        let slot = l.in_args();
        l.push(MInst::SlotAddr {
            dst: d,
            slot,
            off: off as i32,
        });
    }
    for (d, off, t) in from_stack {
        let slot = l.in_args();
        l.push(MInst::Load {
            op: memop(t),
            dst: d,
            mem: AddrMode::Slot {
                slot,
                off: off as i32,
            },
            vol: false,
        });
    }
    // how many times each value is read — the single-use test the munch table
    // needs before it may fold a producer into its consumer
    let mut uses = vec![0u32; h.values.len()];
    for b in &h.blocks {
        for inst in &b.insts {
            inst.uses(|o| {
                if let Operand::Val(v) = o {
                    uses[v as usize] += 1;
                }
            });
        }
        b.term.uses(|o| {
            if let Operand::Val(v) = o {
                uses[v as usize] += 1;
            }
        });
    }
    let mut prologue: Vec<MInst> = std::mem::take(&mut l.f.blocks[h.entry as usize].insts);
    for (bi, b) in h.blocks.iter().enumerate() {
        l.cur = bi as MBlockId;
        if bi == h.entry as usize {
            let p = std::mem::take(&mut prologue);
            l.f.blocks[bi].insts = p;
        }
        l.fuse = l.br.get(&(bi as hir::BlockId)).copied();
        for i in b.insts.iter() {
            if let Some(d) = i.dst() {
                if l.dead.contains(&d) {
                    continue; // it rides inside the instruction that consumes it
                }
            }
            l.inst(i);
        }
        l.terminator(&b.term);
        l.fuse = None;
    }
    l.f
}
