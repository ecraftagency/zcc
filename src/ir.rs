// src/ir.rs — the frontend↔backend boundary (the IR contract).
//
// Position:  parser → AST (ast.rs) ──lower──▶ IR (here) ──▶ codegen/<target> → .s
//
// Invariant: the backend only READS the IR + TyTab; it does not read the AST or
// parser. The IR is the "intermediate grammar" in which every C source semantic
// (lvalue, scope, declarator) has already been lowered — each Inst is a TYPED
// 3-address term; the backend is left with only instruction selection + ABI.
//
// Foundational theorems (each type below maps to one concept):
//   - Val    = an operand of the 3-address algebra (a temporary t or a constant).
//   - Op     = a pure operator SYMBOL; the SEMANTICS (ring ℤ/2^n, field ℝ, signedness)
//              are determined by the accompanying TypeId — separating "operation"
//              from "algebraic structure".
//   - Block+Term = an explicit CFG: every block ends in exactly one terminator
//              (enforced by TYPE — `term` is a field, not the last vector element).
//   - IrFunc = a finite control graph + the temporary type table (Γ: Tmp → TypeId).
//
// Baseline: IR only, optimization deferred. The pass layer is future work — see
// IR.md §5. The interpreter/verifier is a proof-checker (test-side; it does not
// count against the 10k ceiling); the verifier lives here because it is light and
// is an automaton that checks immediately after lowering.
#![allow(dead_code)] // removed once lowering (step 2) + the backend IR→asm (step 3) consume it

use crate::ast::{Ast, Node, NodeId, SyncOp, Ty, TyTab, TypeId, INT, ULONG, VOID};
use std::collections::HashMap;

pub type Tmp = u32; // temporary identifier; indexes into IrFunc.temps (the type table Γ)
pub type BlockId = u32; // indexes into IrFunc.blocks; blocks[0] = entry

/// A 3-address operand. Expressions are NOT nested — the parser has lowered the
/// tree into a sequence of assignments.
#[derive(Clone, Copy, Debug)]
pub enum Val {
    Tmp(Tmp),  // the value of a temporary. Straight lowering is SSA-free (a temp may be
               // reassigned); to_ssa (ssa-qbe fork) re-establishes single-assignment via Inst::Phi.
    Imm(i64),  // integer constant (including constant pointers, char, enum) — width is contextual
    FImm(u64), // floating-point constant as a BIT PATTERN (f32 in the low 32 bits / f64)
}

/// A binary operator symbol — PURELY algebraic. Signedness (signed/unsigned) and
/// floatness are taken from the TypeId accompanying Inst::Bin, not encoded here.
/// Comparisons yield {0, 1}.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Add, Sub, Mul, Div, Rem, // arithmetic: ℤ/2^n (int) or ℝ (float); Div/Rem are signedness-sensitive
    And, Or, Xor, Shl, Shr,  // bitwise: shr is signedness-sensitive (arithmetic vs logical)
    Eq, Ne, Lt, Le, Gt, Ge,  // relational → 0/1; Lt..Ge are signedness-sensitive
}

/// A unary operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Un {
    Neg,  // arithmetic negation (int wraps mod 2^n / float)
    BNot, // ~ (bitwise complement)
}
// Note: logical `!` has already been lowered by the parser into `== 0` (Op::Eq with Imm 0), so no Un is needed.

/// A site that computes a STATIC address (Lea). A dynamic address (pointer +
/// offset) is Bin(Add) on the pointer value; there is no separate Inst for
/// Member/Index (already folded into an Add of the offset).
#[derive(Clone, Debug)]
pub enum Place {
    Local(u32),          // &local variable: frame offset, fp-relative (consult the frame)
    Global(String, i64), // &global ± byte offset (symbol + constant)
    Str(u32),            // &the i-th string literal (index into the strs table)
}

/// The target of a call.
#[derive(Clone, Debug)]
pub enum Callee {
    Sym(String), // direct call by function name
    Ptr(Val),    // indirect call through a function pointer
}

/// An IR instruction. Two CLASSES (see IR.md §2b):
///   CORE   — evaluable by interp, covered by the verifier, (in future) touched by passes.
///   OPAQUE — wraps an exotic construct (va/atomic/asm/…), lowered one-to-one to
///            the backend exactly as the old AST→asm path did; interp treats any
///            function containing one as "impure" (skips folding).
#[derive(Clone, Debug)]
pub enum Inst {
    // ---- CORE ----
    Bin(Tmp, Op, TypeId, Val, Val), // dst = a ⟨op⟩ b, interpreted in TypeId
    Un(Tmp, Un, TypeId, Val),       // dst = ⟨op⟩ a
    Copy(Tmp, TypeId, Val),         // dst = a (rename / load a constant)
    Load(Tmp, TypeId, Val),         // dst = *(addr), width = size(TypeId)
    Store(TypeId, Val, Val),        // *(addr) = val; (ty, addr, val)
    Memcpy(Val, Val, u32),          // copy `size` bytes *(src) → *(dst); (dst, src, size).
    // struct/union assignment (C99 6.5.16). CORE, target-independent: interp can
    // execute it, and each backend lowers it itself (loop / rep-movs / memcpy
    // libcall) — no AST knowledge required.
    Lea(Tmp, Place),                // dst = the address of Place
    Cast(Tmp, TypeId, TypeId, Val), // dst:to = cast(from → to) a  (trunc/ext/f↔i)
    Call(Option<Tmp>, Callee, Vec<Val>, u32), // dst?, callee, args, nfix (variadic ABI)
    // φ-node (no C surface — an SSA artifact). dst = the value flowing in from whichever
    // predecessor transferred control here; arms = [(pred BlockId, Val)]. CORE: interp
    // selects the arm matching the taken predecessor edge. Present ONLY on the path
    // between to_ssa (Stage 2) and out_of_ssa (Stage 3) — straight lowering never emits
    // it, and it MUST be eliminated (φ→copies) before the backend runs.
    Phi(Tmp, TypeId, Vec<(BlockId, Val)>),
    // ---- EXOTIC typed (gradually replacing Opaque; operand-free first) ----
    // dst = &function (GOT if extern, adrp/add if static). A constant-symbol
    // address, with NO Val operand. Kept impure (like Opaque) → no DCE/CSE → invariant asm.
    FunAddr(Tmp, String),   // dst = the address of function `name`
    LabelAddr(Tmp, String), // EXT(gcc): dst = &&label (computed-goto) within the current function
    // memset(addr, 0, sz): zero-initialize a struct/array (C99 6.7.8). Void, with a
    // memory-write side effect → impure like Store, with NO dst.
    Zero(Val, u32), // *(addr .. addr+sz) = 0
    // Variadic AAPCS64 (C99 7.15). Operand = &va_list. VaStart is void; VaArg
    // returns one value (a struct as its address). Impure (reads/writes va_list + save-area).
    VaStart(Val),                    // initialize *(&ap) from prologue state
    VaArg(Tmp, Val, TypeId, u32),    // dst = va_arg(*(&ap), t); tmp = scratch local (HFA gather)
    // EXT(gcc) __builtin_*_overflow: dst = (did a op b overflow?); the result is written to *(rp).
    // op = u8 code; ta/tb = operand types (signedness), rt = the type of *(rp) (signedness+width). Impure.
    Overflow(Tmp, u8, TypeId, TypeId, TypeId, Val, Val, Val), // dst,op,ta,tb,rt,a,b,rp
    VaArea(Tmp, u32), // builtin __va_area__: dst = x29 + off (start of the anonymous-argument area)
    // EXT(gcc): computed-goto "goto *e": branch through a value. Ends the block at
    // runtime (the IR block following it is dead — as with the old Opaque). Impure, with NO dst.
    GotoPtr(Val),
    // C99 6.7.5.2 VLA / __builtin_alloca: dst = a pointer to `size` bytes allocated
    // on the stack (sub sp, rounded to 16). Impure (mutates sp) → no DCE/CSE even if
    // dst is dead; the epilogue `mov sp,x29` reclaims it, reset_sp_base at a depth-0 label (backward goto).
    Alloca(Tmp, Val), // dst = &the allocated region; operand = the byte count
    // Full-ABI call (composite arg/ret, register overflow, float≠8B, long double) —
    // the AAPCS64 C.1–C.11 automaton. Each operand carries its arg TYPE (Val = a
    // scalar value / a struct ADDRESS, matching lower_expr). ret = the return type;
    // sret = the local slot when ret is a struct (dst = &slot). Pure-scalar calls
    // still go through Inst::Call (faster). Impure.
    CallX(Option<Tmp>, Callee, Vec<(Val, TypeId)>, TypeId, u32), // dst?, callee, (val,ty)*, ret, sret-off
    // EXT(gcc) atomics __sync_* (borrowed by C11): LL/SC ldaxr/stlxr. Operands =
    // (ptr[, val [, val2]]) → x0/x1/x2. sz = 4|8 (width), ret = the result type
    // (signedness, canonicalized x0). dst None ⟺ void (Release/Barrier). Impure (memory write + barrier).
    Sync(Option<Tmp>, SyncOp, Vec<Val>, u32, TypeId), // dst?, op, operands, size, ret
    // EXT(gcc) inline asm: template + operands already materialized into Val (inp =
    // input value / memory address loaded into a register; wb = writeback address
    // for a non-mem output). Void, impure (writes memory through output/mem-operands
    // and may clobber). With NO dst.
    Asm(String, Vec<AsmIrOp>),
}

