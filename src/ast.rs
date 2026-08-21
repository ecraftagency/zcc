// AST + type arena — ĐƯỜNG BIÊN frontend/backend của zcc.
// Frontend (lexer → parser) DỰNG các cấu trúc này; backend (codegen/<target>) chỉ ĐỌC.
// Hai tầng không import lẫn nhau — mọi trao đổi đi qua file này.
// Node tham chiếu nhau bằng NodeId(u32) vào arena Vec<Node>, không Box/reference.
// Layout (size/align) đặt ở đây vì zcc lock họ ABI LP64 (arm64 lẫn x86_64:
// int=4, char=1, long=ptr=8); mai này cần target ILP32 thì tham số hóa TyTab.
//
// Quy ước giá trị runtime (hợp đồng parser ↔ codegen): mọi giá trị scalar sống
// trong thanh ghi 64-bit "canonical" — số nguyên sign/zero-extend đúng theo kiểu,
// float/double giữ BIT PATTERN f64 (float được nâng lên double ngay khi load).
// Nhờ vậy default argument promotion (char/short→int, float→double) là no-op.

pub type NodeId = u32;
pub type TypeId = u32;

#[derive(Clone, Copy)]
pub enum Ty {
    Void,
    Char, // signed trên Darwin; "signed char" cũng map vào đây
    UChar,
    Short,
    UShort,
    Int,
    UInt,
    Long,
    ULong,
    Float,
    Double, // long double = double trên arm64 Darwin
    // C99: long double trên ELF (AAPCS64 Linux = binary128). Quy ước zcc:
    // MEMORY là binary128 thật 16/16 (psABI đúng — struct/global/va-slot khớp
    // gcc), REGISTER vẫn canonical f64; nới/hạ tại load/store/biên ABI bằng
    // __extenddftf2/__trunctfdf2 (libgcc). Số học chạy double — float.h khai
    // LDBL_MANT_DIG 53 nên tự nhất quán C99. Darwin KHÔNG sinh type này
    // (parser map long double → DOUBLE, ABI Apple vốn thế).
    LDouble,
    Ptr(TypeId),
    Array(TypeId, u64),
    Struct(u32), // index vào TyTab.structs; union cũng nằm đây (khác nhau lúc dựng offset)
    Func(u32),   // index vào TyTab.fns
    Bitfield(TypeId, u32, u32), // (kiểu chứa, bit offset trong đơn vị, độ rộng bit)
    Bool,        // _Bool: store/cast normalize về 0/1
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
    pub members: Vec<(String, TypeId, u32)>, // (tên, kiểu, offset)
    pub size: u32,
    pub align: u32,
    pub is_union: bool,
}

#[derive(Clone)]
pub struct FnSig {
    pub ret: TypeId,
    pub params: Vec<TypeId>,
    pub pnames: Vec<String>, // tên param (rỗng nếu abstract); old-style: chỉ có tên
    pub variadic: bool,
    pub oldstyle: bool, // "()" hoặc ident-list: gọi không kiểm tra, arg đều "đặt tên"
}

pub struct TyTab {
    pub tys: Vec<Ty>,
    pub structs: Vec<StructDef>,
    pub fns: Vec<FnSig>,
}

