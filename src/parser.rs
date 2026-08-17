// Parser: &[Tok] → AST arena (Vec<Node> + NodeId u32) + type arena (TyTab).
// Grammar C89 (thiếu: initializer {..}, bitfield, struct by value — các vòng sau):
//   decl       = decl_specs (declarator ("=" init)? ("," declarator ...)*)? ";"
//              | decl_specs declarator [old-style decls] block   (funcdef)
//   decl_specs = (storage | qualifier | void/char/short/int/long/signed/unsigned/
//                 float/double | struct/union/enum spec | typedef-name)+
//   declarator = "*" quals* declarator | direct ("[" const? "]" | "(" params ")")*
//   direct     = ident | "(" declarator ")"
//   expr       = assign ("," assign)* ; assign = cond (assign-op assign)?
//   cond → lor → land → bitor → bitxor → bitand → eq → rel → shift → add → mul
//        → cast-expr = "(" typename ")" cast-expr | unary
//   postfix    = primary ("[" e "]" | "." id | "->" id | "(" args ")" | "++" | "--")*
// Chuyển đổi kiểu: parser chèn Node::Cast tại mọi điểm hội tụ (usual arithmetic
// conversions, gán, arg theo prototype, return) — codegen chỉ nhìn type để chọn lệnh.
use crate::ast::{
    Ast, FnSig, Func, GInit, Global, Node, NodeId, StructDef, Ty, TyTab, TypeId, BOOL, CHAR,
    DOUBLE, FLOAT, INT, LONG, SHORT, UCHAR, UINT, ULONG, USHORT, VOID,
};
use crate::lexer::{NumK, Tok};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
enum Storage {
    None,
    Typedef,
    Static,
    Extern,
}

#[derive(Clone, Copy)]
enum Vloc {
    Stack(u32),
    Glob(u32),
    Fn, // prototype trong block shadow biến ngoài — tra tiếp self.fns
}

// cây initializer: expr / danh sách {..} / string literal
#[derive(Clone)]
enum Init {
    E(NodeId),
    L(Vec<(Desig, Init)>),
    S(Vec<u8>, bool), // bool = wide L".."
}
// designator C99 ([i] = / .m =) — clang chấp nhận trong -std=c89
#[derive(Clone)]
enum Desig {
    No,
    Idx(u32),
    Rng(u32, u32), // GNU [lo ... hi]
    Mem(String),
}
// leaf sau khi đổ phẳng initializer: expr hoặc chuỗi byte (string vào mảng char)
enum FlatItem {
    E(NodeId),
    B(Vec<u8>),
}
type ItInit = std::iter::Peekable<std::vec::IntoIter<(Desig, Init)>>;

struct P<'a> {
    toks: &'a [Tok],
    pos: usize,
    nodes: Vec<Node>,
    types: Vec<TypeId>,
    tt: TyTab,
    locals: Vec<(String, TypeId, Vloc)>, // scope = truncate khi đóng block
    cur_off: u32,
    globals: Vec<Global>,
    strs: Vec<Vec<u8>>,
    fns: HashMap<String, TypeId>, // tên hàm → TypeId (Ty::Func)
    static_fns: std::collections::HashSet<String>, // đã khai static → definition giữ internal linkage
    tags: HashMap<String, TypeId>,
    typedefs: HashMap<String, TypeId>,
    enums: HashMap<String, i64>,
    enum_tags: HashMap<String, TypeId>, // tag → underlying (INT | UINT, khớp clang)
    switches: Vec<(Vec<(i64, NodeId)>, Option<NodeId>)>,
    fret: TypeId, // kiểu trả về của hàm đang parse
    va_off: u32,  // offset từ x29 đến vùng arg vô danh (16 + 8*named-stack-params)
    in_fn: bool,  // đang trong body hàm (compound literal: local vs global ẩn)
    attr_aligned: Option<u32>, // aligned(n) lơ lửng từ decl_specs (member: pr23467)
    fname: String, // tên hàm đang parse (label symbol cho &&label trong static init)
}

type R = Result<NodeId, String>;

// trải một GInit (offset gốc off) vào list phẳng (offset, size, item)
fn splice_ginit(off: u32, sz: u32, init: GInit, list: &mut Vec<(u32, u32, GInit)>) {
    match init {
        GInit::List(items) => {
            for (o2, s2, it) in items {
                splice_ginit(off + o2, s2, it, list);
            }
        }
        GInit::None => {}
        other => list.push((off, sz, other)),
    }
}

const ASSIGN_OPS: [(&str, &str); 11] = [
    ("=", ""),
    ("+=", "+"),
    ("-=", "-"),
    ("*=", "*"),
    ("/=", "/"),
    ("%=", "%"),
    ("<<=", "<<"),
    (">>=", ">>"),
    ("&=", "&"),
    ("|=", "|"),
    ("^=", "^"),
];

const TYPE_WORDS: [&str; 21] = [
    "void", "char", "short", "int", "long", "signed", "unsigned", "float", "double", "struct",
    "union", "enum", "const", "volatile", "_Bool", "__const", "__volatile", "__signed",
    "__signed__", "__typeof__", "__typeof", // EXT(gcc): typeof trần KHÔNG nhận (va tên biến C89)
];