/// An inline-asm operand lowered to IR: constraint metadata (carried over from
/// AsmOp) + type + the computed Val. inp = input value / (mem) address loaded into
/// a register in phase 1 (None = pure output). wb = writeback address for a non-mem
/// output (None = no writeback).
#[derive(Clone, Debug)]
pub struct AsmIrOp {
    pub out: bool,
    pub rw: bool,
    pub mem: bool,
    pub fp: bool,
    pub tied: Option<u8>,
    pub pin: Option<u8>,
    pub ty: TypeId,
    pub inp: Option<Val>,
    pub wb: Option<Val>,
}

/// Terminator — transfers control out of a block. A finite automaton over the BlockId set.
#[derive(Clone, Debug)]
pub enum Term {
    Jmp(BlockId),                          // unconditional jump
    Br(Val, BlockId, BlockId),             // cond ≠ 0 → then, otherwise → els
    Ret(Option<Val>),                      // return (void if None)
    Unreachable,                           // after noreturn / a fallthrough sentinel that is never reached
}

/// A basic block: a straight-line instruction sequence + exactly one terminator (a type-level invariant).
#[derive(Clone, Debug)]
pub struct Block {
    pub insts: Vec<Inst>,
    pub term: Term,
}

/// A function in IR form: the CFG (blocks) + the temporary type table (Γ) + the stack frame.
#[derive(Clone)]
pub struct IrFunc {
    pub name: String,
    pub temps: Vec<TypeId>,        // Γ: temporary i has type temps[i]
    pub params: Vec<(u32, TypeId)>, // (frame offset, type) — a parameter placed into its frame
    // slot per the ABI (backend emit_params); interp seeds it directly into memory. NO parameter temporaries.
    pub blocks: Vec<Block>, // blocks[0] = entry
    pub frame: u32,         // frame size (rounded to 16) — for Lea(Local)
    pub ret: TypeId,        // return type
    // EXT(gcc): C labels (goto/&&label) → blocks. The backend additionally emits
    // `lg_fname.name:` at the block so that computed-goto (LabelAddr/GotoPtr) can resolve the label address.
    pub labels: Vec<(String, BlockId)>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Verifier — an automaton for well-formedness (IR.md §3a). Runs after lowering /
// after every pass; rejects malformed IR IMMEDIATELY rather than letting it drift
// down into garbage asm. v1 (baseline) checks:
//   (V1) ref-integrity: every Tmp id < |temps|, every target BlockId < |blocks|.
//   (V2) def-coverage : every temporary that is USED must be DEFINED ≥1 time in the function.
//   (V3) entry        : the function has ≥1 block (blocks[0] = entry).
// (Per-path def-before-use (dominance) is a stronger theorem — enabled once a pass
//  moves instructions; v1 need only preserve reference validity after straight lowering.)
// ─────────────────────────────────────────────────────────────────────────────

/// The temporary DEFINED by this instruction (its destination), if any.
pub(crate) fn inst_def(i: &Inst) -> Option<Tmp> {
    match i {
        Inst::Bin(d, ..)
        | Inst::Un(d, ..)
        | Inst::Copy(d, ..)
        | Inst::Load(d, ..)
        | Inst::Lea(d, ..)
        | Inst::Cast(d, ..)
        | Inst::FunAddr(d, ..)
        | Inst::LabelAddr(d, ..)
        | Inst::VaArg(d, ..)
        | Inst::Overflow(d, ..)
        | Inst::VaArea(d, ..)
        | Inst::Alloca(d, ..)
        | Inst::Phi(d, ..) => Some(*d),
        Inst::Call(d, ..) | Inst::CallX(d, ..) | Inst::Sync(d, ..) => *d,
        Inst::Store(..)
        | Inst::Memcpy(..)
        | Inst::Zero(..)
        | Inst::VaStart(..)
        | Inst::GotoPtr(..)
        | Inst::Asm(..) => None,
    }
}

/// Collect every Tmp that this instruction USES (reads) into `out`.
pub(crate) fn inst_uses(i: &Inst, out: &mut Vec<Tmp>) {
    let mut v = |x: &Val| {
        if let Val::Tmp(t) = x {
            out.push(*t)
        }
    };
    match i {
        Inst::Bin(_, _, _, a, b) => {
            v(a);
            v(b);
        }
        Inst::Un(_, _, _, a) | Inst::Copy(_, _, a) | Inst::Load(_, _, a) | Inst::Cast(_, _, _, a) => {
            v(a)
        }
        Inst::Store(_, a, b) | Inst::Memcpy(a, b, _) => {
            v(a);
            v(b);
        }
        Inst::Zero(a, _)
        | Inst::VaStart(a)
        | Inst::VaArg(_, a, _, _)
        | Inst::GotoPtr(a)
        | Inst::Alloca(_, a) => v(a),
        Inst::Overflow(_, _, _, _, _, a, b, rp) => {
            v(a);
            v(b);
            v(rp);
        }
        // A φ uses every incoming value (semantically the use occurs on the predecessor
        // edge; for def-coverage, membership is what the verifier checks).
        Inst::Phi(_, _, arms) => {
            for (_, a) in arms {
                v(a)
            }
        }
        Inst::Lea(..)
        | Inst::FunAddr(..)
        | Inst::LabelAddr(..)
        | Inst::VaArea(..) => {}
        Inst::Call(_, c, args, _) => {
            if let Callee::Ptr(p) = c {
                v(p)
            }
            for a in args {
                v(a)
            }
        }
        Inst::CallX(_, c, args, _, _) => {
            if let Callee::Ptr(p) = c {
                v(p)
            }
            for (a, _) in args {
                v(a)
            }
        }
        Inst::Sync(_, _, args, _, _) => {
            for a in args {
                v(a)
            }
        }
        Inst::Asm(_, ops) => {
            for op in ops {
                if let Some(x) = &op.inp {
                    v(x)
                }
                if let Some(x) = &op.wb {
                    v(x)
                }
            }
        }
    }
}

/// Collect every Tmp that the terminator USES.
pub(crate) fn term_uses(t: &Term, out: &mut Vec<Tmp>) {
    let mut v = |x: &Val| {
        if let Val::Tmp(t) = x {
            out.push(*t)
        }
    };
    match t {
        Term::Br(c, ..) => v(c),
        Term::Ret(Some(r)) => v(r),
        Term::Jmp(_) | Term::Ret(None) | Term::Unreachable => {}
    }
}

/// The block targets of a terminator (for CFG ref-integrity checking).
pub(crate) fn term_targets(t: &Term, out: &mut Vec<BlockId>) {
    match t {
        Term::Jmp(b) => out.push(*b),
        Term::Br(_, a, b) => {
            out.push(*a);
            out.push(*b);
        }
        Term::Ret(_) | Term::Unreachable => {}
    }
}

/// Check the well-formedness of a function. Returns Err(description) at the FIRST violation.
pub fn verify(f: &IrFunc) -> Result<(), String> {
    // (V3) entry exists
    if f.blocks.is_empty() {
        return Err(format!("{}: empty function (no entry block)", f.name));
    }
    let nt = f.temps.len() as u32;
    let nb = f.blocks.len() as u32;

    // The set of defined temporaries. A parameter is NOT a temporary (it lives in a
    // frame slot, populated by the backend per the ABI) → every parameter read is a
    // Load(mem)→new temporary, so there is no use-before-def.
    let mut defined = vec![false; nt as usize];
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = inst_def(i) {
                if d >= nt {
                    return Err(format!("{}: def temp t{d} out of table |temps|={nt}", f.name));
                }
                defined[d as usize] = true;
            }
        }
    }

    // (V1) ref-integrity + (V2) def-coverage.
    let mut uses = Vec::new();
    let mut targets = Vec::new();
    for (bi, b) in f.blocks.iter().enumerate() {
        for i in &b.insts {
            uses.clear();
            inst_uses(i, &mut uses);
            for &u in &uses {
                if u >= nt {
                    return Err(format!("{}: use temp t{u} out of table |temps|={nt}", f.name));
                }
                if !defined[u as usize] {
                    return Err(format!("{}: temp t{u} used but never defined", f.name));
                }
            }
            // (V1) φ arms name predecessor blocks — those BlockIds must be in range.
            if let Inst::Phi(d, _, arms) = i {
                for (pb, _) in arms {
                    if *pb >= nb {
                        return Err(format!("{}: phi t{d} names block{pb} out of |blocks|={nb}", f.name));
                    }
                }
            }
        }
        uses.clear();
        term_uses(&b.term, &mut uses);
        for &u in &uses {
            if u >= nt {
                return Err(format!("{}: term block{bi} uses t{u} out of |temps|={nt}", f.name));
            }
            if !defined[u as usize] {
                return Err(format!("{}: term block{bi} uses t{u} not yet defined", f.name));
            }
        }
        targets.clear();
        term_targets(&b.term, &mut targets);
        for &tg in &targets {
            if tg >= nb {
                return Err(format!("{}: block{bi} jumps to block{tg} out of |blocks|={nb}", f.name));
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Lowering AST → IR (IR.md §4) — WHERE correctness lives. The classic -O0 model:
// every C variable lives in frame MEMORY (offsets already assigned by the parser);
// temporaries hold only the intermediate value of an expression. lower_expr(n)
// emits instructions and returns a Val holding the result; lower_addr(n) returns a
// Val = the ADDRESS of an lvalue; lower_stmt(n) weaves the blocks + CFG.
// The exotic tail (va/atomic/asm/nested/struct/Switch/goto) → a temporary
// Inst::Opaque(node), with the backend bridge re-emitting the old path (step 3
// gradually replaces this with real lowering).
// ─────────────────────────────────────────────────────────────────────────────

/// Is the FINAL element of a stmt-expr a VALUELESS statement (→ value = void)?
/// Node::Block does NOT belong here: a trailing `{…}` compound-statement is void,
/// but a NESTED `({…})` stmt-expr carries a value — both are the SAME Node::Block
/// and cannot be distinguished by node type, so always recurse through lower_expr
/// (the Block arm) to propagate a value if there is one; the surplus value of a
/// compound-statement is harmless (nobody reads it).
/// Mirror the remaining explicit arms of lower_stmt; keep in sync when a new statement is added.
fn is_stmt_node(n: &Node) -> bool {
    matches!(
        n,
        Node::Ret(_)
            | Node::If(..)
            | Node::While(..)
            | Node::For(..)
            | Node::Do(..)
            | Node::Break
            | Node::Continue
            | Node::Switch(..)
            | Node::Case(_)
            | Node::Label(..)
            | Node::Goto(_)
            | Node::GotoPtr(_)
    )
}

fn map_op(s: &str) -> Op {
    match s {
        "+" => Op::Add, "-" => Op::Sub, "*" => Op::Mul, "/" => Op::Div, "%" => Op::Rem,
        "&" => Op::And, "|" => Op::Or, "^" => Op::Xor, "<<" => Op::Shl, ">>" => Op::Shr,
        "==" => Op::Eq, "!=" => Op::Ne, "<" => Op::Lt, "<=" => Op::Le, ">" => Op::Gt, ">=" => Op::Ge,
        _ => unreachable!("operator is not binary: {s}"),
    }
}

struct Lower<'a> {
    a: &'a Ast,
    temps: Vec<TypeId>,
    blocks: Vec<Block>,
    cur: usize,
    done: bool, // does the current block already have a terminator?
    brk: Vec<BlockId>,
    cont: Vec<BlockId>,
    case_blk: HashMap<u32, BlockId>, // Case-node id → block (the case-label target within a switch)
    label_blk: HashMap<String, BlockId>, // goto-label name → block (lazy, forward/back)
}

impl<'a> Lower<'a> {
    fn t(&mut self, ty: TypeId) -> Tmp {
        self.temps.push(ty);
        (self.temps.len() - 1) as Tmp
    }
    fn push(&mut self, i: Inst) {
        if !self.done {
            self.blocks[self.cur].insts.push(i);
        }
    }
    fn reserve(&mut self) -> BlockId {
        self.blocks.push(Block { insts: Vec::new(), term: Term::Unreachable });
        (self.blocks.len() - 1) as BlockId
    }
    fn goto(&mut self, b: BlockId) {
        self.cur = b as usize;
        self.done = false;
    }
    fn seal(&mut self, term: Term) {
        if !self.done {
            self.blocks[self.cur].term = term;
            self.done = true;
        }
    }
    fn label_block(&mut self, name: String) -> BlockId {
        if let Some(&b) = self.label_blk.get(&name) {
            b
        } else {
            let b = self.reserve();
            self.label_blk.insert(name, b);
            b
        }
    }
    fn scalar(&self, ty: TypeId) -> bool {
        let tt = &self.a.tt;
        tt.is_integer(ty) || tt.is_float(ty) || matches!(tt.tys[ty as usize], Ty::Ptr(_))
    }

    /// The address of an lvalue → Val (the address type is ULONG, 64-bit, no wrapping).
    fn lower_addr(&mut self, n: NodeId) -> Val {
        let a = self.a; // &'a Ast (Copy) — detached from &mut self
        match &a.nodes[n as usize] {
            Node::Var(off) => {
                let off = *off;
                let t = self.t(ULONG);
                self.push(Inst::Lea(t, Place::Local(off)));
                Val::Tmp(t)
            }
            Node::GVar(i) => {
                let name = a.globals[*i as usize].name.clone();
                let t = self.t(ULONG);
                self.push(Inst::Lea(t, Place::Global(name, 0)));
                Val::Tmp(t)
            }
            Node::Member(b, off) => {
                let (b, off) = (*b, *off);
                let base = self.lower_addr(b);
                if off == 0 {
                    base
                } else {
                    let t = self.t(ULONG);
                    self.push(Inst::Bin(t, Op::Add, ULONG, base, Val::Imm(off as i64)));
                    Val::Tmp(t)
                }
            }
            Node::Deref(e) => self.lower_expr(*e),
            Node::Str(i) => {
                let i = *i;
                let t = self.t(ULONG);
                self.push(Inst::Lea(t, Place::Str(i)));
                Val::Tmp(t)
            }
            // exotic lvalue (SRet/Comma/Assign-struct/Cond/Block stmt-expr): an
            // aggregate-typed value IS its address (the by-ref model) — matching the
            // AST-walk addr()=expr(). A scalar assignment used as an lvalue is invalid
            // C, so it never reaches here.
            _ => self.lower_expr(n),
        }
    }

    /// The value (rvalue) of an expression → Val.
    fn lower_expr(&mut self, n: NodeId) -> Val {
        let a = self.a;
        let ty = a.types[n as usize];
        match &a.nodes[n as usize] {
            Node::Num(v) => Val::Imm(*v),
            Node::FNum(f) => Val::FImm(f.to_bits()),
            Node::Str(_) => self.lower_addr(n), // character array decays → pointer
            Node::Var(_) | Node::GVar(_) | Node::Member(..) | Node::Deref(_) => {
                let addr = self.lower_addr(n);
                if self.scalar(ty) {
                    let t = self.t(ty);
                    self.push(Inst::Load(t, ty, addr));
                    Val::Tmp(t)
                } else {
                    addr // array/struct: rvalue = the address (decay / by-ref)
                }
            }
            Node::Addr(e) => self.lower_addr(*e),
            Node::Assign(l, r) => {
                let (l, r) = (*l, *r);
                let lty = a.types[l as usize];
                if self.scalar(lty) {
                    let addr = self.lower_addr(l);
                    let v = self.lower_expr(r);
                    self.push(Inst::Store(lty, addr, v));
                    v
                } else {
                    // struct/union assignment: copy size(ty) bytes; rvalue = the
                    // destination address (C99 6.5.16). LHS address before RHS
                    // (matching the AST reference order).
                    let dst = self.lower_addr(l);
                    let src = self.lower_expr(r);
                    self.push(Inst::Memcpy(dst, src, a.tt.size(lty)));
                    dst
                }
            }
            Node::Bin(op, l, r) => {
                let (op, l, r) = (*op, *l, *r);
                let opty = a.types[l as usize]; // the common type after UAC (already cast by the parser)
                // Emit the RIGHT operand first (matching the AST path — C99 6.5p3
                // leaves operand order unspecified, but matching the reference keeps
                // the differential noise-free: e.g. `x[i] |= foo()` requires foo() to
                // run before x[i] is read). The operand positions stay x=lhs, y=rhs;
                // only the side-effect order changes.
                let y = self.lower_expr(r);
                let x = self.lower_expr(l);
                let t = self.t(ty);
                self.push(Inst::Bin(t, map_op(op), opty, x, y));
                Val::Tmp(t)
            }
            Node::Neg(e) => {
                let v = self.lower_expr(*e);
                let t = self.t(ty);
                self.push(Inst::Un(t, Un::Neg, ty, v));
                Val::Tmp(t)
            }
            Node::Cast(e) => {
                let e = *e;
                let from = a.types[e as usize];
                let v = self.lower_expr(e);
                if from == ty || !self.scalar(from) || !self.scalar(ty) {
                    v // reinterpret / no-op (including to/from a struct pointer)
                } else {
                    let t = self.t(ty);
                    self.push(Inst::Cast(t, from, ty, v));
                    Val::Tmp(t)
                }
            }
            Node::Cond(c, tb, eb) => {
                let (c, tb, eb) = (*c, *tb, *eb);
                let cv = self.lower_expr(c);
                let (tblk, eblk, mblk) = (self.reserve(), self.reserve(), self.reserve());
                let res = self.t(ty);
                self.seal(Term::Br(cv, tblk, eblk));
                self.goto(tblk);
                let tv = self.lower_expr(tb);
                self.push(Inst::Copy(res, ty, tv));
                self.seal(Term::Jmp(mblk));
                self.goto(eblk);
                let ev = self.lower_expr(eb);
                self.push(Inst::Copy(res, ty, ev));
                self.seal(Term::Jmp(mblk));
                self.goto(mblk);
                Val::Tmp(res)
            }
            Node::Comma(l, r) => {
                let (l, r) = (*l, *r);
                self.lower_expr(l);
                self.lower_expr(r)
            }
            Node::Post(op, l, delta) => {
                let (op, l, delta) = (*op, *l, *delta);
                let lty = a.types[l as usize];
                let addr = self.lower_addr(l);
                let old = self.t(lty);
                self.push(Inst::Load(old, lty, addr));
                let nw = self.t(lty);
                let o = if op == "+" { Op::Add } else { Op::Sub };
                self.push(Inst::Bin(nw, o, lty, Val::Tmp(old), Val::Imm(delta)));
                self.push(Inst::Store(lty, addr, Val::Tmp(nw)));
                Val::Tmp(old)
            }
            // Pure-scalar call → Inst::Call (clean IR). Composite arg/ret (struct
            // by-value, HFA, >16B, float≠8B, register overflow) → the ABI automaton
            // C.1–C.11 → Inst::CallX (operands carry TYPES, the emitter ports
            // self.call). A struct return has already been wrapped in SRet by the
            // parser, so in this arm ty is always scalar/void.
            Node::Call(name, args, nfix) => {
                let (name, nfix) = (name.clone(), *nfix);
                let args = args.clone();
                if self.call_composite(&args, ty) {
                    let cargs = self.lower_call_args(&args);
                    return self.emit_callx(Callee::Sym(name), cargs, ty);
                }
                let av: Vec<Val> = args.iter().map(|&x| self.lower_expr(x)).collect();
                if ty == VOID {
                    self.push(Inst::Call(None, Callee::Sym(name), av, nfix));
                    Val::Imm(0)
                } else {
                    let t = self.t(ty);
                    self.push(Inst::Call(Some(t), Callee::Sym(name), av, nfix));
                    Val::Tmp(t)
                }
            }
            Node::CallPtr(e, args, nfix) => {
                let (e, nfix) = (*e, *nfix);
                let args = args.clone();
                if self.call_composite(&args, ty) {
                    let p = self.lower_expr(e);
                    let cargs = self.lower_call_args(&args);
                    return self.emit_callx(Callee::Ptr(p), cargs, ty);
                }
                let p = self.lower_expr(e);
                let av: Vec<Val> = args.iter().map(|&x| self.lower_expr(x)).collect();
                if ty == VOID {
                    self.push(Inst::Call(None, Callee::Ptr(p), av, nfix));
                    Val::Imm(0)
                } else {
                    let t = self.t(ty);
                    self.push(Inst::Call(Some(t), Callee::Ptr(p), av, nfix));
                    Val::Tmp(t)
                }
            }
            // struct-returning call (the parser wraps SRet(call, off, sz)): full ABI +
            // gathering the result (v-reg HFA / x0:x1 ≤16B / x8-sret >16B) into
            // local[off]; the value = &local.
            Node::SRet(call, off, _sz) => {
                let (call, off) = (*call, *off);
                let (pe, name, cargs_nodes) = match &a.nodes[call as usize] {
                    Node::Call(nm, ar, _) => (None, Some(nm.clone()), ar.clone()),
                    Node::CallPtr(e, ar, _) => (Some(*e), None, ar.clone()),
                    _ => unreachable!("SRet wraps a non-call"),
                };
                let callee = match name {
                    Some(nm) => Callee::Sym(nm),
                    None => Callee::Ptr(self.lower_expr(pe.unwrap())),
                };
                let cargs = self.lower_call_args(&cargs_nodes);
                let d = self.t(ULONG); // address of the struct result (&local[off])
                self.push(Inst::CallX(Some(d), callee, cargs, ty, off));
                Val::Tmp(d)
            }
            // exotic with an existing typed Inst (operand-free) → lower directly, not via Opaque.
            Node::FunAddr(name) => {
                let name = name.clone();
                let t = self.t(ty);
                self.push(Inst::FunAddr(t, name));
                Val::Tmp(t)
            }
            Node::LabelAddr(name) => {
                let name = name.clone();
                let t = self.t(ty);
                self.push(Inst::LabelAddr(t, name));
                Val::Tmp(t)
            }
            Node::Zero(l, sz) => {
                let (l, sz) = (*l, *sz);
                let addr = self.lower_addr(l);
                self.push(Inst::Zero(addr, sz));
                Val::Imm(0) // void
            }
            Node::VaStart(ap) => {
                let ap = *ap;
                let addr = self.lower_addr(ap);
                self.push(Inst::VaStart(addr));
                Val::Imm(0) // void
            }
            Node::VaArg(ap, vt, tmp) => {
                let (ap, vt, tmp) = (*ap, *vt, *tmp);
                let addr = self.lower_addr(ap);
                let d = self.t(ty);
                self.push(Inst::VaArg(d, addr, vt, tmp));
                Val::Tmp(d)
            }
            Node::Overflow(op, oa, ob, rp) => {
                let (op, oa, ob, rp) = (*op, *oa, *ob, *rp);
                let (ta, tb) = (a.types[oa as usize], a.types[ob as usize]);
                let rt = a.tt.pointee(a.types[rp as usize]).unwrap();
                let (va, vb) = (self.lower_expr(oa), self.lower_expr(ob));
                let vrp = self.lower_expr(rp);
                let d = self.t(ty);
                self.push(Inst::Overflow(d, op, ta, tb, rt, va, vb, vrp));
                Val::Tmp(d)
            }
            Node::VaArea(off) => {
                let off = *off;
                let d = self.t(ty);
                self.push(Inst::VaArea(d, off));
                Val::Tmp(d)
            }
            Node::Alloca(e) => {
                let sz = self.lower_expr(*e);
                let d = self.t(ty);
                self.push(Inst::Alloca(d, sz));
                Val::Tmp(d)
            }
            Node::Sync(op, args, sz) => {
                let (op, sz) = (*op, *sz);
                let args = args.clone();
                let vals: Vec<Val> = args.iter().map(|&x| self.lower_expr(x)).collect();
                if ty == VOID {
                    self.push(Inst::Sync(None, op, vals, sz, ty));
                    Val::Imm(0)
                } else {
                    let d = self.t(ty);
                    self.push(Inst::Sync(Some(d), op, vals, sz, ty));
                    Val::Tmp(d)
                }
            }
            // EXT(gcc) inline asm (void): materialize each operand → Val. mem = address;
            // pure output = None (writeback only); input/rw = value (+ wb address if out).
            Node::Asm(tpl, ops) => {
                let (tpl, ops) = (tpl.clone(), ops.clone());
                let irops: Vec<AsmIrOp> = ops
                    .iter()
                    .map(|op| {
                        let oty = a.types[op.e as usize];
                        let (inp, wb) = if op.mem {
                            (Some(self.lower_addr(op.e)), None)
                        } else if op.out && !op.rw {
                            (None, Some(self.lower_addr(op.e)))
                        } else {
                            let v = self.lower_expr(op.e);
                            let wb = op.out.then(|| self.lower_addr(op.e));
                            (Some(v), wb)
                        };
                        AsmIrOp {
                            out: op.out,
                            rw: op.rw,
                            mem: op.mem,
                            fp: op.fp,
                            tied: op.tied,
                            pin: op.pin,
                            ty: oty,
                            inp,
                            wb,
                        }
                    })
                    .collect();
                self.push(Inst::Asm(tpl, irops));
                Val::Imm(0) // asm expr = void
            }
            // EXT(gcc): statement-expression `({ s1; …; last })` in expression
            // position: run the statements in sequence (side effects via lower_stmt),
            // the value = the value of the LAST statement if it is an expr-statement
            // (C99-EXT gcc), otherwise void. NO new Inst is needed — a stmt-expr is
            // just "statements + 1 value" in the IR.
            Node::Block(v) => {
                let v = v.clone();
                let Some((&last, init)) = v.split_last() else {
                    return Val::Imm(0); // `({ })` — void
                };
                for &s in init {
                    self.lower_stmt(s);
                }
                if is_stmt_node(&a.nodes[last as usize]) {
                    self.lower_stmt(last);
                    Val::Imm(0)
                } else {
                    if self.done {
                        // the last statement is in dead code (a prior statement sealed
                        // the terminator): open a fresh block so the value-expression
                        // lowers consistently (every def is pushed), avoiding an orphan
                        // temporary (the same bug class as the lower_stmt orphan-temp).
                        let d = self.reserve();
                        self.goto(d);
                    }
                    self.lower_expr(last)
                }
            }
            // Every C99 expression node has a typed arm (evidence: a probe found 0
            // bridge-hits over 3748 real files). A STATEMENT node never enters
            // lower_expr (it goes through lower_stmt).
            _ => unreachable!("lower_expr: node is not a sealed expression"),
        }
    }

    /// Lower each arg → (Val, type). Val = a scalar value / a struct ADDRESS (matching
    /// self.expr): the ABI emitter reads the type to assign a slot and reads the Val
    /// to load a register/stack location.
    fn lower_call_args(&mut self, args: &[NodeId]) -> Vec<(Val, TypeId)> {
        args.iter().map(|&x| (self.lower_expr(x), self.a.types[x as usize])).collect()
    }

    /// Push a CallX (composite, ret scalar/void) and return the result Val.
    fn emit_callx(&mut self, callee: Callee, cargs: Vec<(Val, TypeId)>, ty: TypeId) -> Val {
        if ty == VOID {
            self.push(Inst::CallX(None, callee, cargs, VOID, 0));
            Val::Imm(0)
        } else {
            let d = self.t(ty);
            self.push(Inst::CallX(Some(d), callee, cargs, ty, 0));
            Val::Tmp(d)
        }
    }


    /// Does the call need to bridge to self.call (the ABI automaton C.1–C.11) rather
    /// than a pure-scalar Inst::Call? Yes when: (a) it returns a composite, (b) it has
    /// a composite parameter (struct by-value/HFA/>16B), or (c) it overflows the
    /// registers (GP>8 or FP>8 → the arg must go on the stack). ir_call handles only
    /// the ≤8 GP + ≤8 FP scalar case.
    fn call_composite(&self, args: &[NodeId], ret: TypeId) -> bool {
        let a = self.a;
        if ret != VOID && !self.scalar(ret) {
            return true;
        }
        let (mut gp, mut fp) = (0u32, 0u32);
        for &x in args {
            let ty = a.types[x as usize];
            if !self.scalar(ty) {
                return true;
            }
            if a.tt.is_float(ty) {
                // ir_call passes floats through d-registers (f64) — correct only for
                // `double` (8B). `float` (4B → s-reg fcvt) and `long double` (16B →
                // q-reg) need their own narrow ABI → bridge to self.call.
                if a.tt.size(ty) != 8 {
                    return true;
                }
                fp += 1;
            } else {
                gp += 1;
            }
        }
        gp > 8 || fp > 8
    }

    fn lower_stmt(&mut self, n: NodeId) {
        // DEAD code after a terminator (return/goto/break/continue) is still lowered by
        // Block. `push` drops-when-done, but `t()` allocates a temporary
        // unconditionally → a Cond inside dead-code `goto` revives a block ⟹ the def of
        // an address is dropped (done) while its use comes back to life → an orphan
        // temporary (csmith c0041/c0126, caught by verify V2). Fix: open a FRESH block
        // for each dead statement so it lowers CONSISTENTLY (every def is pushed); the
        // block is unreachable, well-formed, and harmless for the backend to emit; the
        // goto-target label stays reachable via label_block.
        if self.done {
            let d = self.reserve();
            self.goto(d);
        }
        let a = self.a;
        match &a.nodes[n as usize] {
            Node::Block(v) => {
                let v = v.clone();
                for s in v {
                    self.lower_stmt(s);
                }
            }
            Node::Ret(e) => {
                let r = e.map(|e| self.lower_expr(e));
                self.seal(Term::Ret(r));
            }
            Node::If(c, t, e) => {
                let (c, t, e) = (*c, *t, *e);
                let cv = self.lower_expr(c);
                let tblk = self.reserve();
                let mblk = self.reserve();
                let eblk = if e.is_some() { self.reserve() } else { mblk };
                self.seal(Term::Br(cv, tblk, eblk));
                self.goto(tblk);
                self.lower_stmt(t);
                self.seal(Term::Jmp(mblk));
                if let Some(e) = e {
                    self.goto(eblk);
                    self.lower_stmt(e);
                    self.seal(Term::Jmp(mblk));
                }
                self.goto(mblk);
            }
            Node::While(c, b) => {
                let (c, b) = (*c, *b);
                let (cblk, bblk, mblk) = (self.reserve(), self.reserve(), self.reserve());
                self.seal(Term::Jmp(cblk));
                self.goto(cblk);
                let cv = self.lower_expr(c);
                self.seal(Term::Br(cv, bblk, mblk));
                self.cont.push(cblk);
                self.brk.push(mblk);
                self.goto(bblk);
                self.lower_stmt(b);
                self.seal(Term::Jmp(cblk));
                self.cont.pop();
                self.brk.pop();
                self.goto(mblk);
            }
            Node::For(i, c, nx, b) => {
                let (i, c, nx, b) = (*i, *c, *nx, *b);
                if let Some(i) = i {
                    self.lower_stmt(i);
                }
                let (cblk, bblk, nblk, mblk) =
                    (self.reserve(), self.reserve(), self.reserve(), self.reserve());
                self.seal(Term::Jmp(cblk));
                self.goto(cblk);
                match c {
                    Some(c) => {
                        let cv = self.lower_expr(c);
                        self.seal(Term::Br(cv, bblk, mblk));
                    }
                    None => self.seal(Term::Jmp(bblk)),
                }
                self.cont.push(nblk);
                self.brk.push(mblk);
                self.goto(bblk);
                self.lower_stmt(b);
                self.seal(Term::Jmp(nblk));
                self.cont.pop();
                self.brk.pop();
                self.goto(nblk);
                if let Some(nx) = nx {
                    self.lower_expr(nx);
                }
                self.seal(Term::Jmp(cblk));
                self.goto(mblk);
            }
            Node::Do(b, c) => {
                let (b, c) = (*b, *c);
                let (bblk, cblk, mblk) = (self.reserve(), self.reserve(), self.reserve());
                self.seal(Term::Jmp(bblk));
                self.cont.push(cblk);
                self.brk.push(mblk);
                self.goto(bblk);
                self.lower_stmt(b);
                self.seal(Term::Jmp(cblk));
                self.cont.pop();
                self.brk.pop();
                self.goto(cblk);
                let cv = self.lower_expr(c);
                self.seal(Term::Br(cv, bblk, mblk));
                self.goto(mblk);
            }
            Node::Break => {
                let m = *self.brk.last().unwrap();
                self.seal(Term::Jmp(m));
            }
            Node::Continue => {
                let c = *self.cont.last().unwrap();
                self.seal(Term::Jmp(c));
            }
            // Switch: dispatch = a chain of range tests (v-lo ≤u hi-lo) → Br to the
            // case-block; the body keeps its order (fall-through), Case = a block
            // boundary, Break → merge.
            Node::Switch(c, b, cases, def) => {
                let (c, b) = (*c, *b);
                let cases = cases.clone();
                let def = *def;
                let cv = self.lower_expr(c);
                let merge = self.reserve();
                for &(_, _, cid) in &cases {
                    let blk = self.reserve();
                    self.case_blk.insert(cid, blk);
                }
                let defblk = match def {
                    Some(d) => {
                        let blk = self.reserve();
                        self.case_blk.insert(d, blk);
                        blk
                    }
                    None => merge,
                };
                for &(lo, hi, cid) in &cases {
                    let target = self.case_blk[&cid];
                    let d1 = self.t(ULONG);
                    self.push(Inst::Bin(d1, Op::Sub, ULONG, cv, Val::Imm(lo)));
                    let cond = self.t(INT);
                    self.push(Inst::Bin(
                        cond,
                        Op::Le,
                        ULONG,
                        Val::Tmp(d1),
                        Val::Imm(hi.wrapping_sub(lo)),
                    ));
                    let next = self.reserve();
                    self.seal(Term::Br(Val::Tmp(cond), target, next));
                    self.goto(next);
                }
                self.seal(Term::Jmp(defblk));
                let body = self.reserve(); // statements before the first case = unreachable (C)
                self.goto(body);
                self.brk.push(merge);
                self.lower_stmt(b);
                self.seal(Term::Jmp(merge));
                self.brk.pop();
                self.goto(merge);
            }
            Node::Case(st) => {
                let st = *st;
                let blk = self.case_blk[&n]; // the Case node id = the key (already reserved in Switch)
                self.seal(Term::Jmp(blk)); // fall-through into the case label
                self.goto(blk);
                self.lower_stmt(st);
            }
            Node::Label(name, st) => {
                let (name, st) = (name.clone(), *st);
                let blk = self.label_block(name);
                self.seal(Term::Jmp(blk));
                self.goto(blk);
                self.lower_stmt(st);
            }
            Node::Goto(name) => {
                let blk = self.label_block(name.clone());
                self.seal(Term::Jmp(blk));
            }
            // computed goto / non-local goto: still exotic (the step-2 tail)
            Node::GotoPtr(e) => {
                let e = *e;
                let target = self.lower_expr(e);
                self.push(Inst::GotoPtr(target));
            }
            // an expression used as a statement: emit its side effects, discard the result
            _ => {
                self.lower_expr(n);
            }
        }
    }
}

/// AST → a list of IrFunc. The backend reads only this result + the TyTab.
pub fn lower(ast: &Ast) -> Vec<IrFunc> {
    let mut out = Vec::with_capacity(ast.funcs.len());
    for f in &ast.funcs {
        let mut lo = Lower {
            a: ast,
            temps: Vec::new(),
            blocks: vec![Block { insts: Vec::new(), term: Term::Unreachable }],
            cur: 0,
            done: false,
            brk: Vec::new(),
            cont: Vec::new(),
            case_blk: HashMap::new(),
            label_blk: HashMap::new(),
        };
        // Parameters need NO prologue in the IR: the backend emit_params populates the
        // frame slots from the args per the ABI (scalar/float/struct/HFA/variadic)
        // BEFORE the body runs; the body reads every variable (parameters included)
        // through Var(off)→Load — a consistent -O0 model.
        lo.lower_stmt(f.body);
        lo.seal(Term::Ret(None)); // fall off the body (void; main→0 is inserted by the frontend)
        let mut labels: Vec<(String, BlockId)> = lo.label_blk.into_iter().collect();
        labels.sort(); // deterministic (HashMap iteration order is not stable)
        out.push(IrFunc {
            name: f.name.clone(),
            temps: lo.temps,
            params: f.params.clone(),
            blocks: lo.blocks,
            frame: f.frame,
            ret: f.ret,
            labels,
        });
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// CORE arithmetic semantics — the atomic DENOTATION functions, SHARED by interp
// (proof side) and const-fold (opt.rs, release side). A SINGLE definition ⟹ the
// folder and the interpreter CANNOT diverge: this is the faithfulness condition of
// correctness-by-construction — ⟦fold(Bin op a b)⟧ = ⟦Bin op a b⟧ holds precisely
// because fold CALLS eval_bin itself.
// (THEORY.md Part I §A4 partial-evaluation + §III keystone.)
// ─────────────────────────────────────────────────────────────────────────────

/// Canonicalize an integer value to the exact width+signedness of `ty` (ℤ/2^n): the
/// theorem "integer arithmetic wraps at size*8 bits" — exactly the backend's
/// `ext(ct)`. Float: the bit pattern passes through unchanged.
pub(crate) fn canon(tt: &TyTab, ty: TypeId, v: i64) -> i64 {
    if tt.is_float(ty) {
        return v;
    }
    let sz = tt.size(ty);
    if sz >= 8 {
        return v;
    }
    let bits = sz * 8;
    let masked = (v as u64) & ((1u64 << bits) - 1);
    if tt.is_unsigned(ty) {
        masked as i64
    } else {
        let sh = 64 - bits; // sign-extend from `bits`
        ((masked << sh) as i64) >> sh
    }
}

/// ⟦Bin⟧: the pure Op interpreted in the algebraic structure of `ty` (ℝ if float,
/// ℤ/2^n with signedness if int). Comparisons → {0,1}. Err at UB (div/mod by 0) →
/// const-fold MUST skip it (do not fold UB into a constant: keep the instruction so
/// the runtime preserves the target behavior).
pub(crate) fn eval_bin(tt: &TyTab, op: Op, ty: TypeId, x: i64, y: i64) -> Result<i64, String> {
    if tt.is_float(ty) {
        let (a, b) = (f64::from_bits(x as u64), f64::from_bits(y as u64));
        let r = match op {
            Op::Add => a + b,
            Op::Sub => a - b,
            Op::Mul => a * b,
            Op::Div => a / b,
            Op::Eq => return Ok((a == b) as i64),
            Op::Ne => return Ok((a != b) as i64),
            Op::Lt => return Ok((a < b) as i64),
            Op::Le => return Ok((a <= b) as i64),
            Op::Gt => return Ok((a > b) as i64),
            Op::Ge => return Ok((a >= b) as i64),
            _ => return Err("eval_bin: operator invalid on float".into()),
        };
        return Ok(r.to_bits() as i64);
    }
    let u = tt.is_unsigned(ty);
    let r = match op {
        Op::Add => x.wrapping_add(y),
        Op::Sub => x.wrapping_sub(y),
        Op::Mul => x.wrapping_mul(y),
        Op::Div if y == 0 => return Err("eval_bin: divide by 0 (UB)".into()),
        Op::Rem if y == 0 => return Err("eval_bin: mod by 0 (UB)".into()),
        Op::Div if u => ((x as u64) / (y as u64)) as i64,
        Op::Div => x.wrapping_div(y),
        Op::Rem if u => ((x as u64) % (y as u64)) as i64,
        Op::Rem => x.wrapping_rem(y),
        Op::And => x & y,
        Op::Or => x | y,
        Op::Xor => x ^ y,
        Op::Shl => x.wrapping_shl(y as u32),
        Op::Shr if u => ((x as u64) >> (y as u32)) as i64,
        Op::Shr => x >> (y as u32),
        Op::Eq => return Ok((x == y) as i64),
        Op::Ne => return Ok((x != y) as i64),
        Op::Lt => return Ok((if u { (x as u64) < (y as u64) } else { x < y }) as i64),
        Op::Le => return Ok((if u { (x as u64) <= (y as u64) } else { x <= y }) as i64),
        Op::Gt => return Ok((if u { (x as u64) > (y as u64) } else { x > y }) as i64),
        Op::Ge => return Ok((if u { (x as u64) >= (y as u64) } else { x >= y }) as i64),
    };
    Ok(canon(tt, ty, r))
}

/// ⟦Cast⟧: convert between domains (int trunc/ext, i↔f). _Bool normalizes to 0/1
/// (C99 6.3.1.2 / 6.3.1.4). Total (no UB) → const-fold can always fold a constant cast.
pub(crate) fn eval_cast(tt: &TyTab, from: TypeId, to: TypeId, v: i64) -> i64 {
    let is_bool = matches!(tt.tys[to as usize], Ty::Bool);
    match (tt.is_float(from), tt.is_float(to)) {
        (false, false) => {
            if is_bool {
                (v != 0) as i64
            } else {
                canon(tt, to, v)
            }
        }
        (false, true) => {
            let f = if tt.is_unsigned(from) { (v as u64) as f64 } else { v as f64 };
            f.to_bits() as i64
        }
        (true, false) => {
            let f = f64::from_bits(v as u64);
            if is_bool {
                (f != 0.0) as i64
            } else {
                canon(tt, to, f as i64) // truncate toward 0 (C99 6.3.1.4)
            }
        }
        (true, true) => v, // f64 is canonical for both
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Interp — the REFERENCE SEMANTICS ⟦·⟧ of the CORE IR. This is the denotation
// function: it defines what a piece of IR computes, and serves as the ground
// truth for the commuting-square oracle (a pass is correct iff it commutes with
// interp). The full mathematical definition of every Inst and the
// commuting-square theorem live in SEMANTICS.md; it specifies the code below
// one-to-one (each `match inst` / `match term` arm is a rule of §4 / §4b, each
// atomic semantic function a definition of §3). This is test-side (#[cfg(test)]):
// it is excluded from the release binary and does not count against the source
// ceiling (IR.md §7.1); it is a proof-checker, not compiler logic. It is not a
// machine-checked proof — it is a mechanized reference semantics, validated by
// structural exhaustion (the foundation for the later verification stages).
//
// Machine state Σ = ⟨ρ, μ⟩ (SEMANTICS.md §2): ρ : Tmp → 𝕍 is the register file
// (each temporary holds a canonical 64-bit value — integers sign/zero-extended
// to their type, floats as an f64 bit pattern with 32-bit float widened to
// double); μ : [0, frame) → Byte is flat local memory (little-endian, LP64),
// with Lea(Local off) yielding index = frame − off. The observable is the return
// value. Globals, string literals, and exotic instructions are not modeled and
// evaluate to Err = ⊥ (an impure function, outside the CORE space, so the
// commuting square skips it, as it does for undefined behavior).
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ast::{TyTab, TypeId, DOUBLE, INT, UINT};

    fn load_mem(mem: &[u8], addr: i64, sz: u32) -> u64 {
        let a = addr as usize;
        let mut x = 0u64;
        for k in 0..sz as usize {
            x |= (mem[a + k] as u64) << (k * 8);
        }
        x
    }
    fn store_mem(mem: &mut [u8], addr: i64, sz: u32, val: u64) {
        let a = addr as usize;
        for k in 0..sz as usize {
            mem[a + k] = ((val >> (k * 8)) & 0xff) as u8;
        }
    }

    /// Run the IR: interp(entry, args) → the return value. Recurses through Call(Sym).
    pub(crate) fn interp(tt: &TyTab, funcs: &[IrFunc], entry: &str, args: &[i64]) -> Result<i64, String> {
        interp_d(tt, funcs, entry, args, 0)
    }

    /// interp with DEPTH COUNTING: interp uses Rust recursion for Call, so an input
    /// forcing deep recursion (e.g. fact(2^31)) would overflow the HOST stack — not an
    /// IR bug. Hitting the bound → Err (an input "outside the modeled space" → equiv
    /// SKIP, like UB). The bound is chosen many times lower than the real stack for safety.
    fn interp_d(tt: &TyTab, funcs: &[IrFunc], entry: &str, args: &[i64], depth: u32) -> Result<i64, String> {
        if depth > 500 {
            return Err("interp: recursion too deep (input outside the modeled space)".into());
        }
        let f = funcs
            .iter()
            .find(|f| f.name == entry)
            .ok_or_else(|| format!("interp: function {entry} not found"))?;
        let mut reg = vec![0i64; f.temps.len()];
        // Seed the args into the frame slots (the backend emit_params does the same
        // per the ABI): a parameter at offset off ⇒ mem index = frame - off (matching
        // Lea Local below).
        let mut mem = vec![0u8; f.frame as usize];
        for (i, &(off, pty)) in f.params.iter().enumerate() {
            let v = canon(tt, pty, *args.get(i).unwrap_or(&0));
            store_mem(&mut mem, (f.frame - off) as i64, tt.size(pty), v as u64);
        }
        let fetch = |reg: &[i64], v: &Val| -> i64 {
            match v {
                Val::Tmp(t) => reg[*t as usize],
                Val::Imm(x) => *x,
                Val::FImm(b) => *b as i64,
            }
        };

        let mut cur = 0usize;
        let mut prev: Option<usize> = None; // the predecessor edge just taken, for Phi selection
        let mut budget = 10_000_000u64; // guard against infinite loops (small tests)
        loop {
            let b = &f.blocks[cur];
            for inst in &b.insts {
                budget -= 1;
                if budget == 0 {
                    return Err("interp: step budget exceeded (infinite loop?)".into());
                }
                match inst {
                    Inst::Bin(d, op, ty, a, bb) => {
                        let (x, y) = (fetch(&reg, a), fetch(&reg, bb));
                        reg[*d as usize] = eval_bin(tt, *op, *ty, x, y)?;
                    }
                    Inst::Un(d, op, ty, a) => {
                        let x = fetch(&reg, a);
                        let r = match op {
                            Un::Neg if tt.is_float(*ty) => (-f64::from_bits(x as u64)).to_bits() as i64,
                            Un::Neg => canon(tt, *ty, x.wrapping_neg()),
                            Un::BNot => canon(tt, *ty, !x),
                        };
                        reg[*d as usize] = r;
                    }
                    Inst::Copy(d, ty, a) => reg[*d as usize] = canon(tt, *ty, fetch(&reg, a)),
                    Inst::Phi(d, ty, arms) => {
                        // Select the arm for the predecessor edge we arrived on. φ-nodes
                        // are parallel, but SSA freshness (φ dsts are new temps not named
                        // by sibling φ arms) makes sequential evaluation on `reg` correct.
                        let src = prev.ok_or_else(|| {
                            "interp: phi in the entry block (no predecessor edge)".to_string()
                        })?;
                        let arm = arms
                            .iter()
                            .find(|(bb, _)| *bb as usize == src)
                            .ok_or_else(|| format!("interp: phi t{d} has no arm for predecessor block{src}"))?;
                        reg[*d as usize] = canon(tt, *ty, fetch(&reg, &arm.1));
                    }
                    Inst::Load(d, ty, a) => {
                        let addr = fetch(&reg, a);
                        let sz = tt.size(*ty);
                        let raw = load_mem(&mem, addr, sz);
                        reg[*d as usize] = if tt.is_float(*ty) {
                            if sz == 4 {
                                (f32::from_bits(raw as u32) as f64).to_bits() as i64
                            } else {
                                raw as i64
                            }
                        } else {
                            canon(tt, *ty, raw as i64)
                        };
                    }
                    Inst::Store(ty, a, val) => {
                        let addr = fetch(&reg, a);
                        let sz = tt.size(*ty);
                        let v = fetch(&reg, val);
                        let raw = if tt.is_float(*ty) && sz == 4 {
                            (f64::from_bits(v as u64) as f32).to_bits() as u64
                        } else {
                            v as u64
                        };
                        store_mem(&mut mem, addr, sz, raw);
                    }
                    Inst::Memcpy(d, s, sz) => {
                        let (da, sa) = (fetch(&reg, d) as usize, fetch(&reg, s) as usize);
                        for k in 0..*sz as usize {
                            mem[da + k] = mem[sa + k]; // forward copy (matching the backend)
                        }
                    }
                    Inst::Lea(d, p) => match p {
                        // zcc ABI: a local address = x29 − off; flat memory [0,frame)
                        // with index 0 = x29−frame ⟹ index = frame − off.
                        Place::Local(off) => reg[*d as usize] = (f.frame - *off) as i64,
                        Place::Global(..) | Place::Str(_) => {
                            return Err("interp: global/str address not modeled".into())
                        }
                    },
                    Inst::Cast(d, from, to, a) => {
                        reg[*d as usize] = eval_cast(tt, *from, *to, fetch(&reg, a));
                    }
                    Inst::Call(d, c, args, _) => {
                        let Callee::Sym(name) = c else {
                            return Err("interp: indirect call not modeled".into());
                        };
                        let av: Vec<i64> = args.iter().map(|v| fetch(&reg, v)).collect();
                        let r = interp_d(tt, funcs, name, &av, depth + 1)?;
                        if let Some(d) = d {
                            reg[*d as usize] = canon(tt, f.temps[*d as usize], r);
                        }
                    }
                    Inst::FunAddr(..)
                    | Inst::LabelAddr(..)
                    | Inst::Zero(..)
                    | Inst::VaStart(..)
                    | Inst::VaArg(..)
                    | Inst::Overflow(..)
                    | Inst::VaArea(..)
                    | Inst::GotoPtr(..)
                    | Inst::Alloca(..)
                    | Inst::CallX(..)
                    | Inst::Sync(..)
                    | Inst::Asm(..) => {
                        return Err("interp: exotic (symbol/va/overflow/goto/alloca/callX/sync/asm — impure function)".into())
                    }
                }
            }
            let from = cur;
            match &b.term {
                Term::Jmp(t) => cur = *t as usize,
                Term::Br(c, t, e) => cur = if fetch(&reg, c) != 0 { *t } else { *e } as usize,
                Term::Ret(v) => return Ok(v.map(|v| fetch(&reg, &v)).unwrap_or(0)),
                Term::Unreachable => return Err("interp: reached Unreachable".into()),
            }
            prev = Some(from);
        }
    }

    // Quickly build an IrFunc. params = (frame offset, type) — a parameter lives in a slot.
    pub(crate) fn mk(
        name: &str,
        temps: Vec<TypeId>,
        params: Vec<(u32, TypeId)>,
        frame: u32,
        ret: TypeId,
        blocks: Vec<Block>,
    ) -> IrFunc {
        IrFunc { name: name.into(), temps, params, blocks, frame, ret, labels: vec![] }
    }

    // ── Theorem 1: the IR can express recursion + branching + integer arithmetic (factorial).
    // fact(n) = n<=1 ? 1 : n*fact(n-1).  Proof: verify OK + interp(5)=120.
    #[test]
    fn factorial() {
        let tt = TyTab::new();
        // parameter n in frame slot off=16 (index frame−off=0). t0=&n, t1=n, t2=cond,
        // t3=n−1, t4=rec, t5=prod. New model: parameters are Loaded from their slot, no parameter-temp.
        let f = mk(
            "fact",
            vec![ULONG, INT, INT, INT, INT, INT],
            vec![(16, INT)],
            16,
            INT,
            vec![
                Block {
                    insts: vec![
                        Inst::Lea(0, Place::Local(16)),
                        Inst::Load(1, INT, Val::Tmp(0)),
                        Inst::Bin(2, Op::Le, INT, Val::Tmp(1), Val::Imm(1)),
                    ],
                    term: Term::Br(Val::Tmp(2), 1, 2),
                },
                Block { insts: vec![], term: Term::Ret(Some(Val::Imm(1))) },
                Block {
                    insts: vec![
                        Inst::Bin(3, Op::Sub, INT, Val::Tmp(1), Val::Imm(1)),
                        Inst::Call(Some(4), Callee::Sym("fact".into()), vec![Val::Tmp(3)], 1),
                        Inst::Bin(5, Op::Mul, INT, Val::Tmp(1), Val::Tmp(4)),
                    ],
                    term: Term::Ret(Some(Val::Tmp(5))),
                },
            ],
        );
        verify(&f).expect("fact well-formed");
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "fact", &[5]).unwrap(), 120);
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "fact", &[0]).unwrap(), 1);
    }

    // ── Theorem (SSA-1): a φ-node selects the value of the predecessor edge actually
    // taken. Diamond: r = (n<10 ? 100 : 200), r built as φ[(then,t3),(else,t4)].
    // Proof: verify OK + interp on BOTH edges picks the correct arm.
    #[test]
    fn phi_diamond() {
        let tt = TyTab::new();
        // t0=&n, t1=n, t2=cond, t3=then-val, t4=else-val, t5=φ
        let f = mk(
            "diamond",
            vec![ULONG, INT, INT, INT, INT, INT],
            vec![(16, INT)],
            16,
            INT,
            vec![
                Block {
                    insts: vec![
                        Inst::Lea(0, Place::Local(16)),
                        Inst::Load(1, INT, Val::Tmp(0)),
                        Inst::Bin(2, Op::Lt, INT, Val::Tmp(1), Val::Imm(10)),
                    ],
                    term: Term::Br(Val::Tmp(2), 1, 2),
                },
                Block { insts: vec![Inst::Copy(3, INT, Val::Imm(100))], term: Term::Jmp(3) },
                Block { insts: vec![Inst::Copy(4, INT, Val::Imm(200))], term: Term::Jmp(3) },
                Block {
                    insts: vec![Inst::Phi(5, INT, vec![(1, Val::Tmp(3)), (2, Val::Tmp(4))])],
                    term: Term::Ret(Some(Val::Tmp(5))),
                },
            ],
        );
        verify(&f).expect("diamond well-formed");
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "diamond", &[5]).unwrap(), 100);
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "diamond", &[50]).unwrap(), 200);
    }

    // ── Theorem (SSA-2): a φ carries a value across a LOOP back-edge (the real SSA
    // payoff — a live value that changes each iteration is a single-assignment temp,
    // reconciled at the header by φ). sum(1..n) via SSA induction/accumulator φs.
    // Proof: verify OK + interp(5)=15, interp(0)=0 (`prev` tracked through the loop).
    #[test]
    fn phi_loop() {
        let tt = TyTab::new();
        // t0=&n, t1=n, t2=i(φ), t3=sum(φ), t4=cond, t5=sum', t6=i'
        let f = mk(
            "sum",
            vec![ULONG, INT, INT, INT, INT, INT, INT],
            vec![(16, INT)],
            16,
            INT,
            vec![
                // B0 entry: load n, jump to the header
                Block {
                    insts: vec![Inst::Lea(0, Place::Local(16)), Inst::Load(1, INT, Val::Tmp(0))],
                    term: Term::Jmp(1),
                },
                // B1 header: i = φ(entry:1, body:i'), sum = φ(entry:0, body:sum'); i<=n ?
                Block {
                    insts: vec![
                        Inst::Phi(2, INT, vec![(0, Val::Imm(1)), (2, Val::Tmp(6))]),
                        Inst::Phi(3, INT, vec![(0, Val::Imm(0)), (2, Val::Tmp(5))]),
                        Inst::Bin(4, Op::Le, INT, Val::Tmp(2), Val::Tmp(1)),
                    ],
                    term: Term::Br(Val::Tmp(4), 2, 3),
                },
                // B2 body: sum' = sum+i, i' = i+1, back to the header
                Block {
                    insts: vec![
                        Inst::Bin(5, Op::Add, INT, Val::Tmp(3), Val::Tmp(2)),
                        Inst::Bin(6, Op::Add, INT, Val::Tmp(2), Val::Imm(1)),
                    ],
                    term: Term::Jmp(1),
                },
                // B3 exit: return the accumulated sum (the header φ at loop exit)
                Block { insts: vec![], term: Term::Ret(Some(Val::Tmp(3))) },
            ],
        );
        verify(&f).expect("sum well-formed");
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "sum", &[5]).unwrap(), 15);
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "sum", &[0]).unwrap(), 0);
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "sum", &[10]).unwrap(), 55);
    }

    // ── Theorem 2: explicit memory (Lea/Store/Load) round-trips at the correct width.
    // frame[0..4] ← 42 (INT); read back + 8 = 50.
    #[test]
    fn load_store() {
        let tt = TyTab::new();
        let f = mk(
            "ls",
            vec![INT, INT, INT], // t0=addr, t1=loaded, t2=result
            vec![],
            8,
            INT,
            vec![Block {
                insts: vec![
                    Inst::Lea(0, Place::Local(8)), // off=frame ⟹ index 0 (x29−frame)
                    Inst::Store(INT, Val::Tmp(0), Val::Imm(42)),
                    Inst::Load(1, INT, Val::Tmp(0)),
                    Inst::Bin(2, Op::Add, INT, Val::Tmp(1), Val::Imm(8)),
                ],
                term: Term::Ret(Some(Val::Tmp(2))),
            }],
        );
        verify(&f).expect("ls well-formed");
        assert_eq!(interp(&tt, std::slice::from_ref(&f), "ls", &[]).unwrap(), 50);
    }

    // ── Theorem 3: arithmetic wraps at 2^32 for INT (canon), quite unlike ULONG/64-bit.
    #[test]
    fn int_wrap() {
        let tt = TyTab::new();
        // INT_MAX + 1 = INT_MIN (wrap 32-bit signed)
        assert_eq!(eval_bin(&tt, Op::Add, INT, i32::MAX as i64, 1).unwrap(), i32::MIN as i64);
        // unsigned: 0u - 1u = 0xFFFFFFFF (UINT)
        assert_eq!(eval_bin(&tt, Op::Sub, UINT, 0, 1).unwrap(), 0xFFFF_FFFF);
        // unsigned comparison: (uint)-1 > 0  (unlike signed -1 < 0)
        assert_eq!(eval_bin(&tt, Op::Gt, UINT, canon(&tt, UINT, -1), 0).unwrap(), 1);
        assert_eq!(eval_bin(&tt, Op::Lt, INT, -1, 0).unwrap(), 1);
    }

    // ── Theorem 4: cast i↔f and _Bool normalization (C99 6.3.1.2 / 6.3.1.4).
    #[test]
    fn casts() {
        let tt = TyTab::new();
        // int 7 → double 7.0
        let d = eval_cast(&tt, INT, DOUBLE, 7);
        assert_eq!(f64::from_bits(d as u64), 7.0);
        // double 3.9 → int 3 (truncate toward 0)
        let i = eval_cast(&tt, DOUBLE, INT, (3.9f64).to_bits() as i64);
        assert_eq!(i, 3);
        // int 5 → _Bool 1 ; int 0 → _Bool 0
        use crate::ast::BOOL;
        assert_eq!(eval_cast(&tt, INT, BOOL, 5), 1);
        assert_eq!(eval_cast(&tt, INT, BOOL, 0), 0);
    }

    // ── Real lowering: parse a C snippet → lower → verify → interp (oracle).
    // Establishes the commuting square at the lowering level: ⟦lower(AST)⟧ = the program meaning.
    pub(crate) fn compile(name: &str, src: &str) -> (crate::ast::Ast, Vec<IrFunc>) {
        // A UNIQUE filename by hash(src): tests run in parallel, and a shared `name`
        // with a different `src` would race-write the same file → garbage parse.
        // Hashing src ⟹ a different src = a different file.
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        src.hash(&mut h);
        let path = std::env::temp_dir().join(format!("zcc_ir_{name}_{:x}.c", h.finish()));
        std::fs::write(&path, src).unwrap();
        let (t, l, f) = crate::preprocess::preprocess(path.to_str().unwrap(), &[], &[], &[]).unwrap();
        let ast = crate::parser::parse(&t, &l, &f).unwrap();
        let ir = lower(&ast);
        (ast, ir)
    }
    pub(crate) fn run(name: &str, src: &str, entry: &str, args: &[i64]) -> i64 {
        let (ast, ir) = compile(name, src);
        for f in &ir {
            verify(f).unwrap_or_else(|e| panic!("verify {}: {e}", f.name));
        }
        interp(&ast.tt, &ir, entry, args).unwrap()
    }

    // ─────────────────────────────────────────────────────────────────────────
    // equiv — the pass-proof GATE (commuting-square / mechanical translation-validation).
    // The governing invariant that REPLACES the LOC ceiling: a pass P is correct ⟺
    // ∀input, ⟦A⟧(input) = ⟦P(A)⟧(input). We do not TRUST by reasoning — we MEASURE with interp.
    // ─────────────────────────────────────────────────────────────────────────

    /// The input battery for differential interp. arity≤2: **EXHAUST** the small
    /// domain [-6,6]^n (exhaustive → complete for the small value-space boundary) +
    /// the INT_MAX/MIN boundary on each coordinate. arity≥3: a deterministic LCG of
    /// 256 vectors (no Date/random — deterministic so it can resume).
    pub(crate) fn battery(arity: usize) -> Vec<Vec<i64>> {
        let bound: [i64; 8] = [0, 1, -1, i32::MAX as i64, i32::MIN as i64, 255, -256, 1000003];
        if arity == 0 {
            return vec![vec![]];
        }
        let mut out: Vec<Vec<i64>> = Vec::new();
        if arity <= 2 {
            let small: Vec<i64> = (-6..=6).collect();
            if arity == 1 {
                for &x in &small {
                    out.push(vec![x]);
                }
                for &x in &bound {
                    out.push(vec![x]);
                }
            } else {
                for &x in &small {
                    for &y in &small {
                        out.push(vec![x, y]);
                    }
                }
                for &b in &bound {
                    out.push(vec![b, 1]); // the other coordinate = 1 (avoid 0 swallowing multiplication)
                    out.push(vec![1, b]);
                    out.push(vec![b, b]);
                }
            }
            return out;
        }
        let mut s: u64 = 0x2545F4914F6CDD1D;
        for _ in 0..256 {
            let mut v = Vec::with_capacity(arity);
            for _ in 0..arity {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                v.push((s >> 33) as i64 - (1 << 30));
            }
            out.push(v);
        }
        out
    }

    /// ⟦A⟧ ≡ ⟦B⟧ on `entry`? Observable = the RETURN value. UB convention (a diff at
    /// UB is meaningless): before Err ⟹ the input is outside the space (UB/opaque) →
    /// SKIP; before Ok ∧ after Err ⟹ the pass broke a well-defined case → FAIL; both
    /// Ok ⟹ compare. Anti-vacuous: if NO input can be compared (a wholly opaque/UB
    /// function) → Err (forbidding a vacuous "passing" pass).
    pub(crate) fn equiv(tt: &TyTab, a: &[IrFunc], b: &[IrFunc], entry: &str) -> Result<(), String> {
        let fa = a.iter().find(|f| f.name == entry).ok_or("equiv: missing entry in A")?;
        let arity = fa.params.len();
        let mut compared = 0u32;
        for input in battery(arity) {
            match (interp(tt, a, entry, &input), interp(tt, b, entry, &input)) {
                (Ok(x), Ok(y)) => {
                    if x != y {
                        return Err(format!("equiv BROKEN: {entry}{input:?} → A={x} B={y}"));
                    }
                    compared += 1;
                }
                (Ok(x), Err(e)) => {
                    return Err(format!("equiv: pass turned well-defined→error: {entry}{input:?} A={x} B_err={e}"));
                }
                (Err(_), _) => {} // before is outside the space → skip
            }
        }
        if compared == 0 {
            return Err(format!("equiv VACUOUS: {entry} no input could be interpreted — the pass is unproven"));
        }
        Ok(())
    }

    // Self-proof of equiv (validating the validation tool itself — the clean-input
    // rule): identity must be equiv; a SINGLE mutated Op must be caught. If this test
    // is red, every passing verdict is worthless.
    #[test]
    fn equiv_selfproof() {
        let (ast, ir) = compile("eqv", "int f(int a,int b){return a*b + a - 7;}");
        equiv(&ast.tt, &ir, &ir, "f").expect("identity must be equiv");
        let mut bad = ir.clone();
        let mut mutated = false;
        'outer: for f in bad.iter_mut().filter(|f| f.name == "f") {
            for blk in f.blocks.iter_mut() {
                for inst in blk.insts.iter_mut() {
                    if let Inst::Bin(_, op @ Op::Mul, ..) = inst {
                        *op = Op::Add; // Mul → Add: a semantic mutation
                        mutated = true;
                        break 'outer;
                    }
                }
            }
        }
        assert!(mutated, "there must be a Bin(Mul) to mutate");
        assert!(equiv(&ast.tt, &ir, &bad, "f").is_err(), "the Mul→Add mutation must be caught by equiv");
    }

    #[test]
    fn lower_arith() {
        assert_eq!(run("arith", "int f(int a,int b){return a*b+7;}", "f", &[6, 7]), 49);
    }
    #[test]
    fn lower_if() {
        let s = "int mx(int a,int b){if(a>b)return a;return b;}";
        assert_eq!(run("if", s, "mx", &[3, 9]), 9);
        assert_eq!(run("if", s, "mx", &[9, 3]), 9);
    }
    #[test]
    fn lower_struct_assign() {
        // q = p is a COPY (Inst::Memcpy, CORE) — mutating q does NOT touch p. interp
        // can execute it ⟺ struct-assign is already CORE (an Opaque would return Err → panic).
        // f(3,4): p={3,4}; q=p; q.x+=100 ⟹ 3*1000+4*10+103 = 3143.
        let s = "struct P{int x,y;};int f(int a,int b){struct P p,q;p.x=a;p.y=b;q=p;q.x=q.x+100;return p.x*1000+p.y*10+q.x;}";
        assert_eq!(run("sa", s, "f", &[3, 4]), 3143);
    }
    #[test]
    fn lower_for() {
        let s = "int sum(int n){int s=0;int i;for(i=1;i<=n;i=i+1)s=s+i;return s;}";
        assert_eq!(run("for", s, "sum", &[5]), 15);
    }
    #[test]
    fn lower_while_fib() {
        let s = "int fib(int n){int a=0,b=1,i=0;while(i<n){int t=a+b;a=b;b=t;i=i+1;}return a;}";
        assert_eq!(run("fib", s, "fib", &[10]), 55);
    }
    #[test]
    fn lower_ptr() {
        let s = "int viadr(int x){int y=x;int*p=&y;*p=*p+3;return y;}";
        assert_eq!(run("ptr", s, "viadr", &[10]), 13);
    }
    #[test]
    fn lower_recursion() {
        assert_eq!(run("rec", "int fact(int n){if(n<=1)return 1;return n*fact(n-1);}", "fact", &[5]), 120);
    }

    // ── Theorem 5: the verifier CATCHES malformed IR (using an undefined temp + a stray block).
    #[test]
    fn verifier_rejects() {
        // uses t9 (never defined) → def-coverage broken
        let bad = mk(
            "bad",
            vec![INT],
            vec![],
            0,
            INT,
            vec![Block { insts: vec![], term: Term::Ret(Some(Val::Tmp(9))) }],
        );
        assert!(verify(&bad).is_err());
        // jumps to a nonexistent block → CFG ref-integrity broken
        let bad2 = mk(
            "bad2",
            vec![INT],
            vec![],
            0,
            INT,
            vec![Block { insts: vec![], term: Term::Jmp(7) }],
        );
        assert!(verify(&bad2).is_err());
    }
}
