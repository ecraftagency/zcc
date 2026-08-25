// HIR — the target-independent SSA layer (REARCH.md §3).
// THEORY A6 — HIR, the target-independent SSA layer
//
// Three design decisions carry the whole layer, and every file below depends on
// them (REARCH §3.1 / §14):
//   1. SSA from birth. `build.rs` lowers the AST straight into SSA (Braun et al.
//      2013, on-the-fly, no dominance frontiers). There is NO out-of-SSA in HIR.
//   2. Block parameters instead of φ instructions. `jmp bb(a, b)` makes the edge
//      transfer explicit; SSA destruction (in MIR) is then one parallel copy per
//      edge, and the interpreter needs no φ-select rule.
//   3. A closed scalar `Ty`. Signedness and width live in the OPCODE (`sdiv` vs
//      `udiv`, `icmp.slt` vs `icmp.ult`, `sext` vs `zext`), never in a lookup
//      into the frontend's TyTab. After lowering, HIR is independent of TyTab —
//      which is what makes `⟦·⟧` (SEMANTICS.md §3) a closed definition.
pub mod build;
pub mod dom;
pub mod interp;
pub mod pass;
#[cfg(test)]
mod tests;
pub mod verify;

pub type BlockId = u32;
pub type ValueId = u32;

// ── types ──────────────────────────────────────────────────────────────────
// Closed scalar domain. Pointers are I64 (LP64, Article B). Aggregates never
// appear as a value: they live in memory and travel by address (`memcpy`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl Ty {
    pub fn bytes(self) -> u32 {
        match self {
            Ty::I8 => 1,
            Ty::I16 => 2,
            Ty::I32 | Ty::F32 => 4,
            Ty::I64 | Ty::F64 => 8,
        }
    }
    pub fn bits(self) -> u32 {
        self.bytes() * 8
    }
    pub fn is_float(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }
}

// ── operands ───────────────────────────────────────────────────────────────
// A constant is an operand, not an instruction: it has no definition point, so
// it never interferes, never needs a value number, and isel can fold it into an
// immediate field without first proving single-use. The operand's TYPE is the
// enclosing instruction's `ty` field — an operand is never self-describing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operand {
    Val(ValueId),
    /// Integer constant, already truncated to the instruction's type.
    Imm(i64),
    /// Floating constant, as the IEEE-754 BIT PATTERN of the instruction's type.
    Fimm(u64),
}

impl Operand {
    pub fn val(self) -> Option<ValueId> {
        match self {
            Operand::Val(v) => Some(v),
            _ => None,
        }
    }
}

/// A linker-visible address. `Global`/`Str` index into the `Ast`; `Func`/`Label`
/// carry the emitted symbol name (a call may name a function never declared as a
/// global).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Sym {
    Global(u32),
    /// EXT(gcc) `__thread`: the same global, reached through the thread pointer
    /// instead of the page/lo12 pair (THEORY II-4, local-exec model).
    Tls(u32),
    Str(u32),
    Func(String),
    /// EXT(gcc) `&&label` — the address of a block, for computed goto.
    Label(BlockId),
}

// ── opcodes ────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    SDiv,
    UDiv,
    SRem,
    URem,
    And,
    Or,
    Xor,
    Shl,
    LShr,
    AShr,
    FAdd,
    FSub,
    FMul,
    FDiv,
    /// The high half of a full 64×64 product — the only witness to a 64-bit
    /// multiplication overflow (EXT(gcc) `__builtin_mul_overflow`).
    SMulHi,
    UMulHi,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,  // integer two's-complement negation
    Not,  // bitwise complement
    FNeg, // IEEE sign flip (NOT 0-x: it is defined on NaN and -0.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmpOp {
    Eq,
    Ne,
    Slt,
    Sle,
    Sgt,
    Sge,
    Ult,
    Ule,
    Ugt,
    Uge,
    // ordered float compares (false if either operand is NaN)
    FOeq,
    FOne,
    FOlt,
    FOle,
    FOgt,
    FOge,
    /// C99 6.5.9: `a != b` is `!(a == b)`, which is TRUE when either operand is
    /// NaN — an UNORDERED not-equal, not the ordered one. Keeping both spellings
    /// makes the difference impossible to lose in isel.
    FUne,
    /// unordered: true iff either operand is NaN
    FUno,
}

