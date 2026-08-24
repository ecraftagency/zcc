// MIR — the load-bearing machine layer (REARCH.md §5).
//
// One type, two lifecycle states (exactly LLVM's "MIR"):
//   * VIRTUAL phase — SSA over virtual registers, block parameters on edges.
//     Every machine optimization that needs SSA (compare-elimination, auto-inc,
//     the extend lattice, ldp/stp pairing) runs here, on real machine operands
//     rather than on assembly text. This is the whole point of the layer: in
//     rc3 addressing modes, flags and post-index were not expressible in the IR,
//     so every machine optimization was a fragile string peephole.
//   * PHYSICAL phase — after `regalloc`: no virtual register survives, every
//     fixed constraint is satisfied, block parameters are gone (destructed into
//     parallel copies), and `frame`/`layout` have resolved slots and order.
//
// The allocator, liveness, the verifier and the interpreter reach an
// instruction's registers ONLY through `visit`/`visit_mut` and its memory effect
// through `effect`. No component outside `isa.rs` matches on an opcode.
pub mod interp;
pub mod isa;
pub mod pass;
#[cfg(test)]
mod tests;
pub mod verify;

use crate::hir::Sym;

pub type VReg = u32;
pub type MBlockId = u32;

// ── registers ──────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Class {
    /// x0–x30 (AAPCS64 §6.1.1)
    Gpr,
    /// v0–v31
    Fpr,
    /// NZCV — a register class of size 1. Modeling flags this way makes
    /// "two live compares" an ordinary interference (which the allocator
    /// resolves by rematerializing the `cmp`, always legal: a compare is pure)
    /// instead of a hand-written scheduling rule.
    Flags,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct PReg {
    pub class: Class,
    pub num: u8,
}

