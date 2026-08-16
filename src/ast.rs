// AST + type arena — ĐƯỜNG BIÊN frontend/backend của zcc.
// Frontend (lexer → parser) DỰNG các cấu trúc này; backend (codegen/<target>) chỉ ĐỌC.
// Hai tầng không import lẫn nhau — mọi trao đổi đi qua file này.
// Node tham chiếu nhau bằng NodeId(u32) vào arena Vec<Node>, không Box/reference.
// Layout (size/align) đặt ở đây vì zcc lock họ ABI LP64 (arm64 lẫn x86_64:
// int=4, char=1, long=ptr=8); mai này cần target ILP32 thì tham số hóa TyTab.

pub type NodeId = u32;
pub type TypeId = u32;

#[derive(Clone, Copy)]
pub enum Ty {
    Int,
    Char,
    Long,
    Ptr(TypeId),
    Array(TypeId, u32),
    Struct(u32), // index vào TyTab.structs; union cũng nằm đây (khác nhau lúc dựng offset)
}
pub const INT: TypeId = 0;
pub const CHAR: TypeId = 1;
pub const LONG: TypeId = 2;

pub struct StructDef {
    pub members: Vec<(String, TypeId, u32)>, // (tên, kiểu, offset)
    pub size: u32,
    pub align: u32,
}

pub struct TyTab {
    pub tys: Vec<Ty>,
    pub structs: Vec<StructDef>,
}

impl TyTab {
    pub fn new() -> Self {
        // 3 slot đầu cố định khớp INT/CHAR/LONG
        TyTab { tys: vec![Ty::Int, Ty::Char, Ty::Long], structs: Vec::new() }
    }
    pub fn size(&self, t: TypeId) -> u32 {
        match self.tys[t as usize] {
            Ty::Int => 4,
            Ty::Char => 1,
            Ty::Long | Ty::Ptr(_) => 8,
            Ty::Array(e, n) => self.size(e) * n,
            Ty::Struct(s) => self.structs[s as usize].size,
        }
    }
    pub fn align(&self, t: TypeId) -> u32 {
        match self.tys[t as usize] {
            Ty::Array(e, _) => self.align(e),
            Ty::Struct(s) => self.structs[s as usize].align,
            _ => self.size(t),
        }
    }
    pub fn pointee(&self, t: TypeId) -> Option<TypeId> {
        match self.tys[t as usize] {
            Ty::Ptr(p) | Ty::Array(p, _) => Some(p),
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

pub enum Node {
    Num(i64),
    Var(u32),               // offset local dưới frame pointer
    GVar(u32),              // index vào Ast.globals
    Member(NodeId, u32),    // địa chỉ base + offset; kiểu = kiểu member
    Assign(NodeId, NodeId), // lvalue = expr
    Addr(NodeId),
    Deref(NodeId),
    Neg(NodeId),
    Bin(&'static str, NodeId, NodeId), // op = chính punct: "+" "<=" ...
    Ret(NodeId),
    If(NodeId, NodeId, Option<NodeId>),
    While(NodeId, NodeId),
    For(Option<NodeId>, Option<NodeId>, Option<NodeId>, NodeId),
    Block(Vec<NodeId>),
    Call(String, Vec<NodeId>, u32), // nreg: số arg đầu đi thanh ghi; phần sau (arg vô danh
    Str(u32),                       // của hàm variadic) đi theo luật variadic của target
}

pub enum GInit {
    None,
    Num(i64),
    Str(u32),
}

pub struct Global {
    pub name: String,
    pub ty: TypeId,
    pub init: GInit,
}

pub struct Func {
    pub name: String,
    pub params: Vec<(u32, u32)>, // (offset, size) để spill thanh ghi arg vào slot
    pub frame: u32,              // đã tròn 16
    pub body: NodeId,
}

pub struct Ast {
    pub nodes: Vec<Node>,
    pub types: Vec<TypeId>, // song song với nodes
    pub tt: TyTab,
    pub funcs: Vec<Func>,
    pub globals: Vec<Global>,
    pub strs: Vec<Vec<u8>>, // string literal, backend tự chọn section/label
}