impl P<'_> {
    fn push(&mut self, n: Node, t: TypeId) -> NodeId {
        self.nodes.push(n);
        self.types.push(t);
        (self.nodes.len() - 1) as NodeId
    }
    fn ty(&self, n: NodeId) -> TypeId {
        self.types[n as usize]
    }
    fn eat(&mut self, want: &Tok) -> bool {
        let hit = self.toks.get(self.pos) == Some(want);
        self.pos += hit as usize;
        hit
    }
    fn peek(&self, p: &'static str) -> bool {
        self.toks.get(self.pos) == Some(&Tok::Punct(p))
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        let hit = matches!(self.toks.get(self.pos), Some(Tok::Ident(t)) if t == kw);
        self.pos += hit as usize;
        hit
    }
    fn expect(&mut self, want: Tok) -> Result<(), String> {
        if self.eat(&want) {
            Ok(())
        } else {
            Err(format!("cần {:?}, gặp {:?}", want, self.toks.get(self.pos)))
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        if let Some(Tok::Ident(n)) = self.toks.get(self.pos) {
            self.pos += 1;
            Ok(n.clone())
        } else {
            Err(format!("cần tên, gặp {:?}", self.toks.get(self.pos)))
        }
    }
    fn is_type_word(&self, n: &str) -> bool {
        TYPE_WORDS.contains(&n)
            || self.typedefs.contains_key(n)
            || ["typedef", "static", "extern", "auto", "register"].contains(&n)
    }
    // keyword cứng — typedef-name KHÔNG tính (vẫn được làm tên declarator/member)
    fn is_keyword(&self, n: &str) -> bool {
        TYPE_WORDS.contains(&n) || ["typedef", "static", "extern", "auto", "register"].contains(&n)
    }

    // ---- hằng ----
    fn const_expr(&mut self) -> Result<i64, String> {
        let e = self.cond_expr()?;
        self.fold(e)
    }
    fn fold(&self, id: NodeId) -> Result<i64, String> {
        match &self.nodes[id as usize] {
            Node::Num(v) => Ok(*v),
            Node::Neg(e) => Ok(self.fold(*e)?.wrapping_neg()),
            // &((T *)K)->m / &((T *)K)->a[i]: offsetof cổ điển — hằng nguyên
            Node::Addr(x) => {
                let mut e = *x;
                let mut off = 0i64;
                loop {
                    match &self.nodes[e as usize] {
                        Node::Cast(i) => e = *i,
                        Node::Member(b, o) => {
                            off += *o as i64;
                            e = *b;
                        }
                        Node::Deref(p) => return Ok(self.fold(*p)? + off),
                        _ => return Err("cần biểu thức hằng".into()),
                    }
                }
            }
            Node::Cast(e) => {
                // thu hẹp theo kiểu đích để (char)300 v.v. đúng
                let v = self.fold(*e)?;
                let t = self.ty(id);
                Ok(match self.tt.size(t) {
                    1 if self.tt.is_unsigned(t) => v as u8 as i64,
                    1 => v as i8 as i64,
                    2 if self.tt.is_unsigned(t) => v as u16 as i64,
                    2 => v as i16 as i64,
                    4 if self.tt.is_unsigned(t) => v as u32 as i64,
                    4 => v as i32 as i64,
                    _ => v,
                })
            }
            Node::Cond(c, t, e) => {
                if self.fold(*c)? != 0 {
                    self.fold(*t)
                } else {
                    self.fold(*e)
                }
            }
            // cả 2 vế phải fold được — Comma có side effect không phải hằng
            Node::Comma(l, r) => {
                self.fold(*l)?;
                self.fold(*r)
            }
            Node::Bin(op, l, r) => {
                let u = self.tt.is_unsigned(self.ty(id))
                    || self.tt.is_unsigned(self.ty(*l))
                    || self.tt.is_unsigned(self.ty(*r));
                let (l, r) = (self.fold(*l)?, self.fold(*r)?);
                Ok(match *op {
                    "+" => l.wrapping_add(r),
                    "-" => l.wrapping_sub(r),
                    "*" => l.wrapping_mul(r),
                    "/" | "%" if r == 0 => return Err("chia 0 trong hằng".into()),
                    "/" if u => (l as u64 / r as u64) as i64,
                    "/" => l.wrapping_div(r),
                    "%" if u => (l as u64 % r as u64) as i64,
                    "%" => l.wrapping_rem(r),
                    "&" => l & r,
                    "|" => l | r,
                    "^" => l ^ r,
                    "<<" => l.wrapping_shl(r as u32),
                    ">>" if u => ((l as u64).wrapping_shr(r as u32)) as i64,
                    ">>" => l.wrapping_shr(r as u32),
                    "==" => (l == r) as i64,
                    "!=" => (l != r) as i64,
                    "<" if u => ((l as u64) < r as u64) as i64,
                    "<" => (l < r) as i64,
                    "<=" if u => ((l as u64) <= r as u64) as i64,
                    "<=" => (l <= r) as i64,
                    ">" if u => ((l as u64) > r as u64) as i64,
                    ">" => (l > r) as i64,
                    ">=" if u => ((l as u64) >= r as u64) as i64,
                    ">=" => (l >= r) as i64,
                    _ => return Err("op không dùng được trong hằng".into()),
                })
            }
            _ => Err("cần biểu thức hằng".into()),
        }
    }
    // hằng thực cho global init kiểu float
    fn fold_f(&self, id: NodeId) -> Result<f64, String> {
        match &self.nodes[id as usize] {
            Node::FNum(v) => Ok(*v),
            // unsigned 64-bit: 9223372036854775810ul phải thành 9.2e18 chứ không âm
            Node::Num(v) => {
                Ok(if self.tt.is_unsigned(self.ty(id)) { *v as u64 as f64 } else { *v as f64 })
            }
            Node::Neg(e) => Ok(-self.fold_f(*e)?),
            Node::Cast(e) => self.fold_f(*e),
            Node::Bin(op, l, r) => {
                let (l, r) = (self.fold_f(*l)?, self.fold_f(*r)?);
                Ok(match *op {
                    "+" => l + r,
                    "-" => l - r,
                    "*" => l * r,
                    "/" => l / r,
                    _ => return Err("op không dùng được trong hằng thực".into()),
                })
            }
            _ => Err("cần hằng thực".into()),
        }
    }

    // ---- kiểu ----
    // None = token hiện tại không mở đầu một declaration
    fn decl_specs(&mut self) -> Result<Option<(TypeId, Storage)>, String> {
        self.attr_aligned = None; // của declaration trước, không lây
        let mut storage = Storage::None;
        let (mut base, mut direct) = (None::<&str>, None::<TypeId>);
        let (mut uns, mut sgn, mut short, mut longs, mut any) = (false, false, false, 0u32, false);
        loop {
            let n = match self.toks.get(self.pos) {
                Some(Tok::Ident(n)) => n.as_str(),
                _ => break,
            };
            match n {
                "const" | "volatile" | "auto" | "register" | "inline" | "__inline"
                | "__inline__" | "restrict" | "__restrict" | "__restrict__"
                | "__extension__" | "__volatile" | "__volatile__" | "__const" | "__const__"
                | "_Noreturn" => {}
                "__attribute__" | "__asm__" | "__asm" => {
                    let (pk, al) = self.skip_attrs()?;
                    if pk || al.is_some() {
                        // "struct {...} __attribute__((packed / aligned))" hậu tố
                        if let Some(t) = direct {
                            self.repack(t, pk, al);
                        } else if let Some(a) = al {
                            // "int __attribute__((aligned(8))) x" — treo lại cho
                            // declarator/member dùng (pr23467)
                            self.attr_aligned = Some(self.attr_aligned.unwrap_or(1).max(a));
                        }
                    }
                    any = true;
                    continue;
                }
                "typedef" => storage = Storage::Typedef,
                "static" => storage = Storage::Static,
                "extern" => storage = Storage::Extern,
                "void" | "char" | "int" | "float" | "double" | "_Bool" => base = Some(n),
                "short" => short = true,
                "long" => longs += 1,
                "signed" | "__signed" | "__signed__" => sgn = true,
                "unsigned" => uns = true,
                "struct" | "union" => {
                    self.pos += 1;
                    direct = Some(self.struct_union(n == "union")?);
                    any = true;
                    continue;
                }
                "enum" => {
                    self.pos += 1;
                    direct = Some(self.enum_spec()?);
                    any = true;
                    continue;
                }
                // EXT(gcc): __typeof__(expr | typename) đứng như type-specifier
                "__typeof__" | "__typeof" => {
                    self.pos += 1;
                    self.expect(Tok::Punct("("))?;
                    let t = if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n))
                    {
                        self.typename()?
                    } else {
                        let e = self.expr()?; // node thành rác arena, như sizeof
                        self.ty(e)
                    };
                    self.expect(Tok::Punct(")"))?;
                    direct = Some(t);
                    any = true;
                    continue;
                }
                _ => {
                    // typedef-name: chỉ khi chưa có kiểu nào khác
                    if base.is_none() && direct.is_none() && !uns && !sgn && !short && longs == 0 {
                        if let Some(&t) = self.typedefs.get(n) {
                            self.pos += 1;
                            direct = Some(t);
                            any = true;
                            continue;
                        }
                    }
                    break;
                }
            }
            self.pos += 1;
            any = true;
        }
        let n = n_hack(base); // tránh borrow; xem dưới
        if !any {
            return Ok(None);
        }
        if let Some(t) = direct {
            return Ok(Some((t, storage)));
        }
        let t = match n {
            "void" => VOID,
            "char" => {
                if uns {
                    UCHAR
                } else {
                    CHAR
                }
            }
            "float" => FLOAT,
            "double" => DOUBLE, // long double = double
            "_Bool" => BOOL,
            _ => {
                // họ int (kể cả không có "int" tường minh)
                if short {
                    if uns {
                        USHORT
                    } else {
                        SHORT
                    }
                } else if longs > 0 {
                    if uns {
                        ULONG
                    } else {
                        LONG
                    }
                } else {
                    if uns {
                        UINT
                    } else {
                        INT
                    }
                }
            }
        };
        Ok(Some((t, storage)))
    }
    fn struct_union(&mut self, is_union: bool) -> Result<TypeId, String> {
        let (packed, aligned) = self.skip_attrs()?;
        let tag = if let Some(Tok::Ident(n)) = self.toks.get(self.pos) {
            let n = n.clone();
            self.pos += 1;
            Some(n)
        } else {
            None
        };
        if !self.eat(&Tok::Punct("{")) {
            let tag = tag.ok_or("struct/union cần tag hoặc thân")?;
            // forward reference: tạo placeholder incomplete, định nghĩa sau ghi đè
            if let Some(&t) = self.tags.get(&tag) {
                return Ok(t);
            }
            self.tt.structs.push(StructDef {
                members: Vec::new(),
                size: 0,
                align: 1,
                is_union,
            });
            let t = self.tt.add(Ty::Struct(self.tt.structs.len() as u32 - 1));
            self.tags.insert(tag, t);
            return Ok(t);
        }
        // có thân: tag đã có placeholder INCOMPLETE thì định nghĩa vào đúng slot
        // (tự tham chiếu được); tag đã complete → định nghĩa MỚI shadow (scope con)
        let incomplete = tag.as_ref().and_then(|g| self.tags.get(g)).copied().filter(|&t| {
            match self.tt.tys[t as usize] {
                Ty::Struct(si) => self.tt.structs[si as usize].members.is_empty(),
                _ => false,
            }
        });
        let t = match incomplete {
            Some(t) => t,
            None => {
                self.tt.structs.push(StructDef {
                    members: Vec::new(),
                    size: 0,
                    align: 1,
                    is_union,
                });
                let t = self.tt.add(Ty::Struct(self.tt.structs.len() as u32 - 1));
                if let Some(tag) = tag {
                    self.tags.insert(tag, t);
                }
                t
            }
        };
        let Ty::Struct(sidx) = self.tt.tys[t as usize] else { unreachable!() };
        let mut members = Vec::new();
        let (mut off, mut mx) = (0u32, 1u32);
        // trạng thái đơn vị bitfield đang mở (bit_size=0 → không có)
        let (mut bit_unit, mut bit_used, mut bit_size) = (0u32, 0u32, 0u32);
        while !self.eat(&Tok::Punct("}")) {
            let (bt, _) = self.decl_specs()?.ok_or("cần kiểu member")?;
            let attr_al = self.attr_aligned.take().unwrap_or(1);
            // không có declarator: anonymous struct/union (C11, clang cho) → trải
            // member con lên tầng này; kiểu khác (định nghĩa tag) → bỏ qua
            if self.peek(";") {
                if let Ty::Struct(_) = self.tt.tys[bt as usize] {
                    // member ĐƠN tên rỗng (giữ cursor init đúng); truy cập
                    // xuyên qua bằng find_member đệ quy
                    let (sz, al) = (self.tt.size(bt), self.tt.align(bt));
                    let o = if is_union { 0 } else { off.div_ceil(al) * al };
                    members.push((String::new(), bt, o));
                    off = if is_union { off.max(sz) } else { o + sz };
                    mx = mx.max(al);
                    bit_size = 0;
                }
                self.expect(Tok::Punct(";"))?;
                continue;
            }
            loop {
                // bitfield không tên: "int : 3;" — không có declarator
                let (mn, mt) = if self.peek(":") {
                    (String::new(), bt)
                } else {
                    self.declarator(bt, true)?
                };
                if self.eat(&Tok::Punct(":")) {
                    // bitfield: gói vào "đơn vị chứa" size của kiểu khai báo
                    let w = self.const_expr()? as u32;
                    let (s, al) = (self.tt.size(mt), self.tt.align(mt));
                    if w == 0 || bit_size != s * 8 || bit_used + w > bit_size {
                        bit_size = 0; // đóng đơn vị hiện tại
                    }
                    if w > 0 {
                        if bit_size == 0 {
                            let o = if is_union { 0 } else { off.div_ceil(al) * al };
                            bit_unit = o;
                            off = if is_union { off.max(s) } else { o + s };
                            bit_used = 0;
                            bit_size = s * 8;
                        }
                        if !mn.is_empty() {
                            let ft = self.tt.add(Ty::Bitfield(mt, bit_used, w));
                            members.push((mn, ft, bit_unit));
                        }
                        bit_used += w;
                        mx = mx.max(al);
                    }
                } else {
                    bit_size = 0;
                    let sz = self.tt.size(mt);
                    let al = if packed { 1 } else { self.tt.align(mt).max(attr_al) };
                    let o = if is_union { 0 } else { off.div_ceil(al) * al };
                    members.push((mn, mt, o));
                    off = if is_union { off.max(sz) } else { o + sz };
                    mx = mx.max(al);
                }
                if !self.eat(&Tok::Punct(",")) {
                    break;
                }
            }
            self.expect(Tok::Punct(";"))?;
        }
        if let Some(a) = aligned {
            mx = mx.max(a);
        }
        self.tt.structs[sidx as usize] =
            StructDef { members, size: off.div_ceil(mx) * mx, align: mx, is_union };
        Ok(t)
    }
    fn enum_spec(&mut self) -> Result<TypeId, String> {
        let mut tag = String::new();
        if let Some(Tok::Ident(n)) = self.toks.get(self.pos) {
            if !self.is_type_word(n) && self.toks.get(self.pos + 1) != Some(&Tok::Punct("{"))
                || self.toks.get(self.pos + 1) == Some(&Tok::Punct("{"))
            {
                tag = n.clone();
                self.pos += 1;
            }
        }
        if self.eat(&Tok::Punct("{")) {
            let mut val = 0i64;
            let mut any_neg = false;
            while !self.eat(&Tok::Punct("}")) {
                let name = self.ident()?;
                if self.eat(&Tok::Punct("=")) {
                    val = self.const_expr()?;
                }
                any_neg |= val < 0;
                self.enums.insert(name, val);
                val += 1;
                if !self.eat(&Tok::Punct(",")) {
                    self.expect(Tok::Punct("}"))?;
                    break;
                }
            }
            // khớp clang: mọi enumerator không âm → underlying unsigned int
            // (quan trọng cho bitfield kiểu enum: phải zero-extend)
            let t = if any_neg { INT } else { UINT };
            if !tag.is_empty() {
                self.enum_tags.insert(tag, t);
            }
            return Ok(t);
        }
        Ok(self.enum_tags.get(&tag).copied().unwrap_or(INT))
    }
    // declarator đầy đủ C: con trỏ, nested "(...)", suffix mảng/hàm.
    // need_name=false cho abstract declarator (cast, sizeof, param không tên).
    fn declarator(&mut self, mut t: TypeId, need_name: bool) -> Result<(String, TypeId), String> {
        self.skip_attrs()?;
        let mut starred = false; // sau '*' thì ident LUÔN là tên, kể cả trùng typedef
        while self.eat(&Tok::Punct("*")) {
            starred = true;
            t = self.tt.ptr_to(t);
            while self.eat_kw("const")
                || self.eat_kw("volatile")
                || self.eat_kw("restrict")
                || self.eat_kw("__restrict")
                || self.eat_kw("__restrict__")
            {}
            self.skip_attrs()?; // "void *__attribute__((noinline)) f(...)"
        }
        if self.nested_ahead() {
            self.pos += 1; // '('
            let save = self.pos;
            self.declarator(VOID, false)?; // parse nháp tìm ')' — type tạo ra là rác arena
            self.expect(Tok::Punct(")"))?;
            let outer = self.suffixes(t)?; // suffix NGOÀI áp trước (luật inside-out)
            let end = self.pos;
            self.pos = save;
            let res = self.declarator(outer, need_name)?;
            self.pos = end;
            return Ok(res);
        }
        let name = match self.toks.get(self.pos) {
            // tên được phép TRÙNG typedef-name (shadow); chỉ cấm keyword cứng
            Some(Tok::Ident(n)) if (need_name || starred) && !self.is_keyword(n) => {
                let n = n.clone();
                self.pos += 1;
                n
            }
            Some(Tok::Ident(n)) if !self.is_type_word(n) => {
                let n = n.clone();
                self.pos += 1;
                n
            }
            _ if need_name => {
                return Err(format!("cần tên trong declarator, gặp {:?}", self.toks.get(self.pos)))
            }
            _ => String::new(),
        };
        let t = self.suffixes(t)?;
        self.skip_attrs()?;
        Ok((name, t))
    }
    fn nested_ahead(&self) -> bool {
        if !self.peek("(") {
            return false;
        }
        match self.toks.get(self.pos + 1) {
            Some(Tok::Punct("*") | Tok::Punct("(") | Tok::Punct("[")) => true,
            Some(Tok::Ident(n)) => !self.is_type_word(n),
            _ => false,
        }
    }
    // nuốt __attribute__((...)) / __asm__("..") — extension, không ảnh hưởng ngữ nghĩa
    // nuốt __attribute__/__asm__; hiểu packed + aligned(n)
    fn skip_attrs(&mut self) -> Result<(bool, Option<u32>), String> {
        let (mut packed, mut aligned) = (false, None);
        loop {
            if self.eat_kw("__attribute__") {
                self.expect(Tok::Punct("("))?;
                self.expect(Tok::Punct("("))?;
                while !self.eat(&Tok::Punct(")")) {
                    if self.eat(&Tok::Punct(",")) {
                        continue;
                    }
                    let n = self.ident()?;
                    match n.as_str() {
                        "packed" | "__packed__" => packed = true,
                        "aligned" | "__aligned__" => {
                            if self.eat(&Tok::Punct("(")) {
                                let v = self.const_expr()? as u32;
                                self.expect(Tok::Punct(")"))?;
                                aligned = Some(aligned.unwrap_or(0).max(v));
                            } else {
                                aligned = Some(16); // GCC: aligned trần = 16
                            }
                        }
                        _ => {
                            // attr lạ (có thể kèm (args)): nuốt balanced
                            if self.eat(&Tok::Punct("(")) {
                                let mut depth = 1u32;
                                while depth > 0 {
                                    match self.toks.get(self.pos) {
                                        Some(Tok::Punct("(")) => depth += 1,
                                        Some(Tok::Punct(")")) => depth -= 1,
                                        None => return Err("__attribute__ không đóng".into()),
                                        _ => {}
                                    }
                                    self.pos += 1;
                                }
                            }
                        }
                    }
                }
                self.expect(Tok::Punct(")"))?;
            } else if self.eat_kw("__asm__") || self.eat_kw("__asm") {
                self.expect(Tok::Punct("("))?;
                let mut depth = 1u32;
                while depth > 0 {
                    match self.toks.get(self.pos) {
                        Some(Tok::Punct("(")) => depth += 1,
                        Some(Tok::Punct(")")) => depth -= 1,
                        None => return Err("__asm__ không đóng".into()),
                        _ => {}
                    }
                    self.pos += 1;
                }
            } else {
                return Ok((packed, aligned));
            }
        }
    }
    fn suffixes(&mut self, t: TypeId) -> Result<TypeId, String> {
        if self.eat(&Tok::Punct("[")) {
            let n = if self.peek("]") { 0 } else { self.const_expr()? as u64 };
            self.expect(Tok::Punct("]"))?;
            let inner = self.suffixes(t)?; // đa chiều: int a[2][3] = array 2 của array 3
            return Ok(self.tt.add(Ty::Array(inner, n)));
        }
        if self.eat(&Tok::Punct("(")) {
            let sig = self.param_list(t)?;
            self.tt.fns.push(sig);
            return Ok(self.tt.add(Ty::Func(self.tt.fns.len() as u32 - 1)));
        }
        Ok(t)
    }
    fn param_list(&mut self, ret: TypeId) -> Result<FnSig, String> {
        let empty = FnSig {
            ret,
            params: Vec::new(),
            pnames: Vec::new(),
            variadic: false,
            oldstyle: false,
        };
        if self.eat(&Tok::Punct(")")) {
            return Ok(FnSig { oldstyle: true, ..empty }); // () — không thông tin
        }
        if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if n == "void")
            && self.toks.get(self.pos + 1) == Some(&Tok::Punct(")"))
        {
            self.pos += 2;
            return Ok(empty); // (void)
        }
        // old-style ident list: f(a, b)
        if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if !self.is_type_word(n)) {
            let mut pnames = Vec::new();
            loop {
                pnames.push(self.ident()?);
                if !self.eat(&Tok::Punct(",")) {
                    break;
                }
            }
            self.expect(Tok::Punct(")"))?;
            return Ok(FnSig { pnames, oldstyle: true, ..empty });
        }
        let (mut params, mut pnames, mut variadic) = (Vec::new(), Vec::new(), false);
        loop {
            if self.eat(&Tok::Punct("...")) {
                variadic = true;
                break;
            }
            let (bt, _) = self.decl_specs()?.ok_or("cần kiểu tham số")?;
            let (nm, pt) = self.declarator(bt, false)?;
            // điều chỉnh kiểu param: mảng → con trỏ, hàm → con trỏ hàm
            let pt = match self.tt.tys[pt as usize] {
                Ty::Array(e, _) => self.tt.ptr_to(e),
                Ty::Func(_) => self.tt.ptr_to(pt),
                _ => pt,
            };
            params.push(pt);
            pnames.push(nm);
            if !self.eat(&Tok::Punct(",")) {
                break;
            }
        }
        self.expect(Tok::Punct(")"))?;
        Ok(FnSig { ret, params, pnames, variadic, oldstyle: false })
    }
    fn typename(&mut self) -> Result<TypeId, String> {
        let (bt, _) = self.decl_specs()?.ok_or("cần tên kiểu")?;
        let (_, t) = self.declarator(bt, false)?;
        Ok(t)
    }

    // ---- chuyển đổi kiểu ----
    fn cast(&mut self, e: NodeId, to: TypeId) -> NodeId {
        if self.ty(e) == to {
            return e;
        }
        self.push(Node::Cast(e), to)
    }
    fn promote(&self, t: TypeId) -> TypeId {
        match self.tt.tys[t as usize] {
            Ty::Char | Ty::Short => INT,
            Ty::UChar | Ty::UShort | Ty::Bool => INT, // cả ba lọt trong int (LP64)
            Ty::Float => DOUBLE,
            // Bitfield: lên int nếu range lọt int, unsigned int nếu lọt uint,
            // rộng hơn 32 bit thì theo base (ANSI không định nghĩa, theo gcc)
            Ty::Bitfield(b, _, w) => {
                if w < 32 || (w == 32 && !self.tt.is_unsigned(b)) {
                    INT
                } else if w == 32 {
                    UINT
                } else {
                    self.promote(b)
                }
            }
            _ => t,
        }
    }
    fn common_ty(&self, lt: TypeId, rt: TypeId) -> TypeId {
        if self.tt.is_float(lt) || self.tt.is_float(rt) {
            // float+float / float+int → FLOAT (operand phải TRÒN về f32 —
            // 16777217L != (float)16777217e0 phân biệt được); còn lại double.
            // Số học chạy trong double (C89 cho phép dư precision).
            let fl = |t: TypeId| matches!(self.tt.tys[t as usize], Ty::Double)
                || !self.tt.is_integer(t) && !matches!(self.tt.tys[t as usize], Ty::Float);
            return if fl(lt) || fl(rt) { DOUBLE } else { FLOAT };
        }
        if !self.tt.is_integer(lt) || !self.tt.is_integer(rt) {
            return ULONG; // con trỏ v.v.: so sánh/số học 64-bit không dấu
        }
        let (l, r) = (self.promote(lt), self.promote(rt));
        if l == r {
            return l;
        }
        let (ls, rs) = (self.tt.size(l), self.tt.size(r));
        let big = ls.max(rs);
        let uns = if ls == rs {
            self.tt.is_unsigned(l) || self.tt.is_unsigned(r)
        } else if ls > rs {
            self.tt.is_unsigned(l)
        } else {
            self.tt.is_unsigned(r)
        };
        match (big, uns) {
            (8, true) => ULONG,
            (8, false) => LONG,
            (_, true) => UINT,
            _ => INT,
        }
    }
    fn scalar(&self, t: TypeId) -> bool {
        !matches!(self.tt.tys[t as usize], Ty::Struct(_) | Ty::Array(..))
    }
    // L op= R (và ++L/--L): L xuất hiện 2 lần trong cây (load + store) — nếu
    // địa chỉ có side effect (a[*s++] |= 1) phải giữ vào temp, eval đúng 1 lần:
    // (tmp = &L, *tmp = *tmp op R)
    fn opassign(&mut self, l: NodeId, bop: &'static str, r: NodeId) -> R {
        match self.nodes[l as usize] {
            Node::Var(_) | Node::GVar(_) => {
                // địa chỉ tĩnh, khỏi temp
                let r = self.mkbin(bop, l, r)?;
                self.mkassign(l, r)
            }
            _ => {
                let lt = self.ty(l);
                let pt = self.tt.ptr_to(lt);
                let off = self.alloc_local(String::new(), pt);
                let tmp = self.push(Node::Var(off), pt);
                let ad = self.push(Node::Addr(l), pt);
                let sav = self.push(Node::Assign(tmp, ad), pt);
                let tmp2 = self.push(Node::Var(off), pt);
                let ld = self.push(Node::Deref(tmp2), lt);
                let r = self.mkbin(bop, ld, r)?;
                let asn = self.mkassign(ld, r)?;
                let t = self.ty(asn);
                Ok(self.push(Node::Comma(sav, asn), t))
            }
        }
    }
    fn mkassign(&mut self, l: NodeId, r: NodeId) -> R {
        self.check_lval(l)?;
        let lt = self.ty(l);
        let r = if self.scalar(lt) { self.cast(r, lt) } else { r };
        Ok(self.push(Node::Assign(l, r), lt))
    }
    // điều kiện: float phải so != 0.0 (cbz nhìn bit pattern sẽ sai với -0.0)
    fn truthy(&mut self, e: NodeId) -> R {
        if self.tt.is_float(self.ty(e)) {
            let z = self.push(Node::FNum(0.0), DOUBLE);
            self.mkbin("!=", e, z)
        } else {
            Ok(e)
        }
    }
    // Dựng node binary op kèm chèn conversion + scale con trỏ
    fn mkbin(&mut self, op: &'static str, l: NodeId, r: NodeId) -> R {
        let (lp, rp) = (self.tt.pointee(self.ty(l)), self.tt.pointee(self.ty(r)));
        match (op, lp, rp) {
            ("+", None, Some(_)) => self.mkbin("+", r, l), // int + ptr: giao hoán
            ("+" | "-", Some(e), None) => {
                let r = self.cast(r, LONG);
                let sz = self.push(Node::Num(self.tt.size(e) as i64), LONG);
                let r = self.push(Node::Bin("*", r, sz), LONG);
                let t = self.tt.ptr_to(e);
                Ok(self.push(Node::Bin(op, l, r), t))
            }
            ("-", Some(e), Some(_)) => {
                let d = self.push(Node::Bin("-", l, r), LONG);
                let sz = self.push(Node::Num(self.tt.size(e) as i64), LONG);
                Ok(self.push(Node::Bin("/", d, sz), LONG))
            }
            _ => match op {
                "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                    let ct = self.common_ty(self.ty(l), self.ty(r));
                    let (l, r) = (self.cast(l, ct), self.cast(r, ct));
                    Ok(self.push(Node::Bin(op, l, r), INT))
                }
                "<<" | ">>" => {
                    let lt = self.promote(self.ty(l));
                    if self.tt.is_float(lt) {
                        return Err("shift trên số thực".into());
                    }
                    let l = self.cast(l, lt);
                    Ok(self.push(Node::Bin(op, l, r), lt))
                }
                _ => {
                    let ct = self.common_ty(self.ty(l), self.ty(r));
                    if self.tt.is_float(ct) && matches!(op, "%" | "&" | "|" | "^") {
                        return Err(format!("op '{}' trên số thực", op));
                    }
                    let (l, r) = (self.cast(l, ct), self.cast(r, ct));
                    Ok(self.push(Node::Bin(op, l, r), ct))
                }
            },
        }
    }

    // ---- local/scope ----
    fn alloc_local(&mut self, name: String, t: TypeId) -> u32 {
        let (sz, al) = (self.tt.size(t), self.tt.align(t));
        self.cur_off = (self.cur_off + sz).div_ceil(al) * al;
        self.locals.push((name, t, Vloc::Stack(self.cur_off)));
        self.cur_off
    }
    // nhánh bị DCE có chứa label không (goto từ ngoài vào thì không được bỏ)
    fn has_label(&self, id: NodeId) -> bool {
        match &self.nodes[id as usize] {
            // Case cũng là jump target (bảng switch tham chiếu LC{id})
            Node::Label(..) | Node::Case(..) => true,
            Node::If(a, b, c) => {
                self.has_label(*a)
                    || self.has_label(*b)
                    || c.is_some_and(|x| self.has_label(x))
            }
            Node::While(a, b)
            | Node::Do(a, b)
            | Node::Comma(a, b)
            | Node::Assign(a, b)
            | Node::Bin(_, a, b) => self.has_label(*a) || self.has_label(*b),
            Node::For(a, b, c, d) => {
                [a, b, c].iter().any(|x| x.is_some_and(|x| self.has_label(x)))
                    || self.has_label(*d)
            }
            Node::Switch(a, b, ..) => self.has_label(*a) || self.has_label(*b),
            Node::Block(v) => v.iter().any(|&x| self.has_label(x)),
            Node::Cond(a, b, c) => {
                self.has_label(*a) || self.has_label(*b) || self.has_label(*c)
            }
            Node::Ret(e) => e.is_some_and(|x| self.has_label(x)),
            Node::Deref(e)
            | Node::Addr(e)
            | Node::Neg(e)
            | Node::Cast(e)
            | Node::Member(e, _)
            | Node::Zero(e, _)
            | Node::SRet(e, ..)
            | Node::Post(_, e, _)
            | Node::GotoPtr(e)
            | Node::Alloca(e) => self.has_label(*e),
            Node::Call(_, args, _) => args.iter().any(|&x| self.has_label(x)),
            Node::CallPtr(f, args, _) => {
                self.has_label(*f) || args.iter().any(|&x| self.has_label(x))
            }
            _ => false,
        }
    }
    fn check_lval(&self, l: NodeId) -> Result<(), String> {
        if matches!(
            self.nodes[l as usize],
            Node::Var(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..)
        ) {
            Ok(())
        } else {
            Err("cần lvalue".into())
        }
    }

    // ---- statement ----
    fn stmt(&mut self) -> R {
        if let (Some(Tok::Ident(n)), Some(Tok::Punct(":"))) =
            (self.toks.get(self.pos), self.toks.get(self.pos + 1))
        {
            // typedef-name + ":" vẫn là label (declaration không thể bắt đầu bằng ":")
            if !["case", "default"].contains(&n.as_str()) {
                let n = n.clone();
                self.pos += 2;
                let st = self.stmt()?;
                return Ok(self.push(Node::Label(n, st), INT));
            }
        }
        if self.eat_kw("asm") || self.eat_kw("__asm__") || self.eat_kw("__asm") {
            // GNU asm statement: nuốt (template rỗng + constraint = barrier, no-op ở -O0)
            while self.eat_kw("volatile") || self.eat_kw("__volatile__") {}
            self.expect(Tok::Punct("("))?;
            match self.toks.get(self.pos) {
                Some(Tok::Str(s, _)) if s.is_empty() => {}
                _ => return Err("asm chỉ hỗ trợ template rỗng (barrier)".into()),
            }
            let mut depth = 1;
            while depth > 0 {
                match self.toks.get(self.pos) {
                    Some(Tok::Punct("(")) => depth += 1,
                    Some(Tok::Punct(")")) => depth -= 1,
                    None => return Err("asm không đóng".into()),
                    _ => {}
                }
                self.pos += 1;
            }
            self.expect(Tok::Punct(";"))?;
            return Ok(self.push(Node::Block(Vec::new()), INT));
        }
        if self.eat_kw("__label__") {
            // GNU local label declaration — label của mình vốn function-scope, nuốt
            while !self.eat(&Tok::Punct(";")) {
                self.pos += 1;
            }
            return Ok(self.push(Node::Block(Vec::new()), INT));
        }
        if self.eat_kw("return") {
            if self.eat(&Tok::Punct(";")) {
                return Ok(self.push(Node::Ret(None), INT));
            }
            let e = self.expr()?;
            let e = if self.fret == VOID { e } else { self.cast(e, self.fret) };
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Ret(Some(e)), INT))
        } else if self.eat_kw("if") {
            self.expect(Tok::Punct("("))?;
            let c = self.expr()?;
            let c = self.truthy(c)?;
            self.expect(Tok::Punct(")"))?;
            let t = self.stmt()?;
            let e = if self.eat_kw("else") { Some(self.stmt()?) } else { None };
            // điều kiện hằng → giữ đúng nhánh (DCE tối thiểu — clang -O0 cũng
            // làm, torture link_error dựa vào); nhánh bỏ chứa label thì thôi
            if let Ok(v) = self.fold(c) {
                let (keep, drop) = if v != 0 { (Some(t), e) } else { (e, Some(t)) };
                if !drop.is_some_and(|d| self.has_label(d)) {
                    return Ok(keep.unwrap_or_else(|| self.push(Node::Block(Vec::new()), INT)));
                }
            }
            Ok(self.push(Node::If(c, t, e), INT))
        } else if self.eat_kw("while") {
            self.expect(Tok::Punct("("))?;
            let c = self.expr()?;
            let c = self.truthy(c)?;
            self.expect(Tok::Punct(")"))?;
            let b = self.stmt()?;
            Ok(self.push(Node::While(c, b), INT))
        } else if self.eat_kw("for") {
            self.expect(Tok::Punct("("))?;
            // C99 (clang -std=c89 cho): "for (int i = 0; ...)" — init là declaration
            let i = if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n))
            {
                Some(self.stmt()?) // decl-stmt tự nuốt ";"
            } else {
                self.opt_expr(";")?
            };
            let c = match self.opt_expr(";")? {
                Some(c) => Some(self.truthy(c)?),
                None => None,
            };
            let n = self.opt_expr(")")?;
            let b = self.stmt()?;
            Ok(self.push(Node::For(i, c, n, b), INT))
        } else if self.eat_kw("do") {
            let b = self.stmt()?;
            if !self.eat_kw("while") {
                return Err("do cần while".into());
            }
            self.expect(Tok::Punct("("))?;
            let c = self.expr()?;
            let c = self.truthy(c)?;
            self.expect(Tok::Punct(")"))?;
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Do(b, c), INT))
        } else if self.eat_kw("switch") {
            self.expect(Tok::Punct("("))?;
            let c = self.expr()?;
            self.expect(Tok::Punct(")"))?;
            self.switches.push((Vec::new(), None));
            let b = self.stmt()?;
            let (mut cases, def) = self.switches.pop().unwrap();
            // C89 6.6.4.2: case constant convert về type (promoted) của control expr
            let ct = self.promote(self.ty(c));
            if self.tt.size(ct) == 4 {
                let uns = self.tt.is_unsigned(ct);
                for (v, _) in &mut cases {
                    *v = if uns { *v as u32 as i64 } else { *v as i32 as i64 };
                }
            }
            Ok(self.push(Node::Switch(c, b, cases, def), INT))
        } else if self.eat_kw("case") {
            let v = self.const_expr()?;
            self.expect(Tok::Punct(":"))?;
            let st = self.stmt()?;
            let id = self.push(Node::Case(st), INT);
            self.switches.last_mut().ok_or("case ngoài switch")?.0.push((v, id));
            Ok(id)
        } else if self.eat_kw("default") {
            self.expect(Tok::Punct(":"))?;
            let st = self.stmt()?;
            let id = self.push(Node::Case(st), INT);
            self.switches.last_mut().ok_or("default ngoài switch")?.1 = Some(id);
            Ok(id)
        } else if self.eat_kw("break") {
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Break, INT))
        } else if self.eat_kw("continue") {
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Continue, INT))
        } else if self.eat_kw("goto") {
            if self.eat(&Tok::Punct("*")) {
                // GNU computed goto
                let e = self.expr()?;
                self.expect(Tok::Punct(";"))?;
                return Ok(self.push(Node::GotoPtr(e), INT));
            }
            let n = self.ident()?;
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Goto(n), INT))
        } else if self.eat(&Tok::Punct(";")) {
            Ok(self.push(Node::Block(Vec::new()), INT))
        } else if self.eat(&Tok::Punct("{")) {
            let scope = self.locals.len();
            // tag/typedef/enum cũng scope theo block (shadow rồi khôi phục)
            let (ts, ds, es, ets) = (
                self.tags.clone(),
                self.typedefs.clone(),
                self.enums.clone(),
                self.enum_tags.clone(),
            );
            let mut v = Vec::new();
            while !self.eat(&Tok::Punct("}")) {
                v.push(self.stmt()?);
            }
            self.locals.truncate(scope);
            self.tags = ts;
            self.typedefs = ds;
            self.enums = es;
            self.enum_tags = ets;
            Ok(self.push(Node::Block(v), INT))
        } else if let Some((bt, storage)) = self.decl_specs()? {
            // khai báo local (nhiều declarator, có init); typedef/static/extern xử lý riêng
            let mut stmts = Vec::new();
            if !self.eat(&Tok::Punct(";")) {
                loop {
                    let (name, mut t) = self.declarator(bt, true)?;
                    match storage {
                        Storage::Typedef => {
                            self.typedefs.insert(name, t);
                        }
                        _ if matches!(self.tt.tys[t as usize], Ty::Func(_)) => {
                            self.fns.insert(name.clone(), t); // prototype trong hàm
                            self.locals.push((name, t, Vloc::Fn)); // shadow biến cùng tên
                        }
                        Storage::Static => {
                            let g = format!("{}.{}", name, self.globals.len());
                            let init = self.ginit(&mut t)?;
                            self.globals.push(Global {
                                name: g,
                                ty: t,
                                init,
                                is_static: true,
                                is_extern: false,
                            });
                            self.locals.push((
                                name,
                                t,
                                Vloc::Glob(self.globals.len() as u32 - 1),
                            ));
                        }
                        Storage::Extern => {
                            self.globals.push(Global {
                                name: name.clone(),
                                ty: t,
                                init: GInit::None,
                                is_static: false,
                                is_extern: true,
                            });
                            self.locals.push((
                                name,
                                t,
                                Vloc::Glob(self.globals.len() as u32 - 1),
                            ));
                        }
                        _ => {
                            if self.eat(&Tok::Punct("=")) {
                                // scope của tên bắt đầu SAU declarator — init được
                                // tham chiếu chính nó (sizeof *p). Chỉ mảng [] chưa
                                // chốt size mới phải đổ phẳng trước khi cấp slot.
                                let no_size = matches!(self.tt.tys[t as usize], Ty::Array(_, 0));
                                let mut off = 0;
                                if !no_size {
                                    off = self.alloc_local(name.clone(), t);
                                }
                                let (flat, agg) = self.flat_init(&mut t)?;
                                if no_size {
                                    off = self.alloc_local(name, t);
                                }
                                let v = self.push(Node::Var(off), t);
                                // aggregate {..}/"..": zero-fill trước (partial init)
                                if agg
                                    && matches!(
                                        self.tt.tys[t as usize],
                                        Ty::Array(..) | Ty::Struct(_)
                                    )
                                {
                                    let sz = self.tt.size(t);
                                    stmts.push(self.push(Node::Zero(v, sz), VOID));
                                }
                                for (o, mt, item) in flat {
                                    match item {
                                        FlatItem::E(e) => {
                                            let lv = self.push(Node::Member(v, o), mt);
                                            stmts.push(self.mkassign(lv, e)?);
                                        }
                                        FlatItem::B(b) => {
                                            for (bi, &byte) in b.iter().enumerate() {
                                                let bv = self
                                                    .push(Node::Member(v, o + bi as u32), CHAR);
                                                let num =
                                                    self.push(Node::Num(byte as i64), INT);
                                                stmts.push(self.mkassign(bv, num)?);
                                            }
                                        }
                                    }
                                }
                            } else {
                                self.alloc_local(name, t);
                            }
                        }
                    }
                    if !self.eat(&Tok::Punct(",")) {
                        break;
                    }
                }
                self.expect(Tok::Punct(";"))?;
            }
            Ok(self.push(Node::Block(stmts), INT))
        } else {
            let e = self.expr()?;
            self.expect(Tok::Punct(";"))?;
            Ok(e)
        }
    }
    fn opt_expr(&mut self, end: &'static str) -> Result<Option<NodeId>, String> {
        if self.eat(&Tok::Punct(end)) {
            return Ok(None);
        }
        let e = self.expr()?;
        self.expect(Tok::Punct(end))?;
        Ok(Some(e))
    }
    // ---- initializer ----
    // Cây init parse trước, chốt size mảng [], rồi mới hạ xuống assign (local)
    // hoặc phẳng hóa thành (offset, size, item) (global/static).
    fn parse_init(&mut self) -> Result<Init, String> {
        if self.eat(&Tok::Punct("{")) {
            let mut v = Vec::new();
            if !self.eat(&Tok::Punct("}")) {
                loop {
                    // chuỗi designator: .a.j / [2].x / .m[1] — desugar phần đuôi
                    // thành init lồng: ".a.j = v" ≡ ".a = { .j = v }"
                    let mut steps = Vec::new();
                    loop {
                        if self.eat(&Tok::Punct("[")) {
                            let k = self.const_expr()? as u32;
                            if self.eat(&Tok::Punct("...")) {
                                let hi = self.const_expr()? as u32;
                                steps.push(Desig::Rng(k, hi));
                            } else {
                                steps.push(Desig::Idx(k));
                            }
                            self.expect(Tok::Punct("]"))?;
                        } else if self.peek(".") {
                            self.pos += 1;
                            steps.push(Desig::Mem(self.ident()?));
                        } else {
                            break;
                        }
                    }
                    // GNU cổ: "a : 'A'" ≡ ".a = 'A'" (đầu phần tử, không nhầm ?:)
                    if steps.is_empty() {
                        if let (Some(Tok::Ident(n)), Some(Tok::Punct(":"))) =
                            (self.toks.get(self.pos), self.toks.get(self.pos + 1))
                        {
                            steps.push(Desig::Mem(n.clone()));
                            self.pos += 2;
                            let init = self.parse_init()?;
                            v.push((steps.pop().unwrap(), init));
                            if !self.eat(&Tok::Punct(",")) || self.peek("}") {
                                break;
                            }
                            continue;
                        }
                    }
                    if !steps.is_empty() {
                        self.expect(Tok::Punct("="))?;
                    }
                    let mut init = self.parse_init()?;
                    let d = if steps.is_empty() {
                        Desig::No
                    } else {
                        while steps.len() > 1 {
                            let st = steps.pop().unwrap();
                            init = Init::L(vec![(st, init)]);
                        }
                        steps.pop().unwrap()
                    };
                    v.push((d, init));
                    if !self.eat(&Tok::Punct(",")) {
                        break;
                    }
                    if self.peek("}") {
                        break; // trailing comma
                    }
                }
                self.expect(Tok::Punct("}"))?;
            }
            return Ok(Init::L(v));
        }
        if let Some(Tok::Str(b, w)) = self.toks.get(self.pos) {
            let start = self.pos;
            let (mut b, mut w) = (b.clone(), *w);
            self.pos += 1;
            while let Some(Tok::Str(m, w2)) = self.toks.get(self.pos) {
                b.extend_from_slice(m);
                w |= *w2;
                self.pos += 1;
            }
            // string trần mới là Init::S; "abc" + 10 là biểu thức
            if matches!(
                self.toks.get(self.pos),
                None | Some(Tok::Punct("," | ";" | "}"))
            ) {
                return Ok(Init::S(b, w));
            }
            self.pos = start;
        }
        Ok(Init::E(self.assign()?))
    }
    // ---- đổ initializer (mô hình cursor — brace elision chuẩn C89) ----
    // fill_obj: một initializer TRỌN VẸN (braced/string/expr) vào object (t, base).
    fn fill_obj(
        &mut self,
        t: TypeId,
        base: u32,
        init: Init,
        out: &mut Vec<(u32, TypeId, FlatItem)>,
    ) -> Result<(), String> {
        match init {
            Init::S(mut b, wide) => {
                if let Ty::Array(e, n) = self.tt.tys[t as usize] {
                    if wide && self.tt.size(e) == 4 {
                        // wchar_t ws[] = L"..": mỗi char → 4 byte LE + NUL 4 byte
                        let cps = wchars(&b);
                        let mut wb = Vec::with_capacity(cps.len() * 4 + 4);
                        for c in cps {
                            wb.extend_from_slice(&c.to_le_bytes());
                        }
                        wb.extend_from_slice(&[0; 4]);
                        if n > 0 && wb.len() as u64 > n * 4 {
                            wb.truncate((n * 4) as usize);
                        }
                        out.push((base, t, FlatItem::B(wb)));
                        return Ok(());
                    }
                    if self.tt.size(e) == 1 {
                        b.push(0);
                        if n > 0 && b.len() as u64 > n {
                            b.truncate(n as usize); // char s[3] = "abc" hợp lệ C89
                        }
                        out.push((base, t, FlatItem::B(b)));
                        return Ok(());
                    }
                }
                // char *p = "str" / wchar_t *p = L"str"
                let (n, st) = if wide {
                    let cps = wchars(&b);
                    let n = cps.len() as u32;
                    let mut wb = Vec::with_capacity(cps.len() * 4 + 3);
                    for c in cps {
                        wb.extend_from_slice(&c.to_le_bytes());
                    }
                    wb.extend_from_slice(&[0, 0, 0]); // .asciz bù NUL thứ 4
                    b = wb;
                    (n + 1, INT)
                } else {
                    (b.len() as u32 + 1, CHAR)
                };
                self.strs.push(b);
                let i = (self.strs.len() - 1) as u32;
                let st = self.tt.add(Ty::Array(st, n as u64));
                let sn = self.push(Node::Str(i), st);
                out.push((base, t, FlatItem::E(sn)));
                Ok(())
            }
            Init::L(v) => {
                let mut it = v.into_iter().peekable();
                self.fill_list(t, base, &mut it, out)?;
                Ok(())
            }
            Init::E(e) => {
                // scalar "chảy" vào leaf đầu khi t aggregate mà expr khác kiểu
                let (mut t, mut base) = (t, base);
                while !self.scalar(t) && self.ty(e) != t {
                    match self.tt.tys[t as usize] {
                        Ty::Array(el, _) => t = el,
                        Ty::Struct(si) => {
                            let m = self.tt.structs[si as usize]
                                .members
                                .first()
                                .ok_or("init struct rỗng")?;
                            base += m.2;
                            t = m.1;
                        }
                        _ => unreachable!(),
                    }
                }
                out.push((base, t, FlatItem::E(e)));
                Ok(())
            }
        }
    }
    // fill_list: đổ dần iterator (CHIA SẺ giữa các tầng = elision) vào aggregate t.
    // Trả số phần tử array đã chạm (suy size cho T x[]).
    fn fill_list(
        &mut self,
        t: TypeId,
        base: u32,
        it: &mut ItInit,
        out: &mut Vec<(u32, TypeId, FlatItem)>,
    ) -> Result<u32, String> {
        match self.tt.tys[t as usize] {
            Ty::Array(e, n) => {
                let esz = self.tt.size(e);
                let (mut i, mut cnt) = (0u32, 0u32);
                while let Some((d, _)) = it.peek() {
                    match d {
                        Desig::Idx(k) => i = *k, // quay lui index TRƯỚC khi xét đầy
                        Desig::Rng(lo, hi) => {
                            let (lo, hi) = (*lo, *hi);
                            let init = it.next().unwrap().1;
                            for k in lo..=hi {
                                if n != 0 && k as u64 >= n {
                                    break;
                                }
                                self.fill_obj(e, base + k * esz, init.clone(), out)?;
                            }
                            i = hi + 1;
                            cnt = cnt.max(i);
                            continue;
                        }
                        _ => {}
                    }
                    if n != 0 && i as u64 >= n {
                        break;
                    }
                    self.fill_one(e, base + i * esz, it, out)?;
                    i += 1;
                    cnt = cnt.max(i);
                }
                Ok(cnt)
            }
            Ty::Struct(si) => {
                let members = self.tt.structs[si as usize].members.clone();
                let is_union = self.tt.structs[si as usize].is_union;
                let mut i = 0usize;
                while it.peek().is_some() {
                    if let Some((Desig::Mem(nm), _)) = it.peek() {
                        // designator quay lui cursor TRƯỚC khi xét đầy; không thấy
                        // trực tiếp thì trỏ vào anon member chứa nó (descend tự tìm lại)
                        i = match members.iter().position(|(mn, ..)| mn == nm) {
                            Some(k) => k,
                            None => match members.iter().position(|(mn, mt2, _)| {
                                mn.is_empty()
                                    && matches!(self.tt.tys[*mt2 as usize], Ty::Struct(s2)
                                        if self.find_member(s2, nm).is_some())
                            }) {
                                Some(k) => k,
                                None => break, // member của tầng NGOÀI → trả cursor về caller
                            },
                        };
                    }
                    if i >= members.len() {
                        break;
                    }
                    let (_, mt, off) = members[i].clone();
                    self.fill_one(mt, base + off, it, out)?;
                    i += 1;
                    if is_union {
                        break;
                    }
                }
                Ok(0)
            }
            _ => {
                let init = it.next().ok_or("initializer rỗng")?.1;
                self.fill_obj(t, base, init, out)?;
                Ok(1)
            }
        }
    }
    // Một sub-object: phần tử kế là braced/string → tiêu thụ nguyên; scalar expr
    // trên sub-object aggregate → elision: descend với CÙNG iterator.
    fn fill_one(
        &mut self,
        t: TypeId,
        base: u32,
        it: &mut ItInit,
        out: &mut Vec<(u32, TypeId, FlatItem)>,
    ) -> Result<(), String> {
        // string là "nguyên khối" với array/char*; với struct thì elision descend
        let braced = match it.peek() {
            Some((_, Init::L(_))) => true,
            Some((_, Init::S(..))) => !matches!(self.tt.tys[t as usize], Ty::Struct(_)),
            _ => false,
        };
        // expr kiểu struct KHỚP member → init nguyên khối, không elision descend
        let whole = matches!(
            it.peek(),
            Some((_, Init::E(e))) if {
                let et = self.ty(*e);
                match (self.tt.tys[t as usize], self.tt.tys[et as usize]) {
                    (Ty::Struct(a), Ty::Struct(b)) => a == b,
                    _ => false,
                }
            }
        );
        if braced || whole || self.scalar(t) {
            let init = it.next().unwrap().1;
            self.fill_obj(t, base, init, out)
        } else {
            self.fill_list(t, base, it, out).map(|_| ())
        }
    }
    // parse init + đổ phẳng + chốt size mảng []. Trả (flat, init có phải {..}/"..").
    fn flat_init(
        &mut self,
        t: &mut TypeId,
    ) -> Result<(Vec<(u32, TypeId, FlatItem)>, bool), String> {
        let init = self.parse_init()?;
        let agg = matches!(init, Init::L(_) | Init::S(..));
        let mut flat = Vec::new();
        match (self.tt.tys[*t as usize], init) {
            (Ty::Array(e, 0), Init::L(v)) => {
                let mut it = v.into_iter().peekable();
                let n = self.fill_list(*t, 0, &mut it, &mut flat)?;
                *t = self.tt.add(Ty::Array(e, n.max(1) as u64));
            }
            (Ty::Array(e, 0), Init::S(b, w)) => {
                *t = self.tt.add(Ty::Array(e, b.len() as u64 + 1));
                self.fill_obj(*t, 0, Init::S(b, w), &mut flat)?;
            }
            (_, init) => self.fill_obj(*t, 0, init, &mut flat)?,
        }
        Ok((flat, agg))
    }
    // một item hằng: số / bits float / địa chỉ symbol / string
    // nhận diện (&&a - &&b) — kể cả dạng ((a-b)/1) do ptr-diff — trả cặp symbol
    fn label_diff(&self, mut e: NodeId) -> Option<(String, String)> {
        loop {
            match &self.nodes[e as usize] {
                Node::Cast(i) => e = *i,
                Node::Bin("/", x, d) if matches!(self.nodes[*d as usize], Node::Num(1)) => {
                    e = *x;
                }
                _ => break,
            }
        }
        let Node::Bin("-", l, r) = self.nodes[e as usize] else { return None };
        let strip = |mut n: NodeId| {
            while let Node::Cast(i) = self.nodes[n as usize] {
                n = i;
            }
            n
        };
        let (l, r) = (strip(l), strip(r));
        if let (Node::LabelAddr(a), Node::LabelAddr(b)) =
            (&self.nodes[l as usize], &self.nodes[r as usize])
        {
            let f = &self.fname;
            return Some((format!("lg_{f}.{a}"), format!("lg_{f}.{b}")));
        }
        None
    }
    // giá trị con trỏ hằng: symbol + offset byte (mkbin đã scale ptr arith ra byte)
    fn gaddr(&self, mut e: NodeId) -> Option<(String, i64)> {
        loop {
            match &self.nodes[e as usize] {
                Node::Cast(i) => e = *i,
                Node::FunAddr(n) => return Some((n.clone(), 0)),
                Node::Addr(x) => return self.glval(*x),
                // chỉ array/function decay mới là giá trị con trỏ; scalar phải qua &
                Node::GVar(gi) => {
                    let g = &self.globals[*gi as usize];
                    return matches!(self.tt.tys[g.ty as usize], Ty::Array(..) | Ty::Func(_))
                        .then(|| (g.name.clone(), 0));
                }
                Node::Bin(op @ ("+" | "-"), l, r) => {
                    let (s, k) = self.gaddr(*l)?;
                    let d = self.fold(*r).ok()?;
                    return Some((s, if *op == "+" { k + d } else { k - d }));
                }
                _ => return None,
            }
        }
    }
    // lvalue path hằng → symbol + offset byte
    fn glval(&self, mut e: NodeId) -> Option<(String, i64)> {
        loop {
            match &self.nodes[e as usize] {
                Node::Cast(i) => e = *i,
                Node::GVar(gi) => return Some((self.globals[*gi as usize].name.clone(), 0)),
                Node::Deref(p) => return self.gaddr(*p),
                Node::Member(b, off) => {
                    let (s, k) = self.glval(*b)?;
                    return Some((s, k + *off as i64));
                }
                _ => return None,
            }
        }
    }
    fn gitem(&mut self, e0: NodeId, t: TypeId) -> Result<GInit, String> {
        // lột cast CHỈ để nhận diện pattern địa chỉ (str/&g/decay); fold số
        // phải dùng node gốc kẻo mất truncation của (unsigned int)-4 (pr39240)
        let mut e = e0;
        while let Node::Cast(inner) = self.nodes[e as usize] {
            e = inner;
        }
        // "abc" + k / &"abc"[k] → địa chỉ giữa string
        let stroff = |p: &Self, mut x: NodeId| -> Option<(u32, i64)> {
            while let Node::Cast(i) = p.nodes[x as usize] {
                x = i;
            }
            match p.nodes[x as usize] {
                Node::Str(i) => Some((i, 0)),
                Node::Bin("+", l, r) => {
                    let mut l = l;
                    while let Node::Cast(i) = p.nodes[l as usize] {
                        l = i;
                    }
                    if let Node::Str(i) = p.nodes[l as usize] {
                        return Some((i, p.fold(r).ok()?));
                    }
                    None
                }
                _ => None,
            }
        };
        match &self.nodes[e as usize] {
            Node::Str(i) => return Ok(GInit::Str(*i)),
            Node::Bin("+", ..) if stroff(self, e).is_some() => {
                let (i, k) = stroff(self, e).unwrap();
                return Ok(GInit::StrOff(i, k));
            }
            Node::Addr(inner) => {
                if let Node::Deref(x) = self.nodes[*inner as usize] {
                    if let Some((i, k)) = stroff(self, x) {
                        return Ok(GInit::StrOff(i, k));
                    }
                }
            }
            Node::LabelAddr(n) => {
                // symbol khớp quy ước label của codegen (không gạch dưới đầu)
                return Ok(GInit::Addr(format!("\x01lg_{}.{}", self.fname, n), 0));
            }
            // &&a - &&b: hiệu 2 label (GNU jump table tĩnh); ptr-diff void*
            // bị mkbin bọc "/1" nên phải bóc
            Node::Bin("/" | "-", ..) => {
                if let Some((a, b)) = self.label_diff(e) {
                    return Ok(GInit::Diff(a, b));
                }
            }
            _ => {}
        }
        // address constant tổng quát: &g.m, &a[i], (arr+1)->m... → symbol + offset
        if let Some((s, k)) = self.gaddr(e) {
            return Ok(GInit::Addr(s, k));
        }
        if self.tt.is_float(t) {
            let v = self.fold_f(e0)?;
            let bits =
                if self.tt.size(t) == 4 { (v as f32).to_bits() as i64 } else { v.to_bits() as i64 };
            return Ok(GInit::Num(bits));
        }
        // hằng nguyên từ biểu thức thực: (int)1.9 v.v. — fold_f rồi truncate
        if self.tt.is_float(self.ty(e0)) || self.tt.is_float(self.ty(e)) {
            if let Ok(v) = self.fold_f(e) {
                // cast Rust saturate: unsigned phải đi đường u64 kẻo 1.8e19 → i64::MAX
                let n = if self.tt.is_unsigned(t) { v as u64 as i64 } else { v as i64 };
                return Ok(GInit::Num(n));
            }
        }
        Ok(GInit::Num(self.fold(e0)?))
    }
    // init global/static: trả về GInit + chốt size mảng []
    fn ginit(&mut self, t: &mut TypeId) -> Result<GInit, String> {
        if !self.eat(&Tok::Punct("=")) {
            return Ok(GInit::None);
        }
        let (flat, _) = self.flat_init(t)?;
        self.flat_to_ginit(flat)
    }
    fn flat_to_ginit(
        &mut self,
        flat: Vec<(u32, TypeId, FlatItem)>,
    ) -> Result<GInit, String> {
        let mut list: Vec<(u32, u32, GInit)> = Vec::new();
        for (off, mt, item) in flat {
            match item {
                FlatItem::E(e) => {
                    if self.tt.size(mt) == 0 {
                        continue; // empty struct (GNU): không có data
                    }
                    // aggregate = compound literal ẩn (GVar static) → trải init nó vào đây
                    if matches!(self.tt.tys[mt as usize], Ty::Struct(_) | Ty::Array(..)) {
                        let mut e2 = e;
                        while let Node::Cast(i2) = self.nodes[e2 as usize] {
                            e2 = i2;
                        }
                        if let Node::GVar(gi) = self.nodes[e2 as usize] {
                            let init = self.globals[gi as usize].init.clone();
                            let sz = self.tt.size(mt);
                            splice_ginit(off, sz, init, &mut list);
                            continue;
                        }
                    }
                    // bitfield: gộp (OR) các field chung một đơn vị chứa
                    if let Ty::Bitfield(b, boff, w) = self.tt.tys[mt as usize] {
                        let mask = (!0u64 >> (64 - w)) as i64; // w ≥ 1 (w=0 không có tên field)
                        let v = (self.fold(e)? & mask) << boff;
                        if let Some(p) = list.iter_mut().find(|x| x.0 == off) {
                            if let GInit::Num(old) = p.2 {
                                p.2 = GInit::Num(old | v);
                                continue;
                            }
                        }
                        list.push((off, self.tt.size(b), GInit::Num(v)));
                        continue;
                    }
                    let gi = self.gitem(e, mt)?;
                    list.push((off, self.tt.size(mt), gi));
                }
                FlatItem::B(b) => {
                    let n = b.len() as u32;
                    list.push((off, n, GInit::Bytes(b)));
                }
            }
        }
        // designator có thể ra ngoài thứ tự / ghi đè: sort ổn định + giữ bản CUỐI
        list.sort_by_key(|x| x.0);
        let mut ded: Vec<(u32, u32, GInit)> = Vec::new();
        for it in list {
            if ded.last().map(|l| l.0) == Some(it.0) {
                *ded.last_mut().unwrap() = it;
            } else {
                ded.push(it);
            }
        }
        Ok(GInit::List(ded))
    }

    // ---- expression ----
    fn expr(&mut self) -> R {
        let mut l = self.assign()?;
        while self.eat(&Tok::Punct(",")) {
            let r = self.assign()?;
            let t = self.ty(r);
            l = self.push(Node::Comma(l, r), t);
        }
        Ok(l)
    }
    fn assign(&mut self) -> R {
        let l = self.cond_expr()?;
        for (tok, bop) in ASSIGN_OPS {
            if self.eat(&Tok::Punct(tok)) {
                self.check_lval(l)?;
                let r = self.assign()?;
                if bop.is_empty() {
                    return self.mkassign(l, r);
                }
                return self.opassign(l, bop, r);
            }
        }
        Ok(l)
    }
    fn cond_expr(&mut self) -> R {
        let c = self.lor()?;
        if !self.eat(&Tok::Punct("?")) {
            return Ok(c);
        }
        let c = self.truthy(c)?;
        // GNU elvis "a ?: b": vế giữa = chính cond, KHÔNG eval lại (codegen nhận
        // diện tb==cond để giữ x0)
        if self.eat(&Tok::Punct(":")) {
            let e = self.cond_expr()?;
            let t = self.ty(c);
            return Ok(self.push(Node::Cond(c, c, e), t));
        }
        let t = self.expr()?;
        self.expect(Tok::Punct(":"))?;
        let e = self.cond_expr()?;
        // hai vế hội tụ về kiểu chung (scalar); struct/ptr giữ vế trái
        let (tt_, te) = (self.ty(t), self.ty(e));
        if self.scalar(tt_) && self.scalar(te) && self.tt.is_integer(tt_) | self.tt.is_float(tt_) {
            let ct = self.common_ty(tt_, te);
            let (t, e) = (self.cast(t, ct), self.cast(e, ct));
            Ok(self.push(Node::Cond(c, t, e), ct))
        } else {
            Ok(self.push(Node::Cond(c, t, e), tt_))
        }
    }
    fn lor(&mut self) -> R {
        let mut l = self.land()?;
        while self.eat(&Tok::Punct("||")) {
            let l2 = self.truthy(l)?;
            let r = self.land()?;
            let r = self.truthy(r)?;
            let one = self.push(Node::Num(1), INT);
            let zero = self.push(Node::Num(0), INT);
            let rb = self.push(Node::Cond(r, one, zero), INT);
            l = self.push(Node::Cond(l2, one, rb), INT);
        }
        Ok(l)
    }
    fn land(&mut self) -> R {
        let mut l = self.bitor()?;
        while self.eat(&Tok::Punct("&&")) {
            let l2 = self.truthy(l)?;
            let r = self.bitor()?;
            let r = self.truthy(r)?;
            let one = self.push(Node::Num(1), INT);
            let zero = self.push(Node::Num(0), INT);
            let rb = self.push(Node::Cond(r, one, zero), INT);
            l = self.push(Node::Cond(l2, rb, zero), INT);
        }
        Ok(l)
    }
    fn bin(&mut self, ops: &[&'static str], next: fn(&mut Self) -> R) -> R {
        let mut l = next(self)?;
        'again: loop {
            for &op in ops {
                if self.eat(&Tok::Punct(op)) {
                    let r = next(self)?;
                    l = self.mkbin(op, l, r)?;
                    continue 'again;
                }
            }
            return Ok(l);
        }
    }
    fn bitor(&mut self) -> R {
        self.bin(&["|"], Self::bitxor)
    }
    fn bitxor(&mut self) -> R {
        self.bin(&["^"], Self::bitand)
    }
    fn bitand(&mut self) -> R {
        self.bin(&["&"], Self::equality)
    }
    fn equality(&mut self) -> R {
        self.bin(&["==", "!="], Self::relational)
    }
    fn relational(&mut self) -> R {
        self.bin(&["<", "<=", ">", ">="], Self::shift)
    }
    fn shift(&mut self) -> R {
        self.bin(&["<<", ">>"], Self::add)
    }
    fn add(&mut self) -> R {
        self.bin(&["+", "-"], Self::mul)
    }
    fn mul(&mut self) -> R {
        self.bin(&["*", "/", "%"], Self::unary)
    }
    fn incdec_pre(&mut self, op: &'static str) -> R {
        let e = self.unary()?;
        self.check_lval(e)?;
        let one = self.push(Node::Num(1), INT);
        self.opassign(e, op, one)
    }
    fn unary(&mut self) -> R {
        // cast: "(" typename ")"
        if self.peek("(") {
            if let Some(Tok::Ident(n)) = self.toks.get(self.pos + 1) {
                if self.is_type_word(n) || n == "__attribute__" {
                    self.pos += 1;
                    let ty = self.typename()?;
                    self.expect(Tok::Punct(")"))?;
                    // compound literal (C99, clang chấp nhận ở -std=c89): "(T){...}"
                    if self.peek("{") {
                        let mut t = ty;
                        if !self.in_fn {
                            // global scope: vật thể ẩn dạng static + init hằng
                            let (flat, _) = self.flat_init(&mut t)?;
                            let init = self.flat_to_ginit(flat)?;
                            let name = format!("__cl{}", self.globals.len());
                            self.globals.push(Global {
                                name,
                                ty: t,
                                init,
                                is_static: true,
                                is_extern: false,
                            });
                            let g = self.push(Node::GVar(self.globals.len() as u32 - 1), t);
                            return self.postfix_ops(g);
                        }
                        let (flat, _) = self.flat_init(&mut t)?;
                        let off = self.alloc_local(String::new(), t);
                        let v = self.push(Node::Var(off), t);
                        let mut acc = if matches!(
                            self.tt.tys[t as usize],
                            Ty::Array(..) | Ty::Struct(_)
                        ) {
                            let sz = self.tt.size(t);
                            Some(self.push(Node::Zero(v, sz), VOID))
                        } else {
                            None
                        };
                        for (o, mt, item) in flat {
                            let mut add = |p: &mut Self, e: NodeId| {
                                acc = Some(match acc {
                                    Some(a) => p.push(Node::Comma(a, e), VOID),
                                    None => e,
                                });
                            };
                            match item {
                                FlatItem::E(e) => {
                                    let lv = self.push(Node::Member(v, o), mt);
                                    let a = self.mkassign(lv, e)?;
                                    add(self, a);
                                }
                                FlatItem::B(b) => {
                                    for (bi, &byte) in b.iter().enumerate() {
                                        let bv =
                                            self.push(Node::Member(v, o + bi as u32), CHAR);
                                        let num = self.push(Node::Num(byte as i64), INT);
                                        let a = self.mkassign(bv, num)?;
                                        add(self, a);
                                    }
                                }
                            }
                        }
                        // lvalue hóa: Deref(Comma(inits, &temp)) — gán/& được như C99
                        let res = self.push(Node::Var(off), t);
                        let pt = self.tt.ptr_to(t);
                        let ad = self.push(Node::Addr(res), pt);
                        let chain = match acc {
                            Some(a) => self.push(Node::Comma(a, ad), pt),
                            None => ad,
                        };
                        let d = self.push(Node::Deref(chain), t);
                        return self.postfix_ops(d);
                    }
                    let e = self.unary()?;
                    return Ok(self.cast(e, ty));
                }
            }
        }
        if self.eat(&Tok::Punct("-")) {
            let e = self.unary()?;
            let t = self.ty(e);
            let t = if self.tt.is_float(t) { t } else { self.promote(t) };
            let e = if self.tt.is_float(t) { e } else { self.cast(e, t) };
            Ok(self.push(Node::Neg(e), t))
        } else if self.eat(&Tok::Punct("+")) {
            self.unary()
        } else if self.eat(&Tok::Punct("!")) {
            let e = self.unary()?;
            if self.tt.is_float(self.ty(e)) {
                let z = self.push(Node::FNum(0.0), DOUBLE);
                self.mkbin("==", e, z)
            } else {
                let z = self.push(Node::Num(0), INT);
                self.mkbin("==", e, z)
            }
        } else if self.eat(&Tok::Punct("~")) {
            let e = self.unary()?;
            let m = self.push(Node::Num(-1), INT);
            self.mkbin("^", e, m)
        } else if self.eat(&Tok::Punct("++")) {
            self.incdec_pre("+")
        } else if self.eat(&Tok::Punct("--")) {
            self.incdec_pre("-")
        } else if self.eat(&Tok::Punct("*")) {
            let e = self.unary()?;
            let t = match self.tt.tys[self.ty(e) as usize] {
                Ty::Ptr(p) | Ty::Array(p, _) => p,
                Ty::Func(_) => self.ty(e), // *f trên hàm = chính nó
                _ => return Err("deref thứ không phải con trỏ".into()),
            };
            Ok(self.push(Node::Deref(e), t))
        } else if self.eat(&Tok::Punct("&&")) {
            // GNU "&&label": && ở vị trí prefix không thể là logical-and
            let n = self.ident()?;
            let t = self.tt.ptr_to(VOID);
            Ok(self.push(Node::LabelAddr(n), t))
        } else if self.eat(&Tok::Punct("&")) {
            let e = self.unary()?;
            if matches!(self.nodes[e as usize], Node::FunAddr(_)) {
                return Ok(e); // &f = f với hàm
            }
            let t = self.tt.ptr_to(self.ty(e));
            Ok(self.push(Node::Addr(e), t))
        } else if self.eat_kw("_Alignof") || self.eat_kw("__alignof__") || self.eat_kw("__alignof")
        {
            let al = if self.peek("(")
                && matches!(self.toks.get(self.pos + 1), Some(Tok::Ident(n)) if self.is_type_word(n))
            {
                self.pos += 1;
                let t = self.typename()?;
                self.expect(Tok::Punct(")"))?;
                self.tt.align(t)
            } else {
                let save = self.nodes.len();
                let e = self.unary()?;
                let t = self.ty(e);
                self.nodes.truncate(save);
                self.types.truncate(save);
                self.tt.align(t)
            };
            return Ok(self.push(Node::Num(al as i64), ULONG));
        } else if self.eat_kw("sizeof") {
            // sizeof(typename) | sizeof unary
            let sz = if self.peek("(")
                && matches!(self.toks.get(self.pos + 1), Some(Tok::Ident(n)) if self.is_type_word(n))
            {
                self.pos += 1;
                let t = self.typename()?;
                self.expect(Tok::Punct(")"))?;
                self.tt.size64(t)
            } else {
                let e = self.unary()?; // node toán hạng thành rác arena, chấp nhận
                self.tt.size64(self.ty(e))
            };
            Ok(self.push(Node::Num(sz as i64), ULONG))
        } else {
            self.postfix()
        }
    }
    fn post_incdec(&mut self, op: &'static str, e: NodeId) -> R {
        self.check_lval(e)?;
        let t = self.ty(e);
        if self.tt.is_float(t) {
            // hiếm: desugar (x op= 1) op⁻¹ 1 — chấp nhận lệch rounding lý thuyết
            let one = self.push(Node::FNum(1.0), DOUBLE);
            let v = self.mkbin(op, e, one)?;
            let a = self.mkassign(e, v)?;
            let one2 = self.push(Node::FNum(1.0), DOUBLE);
            return self.mkbin(if op == "+" { "-" } else { "+" }, a, one2);
        }
        let delta = self.tt.pointee(t).map_or(1, |p| self.tt.size(p) as i64);
        Ok(self.push(Node::Post(op, e, delta), t))
    }
    // hoàn thiện một lời gọi: chèn cast theo prototype + default promotion
    fn finish_call(&mut self, callee: NodeId, mut args: Vec<NodeId>) -> R {
        let sig = self
            .tt
            .fnsig(self.ty(callee))
            .map(|s| (s.ret, s.params.clone(), s.variadic, s.oldstyle));
        let (ret, params, variadic, oldstyle) =
            sig.ok_or("gọi thứ không phải hàm/con trỏ hàm")?;
        for (i, a) in args.iter_mut().enumerate() {
            if i < params.len() && !oldstyle {
                *a = self.cast(*a, params[i]);
            } else {
                // default argument promotions
                let t = self.ty(*a);
                let pt = if self.tt.is_float(t) { DOUBLE } else { self.promote(t) };
                *a = self.cast(*a, pt);
            }
        }
        let nreg = if variadic { params.len() as u32 } else { args.len() as u32 };
        // struct >16B by value: ABI truyền GIÁN TIẾP — copy vào temp, đưa con trỏ.
        // Ngoại lệ: HFA có NAMED arg đi bằng v0-v7 (anonymous variadic vẫn gián tiếp)
        for (i, a) in args.iter_mut().enumerate() {
            let t = self.ty(*a);
            if matches!(self.tt.tys[t as usize], Ty::Struct(_))
                && self.tt.size(t) > 16
                && ((i as u32) >= nreg || self.tt.hfa(t).is_none())
            {
                let off = self.alloc_local(String::new(), t);
                let tmp = self.push(Node::Var(off), t);
                let asg = self.push(Node::Assign(tmp, *a), t);
                let tmp2 = self.push(Node::Var(off), t);
                let pt = self.tt.ptr_to(t);
                let addr = self.push(Node::Addr(tmp2), pt);
                *a = self.push(Node::Comma(asg, addr), pt);
            }
        }
        let call = if let Node::FunAddr(name) = &self.nodes[callee as usize] {
            let name = name.clone();
            self.push(Node::Call(name, args, nreg), ret)
        } else {
            self.push(Node::CallPtr(callee, args, nreg), ret)
        };
        // trả struct: ≤16B hạ x0/x1 xuống temp; >16B callee ghi thẳng temp qua x8
        if matches!(self.tt.tys[ret as usize], Ty::Struct(_)) {
            let sz = self.tt.size(ret);
            // ≤16B: đệm 16 byte để codegen str nguyên 8-byte không đè slot khác
            let pad = self.tt.add(Ty::Array(CHAR, sz.max(16) as u64));
            let off = self.alloc_local(String::new(), pad);
            return Ok(self.push(Node::SRet(call, off, sz), ret));
        }
        Ok(call)
    }
    fn postfix(&mut self) -> R {
        let e = self.primary()?;
        self.postfix_ops(e)
    }
    // vòng hậu tố tách riêng để compound literal cũng nối được: (int[]){..}[i]
    fn postfix_ops(&mut self, mut e: NodeId) -> R {
        loop {
            if self.eat(&Tok::Punct("[")) {
                let i = self.expr()?;
                self.expect(Tok::Punct("]"))?;
                let sum = self.mkbin("+", e, i)?;
                let t =
                    self.tt.pointee(self.ty(sum)).ok_or("index thứ không phải mảng/con trỏ")?;
                e = self.push(Node::Deref(sum), t);
            } else if self.eat(&Tok::Punct("(")) {
                let mut args = Vec::new();
                if !self.eat(&Tok::Punct(")")) {
                    loop {
                        args.push(self.assign()?);
                        if !self.eat(&Tok::Punct(",")) {
                            break;
                        }
                    }
                    self.expect(Tok::Punct(")"))?;
                }
                e = self.finish_call(e, args)?;
            } else if self.eat(&Tok::Punct(".")) {
                e = self.member(e)?;
            } else if self.eat(&Tok::Punct("->")) {
                let t = self.tt.pointee(self.ty(e)).ok_or("-> trên thứ không phải con trỏ")?;
                let d = self.push(Node::Deref(e), t);
                e = self.member(d)?;
            } else if self.eat(&Tok::Punct("++")) {
                e = self.post_incdec("+", e)?;
            } else if self.eat(&Tok::Punct("--")) {
                e = self.post_incdec("-", e)?;
            } else {
                return Ok(e);
            }
        }
    }
    fn member(&mut self, base: NodeId) -> R {
        let name = self.ident()?;
        let Ty::Struct(sd) = self.tt.tys[self.ty(base) as usize] else {
            return Err("truy cập member trên thứ không phải struct/union".into());
        };
        let (mt, off) =
            self.find_member(sd, &name).ok_or(format!("không có member: {}", name))?;
        Ok(self.push(Node::Member(base, off), mt))
    }
    // attr packed/aligned đứng SAU thân: tính lại layout tại chỗ
    fn repack(&mut self, t: TypeId, packed: bool, aligned: Option<u32>) {
        let Ty::Struct(si) = self.tt.tys[t as usize] else { return };
        let sd = &self.tt.structs[si as usize];
        if sd.is_union
            || sd.members.iter().any(|m| matches!(self.tt.tys[m.1 as usize], Ty::Bitfield(..)))
        {
            return;
        }
        let mut members = sd.members.clone();
        // packed: align hạ về 1 NHƯNG giữ phần aligned tường minh trước đó
        // (sd.align vượt align tự nhiên của member ⟺ có aligned(n) đứng trước)
        let natural =
            sd.members.iter().map(|m| self.tt.align(m.1)).max().unwrap_or(1);
        let mut align = if packed {
            if sd.align > natural { sd.align } else { 1 }
        } else {
            sd.align
        };
        let mut off = 0u32;
        if packed {
            for m in members.iter_mut() {
                m.2 = off;
                off += self.tt.size(m.1);
            }
        } else {
            off = sd.size;
        }
        if let Some(a) = aligned {
            align = align.max(a);
        }
        self.tt.structs[si as usize] = StructDef {
            members,
            size: off.div_ceil(align) * align,
            align,
            is_union: false,
        };
    }
    // tìm member theo tên, xuyên qua anonymous struct/union (tên rỗng)
    fn find_member(&self, sd: u32, name: &str) -> Option<(TypeId, u32)> {
        for (n, t, o) in &self.tt.structs[sd as usize].members {
            if n == name {
                return Some((*t, *o));
            }
            if n.is_empty() {
                if let Ty::Struct(si2) = self.tt.tys[*t as usize] {
                    if let Some((mt, mo)) = self.find_member(si2, name) {
                        return Some((mt, o + mo));
                    }
                }
            }
        }
        None
    }
    fn primary(&mut self) -> R {
        if self.eat(&Tok::Punct("(")) {
            // GNU statement expression ({ ...; expr; }): giá trị = stmt cuối
            if self.peek("{") {
                self.pos += 1;
                let scope = self.locals.len();
                let (ts, ds, es, ets) = (
                    self.tags.clone(),
                    self.typedefs.clone(),
                    self.enums.clone(),
                    self.enum_tags.clone(),
                );
                let mut v = Vec::new();
                while !self.eat(&Tok::Punct("}")) {
                    v.push(self.stmt()?);
                }
                self.locals.truncate(scope);
                self.tags = ts;
                self.typedefs = ds;
                self.enums = es;
                self.enum_tags = ets;
                self.expect(Tok::Punct(")"))?;
                let t = v.last().map_or(INT, |&s| self.ty(s));
                return Ok(self.push(Node::Block(v), t));
            }
            let e = self.expr()?;
            self.expect(Tok::Punct(")"))?;
            Ok(e)
        } else if let Some(&Tok::Num(v, k)) = self.toks.get(self.pos) {
            self.pos += 1;
            let t = match k {
                NumK::I => INT,
                NumK::U => UINT,
                NumK::L => LONG,
                NumK::UL => ULONG,
            };
            Ok(self.push(Node::Num(v), t))
        } else if let Some(&Tok::FNum(v, dbl)) = self.toks.get(self.pos) {
            self.pos += 1;
            let v = if dbl { v } else { v as f32 as f64 };
            Ok(self.push(Node::FNum(v), if dbl { DOUBLE } else { FLOAT }))
        } else if let Some(Tok::Str(bytes, w)) = self.toks.get(self.pos) {
            let (mut bytes, mut w) = (bytes.clone(), *w);
            self.pos += 1;
            while let Some(Tok::Str(more, w2)) = self.toks.get(self.pos) {
                bytes.extend_from_slice(more); // phase 6: nối string liền kề
                w |= *w2;
                self.pos += 1;
            }
            if w {
                // wide: mỗi ký tự → wchar_t (int) little-endian; .asciz thêm
                // 1 NUL nên tự pad 3 — đủ terminator 4 byte
                let cps = wchars(&bytes);
                let n = cps.len() as u32;
                let mut wb = Vec::with_capacity(cps.len() * 4 + 3);
                for c in cps {
                    wb.extend_from_slice(&c.to_le_bytes());
                }
                wb.extend_from_slice(&[0, 0, 0]);
                self.strs.push(wb);
                let i = (self.strs.len() - 1) as u32;
                let t = self.tt.add(Ty::Array(INT, n as u64 + 1));
                return Ok(self.push(Node::Str(i), t));
            }
            self.strs.push(bytes);
            let i = (self.strs.len() - 1) as u32;
            let t = self.tt.add(Ty::Array(CHAR, self.strs[i as usize].len() as u64 + 1));
            Ok(self.push(Node::Str(i), t))
        } else if let Some(Tok::Ident(_)) = self.toks.get(self.pos) {
            let n = self.ident()?;
            if let Some((t, loc)) =
                self.locals.iter().rev().find(|(l, ..)| *l == n).map(|&(_, t, o)| (t, o))
            {
                match loc {
                    Vloc::Stack(off) => return Ok(self.push(Node::Var(off), t)),
                    Vloc::Glob(gi) => return Ok(self.push(Node::GVar(gi), t)),
                    Vloc::Fn => {} // rơi xuống nhánh tra self.fns phía dưới
                }
            }
            if n == "__va_area__" {
                let t = self.tt.ptr_to(CHAR);
                return Ok(self.push(Node::VaArea(self.va_off), t));
            }
            if n == "__func__" || n == "__FUNCTION__" || n == "__PRETTY_FUNCTION__" {
                let bytes = self.fname.clone().into_bytes();
                let ln = bytes.len() as u32;
                self.strs.push(bytes);
                let i = (self.strs.len() - 1) as u32;
                let t = self.tt.add(Ty::Array(CHAR, ln as u64 + 1));
                return Ok(self.push(Node::Str(i), t));
            }
            if n == "__builtin_classify_type" {
                // hằng class của kiểu arg (không eval): int=1 ptr=5 real=8
                // struct=12 union=13 — đủ cho torture
                self.expect(Tok::Punct("("))?;
                let mark = self.nodes.len();
                let e = self.expr()?;
                self.expect(Tok::Punct(")"))?;
                let t = self.ty(e);
                self.nodes.truncate(mark);
                self.types.truncate(mark);
                let cls = match self.tt.tys[t as usize] {
                    Ty::Void => 0,
                    Ty::Float | Ty::Double => 8,
                    Ty::Ptr(_) | Ty::Array(..) | Ty::Func(_) => 5,
                    Ty::Struct(si) => {
                        if self.tt.structs[si as usize].is_union {
                            13
                        } else {
                            12
                        }
                    }
                    _ => 1,
                };
                return Ok(self.push(Node::Num(cls), INT));
            }
            if n == "__builtin_offsetof" {
                self.expect(Tok::Punct("("))?;
                let ty = self.typename()?;
                self.expect(Tok::Punct(","))?;
                // member-designator: ident ("." ident | "[" hằng "]")*
                let (mut t, mut off) = (ty, 0i64);
                loop {
                    let name = self.ident()?;
                    let Ty::Struct(si) = self.tt.tys[t as usize] else {
                        return Err("offsetof trên thứ không phải struct".into());
                    };
                    let (mt, mo) = self
                        .find_member(si, &name)
                        .ok_or_else(|| format!("offsetof: không có member {name}"))?;
                    t = mt;
                    off += mo as i64;
                    loop {
                        if self.eat(&Tok::Punct("[")) {
                            let i = self.const_expr()?;
                            self.expect(Tok::Punct("]"))?;
                            let e = self.tt.pointee(t).ok_or("offsetof: index trên non-array")?;
                            off += i * self.tt.size(e) as i64;
                            t = e;
                        } else {
                            break;
                        }
                    }
                    if !self.eat(&Tok::Punct(".")) {
                        break;
                    }
                }
                self.expect(Tok::Punct(")"))?;
                return Ok(self.push(Node::Num(off), ULONG));
            }
            if let Some(&v) = self.enums.get(&n) {
                return Ok(self.push(Node::Num(v), INT));
            }
            if let Some(gi) = self.globals.iter().position(|g| g.name == n) {
                let t = self.globals[gi].ty;
                return Ok(self.push(Node::GVar(gi as u32), t));
            }
            if let Some(&t) = self.fns.get(&n) {
                let pt = self.tt.ptr_to(t); // function designator decay
                return Ok(self.push(Node::FunAddr(n), pt));
            }
            if self.peek("(") {
                // __builtin_abort... → abort (GCC builtin đổ về libc)
                let n = n.strip_prefix("__builtin_").map(str::to_string).unwrap_or(n);
                if n == "alloca" {
                    // không có symbol libc; sub sp trực tiếp (epilogue mov sp,x29 thu hồi)
                    self.expect(Tok::Punct("("))?;
                    let e = self.expr()?;
                    self.expect(Tok::Punct(")"))?;
                    let t = self.tt.ptr_to(VOID);
                    return Ok(self.push(Node::Alloca(e), t));
                }
                if let Some(&t) = self.fns.get(&n) {
                    let pt = self.tt.ptr_to(t);
                    return Ok(self.push(Node::FunAddr(n), pt));
                }
                // gọi hàm chưa khai báo: implicit int, old-style
                let sig = FnSig {
                    ret: INT,
                    params: Vec::new(),
                    pnames: Vec::new(),
                    variadic: false,
                    oldstyle: true,
                };
                self.tt.fns.push(sig);
                let ft = self.tt.add(Ty::Func(self.tt.fns.len() as u32 - 1));
                self.fns.insert(n.clone(), ft);
                let pt = self.tt.ptr_to(ft);
                return Ok(self.push(Node::FunAddr(n), pt));
            }
            Err(format!("biến chưa khai báo: {}", n))
        } else {
            Err(format!("cần expr, gặp {:?}", self.toks.get(self.pos)))
        }
    }
    fn program(&mut self) -> Result<Vec<Func>, String> {
        let mut funcs = Vec::new();
        while self.pos < self.toks.len() {
            let (bt, storage) = match self.decl_specs()? {
                Some(x) => x,
                None => (INT, Storage::None), // implicit int: main() {...}
            };
            if self.eat(&Tok::Punct(";")) {
                continue; // định nghĩa struct/union/enum thuần
            }
            let (name, t) = self.declarator(bt, true)?;
            // funcdef: declarator ra kiểu Func và theo sau là "{" hoặc old-style decl list
            if let Ty::Func(fidx) = self.tt.tys[t as usize] {
                let is_def = self.peek("{")
                    || matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n) && n != "typedef");
                if is_def {
                    self.fns.insert(name.clone(), t);
                    let sig = self.tt.fns[fidx as usize].clone();
                    self.locals.clear();
                    self.cur_off = 0;
                    self.fret = sig.ret;
                    self.fname = name.clone();
                    // old-style: parse decl list gán kiểu cho từng tên param
                    let mut ptypes: HashMap<String, TypeId> = HashMap::new();
                    if sig.oldstyle {
                        while !self.peek("{") {
                            let (dbt, _) = self.decl_specs()?.ok_or("cần kiểu param old-style")?;
                            loop {
                                let (dn, dt) = self.declarator(dbt, true)?;
                                let dt = match self.tt.tys[dt as usize] {
                                    Ty::Array(e, _) => self.tt.ptr_to(e),
                                    Ty::Func(_) => self.tt.ptr_to(dt),
                                    Ty::Float => DOUBLE, // old-style promotion
                                    _ => dt,
                                };
                                ptypes.insert(dn, dt);
                                if !self.eat(&Tok::Punct(",")) {
                                    break;
                                }
                            }
                            self.expect(Tok::Punct(";"))?;
                        }
                    }
                    let mut params = Vec::new();
                    for (i, pn) in sig.pnames.iter().enumerate() {
                        let pt = if sig.oldstyle {
                            ptypes.get(pn).copied().unwrap_or(INT)
                        } else {
                            sig.params[i]
                        };
                        let off = self.alloc_local(pn.clone(), pt);
                        params.push((off, pt));
                    }
                    // mirror bộ đếm spill của codegen: arg vô danh variadic bắt đầu
                    // ngay sau các named param tràn stack (thường là [x29+16])
                    let (mut gp, mut fp, mut nstk) = (0u32, 0u32, 0u32);
                    for &(_, pt) in &params {
                        if let Some((_, n)) = self.tt.hfa(pt) {
                            if fp + n <= 8 {
                                fp += n;
                            } else {
                                fp = 8; // AAPCS: HFA tràn thì khóa luôn v-reg còn lại
                                nstk += self.tt.size(pt).div_ceil(8);
                            }
                        } else if matches!(self.tt.tys[pt as usize], Ty::Struct(_)) {
                            let need = if self.tt.size(pt) > 16 {
                                1 // >16B: nhận CON TRỎ
                            } else if self.tt.size(pt) > 8 {
                                2
                            } else {
                                1
                            };
                            if gp + need <= 8 {
                                gp += need;
                            } else {
                                nstk += need;
                            }
                        } else if self.tt.is_float(pt) {
                            if fp < 8 {
                                fp += 1;
                            } else {
                                nstk += 1;
                            }
                        } else if gp < 8 {
                            gp += 1;
                        } else {
                            nstk += 1;
                        }
                    }
                    self.va_off = 16 + 8 * nstk;
                    // trả struct >16B: giấu con trỏ đích (x8 lúc vào hàm) trong slot riêng
                    let sret = if matches!(self.tt.tys[sig.ret as usize], Ty::Struct(_))
                        && self.tt.size(sig.ret) > 16
                        && self.tt.hfa(sig.ret).is_none()
                    {
                        self.alloc_local(String::new(), ULONG)
                    } else {
                        0
                    };
                    self.in_fn = true;
                    let body = self.stmt()?;
                    self.in_fn = false;
                    let is_static =
                        storage == Storage::Static || self.static_fns.contains(&name);
                    funcs.push(Func {
                        name,
                        params,
                        frame: (self.cur_off + 15) & !15,
                        body,
                        ret: sig.ret,
                        is_static,
                        variadic: sig.variadic,
                        sret,
                    });
                    continue;
                }
            }
            // không phải funcdef: chuỗi declarator "a, *b, c[2];" — cái đầu đã parse
            let mut cur = (name, t);
            loop {
                let (name, mut t) = cur;
                if storage == Storage::Typedef {
                    self.typedefs.insert(name, t);
                    self.eat(&Tok::Punct("=")); // typedef không init; phòng hờ
                } else if matches!(self.tt.tys[t as usize], Ty::Func(_)) {
                    if storage == Storage::Static {
                        self.static_fns.insert(name.clone());
                    }
                    self.fns.insert(name, t); // prototype
                } else {
                    let init = self.ginit(&mut t)?;
                    let is_extern = storage == Storage::Extern && matches!(init, GInit::None);
                    // tentative definition: int x; int x = 3; int x; → MỘT symbol
                    if let Some(gi) = self.globals.iter().position(|g| g.name == name) {
                        let bigger = self.tt.size(t) > self.tt.size(self.globals[gi].ty);
                        let g = &mut self.globals[gi];
                        if bigger {
                            g.ty = t; // int a[]; → int a[3]; hoàn thiện kiểu
                        }
                        if !matches!(init, GInit::None) {
                            g.init = init;
                        }
                        g.is_extern = g.is_extern && is_extern;
                    } else {
                        self.globals.push(Global {
                            name,
                            ty: t,
                            init,
                            is_static: storage == Storage::Static,
                            is_extern,
                        });
                    }
                }
                if !self.eat(&Tok::Punct(",")) {
                    break;
                }
                cur = self.declarator(bt, true)?;
            }
            self.expect(Tok::Punct(";"))?;
        }
        Ok(funcs)
    }
}