impl PReg {
    pub const fn gpr(n: u8) -> PReg {
        PReg {
            class: Class::Gpr,
            num: n,
        }
    }
    pub const fn fpr(n: u8) -> PReg {
        PReg {
            class: Class::Fpr,
            num: n,
        }
    }
    pub const NZCV: PReg = PReg {
        class: Class::Flags,
        num: 0,
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Reg {
    V(VReg),
    P(PReg),
}

impl Reg {
    pub fn vreg(self) -> Option<VReg> {
        match self {
            Reg::V(v) => Some(v),
            Reg::P(_) => None,
        }
    }
    pub fn preg(self) -> Option<PReg> {
        match self {
            Reg::P(p) => Some(p),
            Reg::V(_) => None,
        }
    }
}

/// The width a value occupies, which is what decides its `w`/`x`/`s`/`d` form
/// and its spill-slot size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    W32,
    W64,
    S,
    D,
}

impl Width {
    pub fn class(self) -> Class {
        match self {
            Width::W32 | Width::W64 => Class::Gpr,
            Width::S | Width::D => Class::Fpr,
        }
    }
    pub fn bytes(self) -> u32 {
        match self {
            Width::W32 | Width::S => 4,
            Width::W64 | Width::D => 8,
        }
    }
    /// the `x` vs `w` bit of an A64 ALU encoding
    pub fn is64(self) -> bool {
        matches!(self, Width::W64 | Width::D)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VRegInfo {
    pub class: Class,
    pub width: Width,
}

/// A set of physical registers — a call's clobber list, the callee-saved set.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RegSet {
    pub gpr: u32,
    pub fpr: u32,
}

impl RegSet {
    pub fn add(&mut self, p: PReg) {
        match p.class {
            Class::Gpr => self.gpr |= 1 << p.num,
            Class::Fpr => self.fpr |= 1 << p.num,
            Class::Flags => {}
        }
    }
    pub fn has(&self, p: PReg) -> bool {
        match p.class {
            Class::Gpr => self.gpr >> p.num & 1 != 0,
            Class::Fpr => self.fpr >> p.num & 1 != 0,
            Class::Flags => false,
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = PReg> + '_ {
        (0..32u8)
            .filter(|n| self.gpr >> n & 1 != 0)
            .map(PReg::gpr)
            .chain((0..32u8).filter(|n| self.fpr >> n & 1 != 0).map(PReg::fpr))
    }
}

// ── operands ───────────────────────────────────────────────────────────────
/// A stack object: a spill slot, an `alloca`, the outgoing-argument area. Byte
/// offsets are assigned by `pass/frame.rs`, so everything before it is
/// frame-layout independent.
pub type SlotId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShiftKind {
    Lsl,
    Lsr,
    Asr,
    Ror,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExtKind {
    Uxtb,
    Uxth,
    Uxtw,
    Uxtx,
    Sxtb,
    Sxth,
    Sxtw,
    Sxtx,
}

/// The second source of an A64 data-processing instruction: a register, that
/// register shifted, that register extended, or an immediate. `isa.rs` owns
/// which immediates are actually encodable.
#[derive(Clone, Copy, Debug)]
pub enum Rhs {
    Reg(Reg),
    Shifted(Reg, ShiftKind, u8),
    Extended(Reg, ExtKind, u8),
    /// `imm12` (optionally `<< 12`) for add/sub, or a logical bitmask immediate
    Imm(i64),
}

#[derive(Clone, Debug)]
pub enum AddrMode {
    /// `[base, #off]` — scaled-unsigned or signed-9 depending on the width
    BaseImm { base: Reg, off: i32 },
    /// `[base, idx, ext #shift]`
    BaseReg {
        base: Reg,
        idx: Reg,
        ext: Option<ExtKind>,
        shift: u8,
    },
    /// `[base, #off]!` — DEFINES a new base register (SSA), hence `wb`
    PreIdx { base: Reg, wb: Reg, off: i32 },
    /// `[base], #off` — likewise
    PostIdx { base: Reg, wb: Reg, off: i32 },
    /// a stack object; `frame` rewrites this into `BaseImm` on sp or x29
    Slot { slot: SlotId, off: i32 },
    /// `[sym + :lo12:]` after an `adrp` into `base`
    SymLo12 { base: Reg, sym: Sym },
}

/// AArch64 condition codes (ARM DDI 0487 C1.2.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CC {
    Eq,
    Ne,
    Hs,
    Lo,
    Mi,
    Pl,
    Vs,
    Vc,
    Hi,
    Ls,
    Ge,
    Lt,
    Gt,
    Le,
}

impl CC {
    pub fn invert(self) -> CC {
        use CC::*;
        match self {
            Eq => Ne,
            Ne => Eq,
            Hs => Lo,
            Lo => Hs,
            Mi => Pl,
            Pl => Mi,
            Vs => Vc,
            Vc => Vs,
            Hi => Ls,
            Ls => Hi,
            Ge => Lt,
            Lt => Ge,
            Gt => Le,
            Le => Gt,
        }
    }
}

// ── opcodes ────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AluOp {
    Add,
    Sub,
    And,
    Orr,
    Eor,
    Bic,
    Orn,
    Eon,
    Lsl,
    Lsr,
    Asr,
    Mul,
    SDiv,
    UDiv,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alu3Op {
    Madd,
    Msub,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExtOp {
    Sxtb,
    Sxth,
    Sxtw,
    Uxtb,
    Uxth,
}

/// Load/store access width and extension — `ldrb/ldrsb/ldrh/ldrsh/ldrsw/ldr`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemOp {
    B,
    SB,
    H,
    SH,
    W,
    SW,
    X,
    /// FP forms: `ldr s` / `ldr d`
    S,
    D,
}

impl MemOp {
    pub fn bytes(self) -> u32 {
        match self {
            MemOp::B | MemOp::SB => 1,
            MemOp::H | MemOp::SH => 2,
            MemOp::W | MemOp::SW | MemOp::S => 4,
            MemOp::X | MemOp::D => 8,
        }
    }
    pub fn class(self) -> Class {
        match self {
            MemOp::S | MemOp::D => Class::Fpr,
            _ => Class::Gpr,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FpOp {
    Fadd,
    Fsub,
    Fmul,
    Fdiv,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FpUnOp {
    Fneg,
    Fabs,
    Fsqrt,
    /// `fcvt` between s and d — the destination width says which direction
    Fcvt,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CvtOp {
    Scvtf,
    Ucvtf,
    Fcvtzs,
    Fcvtzu,
}

// ── the instruction ────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub enum MInst {
    /// `op dst, a, rhs` (+ the S form defining NZCV in `flags`)
    Alu {
        op: AluOp,
        w: Width,
        dst: Reg,
        a: Reg,
        b: Rhs,
        flags: Option<Reg>,
    },
    /// `madd/msub dst, a, b, c`
    Alu3 {
        op: Alu3Op,
        w: Width,
        dst: Reg,
        a: Reg,
        b: Reg,
        c: Reg,
    },
    /// `cmp/cmn/tst` — defines NZCV only
    Cmp {
        kind: CmpKind,
        w: Width,
        a: Reg,
        b: Rhs,
        flags: Reg,
    },
    /// Materialize an integer constant. ONE instruction here, expanded by the
    /// emitter into the `movz/movn/movk` chain `isa::mov_chain` computes — the
    /// chain is a read-modify-write of one register and so is not expressible in
    /// SSA. Its cost is not hidden: `isa::mov_chain(imm).len()` gives the exact
    /// instruction count before anything is emitted, which is what the cost
    /// square (REARCH §10) needs.
    MovImm {
        w: Width,
        dst: Reg,
        imm: i64,
    },
    /// `sxtb/sxth/sxtw/uxtb/uxth`; `w` is the DESTINATION width, which chooses
    /// the `w` or `x` form (`sxtb x0, w0` differs from `sxtb w0, w0`).
    Ext {
        op: ExtOp,
        w: Width,
        dst: Reg,
        src: Reg,
    },
    Load {
        op: MemOp,
        dst: Reg,
        mem: AddrMode,
        vol: bool,
    },
    Store {
        op: MemOp,
        src: Reg,
        mem: AddrMode,
        vol: bool,
    },
    /// `adrp dst, sym` — the page address; pairs with `AddrMode::SymLo12`
    Adrp {
        dst: Reg,
        sym: Sym,
        /// `:got:` form for a preemptible symbol under -fPIC
        got: bool,
    },
    /// `add dst, base, :lo12:sym`
    AddLo12 {
        dst: Reg,
        base: Reg,
        sym: Sym,
        got: bool,
    },
    /// `csel/csinc/csinv/csneg dst, a, b, cc`
    CSel {
        op: CSelOp,
        w: Width,
        dst: Reg,
        a: Reg,
        b: Reg,
        cc: CC,
        flags: Reg,
    },
    /// `cset dst, cc`
    CSet {
        w: Width,
        dst: Reg,
        cc: CC,
        flags: Reg,
    },
    FpAlu {
        op: FpOp,
        w: Width,
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    FpUn {
        op: FpUnOp,
        w: Width,
        dst: Reg,
        src: Reg,
        /// source width, which differs from `w` for `fcvt`
        sw: Width,
    },
    FpCmp {
        w: Width,
        a: Reg,
        b: Reg,
        /// `fcmp a, #0.0` — the only encodable FP immediate in a compare
        zero: bool,
        flags: Reg,
    },
    /// integer ↔ floating conversion
    FpCvt {
        op: CvtOp,
        dw: Width,
        sw: Width,
        dst: Reg,
        src: Reg,
    },
    /// `fmov` — reg-to-reg in either direction, including GPR↔FPR bit moves
    FMov {
        dw: Width,
        sw: Width,
        dst: Reg,
        src: Reg,
    },
    /// The call pseudo. `uses`/`defs` carry the AAPCS64 fixed constraints, so the
    /// allocator needs no notion of "argument register"; `clobbers` makes a value
    /// live across the call simply un-colorable in a caller-saved register — the
    /// "crossing a call" rule falls out of ordinary constraint satisfaction.
    Call {
        callee: CallTarget,
        uses: Vec<(Reg, PReg)>,
        defs: Vec<(Reg, PReg)>,
        clobbers: RegSet,
        /// bytes of outgoing stack arguments (AAPCS64 NSAA)
        stack_bytes: u32,
        /// sibling call: `b` after the epilogue instead of `bl` (REARCH §16 ★3)
        tail: bool,
    },
    /// Register-to-register move, the coalescing candidate.
    Copy {
        w: Width,
        dst: Reg,
        src: Reg,
    },
    /// Simultaneous assignment; `destruct` sequentializes it with the reserved
    /// scratch register breaking cycles.
    ParallelCopy(Vec<(Reg, Reg, Width)>),
    Spill {
        slot: SlotId,
        src: Reg,
        w: Width,
    },
    Reload {
        slot: SlotId,
        dst: Reg,
        w: Width,
    },
    /// address of a stack object (`alloca`, an aggregate local)
    SlotAddr {
        dst: Reg,
        slot: SlotId,
        off: i32,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmpKind {
    Cmp,
    Cmn,
    Tst,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MovKind {
    Z,
    N,
    K,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CSelOp {
    Csel,
    Csinc,
    Csinv,
    Csneg,
}

#[derive(Clone, Debug)]
pub enum CallTarget {
    Direct(String),
    Indirect(Reg),
}

// ── terminators ────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct MTarget {
    pub block: MBlockId,
    pub args: Vec<Reg>,
}

#[derive(Clone, Debug)]
pub enum MTerm {
    B(MTarget),
    /// `b.cc` on a flag value — the flags register is an ordinary use
    Bcc(CC, Reg, MTarget, MTarget),
    /// `cbz/cbnz` — no compare instruction at all
    Cbz {
        w: Width,
        reg: Reg,
        zero: bool,
        t: MTarget,
        f: MTarget,
    },
    /// `tbz/tbnz` — single-bit test
    Tb {
        w: Width,
        reg: Reg,
        bit: u8,
        set: bool,
        t: MTarget,
        f: MTarget,
    },
    /// A dense switch; `layout` lowers it to `adr`+`ldr`+`br` with a table in
    /// `.rodata`, or `isel` never builds it and emits a compare tree instead.
    Switch {
        idx: Reg,
        table: Vec<MTarget>,
        default: MTarget,
    },
    Ret,
    /// EXT(gcc) computed goto
    BrReg(Reg, Vec<MBlockId>),
    Unreachable,
}

impl MTerm {
    pub fn targets(&self) -> Vec<&MTarget> {
        match self {
            MTerm::B(t) => vec![t],
            MTerm::Bcc(_, _, a, b) => vec![a, b],
            MTerm::Cbz { t, f, .. } | MTerm::Tb { t, f, .. } => vec![t, f],
            MTerm::Switch { table, default, .. } => {
                let mut v: Vec<&MTarget> = table.iter().collect();
                v.push(default);
                v
            }
            MTerm::Ret | MTerm::BrReg(..) | MTerm::Unreachable => vec![],
        }
    }
    pub fn targets_mut(&mut self) -> Vec<&mut MTarget> {
        match self {
            MTerm::B(t) => vec![t],
            MTerm::Bcc(_, _, a, b) => vec![a, b],
            MTerm::Cbz { t, f, .. } | MTerm::Tb { t, f, .. } => vec![t, f],
            MTerm::Switch { table, default, .. } => {
                let mut v: Vec<&mut MTarget> = table.iter_mut().collect();
                v.push(default);
                v
            }
            MTerm::Ret | MTerm::BrReg(..) | MTerm::Unreachable => vec![],
        }
    }
    pub fn succs(&self) -> Vec<MBlockId> {
        match self {
            MTerm::BrReg(_, bs) => bs.clone(),
            _ => self.targets().iter().map(|t| t.block).collect(),
        }
    }
}

// ── the operand visitor: the ONLY way anything reads an instruction's registers ─
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Constraint {
    Use,
    Def,
    UseFixed(PReg),
    DefFixed(PReg),
}

/// What an instruction does to memory — the dependence oracle every memory pass
/// (and, at O2, a list scheduler) consults.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemEffect {
    None,
    Read,
    Write,
    /// a call, a volatile access, an atomic: ordered against everything
    Barrier,
}

macro_rules! visit_rhs {
    ($b:expr, $f:expr) => {
        match $b {
            Rhs::Reg(r) | Rhs::Shifted(r, ..) | Rhs::Extended(r, ..) => $f(r, Constraint::Use),
            Rhs::Imm(_) => {}
        }
    };
}

macro_rules! visit_addr {
    ($m:expr, $f:expr) => {
        match $m {
            AddrMode::BaseImm { base, .. } => $f(base, Constraint::Use),
            AddrMode::BaseReg { base, idx, .. } => {
                $f(base, Constraint::Use);
                $f(idx, Constraint::Use);
            }
            AddrMode::PreIdx { base, wb, .. } | AddrMode::PostIdx { base, wb, .. } => {
                $f(base, Constraint::Use);
                $f(wb, Constraint::Def);
            }
            AddrMode::Slot { .. } => {}
            AddrMode::SymLo12 { base, .. } => $f(base, Constraint::Use),
        }
    };
}

impl MInst {
    pub fn effect(&self) -> MemEffect {
        match self {
            MInst::Load { vol: true, .. } | MInst::Store { vol: true, .. } => MemEffect::Barrier,
            MInst::Load { .. } | MInst::Reload { .. } => MemEffect::Read,
            MInst::Store { .. } | MInst::Spill { .. } => MemEffect::Write,
            MInst::Call { .. } => MemEffect::Barrier,
            _ => MemEffect::None,
        }
    }

    pub fn visit(&self, f: &mut impl FnMut(Reg, Constraint)) {
        let mut g = |r: &Reg, c: Constraint| f(*r, c);
        match self {
            MInst::Alu { dst, a, b, flags, .. } => {
                g(a, Constraint::Use);
                visit_rhs!(b, g);
                g(dst, Constraint::Def);
                if let Some(fl) = flags {
                    g(fl, Constraint::Def);
                }
            }
            MInst::Alu3 { dst, a, b, c, .. } => {
                g(a, Constraint::Use);
                g(b, Constraint::Use);
                g(c, Constraint::Use);
                g(dst, Constraint::Def);
            }
            MInst::Cmp { a, b, flags, .. } => {
                g(a, Constraint::Use);
                visit_rhs!(b, g);
                g(flags, Constraint::Def);
            }
            MInst::MovImm { dst, .. } => g(dst, Constraint::Def),
            MInst::Ext { dst, src, .. } | MInst::Copy { dst, src, .. } => {
                g(src, Constraint::Use);
                g(dst, Constraint::Def);
            }
            MInst::Load { dst, mem, .. } => {
                visit_addr!(mem, g);
                g(dst, Constraint::Def);
            }
            MInst::Store { src, mem, .. } => {
                g(src, Constraint::Use);
                visit_addr!(mem, g);
            }
            MInst::Adrp { dst, .. } => g(dst, Constraint::Def),
            MInst::AddLo12 { dst, base, .. } => {
                g(base, Constraint::Use);
                g(dst, Constraint::Def);
            }
            MInst::CSel { dst, a, b, flags, .. } => {
                g(a, Constraint::Use);
                g(b, Constraint::Use);
                g(flags, Constraint::Use);
                g(dst, Constraint::Def);
            }
            MInst::CSet { dst, flags, .. } => {
                g(flags, Constraint::Use);
                g(dst, Constraint::Def);
            }
            MInst::FpAlu { dst, a, b, .. } => {
                g(a, Constraint::Use);
                g(b, Constraint::Use);
                g(dst, Constraint::Def);
            }
            MInst::FpUn { dst, src, .. }
            | MInst::FpCvt { dst, src, .. }
            | MInst::FMov { dst, src, .. } => {
                g(src, Constraint::Use);
                g(dst, Constraint::Def);
            }
            MInst::FpCmp { a, b, zero, flags, .. } => {
                g(a, Constraint::Use);
                if !*zero {
                    g(b, Constraint::Use);
                }
                g(flags, Constraint::Def);
            }
            MInst::Call {
                callee,
                uses,
                defs,
                ..
            } => {
                if let CallTarget::Indirect(r) = callee {
                    g(r, Constraint::Use);
                }
                for (r, p) in uses {
                    g(r, Constraint::UseFixed(*p));
                }
                for (r, p) in defs {
                    g(r, Constraint::DefFixed(*p));
                }
            }
            MInst::ParallelCopy(pairs) => {
                for (_, s, _) in pairs.iter() {
                    g(s, Constraint::Use);
                }
                for (d, _, _) in pairs.iter() {
                    g(d, Constraint::Def);
                }
            }
            MInst::Spill { src, .. } => g(src, Constraint::Use),
            MInst::Reload { dst, .. } => g(dst, Constraint::Def),
            MInst::SlotAddr { dst, .. } => g(dst, Constraint::Def),
        }
    }

    pub fn visit_mut(&mut self, f: &mut impl FnMut(&mut Reg, Constraint)) {
        match self {
            MInst::Alu { dst, a, b, flags, .. } => {
                f(a, Constraint::Use);
                visit_rhs!(b, f);
                f(dst, Constraint::Def);
                if let Some(fl) = flags {
                    f(fl, Constraint::Def);
                }
            }
            MInst::Alu3 { dst, a, b, c, .. } => {
                f(a, Constraint::Use);
                f(b, Constraint::Use);
                f(c, Constraint::Use);
                f(dst, Constraint::Def);
            }
            MInst::Cmp { a, b, flags, .. } => {
                f(a, Constraint::Use);
                visit_rhs!(b, f);
                f(flags, Constraint::Def);
            }
            MInst::MovImm { dst, .. } => f(dst, Constraint::Def),
            MInst::Ext { dst, src, .. } | MInst::Copy { dst, src, .. } => {
                f(src, Constraint::Use);
                f(dst, Constraint::Def);
            }
            MInst::Load { dst, mem, .. } => {
                visit_addr!(mem, f);
                f(dst, Constraint::Def);
            }
            MInst::Store { src, mem, .. } => {
                f(src, Constraint::Use);
                visit_addr!(mem, f);
            }
            MInst::Adrp { dst, .. } => f(dst, Constraint::Def),
            MInst::AddLo12 { dst, base, .. } => {
                f(base, Constraint::Use);
                f(dst, Constraint::Def);
            }
            MInst::CSel { dst, a, b, flags, .. } => {
                f(a, Constraint::Use);
                f(b, Constraint::Use);
                f(flags, Constraint::Use);
                f(dst, Constraint::Def);
            }
            MInst::CSet { dst, flags, .. } => {
                f(flags, Constraint::Use);
                f(dst, Constraint::Def);
            }
            MInst::FpAlu { dst, a, b, .. } => {
                f(a, Constraint::Use);
                f(b, Constraint::Use);
                f(dst, Constraint::Def);
            }
            MInst::FpUn { dst, src, .. }
            | MInst::FpCvt { dst, src, .. }
            | MInst::FMov { dst, src, .. } => {
                f(src, Constraint::Use);
                f(dst, Constraint::Def);
            }
            MInst::FpCmp { a, b, zero, flags, .. } => {
                f(a, Constraint::Use);
                if !*zero {
                    f(b, Constraint::Use);
                }
                f(flags, Constraint::Def);
            }
            MInst::Call {
                callee,
                uses,
                defs,
                ..
            } => {
                if let CallTarget::Indirect(r) = callee {
                    f(r, Constraint::Use);
                }
                for (r, p) in uses.iter_mut() {
                    f(r, Constraint::UseFixed(*p));
                }
                for (r, p) in defs.iter_mut() {
                    f(r, Constraint::DefFixed(*p));
                }
            }
            MInst::ParallelCopy(pairs) => {
                for (_, s, _) in pairs.iter_mut() {
                    f(s, Constraint::Use);
                }
                for (d, _, _) in pairs.iter_mut() {
                    f(d, Constraint::Def);
                }
            }
            MInst::Spill { src, .. } => f(src, Constraint::Use),
            MInst::Reload { dst, .. } => f(dst, Constraint::Def),
            MInst::SlotAddr { dst, .. } => f(dst, Constraint::Def),
        }
    }

    /// The single defined register, when there is exactly one (the SSA case).
    pub fn def_reg(&self) -> Option<Reg> {
        let mut d = None;
        let mut n = 0;
        self.visit(&mut |r, c| {
            if matches!(c, Constraint::Def | Constraint::DefFixed(_)) {
                n += 1;
                d = Some(r);
            }
        });
        if n == 1 { d } else { None }
    }
}

impl MTerm {
    pub fn visit(&self, f: &mut impl FnMut(Reg, Constraint)) {
        match self {
            MTerm::Bcc(_, fl, ..) => f(*fl, Constraint::Use),
            MTerm::Cbz { reg, .. } | MTerm::Tb { reg, .. } => f(*reg, Constraint::Use),
            MTerm::Switch { idx, .. } => f(*idx, Constraint::Use),
            MTerm::BrReg(r, _) => f(*r, Constraint::Use),
            MTerm::B(_) | MTerm::Ret | MTerm::Unreachable => {}
        }
        for t in self.targets() {
            for a in &t.args {
                f(*a, Constraint::Use);
            }
        }
    }
    pub fn visit_mut(&mut self, f: &mut impl FnMut(&mut Reg, Constraint)) {
        match self {
            MTerm::Bcc(_, fl, ..) => f(fl, Constraint::Use),
            MTerm::Cbz { reg, .. } | MTerm::Tb { reg, .. } => f(reg, Constraint::Use),
            MTerm::Switch { idx, .. } => f(idx, Constraint::Use),
            MTerm::BrReg(r, _) => f(r, Constraint::Use),
            MTerm::B(_) | MTerm::Ret | MTerm::Unreachable => {}
        }
        for t in self.targets_mut() {
            for a in t.args.iter_mut() {
                f(a, Constraint::Use);
            }
        }
    }
}

// ── containers ─────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug)]
pub struct StackSlot {
    pub size: u32,
    pub align: u32,
    pub kind: SlotKind,
    /// byte offset from the frame base, assigned by `pass/frame.rs`
    pub off: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotKind {
    /// a C local / the R0 frame block / an `alloca`
    Local,
    /// a register spilled by the allocator
    Spill,
    /// the outgoing stack-argument area (AAPCS64 NSAA)
    OutArgs,
}

#[derive(Clone, Debug)]
pub struct MBlock {
    pub params: Vec<Reg>,
    pub insts: Vec<MInst>,
    pub term: MTerm,
    pub weight: u32,
}

#[derive(Clone, Debug)]
pub struct MFunc {
    pub name: String,
    pub blocks: Vec<MBlock>,
    pub vregs: Vec<VRegInfo>,
    pub slots: Vec<StackSlot>,
    pub entry: MBlockId,
    pub is_static: bool,
    pub is_weak: bool,
    /// Emission order, set by `pass/layout.rs`; unreachable blocks are absent.
    /// Empty until layout has run.
    pub order: Vec<MBlockId>,
    /// Set by `pass/frame.rs` once slot offsets exist. It is NOT inferable from
    /// `frame_size`: a function that needs no stack at all has a laid-out frame
    /// of size zero, and conflating the two costs every leaf function a `sub sp`
    /// / `add sp` pair it does not need.
    pub laid_out: bool,
    /// filled by `frame`: total frame size and the callee-saved registers used
    pub frame_size: u32,
    pub saved: RegSet,
    /// true once `regalloc` has run — the verifier switches obligations on it
    pub physical: bool,
}

impl MFunc {
    pub fn new_vreg(&mut self, width: Width) -> Reg {
        self.vregs.push(VRegInfo {
            class: width.class(),
            width,
        });
        Reg::V((self.vregs.len() - 1) as VReg)
    }
    pub fn new_flags(&mut self) -> Reg {
        self.vregs.push(VRegInfo {
            class: Class::Flags,
            width: Width::W32,
        });
        Reg::V((self.vregs.len() - 1) as VReg)
    }
    pub fn new_block(&mut self) -> MBlockId {
        self.blocks.push(MBlock {
            params: Vec::new(),
            insts: Vec::new(),
            term: MTerm::Unreachable,
            weight: 1,
        });
        (self.blocks.len() - 1) as MBlockId
    }
    pub fn new_slot(&mut self, size: u32, align: u32, kind: SlotKind) -> SlotId {
        self.slots.push(StackSlot {
            size,
            align,
            kind,
            off: 0,
        });
        (self.slots.len() - 1) as SlotId
    }
    pub fn class_of(&self, r: Reg) -> Class {
        match r {
            Reg::V(v) => self.vregs[v as usize].class,
            Reg::P(p) => p.class,
        }
    }
}

#[derive(Debug)]
pub struct MModule {
    pub funcs: Vec<MFunc>,
}