impl CmpOp {
    pub fn is_float(self) -> bool {
        matches!(
            self,
            CmpOp::FOeq
                | CmpOp::FOne
                | CmpOp::FOlt
                | CmpOp::FOle
                | CmpOp::FOgt
                | CmpOp::FOge
                | CmpOp::FUne
                | CmpOp::FUno
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CvtOp {
    Sext,
    Zext,
    Trunc,
    FpToSi,
    FpToUi,
    SiToFp,
    UiToFp,
    FpExt,
    FpTrunc,
    /// reinterpret the bits (I64↔F64, I32↔F32) — the union/`memcpy` idiom
    Bitcast,
}

/// C99 6.5p7 effective-type alias class. 0 = "may alias anything" (the only class
/// R0/R1 produce). The field is carried from day one because retrofitting an
/// alias tag through every load/store later is expensive; TBAA (REARCH §16 ★1)
/// is the pass that will finally read it.
pub type AClass = u32;
/// THEORY A6 — HIR's alias-class sentinel
pub const ACLASS_ANY: AClass = 0;

// ── instructions ───────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub enum Inst {
    Bin {
        dst: ValueId,
        op: BinOp,
        ty: Ty,
        a: Operand,
        b: Operand,
    },
    Un {
        dst: ValueId,
        op: UnOp,
        ty: Ty,
        a: Operand,
    },
    /// `dst : I32` ∈ {0, 1}; `ty` is the type of the COMPARED operands.
    Cmp {
        dst: ValueId,
        op: CmpOp,
        ty: Ty,
        a: Operand,
        b: Operand,
    },
    Cvt {
        dst: ValueId,
        op: CvtOp,
        from: Ty,
        to: Ty,
        a: Operand,
    },
    Load {
        dst: ValueId,
        ty: Ty,
        addr: Operand,
        aclass: AClass,
        /// C99 6.7.3: a volatile access may not be removed, duplicated or reordered.
        vol: bool,
    },
    Store {
        ty: Ty,
        addr: Operand,
        val: Operand,
        aclass: AClass,
        vol: bool,
    },
    /// Address of a static stack object (`Func.slots`), plus a constant offset.
    SlotAddr {
        dst: ValueId,
        slot: u32,
        off: i64,
    },
    /// Address of a linker symbol.
    SymAddr {
        dst: ValueId,
        sym: Sym,
    },
    /// C99 6.5.15 conditional, once both arms are known side-effect-free.
    Select {
        dst: ValueId,
        ty: Ty,
        c: Operand,
        a: Operand,
        b: Operand,
    },
    Call {
        dst: Option<ValueId>,
        sig: Sig,
        callee: Callee,
        args: Vec<Operand>,
        /// Where a COMPOSITE result is deposited. An aggregate is never an HIR
        /// value (it has no scalar `Ty`), so a struct-returning call names the
        /// destination address instead of defining a value. isel then realizes
        /// AAPCS64 §6.9 either way: ≤16 bytes come back in registers and are
        /// stored here, larger results are written by the callee through the x8
        /// this address supplies.
        sret: Option<Operand>,
    },
    /// Dynamic stack allocation (C99 VLA / EXT alloca). `dst : I64` is the base.
    Alloca {
        dst: ValueId,
        size: Operand,
        align: u32,
    },
    MemCpy {
        dst: Operand,
        src: Operand,
        len: u64,
    },
    MemSet {
        dst: Operand,
        byte: Operand,
        len: u64,
    },
    /// The EXT/builtin surface: opaque to every pass (`Effect::Call`), expanded by
    /// isel. Kept as ONE instruction so no pass needs a builtin list.
    Intrinsic {
        dst: Option<ValueId>,
        kind: IntrinKind,
        args: Vec<Operand>,
    },
}

#[derive(Clone, Debug)]
pub enum Callee {
    Direct(String),
    Indirect(Operand),
}

#[derive(Clone, Debug)]
pub enum IntrinKind {
    /// `__builtin_va_start(ap, last)`: fill the five AAPCS64 `va_list` fields.
    /// Expanded by isel, because the two register counters it records are the
    /// ABI's, and the ABI lives in one place (`isel/abi.rs`).
    VaStart,
    VaArg(Ty),
    /// EXT(gcc) `__va_area__`: the address of the first UNNAMED stack argument.
    /// `args[0]` is its byte offset into the caller's argument area.
    VaArea,
    /// EXT(gcc) `__sync_*` (ARM DDI 0487 B2.9), as the three primitives the
    /// retry loop is built from. The loop itself is ordinary HIR control flow —
    /// see `build::sync`.
    LdAxr(Ty),
    /// `args = [addr, value]`; `dst : I32` is 0 when the store succeeded
    StlXr(Ty),
    /// `args = [addr, value]` — a release store
    Stlr(Ty),
    /// `dmb ish`
    Dmb,
    /// EXT(gcc) `__builtin_{add,sub,mul}_overflow`: op 0=+ 1=- 2=*.
    Overflow {
        op: u8,
        ty: Ty,
        signed: bool,
    },
    /// C99 long double on ELF: MEMORY is binary128, the value in a register is
    /// canonical f64 (THEORY II-2 — `float.h` declares `LDBL_MANT_DIG` 53, so
    /// the model is self-consistent), and the two are bridged at every
    /// load/store/ABI boundary by the libgcc soft-float pair. `LdLoad(addr) →
    /// F64` is `__trunctfdf2`; `LdStore(addr, F64)` is `__extenddftf2` followed
    /// by the 16-byte store. isel owns the expansion because the quad only
    /// exists in a machine register.
    LdLoad,
    LdStore,
    /// EXT(gcc) inline asm. The template is opaque; the operands are not. The
    /// argument list runs in operand order: a `"m"` operand contributes its
    /// ADDRESS, an output its destination address, a `"+"` output the address
    /// AND its current value, an input its value.
    Asm {
        tmpl: String,
        ops: Vec<AsmOperand>,
    },
}

/// One inline-asm operand as HIR sees it: the constraint bits from the AST plus
/// the scalar type, which is what decides the register class and the store width.
#[derive(Clone, Copy, Debug)]
pub struct AsmOperand {
    pub out: bool,
    pub rw: bool,
    pub mem: bool,
    pub fp: bool,
    pub tied: Option<u8>,
    pub pin: Option<u8>,
    pub ty: Ty,
}

/// Effect class — the single table DCE / CSE / GVN / LICM / sinking consult
/// (REARCH §3.1). No pass carries a hand-written opcode list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    Pure,
    Read,
    Write,
    Call,
}

impl Inst {
    pub fn effect(&self) -> Effect {
        match self {
            Inst::Bin { .. }
            | Inst::Un { .. }
            | Inst::Cmp { .. }
            | Inst::Cvt { .. }
            | Inst::SlotAddr { .. }
            | Inst::SymAddr { .. }
            | Inst::Select { .. } => Effect::Pure,
            Inst::Load { vol, .. } => {
                if *vol {
                    Effect::Call // a volatile read is as opaque as a call
                } else {
                    Effect::Read
                }
            }
            Inst::Store { vol, .. } => {
                if *vol {
                    Effect::Call
                } else {
                    Effect::Write
                }
            }
            Inst::MemCpy { .. } | Inst::MemSet { .. } => Effect::Write,
            // An alloca moves the stack pointer: never duplicated, never sunk.
            Inst::Alloca { .. } | Inst::Call { .. } | Inst::Intrinsic { .. } => Effect::Call,
        }
    }