impl TyTab {
    pub fn new() -> Self {
        TyTab {
            // thứ tự khớp các hằng VOID..DOUBLE phía trên
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
            Ty::Void => 1, // cho void* arith kiểu GNU; sizeof(void) C89 vốn không hợp lệ
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
    // sizeof cần u64: mảng khai trong sizeof(int[2^33][...]) hợp lệ dù không
    // cấp phát được object cỡ đó (layout thật vẫn dùng size() u32)
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
    // gcc DATA_ALIGNMENT/LOCAL_ALIGNMENT (aarch64): aggregate (array/struct)
    // nâng align tối thiểu 8 (BITS_PER_WORD) để tăng tốc truy cập — zcc theo cho
    // drop-in parity: static/stack array land như gcc (đo: gcc cho char[2..64]
    // đều 8). Scalar giữ natural.
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
    // HFA (AAPCS64): struct mà mọi member cùng là float hoặc cùng double, 1-4 cái
    // → đi/về bằng v0-v7 thay vì GPR/gián tiếp. Trả (là double, số member).
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
    // FnSig của một giá trị gọi được: hàm hoặc con trỏ hàm
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

// EXT(gcc): họ builtin atomic __sync_* (M12) — bảng tên ở ext.rs, codegen phát
// vòng LL/SC ldaxr/stlxr. Nằm ở ast.rs vì đây là boundary parser ↔ codegen.
#[derive(Clone, Copy, Debug)]
pub enum SyncOp {
    FetchAdd, // __sync_fetch_and_add — trả giá trị CŨ
    AddFetch, // __sync_add_and_fetch — trả giá trị MỚI
    FetchSub,
    SubFetch,
    ValCas,  // __sync_val_compare_and_swap — trả giá trị cũ
    BoolCas, // __sync_bool_compare_and_swap — trả 1/0
    FetchAnd, // __sync_fetch_and_and — postgres18 atomics/generic-gcc.h
    FetchOr,  // __sync_fetch_and_or
    FetchXor, // __sync_fetch_and_xor
    TestSet, // __sync_lock_test_and_set — atomic exchange, trả giá trị cũ
    Release, // __sync_lock_release — ghi 0 với release barrier
    Barrier, // __sync_synchronize — dmb ish
}

pub enum Node {
    Num(i64),
    FNum(f64),
    Var(u32),               // offset local dưới frame pointer
    GVar(u32),              // index vào Ast.globals
    Member(NodeId, u32),    // địa chỉ base + offset; kiểu = kiểu member
    Assign(NodeId, NodeId), // lvalue = expr
    Addr(NodeId),
    Deref(NodeId),
    Neg(NodeId),
    Cast(NodeId), // kiểu đích = kiểu của chính node này trong Ast.types
    Bin(&'static str, NodeId, NodeId), // op = chính punct: "+" "<=" ...
    Cond(NodeId, NodeId, NodeId), // ?: — && || ! cũng desugar về đây/Bin
    Comma(NodeId, NodeId),
    Post(&'static str, NodeId, i64), // x++/x--: op "+"/"-", lvalue, delta (1 | sizeof pointee)
    Ret(Option<NodeId>),
    If(NodeId, NodeId, Option<NodeId>),
    While(NodeId, NodeId),
    For(Option<NodeId>, Option<NodeId>, Option<NodeId>, NodeId),
    Do(NodeId, NodeId), // body, cond
    // cond, body, (giá trị case → NodeId của Case), Case default. Label đích
    // của case = "LC{node id của Case}" — id arena duy nhất nên khỏi cấp phát.
    // cases = (lo, hi, Case-id); single value ⟺ lo==hi. EXT(gcc): case lo...hi.
    Switch(NodeId, NodeId, Vec<(i64, i64, NodeId)>, Option<NodeId>),
    Case(NodeId),
    Break,
    Continue,
    Goto(String),
    GotoPtr(NodeId),   // GNU "goto *e" — br qua giá trị
    LabelAddr(String), // GNU "&&label" — địa chỉ label trong hàm hiện tại
    Label(String, NodeId),
    Block(Vec<NodeId>),
    Call(String, Vec<NodeId>, u32), // gọi thẳng theo tên; nreg = số arg đầu "đặt tên"
    CallPtr(NodeId, Vec<NodeId>, u32), // gọi qua con trỏ hàm (blr)
    SRet(NodeId, u32, u32), // call trả struct ≤16B: (call, offset temp local, size) — giá trị = địa chỉ temp
    Zero(NodeId, u32),      // ghi 0 lên `size` byte tại lvalue (zero-fill trước initializer)
    FunAddr(String),        // địa chỉ hàm theo tên (qua GOT)
    VaArea(u32),            // builtin __va_area__: x29 + offset (đầu vùng arg vô danh variadic)
    // ELF/AAPCS: va_list = struct 32 byte, không phải char* — hai builtin thật
    // (Darwin không sinh: stdarg.h nhánh Apple vẫn đi đường macro + VaArea)
    VaStart(NodeId),       // __builtin_va_start(ap, last): điền 5 field từ prologue
    // __builtin_va_arg(ap, T): chọn vùng GP/VR/stack theo T; u32 = offset local
    // scratch (parser cấp khi T là struct — HFA từ VR save area nằm rải mỗi
    // member 1 q-slot 16B, backend gather về scratch liên tục)
    VaArg(NodeId, TypeId, u32),
    Sync(SyncOp, Vec<NodeId>, u32), // EXT(gcc): atomics; args = (ptr[, val[, val2]]), u32 = size operand 4|8
    // EXT(gcc): __builtin_{add,sub,mul}_overflow(a, b, res). op 0=+ 1=- 2=*.
    // Ngữ nghĩa ℤ: tính a∘b vô hạn hạng, *res = cắt về typeof(*res), trả 1 nếu
    // KHÔNG biểu diễn được. Signedness/width đọc từ types[] tại codegen.
    Overflow(u8, NodeId, NodeId, NodeId),
    // EXT(gcc): inline asm subset (xxhash/M14; musl/M17 nới constraint).
    // Đánh số operand kiểu GCC: %0.. = outputs rồi inputs.
    Asm(String, Vec<AsmOp>),
    Alloca(NodeId), // cấp phát động trên stack; epilogue mov sp,x29 tự thu hồi
    Str(u32),
}

// EXT(gcc): một operand asm — bề mặt musl đòi (syscall/atomic/math):
// "r"/"=r"/"+r"/"=&r" (pool GP x9..x15), "w" các biến thể (pool FP v16..v22),
// "Q"/"m" (truyền ĐỊA CHỈ, %k in "[xN]"), "0".. (chung reg với operand k),
// pin từ `register long v __asm__("x8")`. Ở ast.rs vì là boundary parser↔codegen.
#[derive(Clone)]
pub struct AsmOp {
    pub e: NodeId,
    pub out: bool,        // thuộc section output
    pub rw: bool,         // "+": nạp giá trị trước template
    pub mem: bool,        // "Q"/"m": operand là địa chỉ bộ nhớ
    pub fp: bool,         // "w": v-register
    pub tied: Option<u8>, // "k": dùng chung reg với operand k
    pub pin: Option<u8>,  // số reg GP ghim cứng (x8 → 8)
}

#[derive(Clone)]
pub enum GInit {
    None,
    Num(i64), // với kiểu float: đây là BIT PATTERN (f32/f64 theo size)
    Str(u32),
    Addr(String, i64), // symbol + offset byte: int *p = &g.m; (prefix \x01 = tên đủ, không thêm _)
    Diff(String, String), // hiệu 2 symbol: &&a - &&b (GNU, static jump table)
    StrOff(u32, i64),  // địa chỉ vào giữa string literal: "abc" + 1, &"X"[0]
    Bytes(Vec<u8>),    // char arr[] = "..." (đã pad đủ size)
    List(Vec<(u32, u32, GInit)>), // initializer phẳng hóa: (offset, size, item)
}

pub struct Global {
    pub name: String,
    pub ty: TypeId,
    pub init: GInit,
    pub is_static: bool,
    pub is_extern: bool, // chỉ khai báo — không phát storage
    // EXT(gcc): __thread — storage TLS Mach-O (__thread_data + descriptor
    // __thread_vars), access qua @TLVPPAGE + gọi tlv_get_addr
    pub is_tls: bool,
    // EXT(gcc): __attribute__((weak)) — .weak thay .globl (musl libc.h `weak`)
    pub is_weak: bool,
}

pub struct Func {
    pub name: String,
    pub params: Vec<(u32, TypeId)>, // (offset, kiểu) để spill thanh ghi arg vào slot
    pub frame: u32,                 // đã tròn 16
    pub body: NodeId,
    pub ret: TypeId,
    pub is_static: bool,
    // EXT(gcc): định nghĩa inline (không có declaration trần C99 6.7.4p7):
    // DCE khi không ai dùng (như clang); non-static thì phát external weak
    // để nhiều TU cùng phát vẫn coalesce, không duplicate
    pub is_inline: bool,
    pub is_weak: bool, // EXT(gcc): __attribute__((weak)) trên definition
    pub variadic: bool,
    pub sret: u32, // ≠0: slot giấu con trỏ x8 (trả struct >16B gián tiếp)
    // C99 6.8.6.1: có VLA → codegen phải reset SP về base tại label (goto rời
    // scope VLA phải dealloc; nếu không, VLA trong vòng lặp goto tràn stack)
    pub has_vla: bool,
}

// Target duy nhất: AArch64 ELF (Linux). Hành vi ABI-specific (char signedness,
// va, long double width) cố định ELF trong frontend; khi thêm target thứ 2 các
// tham số đó về TyTab (không rải điều kiện) + backend file mới + nhánh IR-lowering.
pub struct Ast {
    pub nodes: Vec<Node>,
    pub types: Vec<TypeId>, // song song với nodes
    pub tt: TyTab,
    pub funcs: Vec<Func>,
    pub globals: Vec<Global>,
    pub strs: Vec<Vec<u8>>, // string literal, backend tự chọn section/label
    // EXT(gcc): __asm__("...") cấp toàn cục — phát verbatim (musl crt_arch.h
    // định nghĩa _start bằng asm trần)
    pub raw_asm: Vec<String>,
    // EXT(gcc): __attribute__((alias("old"))) — (tên mới, tên cũ, weak?);
    // musl weak_alias() dệt toàn bộ bề mặt public symbol bằng món này
    pub aliases: Vec<(String, String, bool)>,
    // -fPIC (driver set sau parse): ELF .so — global non-static preemptible,
    // backend phải đi GOT thay adrp trực tiếp
    pub pic: bool,
    // EXT(gcc): prototype/extern mang weak — TU phát .weak để undef ref là weak
    pub weak_decls: Vec<String>,
}
