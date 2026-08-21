// AST + type arena — the frontend/backend boundary of zcc.
// The frontend (lexer → parser) BUILDS these structures; the backend
// (codegen/<target>) only READS them. The two layers do not import each other;
// all exchange passes through this file.
// Nodes reference one another by NodeId(u32) into the arena Vec<Node>, not by
// Box/reference.
// Layout (size/align) lives here because zcc locks the LP64 ABI family (arm64 as
// well as x86_64: int=4, char=1, long=ptr=8); a future ILP32 target would
// parameterize TyTab.
//
// Runtime value convention (the parser ↔ codegen contract): every scalar value
// lives in a 64-bit "canonical" register — integers are sign/zero-extended
// according to their type, float/double retain the f64 BIT PATTERN (a float is
// widened to double as soon as it is loaded). As a result, default argument
// promotion (char/short→int, float→double) is a no-op.

pub type NodeId = u32;
pub type TypeId = u32;

#[derive(Clone, Copy)]
pub enum Ty {
    Void,
    Char, // the SIGNED char (only via explicit "signed char"); plain `char` is unsigned on AArch64 Linux → UChar
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Long,
    ULong,
    Float,
    Double, // long double = double on arm64 Darwin
    // C99: long double on ELF (AAPCS64 Linux = binary128). zcc convention:
    // MEMORY is true 16/16 binary128 (psABI-correct — struct/global/va-slot match
    // gcc), the REGISTER stays canonical f64; widening/narrowing happens at
    // load/store/ABI boundaries via __extenddftf2/__trunctfdf2 (libgcc).
    // Arithmetic runs at double — float.h declares LDBL_MANT_DIG 53, so this is
    // self-consistent under C99. Darwin does NOT produce this type (the parser
    // maps long double → DOUBLE, which is the native Apple ABI).
    LDouble,
    Ptr(TypeId),
    Array(TypeId, u64),
    Struct(u32), // index into TyTab.structs; unions live here too (they differ only at offset construction)
    Func(u32),   // index into TyTab.fns
    Bitfield(TypeId, u32, u32), // (container type, bit offset within the unit, bit width)
    Bool,        // _Bool: store/cast normalizes to 0/1
}
pub const VOID: TypeId = 0;
pub const CHAR: TypeId = 1;
pub const UCHAR: TypeId = 2;
pub const SHORT: TypeId = 3;
pub const USHORT: TypeId = 4;
pub const INT: TypeId = 5;
pub const UINT: TypeId = 6;
pub const LONG: TypeId = 7;
pub const ULONG: TypeId = 8;
pub const FLOAT: TypeId = 9;
pub const DOUBLE: TypeId = 10;
pub const BOOL: TypeId = 11;
pub const LDOUBLE: TypeId = 12;

#[derive(Clone)]
pub struct StructDef {
    pub members: Vec<(String, TypeId, u32)>, // (name, type, offset)
    pub size: u32,
    pub align: u32,
    pub is_union: bool,
}

#[derive(Clone)]
pub struct FnSig {
    pub ret: TypeId,
    pub params: Vec<TypeId>,
    pub pnames: Vec<String>, // parameter names (empty if abstract); old-style: name only
    pub variadic: bool,
    pub oldstyle: bool, // "()" or ident-list: calls are unchecked, all args are "named"
}

pub struct TyTab {
    pub tys: Vec<Ty>,
    pub structs: Vec<StructDef>,
    pub fns: Vec<FnSig>,
}