    pub fn dst(&self) -> Option<ValueId> {
        match *self {
            Inst::Bin { dst, .. }
            | Inst::Un { dst, .. }
            | Inst::Cmp { dst, .. }
            | Inst::Cvt { dst, .. }
            | Inst::Load { dst, .. }
            | Inst::SlotAddr { dst, .. }
            | Inst::SymAddr { dst, .. }
            | Inst::Select { dst, .. }
            | Inst::Alloca { dst, .. } => Some(dst),
            Inst::Call { dst, .. } | Inst::Intrinsic { dst, .. } => dst,
            Inst::Store { .. } | Inst::MemCpy { .. } | Inst::MemSet { .. } => None,
        }
    }

    /// Every operand read by this instruction, in evaluation order. The single
    /// visitor liveness, DCE, verification and rewriting go through.
    pub fn uses(&self, mut f: impl FnMut(Operand)) {
        match self {
            Inst::Bin { a, b, .. } | Inst::Cmp { a, b, .. } => {
                f(*a);
                f(*b);
            }
            Inst::Un { a, .. } | Inst::Cvt { a, .. } => f(*a),
            Inst::Load { addr, .. } => f(*addr),
            Inst::Store { addr, val, .. } => {
                f(*addr);
                f(*val);
            }
            Inst::SlotAddr { .. } | Inst::SymAddr { .. } => {}
            Inst::Select { c, a, b, .. } => {
                f(*c);
                f(*a);
                f(*b);
            }
            Inst::Call { callee, args, sret, .. } => {
                if let Callee::Indirect(o) = callee {
                    f(*o);
                }
                args.iter().for_each(|a| f(*a));
                if let Some(o) = sret {
                    f(*o);
                }
            }
            Inst::Alloca { size, .. } => f(*size),
            Inst::MemCpy { dst, src, .. } => {
                f(*dst);
                f(*src);
            }
            Inst::MemSet { dst, byte, .. } => {
                f(*dst);
                f(*byte);
            }
            Inst::Intrinsic { args, .. } => args.iter().for_each(|a| f(*a)),
        }
    }