// workaround borrow: decl_specs giữ &str từ token — copy ra ngoài vòng đời
fn n_hack(base: Option<&str>) -> &str {
    base.unwrap_or("")
}

pub fn parse(toks: &[Tok], locs: &[(u32, u32)], files: &[String]) -> Result<Ast, String> {
    let mut p = P {
        toks,
        pos: 0,
        nodes: Vec::new(),
        types: Vec::new(),
        tt: TyTab::new(),
        locals: Vec::new(),
        cur_off: 0,
        globals: Vec::new(),
        strs: Vec::new(),
        fns: HashMap::new(),
        static_fns: std::collections::HashSet::new(),
        tags: HashMap::new(),
        typedefs: HashMap::new(),
        enums: HashMap::new(),
        enum_tags: HashMap::new(),
        switches: Vec::new(),
        fret: INT,
        va_off: 16,
        in_fn: false,
        attr_aligned: None,
        fname: String::new(),
    };
    // libc trả con trỏ: gọi không prototype (torture hay thế) mà để implicit
    // int thì sxtw cắt nửa cao địa chỉ heap → seed sẵn kiểu trả về đúng.
    {
        let pc = p.tt.ptr_to(CHAR);
        for f in [
            "malloc", "calloc", "realloc", "memcpy", "memmove", "memset", "strcpy", "strncpy",
            "strcat", "strncat", "strchr", "strrchr", "strstr", "strdup", "getenv",
        ] {
            let sig = FnSig {
                ret: pc,
                params: Vec::new(),
                pnames: Vec::new(),
                variadic: false,
                oldstyle: true,
            };
            p.tt.fns.push(sig);
            let ft = p.tt.add(Ty::Func(p.tt.fns.len() as u32 - 1));
            p.fns.insert(f.into(), ft);
        }
        // libm trả double (implicit int sẽ đọc x0 thay vì d0)
        for f in ["copysign", "fabs", "sqrt", "floor", "ceil", "fmod", "pow", "atan2"] {
            let sig = FnSig {
                ret: DOUBLE,
                params: Vec::new(),
                pnames: Vec::new(),
                variadic: false,
                oldstyle: true,
            };
            p.tt.fns.push(sig);
            let ft = p.tt.add(Ty::Func(p.tt.fns.len() as u32 - 1));
            p.fns.insert(f.into(), ft);
        }
        // printf family variadic: arg vô danh phải LÊN STACK (Apple) — oldstyle
        // (toàn "đặt tên") sẽ bỏ vào register và libc đọc rác
        for (f, nfix) in [
            ("printf", 1),
            ("sprintf", 2),
            ("snprintf", 3),
            ("fprintf", 2),
            ("scanf", 1),
            ("sscanf", 2),
            ("fscanf", 2),
        ] {
            let sig = FnSig {
                ret: INT,
                params: vec![pc; nfix],
                pnames: vec![String::new(); nfix],
                variadic: true,
                oldstyle: false,
            };
            p.tt.fns.push(sig);
            let ft = p.tt.add(Ty::Func(p.tt.fns.len() as u32 - 1));
            p.fns.insert(f.into(), ft);
        }
    }
    let funcs = p.program().map_err(|e| {
        // vị trí lỗi = token hiện tại của parser → file:line từ preprocess
        match locs.get(p.pos.min(locs.len().saturating_sub(1))) {
            Some(&(f, l)) => format!("{}:{}: {}", files.get(f as usize).map_or("?", |s| s), l, e),
            None => e,
        }
    })?;
    Ok(Ast {
        nodes: p.nodes,
        types: p.types,
        tt: p.tt,
        funcs,
        globals: p.globals,
        strs: p.strs,
    })
}

// L"..": nguồn là UTF-8 → giải mã ra code point cho wchar_t (byte escape lẻ
// >127 không phải UTF-8 hợp lệ sẽ thành U+FFFD — chấp nhận)
fn wchars(b: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(b).chars().map(|c| c as u32).collect()
}