impl TyTab {
    pub fn new() -> Self {
        TyTab {
            // order matches the VOID..DOUBLE constants above
            tys: vec![
                Ty::Void,
                Ty::Char,
                Ty::UChar,
                Ty::Short,
                Ty::UShort,
                Ty::Int,
                Ty::UInt,
                Ty::Long,
                Ty::ULong,
                Ty::Float,
                Ty::Double,
                Ty::Bool,
                Ty::LDouble,
            ],
            structs: Vec::new(),
            fns: Vec::new(),
        }
    }
    pub fn size(&self, t: TypeId) -> u32 {
        match self.tys[t as usize] {
            Ty::Void => 1, // EXT(gcc): void* arithmetic; sizeof(void) is invalid under C89
            Ty::Char | Ty::UChar | Ty::Bool => 1,
            Ty::Short | Ty::UShort => 2,
            Ty::Int | Ty::UInt | Ty::Float => 4,
            Ty::Long | Ty::ULong | Ty::Double | Ty::Ptr(_) | Ty::Func(_) => 8,
            Ty::LDouble => 16,
            Ty::Array(e, n) => self.size(e).wrapping_mul(n as u32),
            Ty::Struct(s) => self.structs[s as usize].size,
            Ty::Bitfield(b, ..) => self.size(b),
        }
    }
    // sizeof needs u64: an array declared in sizeof(int[2^33][...]) is valid even
    // though no object of that size can be allocated (actual layout still uses
    // size() u32)
    pub fn size64(&self, t: TypeId) -> u64 {
        match self.tys[t as usize] {
            Ty::Array(e, n) => self.size64(e) * n,
            _ => self.size(t) as u64,
        }
    }
    pub fn align(&self, t: TypeId) -> u32 {
        match self.tys[t as usize] {
            Ty::Array(e, _) => self.align(e),
            Ty::Struct(s) => self.structs[s as usize].align,
            _ => self.size(t),
        }
    }
    // gcc DATA_ALIGNMENT/LOCAL_ALIGNMENT (aarch64): an aggregate (array/struct)
    // raises its alignment to at least 8 (BITS_PER_WORD) to speed up access — zcc
    // follows for drop-in parity: static/stack arrays land as gcc does (measured:
    // gcc aligns char[2..64] to 8). Scalars keep their natural alignment.
    pub fn data_align(&self, t: TypeId) -> u32 {
        match self.tys[t as usize] {
            Ty::Array(..) | Ty::Struct(_) => self.align(t).max(8),
            _ => self.align(t),
        }
    }
    pub fn pointee(&self, t: TypeId) -> Option<TypeId> {
        match self.tys[t as usize] {
            Ty::Ptr(p) | Ty::Array(p, _) => Some(p),
            _ => None,
        }
    }
    pub fn is_float(&self, t: TypeId) -> bool {
        matches!(self.tys[t as usize], Ty::Float | Ty::Double | Ty::LDouble)
    }
    pub fn is_unsigned(&self, t: TypeId) -> bool {
        match self.tys[t as usize] {
            Ty::UChar | Ty::UShort | Ty::UInt | Ty::ULong | Ty::Ptr(_) | Ty::Bool => true,
            Ty::Bitfield(b, ..) => self.is_unsigned(b),
            _ => false,
        }
    }
    pub fn is_integer(&self, t: TypeId) -> bool {
        matches!(
            self.tys[t as usize],
            Ty::Char
                | Ty::UChar
                | Ty::Short
                | Ty::UShort
                | Ty::Int
                | Ty::UInt
                | Ty::Long
                | Ty::ULong
                | Ty::Bitfield(..)
                | Ty::Bool
        )
    }
    // HFA (AAPCS64): a struct whose members are all float or all double, 1-4 of
    // them → passed/returned in v0-v7 instead of GPRs/indirectly. Returns
    // (is double, member count).
    pub fn hfa(&self, t: TypeId) -> Option<(bool, u32)> {
        let Ty::Struct(si) = self.tys[t as usize] else {
            return None;
        };
        let sd = &self.structs[si as usize];
        if sd.is_union || sd.members.is_empty() || sd.members.len() > 4 {
            return None;
        }
        let dbl = matches!(self.tys[sd.members[0].1 as usize], Ty::Double);
        for &(_, mt, _) in &sd.members {
            match self.tys[mt as usize] {
                Ty::Float if !dbl => {}
                Ty::Double if dbl => {}
                _ => return None,
            }
        }
        Some((dbl, sd.members.len() as u32))
    }
    // FnSig of a callable value: a function or a function pointer
    pub fn fnsig(&self, t: TypeId) -> Option<&FnSig> {
        match self.tys[t as usize] {
            Ty::Func(i) => Some(&self.fns[i as usize]),
            Ty::Ptr(p) => match self.tys[p as usize] {
                Ty::Func(i) => Some(&self.fns[i as usize]),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn add(&mut self, ty: Ty) -> TypeId {
        self.tys.push(ty);
        (self.tys.len() - 1) as TypeId
    }
    pub fn ptr_to(&mut self, t: TypeId) -> TypeId {
        self.add(Ty::Ptr(t))
    }
}

// EXT(gcc): the __sync_* atomic builtin family — the name table is in ext.rs,
// codegen emits an LL/SC ldaxr/stlxr loop. It lives in ast.rs because this is the
// parser ↔ codegen boundary.
#[derive(Clone, Copy, Debug)]
pub enum SyncOp {
    FetchAdd, // __sync_fetch_and_add — returns the OLD value
    AddFetch, // __sync_add_and_fetch — returns the NEW value
    FetchSub,
    SubFetch,
    ValCas,  // __sync_val_compare_and_swap — returns the old value
    BoolCas, // __sync_bool_compare_and_swap — returns 1/0
    FetchAnd, // __sync_fetch_and_and — postgres18 atomics/generic-gcc.h
    FetchOr,  // __sync_fetch_and_or
    FetchXor, // __sync_fetch_and_xor
    TestSet, // __sync_lock_test_and_set — atomic exchange, returns the old value
    Release, // __sync_lock_release — writes 0 with a release barrier
    Barrier, // __sync_synchronize — dmb ish
}

pub enum Node {
    Num(i64),
    FNum(f64),
    Var(u32),               // local offset below the frame pointer
    GVar(u32),              // index into Ast.globals
    Member(NodeId, u32),    // base address + offset; type = member type
    Assign(NodeId, NodeId), // lvalue = expr
    Addr(NodeId),
    Deref(NodeId),
    Neg(NodeId),
    Cast(NodeId), // target type = this node's own type in Ast.types
    Bin(&'static str, NodeId, NodeId), // op = the punctuator itself: "+" "<=" ...
    Cond(NodeId, NodeId, NodeId), // ?: — && || ! also desugar to this/Bin
    Comma(NodeId, NodeId),
    Post(&'static str, NodeId, i64), // x++/x--: op "+"/"-", lvalue, delta (1 | sizeof pointee)
    Ret(Option<NodeId>),
    If(NodeId, NodeId, Option<NodeId>),
    While(NodeId, NodeId),
    For(Option<NodeId>, Option<NodeId>, Option<NodeId>, NodeId),
    Do(NodeId, NodeId), // body, cond
    // cond, body, (case value → NodeId of the Case), default Case. The target
    // label of a case is "LC{node id of the Case}" — the arena id is unique, so
    // no separate allocation is needed. cases = (lo, hi, Case-id); a single value
    // ⟺ lo==hi. EXT(gcc): case lo...hi.
    Switch(NodeId, NodeId, Vec<(i64, i64, NodeId)>, Option<NodeId>),
    Case(NodeId),
    Break,
    Continue,
    Goto(String),
    GotoPtr(NodeId),   // EXT(gcc): "goto *e" — branch through a value
    LabelAddr(String), // EXT(gcc): "&&label" — address of a label in the current function
    Label(String, NodeId),
    Block(Vec<NodeId>),
    Call(String, Vec<NodeId>, u32), // direct call by name; nreg = count of leading "named" args
    CallPtr(NodeId, Vec<NodeId>, u32), // call through a function pointer (blr)
    SRet(NodeId, u32, u32), // call returning a struct ≤16B: (call, local temp offset, size) — value = temp address
    Zero(NodeId, u32),      // write 0 over `size` bytes at an lvalue (zero-fill before an initializer)
    FunAddr(String),        // address of a function by name (via GOT)
    VaArea(u32),            // builtin __va_area__: x29 + offset (start of the variadic anonymous-arg area)
    // ELF/AAPCS: va_list = a 32-byte struct, not char* — two real builtins
    // (not produced on Darwin: the Apple branch of stdarg.h still goes via
    // macros + VaArea)
    VaStart(NodeId),       // __builtin_va_start(ap, last): fill 5 fields from the prologue
    // __builtin_va_arg(ap, T): select the GP/VR/stack area according to T; u32 =
    // local scratch offset (the parser allocates it when T is a struct — an HFA
    // from the VR save area is scattered one 16B q-slot per member, and the
    // backend gathers it into contiguous scratch)
    VaArg(NodeId, TypeId, u32),
    Sync(SyncOp, Vec<NodeId>, u32), // EXT(gcc): atomics; args = (ptr[, val[, val2]]), u32 = operand size 4|8
    // EXT(gcc): __builtin_{add,sub,mul}_overflow(a, b, res). op 0=+ 1=- 2=*.
    // ℤ semantics: compute a∘b at infinite precision, *res = truncate to
    // typeof(*res), return 1 if it is NOT representable. Signedness/width are read
    // from types[] at codegen.
    Overflow(u8, NodeId, NodeId, NodeId),
    // EXT(gcc): inline asm subset (xxhash/M14; musl/M17 widens the constraints).
    // GCC-style operand numbering: %0.. = outputs then inputs.
    Asm(String, Vec<AsmOp>),
    Alloca(NodeId), // dynamic allocation on the stack; the epilogue mov sp,x29 reclaims it
    Str(u32),
}

// EXT(gcc): one asm operand — the surface musl requires (syscall/atomic/math):
// "r"/"=r"/"+r"/"=&r" (GP pool x9..x15), "w" and its variants (FP pool v16..v22),
// "Q"/"m" (pass an ADDRESS, %k prints "[xN]"), "0".. (share a register with
// operand k), a pin from `register long v __asm__("x8")`. In ast.rs because it is
// the parser↔codegen boundary.
#[derive(Clone)]
pub struct AsmOp {
    pub e: NodeId,
    pub out: bool,        // belongs to the output section
    pub rw: bool,         // "+": load the value before the template
    pub mem: bool,        // "Q"/"m": the operand is a memory address
    pub fp: bool,         // "w": v-register
    pub tied: Option<u8>, // "k": share a register with operand k
    pub pin: Option<u8>,  // a hard-pinned GP register number (x8 → 8)
}

#[derive(Clone)]
pub enum GInit {
    None,
    Num(i64), // for a float type: this is the BIT PATTERN (f32/f64 depending on size)
    Str(u32),
    Addr(String, i64), // symbol + byte offset: int *p = &g.m; (prefix \x01 = literal name, no leading _)
    Diff(String, String), // EXT(gcc): difference of two symbols &&a - &&b (static jump table)
    StrOff(u32, i64),  // address into the middle of a string literal: "abc" + 1, &"X"[0]
    Bytes(Vec<u8>),    // char arr[] = "..." (already padded to size)
    List(Vec<(u32, u32, GInit)>), // flattened initializer: (offset, size, item)
}

pub struct Global {
    pub name: String,
    pub ty: TypeId,
    pub init: GInit,
    pub is_static: bool,
    pub is_extern: bool, // declaration only — no storage emitted
    // EXT(gcc): __thread — Mach-O TLS storage (__thread_data + the __thread_vars
    // descriptor), accessed via @TLVPPAGE + a call to tlv_get_addr
    pub is_tls: bool,
    // EXT(gcc): __attribute__((weak)) — .weak instead of .globl (musl libc.h `weak`)
    pub is_weak: bool,
}

pub struct Func {
    pub name: String,
    pub params: Vec<(u32, TypeId)>, // (offset, type) for spilling argument registers to slots
    pub frame: u32,                 // already rounded to 16
    pub body: NodeId,
    pub ret: TypeId,
    pub is_static: bool,
    // EXT(gcc): an inline definition (with no plain declaration, C99 6.7.4p7):
    // subject to DCE when unused (as clang does); when non-static it is emitted as
    // external weak so multiple TUs emitting it coalesce rather than duplicate
    pub is_inline: bool,
    pub is_weak: bool, // EXT(gcc): __attribute__((weak)) on a definition
    pub variadic: bool,
    pub sret: u32, // ≠0: the hidden x8-pointer slot (struct >16B returned indirectly)
    // C99 6.8.6.1: presence of a VLA → codegen must reset SP to base at a label (a
    // goto leaving the VLA scope must deallocate; otherwise a VLA inside a goto
    // loop overflows the stack)
    pub has_vla: bool,
}

// Sole target: AArch64 ELF (Linux). ABI-specific behavior (char signedness, va,
// long double width) is fixed to ELF in the frontend; adding a second target
// would move those parameters into TyTab (rather than scattering conditionals) +
// a new backend file + an IR-lowering branch.
pub struct Ast {
    pub nodes: Vec<Node>,
    pub types: Vec<TypeId>, // parallel to nodes
    pub tt: TyTab,
    pub funcs: Vec<Func>,
    pub globals: Vec<Global>,
    pub strs: Vec<Vec<u8>>, // string literals; the backend chooses the section/label
    // EXT(gcc): a top-level __asm__("...") — emitted verbatim (musl crt_arch.h
    // defines _start with bare asm)
    pub raw_asm: Vec<String>,
    // EXT(gcc): __attribute__((alias("old"))) — (new name, old name, weak?);
    // musl weak_alias() weaves its entire public-symbol surface out of this
    pub aliases: Vec<(String, String, bool)>,
    // -fPIC (set by the driver after parse): ELF .so — a non-static global is
    // preemptible, so the backend must go through the GOT instead of a direct adrp
    pub pic: bool,
    // EXT(gcc): a weak prototype/extern — the TU emits .weak so the undefined
    // reference is weak
    pub weak_decls: Vec<String>,
    // C99 6.7.3: the TU contains volatile. The IR does not model volatile (the
    // qualifier is dropped), so ⟦·⟧-preservation of the opt pass is only proven
    // for volatile-free input → this flag disables the (default-on) optimizer, so
    // a volatile function runs WITHIN the fragment the theorem covers. Sound by
    // construction.
    pub has_volatile: bool,
}