    pub fn uses_mut(&mut self, mut f: impl FnMut(&mut Operand)) {
        match self {
            Inst::Bin { a, b, .. } | Inst::Cmp { a, b, .. } => {
                f(a);
                f(b);
            }
            Inst::Un { a, .. } | Inst::Cvt { a, .. } => f(a),
            Inst::Load { addr, .. } => f(addr),
            Inst::Store { addr, val, .. } => {
                f(addr);
                f(val);
            }
            Inst::SlotAddr { .. } | Inst::SymAddr { .. } => {}
            Inst::Select { c, a, b, .. } => {
                f(c);
                f(a);
                f(b);
            }
            Inst::Call { callee, args, sret, .. } => {
                if let Callee::Indirect(o) = callee {
                    f(o);
                }
                args.iter_mut().for_each(&mut f);
                if let Some(o) = sret {
                    f(o);
                }
            }
            Inst::Alloca { size, .. } => f(size),
            Inst::MemCpy { dst, src, .. } => {
                f(dst);
                f(src);
            }
            Inst::MemSet { dst, byte, .. } => {
                f(dst);
                f(byte);
            }
            Inst::Intrinsic { args, .. } => args.iter_mut().for_each(f),
        }
    }
}

// ── terminators ────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct Target {
    pub block: BlockId,
    pub args: Vec<Operand>,
}

#[derive(Clone, Debug)]
pub enum Term {
    Jmp(Target),
    /// `br %c, then, else` — `%c` is I32, tested ≠ 0.
    Br(Operand, Target, Target),
    Switch(Operand, Ty, Vec<(i64, Target)>, Target),
    Ret(Option<Operand>),
    /// C99 6.9.1p12 falling off `main`, or an unreachable tail after `noreturn`.
    Unreachable,
    /// EXT(gcc) `goto *e`; the list is every block whose address is taken (the
    /// CFG edge set — without it the successors are unknown and dominance is
    /// meaningless).
    GotoPtr(Operand, Vec<BlockId>),
}

impl Term {
    pub fn targets(&self) -> Vec<&Target> {
        match self {
            Term::Jmp(t) => vec![t],
            Term::Br(_, a, b) => vec![a, b],
            Term::Switch(_, _, cases, d) => {
                let mut v: Vec<&Target> = cases.iter().map(|(_, t)| t).collect();
                v.push(d);
                v
            }
            Term::Ret(_) | Term::Unreachable | Term::GotoPtr(..) => vec![],
        }
    }
    pub fn targets_mut(&mut self) -> Vec<&mut Target> {
        match self {
            Term::Jmp(t) => vec![t],
            Term::Br(_, a, b) => vec![a, b],
            Term::Switch(_, _, cases, d) => {
                let mut v: Vec<&mut Target> = cases.iter_mut().map(|(_, t)| t).collect();
                v.push(d);
                v
            }
            Term::Ret(_) | Term::Unreachable | Term::GotoPtr(..) => vec![],
        }
    }
    /// Successor blocks, including `GotoPtr`'s address-taken set.
    pub fn succs(&self) -> Vec<BlockId> {
        match self {
            Term::GotoPtr(_, bs) => bs.clone(),
            _ => self.targets().iter().map(|t| t.block).collect(),
        }
    }
    pub fn uses(&self, mut f: impl FnMut(Operand)) {
        match self {
            Term::Br(c, ..) | Term::Switch(c, ..) | Term::GotoPtr(c, _) => f(*c),
            Term::Ret(Some(v)) => f(*v),
            _ => {}
        }
        for t in self.targets() {
            t.args.iter().for_each(|a| f(*a));
        }
    }
}

// ── signatures (the C-level view a call must carry for AAPCS64) ────────────
/// A parameter/return as the ABI classifier sees it. HIR never breaks an
/// aggregate apart: isel (`isel/abi.rs`) owns AAPCS64 C.1–C.15.
#[derive(Clone, PartialEq, Debug)]
pub enum PTy {
    S(Ty),
    /// binary128 `long double` (AAPCS64: a 16-byte FP value in a V register).
    LDouble,
    Agg {
        size: u32,
        align: u32,
        /// AAPCS64 §5.9.5 HFA/HVA: (element is double, element count).
        hfa: Option<(bool, u32)>,
    },
}

#[derive(Clone, PartialEq, Debug)]
pub struct Sig {
    pub params: Vec<PTy>,
    pub ret: Option<PTy>,
    /// Number of NAMED parameters; `variadic` args start here (AAPCS64 §6.4.2).
    pub nfix: u32,
    pub variadic: bool,
}

// ── containers ─────────────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct Block {
    pub params: Vec<ValueId>,
    pub insts: Vec<Inst>,
    pub term: Term,
    /// The C `goto` labels that land on this block. Two things read them:
    /// C99 6.8.6.1 (a function with a VLA deallocates back to the frame base
    /// here), and EXT(gcc) `&&label`, whose address a STATIC initializer may
    /// take — so the label needs a real emitted symbol, not just a block index.
    pub labels: Vec<String>,
    /// Static execution-frequency estimate (Ball & Larus 1993). Advisory only:
    /// it drives block layout and spill next-use weighting and carries NO
    /// semantic obligation, so no pass needs a commuting square for it.
    pub weight: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ValueInfo {
    pub ty: Ty,
    pub def: Def,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Def {
    /// defined by `blocks[b].insts[i]`
    Inst(BlockId, u32),
    /// `blocks[b].params[k]`
    Param(BlockId, u32),
    /// the k-th incoming function parameter (entry block only)
    FuncParam(u32),
}

/// A static stack object: a C local whose address is observable, an aggregate, or
/// (R0/R1) the whole parser-assigned frame.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub size: u32,
    pub align: u32,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub sig: Sig,
    pub blocks: Vec<Block>,
    pub values: Vec<ValueInfo>,
    pub slots: Vec<Slot>,
    pub entry: BlockId,
    pub is_static: bool,
    pub is_weak: bool,
    /// C99 6.7.5.2: this function declares a variable-length array. Only the
    /// label-deallocation rule above reads it; a bare `alloca` does NOT set it
    /// (it has no scope to leave), which is exactly gcc's distinction.
    pub has_vla: bool,
    /// The EXTENT of every object inside slot 0, as (offset within the slot,
    /// size), carried over from `ast::Func::objs`. `SlotAddr` gives an offset;
    /// without an extent a pass cannot tell where one local ends and the next
    /// begins, and must treat an escaped address as reaching the whole frame.
    /// With it, C99 6.5.6p8 (pointer arithmetic is defined only within the
    /// object) bounds the escape to one object and `pass/sroa.rs` may promote
    /// the rest.
    pub objs: Vec<(i64, u32)>,
}

impl Func {
    pub fn new_value(&mut self, ty: Ty, def: Def) -> ValueId {
        self.values.push(ValueInfo { ty, def });
        (self.values.len() - 1) as ValueId
    }
    pub fn ty_of(&self, v: ValueId) -> Ty {
        self.values[v as usize].ty
    }
    pub fn block(&self, b: BlockId) -> &Block {
        &self.blocks[b as usize]
    }
    pub fn block_mut(&mut self, b: BlockId) -> &mut Block {
        &mut self.blocks[b as usize]
    }
    pub fn new_block(&mut self) -> BlockId {
        self.blocks.push(Block {
            params: Vec::new(),
            insts: Vec::new(),
            term: Term::Unreachable,
            weight: 1,
            labels: Vec::new(),
        });
        (self.blocks.len() - 1) as BlockId
    }
}

#[derive(Clone, Debug)]
pub struct Module {
    pub funcs: Vec<Func>,
}
