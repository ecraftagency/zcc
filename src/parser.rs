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
    Ast, FnSig, Func, GInit, Global, Node, NodeId, StructDef, Ty, TyTab, TypeId, CHAR, DOUBLE,
    FLOAT, INT, LONG, SHORT, UCHAR, UINT, ULONG, USHORT, VOID,
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
}

// cây initializer: expr / danh sách {..} / string literal
enum Init {
    E(NodeId),
    L(Vec<Init>),
    S(Vec<u8>),
}

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
    tags: HashMap<String, TypeId>,
    typedefs: HashMap<String, TypeId>,
    enums: HashMap<String, i64>,
    switches: Vec<(Vec<(i64, NodeId)>, Option<NodeId>)>,
    fret: TypeId, // kiểu trả về của hàm đang parse
    va_off: u32,  // offset từ x29 đến vùng arg vô danh (16 + 8*named-stack-params)
}

type R = Result<NodeId, String>;

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

const TYPE_WORDS: [&str; 14] = [
    "void", "char", "short", "int", "long", "signed", "unsigned", "float", "double", "struct",
    "union", "enum", "const", "volatile",
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

    // ---- hằng ----
    fn const_expr(&mut self) -> Result<i64, String> {
        let e = self.cond_expr()?;
        self.fold(e)
    }
    fn fold(&self, id: NodeId) -> Result<i64, String> {
        match &self.nodes[id as usize] {
            Node::Num(v) => Ok(*v),
            Node::Neg(e) => Ok(self.fold(*e)?.wrapping_neg()),
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
            Node::Comma(_, r) => self.fold(*r),
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
            Node::Num(v) => Ok(*v as f64),
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
        let mut storage = Storage::None;
        let (mut base, mut direct) = (None::<&str>, None::<TypeId>);
        let (mut uns, mut sgn, mut short, mut longs, mut any) = (false, false, false, 0u32, false);
        loop {
            let n = match self.toks.get(self.pos) {
                Some(Tok::Ident(n)) => n.as_str(),
                _ => break,
            };
            match n {
                "const" | "volatile" | "auto" | "register" => {}
                "typedef" => storage = Storage::Typedef,
                "static" => storage = Storage::Static,
                "extern" => storage = Storage::Extern,
                "void" | "char" | "int" | "float" | "double" => base = Some(n),
                "short" => short = true,
                "long" => longs += 1,
                "signed" => sgn = true,
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
            self.tt.structs.push(StructDef { members: Vec::new(), size: 0, align: 1 });
            let t = self.tt.add(Ty::Struct(self.tt.structs.len() as u32 - 1));
            self.tags.insert(tag, t);
            return Ok(t);
        }
        // có thân: tag đã có placeholder thì định nghĩa vào đúng slot (tự tham chiếu được)
        let t = match tag.as_ref().and_then(|g| self.tags.get(g)).copied() {
            Some(t) => t,
            None => {
                self.tt.structs.push(StructDef { members: Vec::new(), size: 0, align: 1 });
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
        while !self.eat(&Tok::Punct("}")) {
            let (bt, _) = self.decl_specs()?.ok_or("cần kiểu member")?;
            loop {
                let (mn, mt) = self.declarator(bt, true)?;
                let (sz, al) = (self.tt.size(mt), self.tt.align(mt));
                let o = if is_union { 0 } else { off.div_ceil(al) * al };
                members.push((mn, mt, o));
                off = if is_union { off.max(sz) } else { o + sz };
                mx = mx.max(al);
                if !self.eat(&Tok::Punct(",")) {
                    break;
                }
            }
            self.expect(Tok::Punct(";"))?;
        }
        self.tt.structs[sidx as usize] =
            StructDef { members, size: off.div_ceil(mx) * mx, align: mx };
        Ok(t)
    }
    fn enum_spec(&mut self) -> Result<TypeId, String> {
        if let Some(Tok::Ident(n)) = self.toks.get(self.pos) {
            if !self.is_type_word(n) && self.toks.get(self.pos + 1) != Some(&Tok::Punct("{"))
                || self.toks.get(self.pos + 1) == Some(&Tok::Punct("{"))
            {
                self.pos += 1; // tag: nuốt, enum = int nên không cần bảng riêng
            }
        }
        if self.eat(&Tok::Punct("{")) {
            let mut val = 0i64;
            while !self.eat(&Tok::Punct("}")) {
                let name = self.ident()?;
                if self.eat(&Tok::Punct("=")) {
                    val = self.const_expr()?;
                }
                self.enums.insert(name, val);
                val += 1;
                if !self.eat(&Tok::Punct(",")) {
                    self.expect(Tok::Punct("}"))?;
                    break;
                }
            }
        }
        Ok(INT)
    }
    // declarator đầy đủ C: con trỏ, nested "(...)", suffix mảng/hàm.
    // need_name=false cho abstract declarator (cast, sizeof, param không tên).
    fn declarator(&mut self, mut t: TypeId, need_name: bool) -> Result<(String, TypeId), String> {
        while self.eat(&Tok::Punct("*")) {
            t = self.tt.ptr_to(t);
            while self.eat_kw("const") || self.eat_kw("volatile") {}
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
        Ok((name, t))
    }
    fn nested_ahead(&self) -> bool {
        if !self.peek("(") {
            return false;
        }
        match self.toks.get(self.pos + 1) {
            Some(Tok::Punct("*") | Tok::Punct("(")) => true,
            Some(Tok::Ident(n)) => !self.is_type_word(n),
            _ => false,
        }
    }
    fn suffixes(&mut self, t: TypeId) -> Result<TypeId, String> {
        if self.eat(&Tok::Punct("[")) {
            let n = if self.peek("]") { 0 } else { self.const_expr()? as u32 };
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
            Ty::UChar | Ty::UShort => INT, // cả hai lọt trong int (LP64)
            Ty::Float => DOUBLE,
            _ => t,
        }
    }
    fn common_ty(&self, lt: TypeId, rt: TypeId) -> TypeId {
        if self.tt.is_float(lt) || self.tt.is_float(rt) {
            return DOUBLE; // tính trong double — C89 cho phép dư precision
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
            if !["case", "default"].contains(&n.as_str()) && !self.is_type_word(n) {
                let n = n.clone();
                self.pos += 2;
                let st = self.stmt()?;
                return Ok(self.push(Node::Label(n, st), INT));
            }
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
            let i = self.opt_expr(";")?;
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
            let (cases, def) = self.switches.pop().unwrap();
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
            let n = self.ident()?;
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Goto(n), INT))
        } else if self.eat(&Tok::Punct(";")) {
            Ok(self.push(Node::Block(Vec::new()), INT))
        } else if self.eat(&Tok::Punct("{")) {
            let scope = self.locals.len();
            let mut v = Vec::new();
            while !self.eat(&Tok::Punct("}")) {
                v.push(self.stmt()?);
            }
            self.locals.truncate(scope);
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
                            self.fns.insert(name, t); // prototype trong hàm
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
                                // parse init TRƯỚC khi cấp slot: int b[] = {..} cần size
                                let init = self.parse_init()?;
                                t = self.infer_len(t, &init);
                                let off = self.alloc_local(name, t);
                                let v = self.push(Node::Var(off), t);
                                // aggregate {..}/"..": zero-fill trước (partial init)
                                if matches!(&init, Init::L(_) | Init::S(_))
                                    && matches!(
                                        self.tt.tys[t as usize],
                                        Ty::Array(..) | Ty::Struct(_)
                                    )
                                {
                                    let sz = self.tt.size(t);
                                    stmts.push(self.push(Node::Zero(v, sz), VOID));
                                }
                                self.apply_init(v, t, init, &mut stmts)?;
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
                    v.push(self.parse_init()?);
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
        if let Some(Tok::Str(b)) = self.toks.get(self.pos) {
            let mut b = b.clone();
            self.pos += 1;
            while let Some(Tok::Str(m)) = self.toks.get(self.pos) {
                b.extend_from_slice(m);
                self.pos += 1;
            }
            return Ok(Init::S(b));
        }
        Ok(Init::E(self.assign()?))
    }
    // mảng T x[] — suy size từ init
    fn infer_len(&mut self, t: TypeId, init: &Init) -> TypeId {
        if let Ty::Array(e, 0) = self.tt.tys[t as usize] {
            let n = match init {
                Init::L(v) => v.len() as u32,
                Init::S(b) => b.len() as u32 + 1,
                _ => 1,
            };
            return self.tt.add(Ty::Array(e, n));
        }
        t
    }
    // hạ init xuống chuỗi assign trên lvalue (local)
    fn apply_init(
        &mut self,
        lval: NodeId,
        t: TypeId,
        init: Init,
        stmts: &mut Vec<NodeId>,
    ) -> Result<(), String> {
        match (init, self.tt.tys[t as usize]) {
            (Init::S(b), Ty::Array(e, n)) if self.tt.size(e) == 1 => {
                for i in 0..((b.len() as u32 + 1).min(n)) {
                    let v = *b.get(i as usize).map(|x| x).unwrap_or(&0);
                    let idx = self.push(Node::Num(i as i64), LONG);
                    let sum = self.mkbin("+", lval, idx)?;
                    let el = self.push(Node::Deref(sum), e);
                    let num = self.push(Node::Num(v as i64), INT);
                    let a = self.mkassign(el, num)?;
                    stmts.push(a);
                }
                Ok(())
            }
            (Init::L(v), Ty::Array(e, _)) => {
                for (i, it) in v.into_iter().enumerate() {
                    let idx = self.push(Node::Num(i as i64), LONG);
                    let sum = self.mkbin("+", lval, idx)?;
                    let el = self.push(Node::Deref(sum), e);
                    self.apply_init(el, e, it, stmts)?;
                }
                Ok(())
            }
            (Init::L(v), Ty::Struct(si)) => {
                let members: Vec<(TypeId, u32)> = self.tt.structs[si as usize]
                    .members
                    .iter()
                    .map(|&(_, t, o)| (t, o))
                    .collect();
                for (it, (mt, off)) in v.into_iter().zip(members) {
                    let m = self.push(Node::Member(lval, off), mt);
                    self.apply_init(m, mt, it, stmts)?;
                }
                Ok(())
            }
            (Init::L(v), _) => {
                // scalar = {expr}
                let it = v.into_iter().next().ok_or("initializer rỗng")?;
                self.apply_init(lval, t, it, stmts)
            }
            (Init::S(b), _) => {
                // char *p = "str"
                self.strs.push(b);
                let i = (self.strs.len() - 1) as u32;
                let n = self.strs[i as usize].len() as u32 + 1;
                let st = self.tt.add(Ty::Array(CHAR, n));
                let sn = self.push(Node::Str(i), st);
                let a = self.mkassign(lval, sn)?;
                stmts.push(a);
                Ok(())
            }
            (Init::E(e), _) => {
                let a = self.mkassign(lval, e)?;
                stmts.push(a);
                Ok(())
            }
        }
    }
    // phẳng hóa init hằng cho global/static
    fn gflatten(
        &mut self,
        t: TypeId,
        init: Init,
        base: u32,
        out: &mut Vec<(u32, u32, GInit)>,
    ) -> Result<(), String> {
        match (init, self.tt.tys[t as usize]) {
            (Init::S(mut b), Ty::Array(e, n)) if self.tt.size(e) == 1 => {
                b.push(0);
                b.resize(n as usize, 0);
                out.push((base, n, GInit::Bytes(b)));
                Ok(())
            }
            (Init::L(v), Ty::Array(e, _)) => {
                let esz = self.tt.size(e);
                for (i, it) in v.into_iter().enumerate() {
                    self.gflatten(e, it, base + i as u32 * esz, out)?;
                }
                Ok(())
            }
            (Init::L(v), Ty::Struct(si)) => {
                let members: Vec<(TypeId, u32)> = self.tt.structs[si as usize]
                    .members
                    .iter()
                    .map(|&(_, t, o)| (t, o))
                    .collect();
                for (it, (mt, off)) in v.into_iter().zip(members) {
                    self.gflatten(mt, it, base + off, out)?;
                }
                Ok(())
            }
            (Init::L(v), _) => {
                let it = v.into_iter().next().ok_or("initializer rỗng")?;
                self.gflatten(t, it, base, out)
            }
            (Init::S(b), _) => {
                self.strs.push(b);
                out.push((base, 8, GInit::Str((self.strs.len() - 1) as u32)));
                Ok(())
            }
            (Init::E(e), _) => {
                let item = self.gitem(e, t)?;
                out.push((base, self.tt.size(t), item));
                Ok(())
            }
        }
    }
    // một item hằng: số / bits float / địa chỉ symbol / string
    fn gitem(&mut self, mut e: NodeId, t: TypeId) -> Result<GInit, String> {
        while let Node::Cast(inner) = self.nodes[e as usize] {
            e = inner;
        }
        match &self.nodes[e as usize] {
            Node::Str(i) => return Ok(GInit::Str(*i)),
            Node::FunAddr(n) => return Ok(GInit::Addr(n.clone())),
            Node::GVar(gi) => {
                let g = &self.globals[*gi as usize];
                if matches!(self.tt.tys[g.ty as usize], Ty::Array(..)) {
                    return Ok(GInit::Addr(g.name.clone())); // array decay
                }
            }
            Node::Addr(inner) => {
                if let Node::GVar(gi) = self.nodes[*inner as usize] {
                    return Ok(GInit::Addr(self.globals[gi as usize].name.clone()));
                }
            }
            _ => {}
        }
        if self.tt.is_float(t) {
            let v = self.fold_f(e)?;
            let bits =
                if self.tt.size(t) == 4 { (v as f32).to_bits() as i64 } else { v.to_bits() as i64 };
            return Ok(GInit::Num(bits));
        }
        Ok(GInit::Num(self.fold(e)?))
    }
    // init global/static: trả về GInit + chốt size mảng []
    fn ginit(&mut self, t: &mut TypeId) -> Result<GInit, String> {
        if !self.eat(&Tok::Punct("=")) {
            return Ok(GInit::None);
        }
        let init = self.parse_init()?;
        *t = self.infer_len(*t, &init);
        let mut list = Vec::new();
        self.gflatten(*t, init, 0, &mut list)?;
        Ok(GInit::List(list))
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
                let r = if bop.is_empty() { r } else { self.mkbin(bop, l, r)? };
                return self.mkassign(l, r);
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
        let v = self.mkbin(op, e, one)?;
        self.mkassign(e, v)
    }
    fn unary(&mut self) -> R {
        // cast: "(" typename ")"
        if self.peek("(") {
            if let Some(Tok::Ident(n)) = self.toks.get(self.pos + 1) {
                if self.is_type_word(n) {
                    self.pos += 1;
                    let ty = self.typename()?;
                    self.expect(Tok::Punct(")"))?;
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
        } else if self.eat(&Tok::Punct("&")) {
            let e = self.unary()?;
            if matches!(self.nodes[e as usize], Node::FunAddr(_)) {
                return Ok(e); // &f = f với hàm
            }
            let t = self.tt.ptr_to(self.ty(e));
            Ok(self.push(Node::Addr(e), t))
        } else if self.eat_kw("sizeof") {
            // sizeof(typename) | sizeof unary
            let sz = if self.peek("(")
                && matches!(self.toks.get(self.pos + 1), Some(Tok::Ident(n)) if self.is_type_word(n))
            {
                self.pos += 1;
                let t = self.typename()?;
                self.expect(Tok::Punct(")"))?;
                self.tt.size(t)
            } else {
                let e = self.unary()?; // node toán hạng thành rác arena, chấp nhận
                self.tt.size(self.ty(e))
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
        for a in &args {
            let t = self.ty(*a);
            if matches!(self.tt.tys[t as usize], Ty::Struct(_)) && self.tt.size(t) > 16 {
                return Err("struct >16 byte by value: chưa hỗ trợ".into());
            }
        }
        let call = if let Node::FunAddr(name) = &self.nodes[callee as usize] {
            let name = name.clone();
            self.push(Node::Call(name, args, nreg), ret)
        } else {
            self.push(Node::CallPtr(callee, args, nreg), ret)
        };
        // trả struct ≤16B: hạ x0/x1 xuống temp local ẩn, giá trị = địa chỉ temp
        if matches!(self.tt.tys[ret as usize], Ty::Struct(_)) {
            let sz = self.tt.size(ret);
            if sz > 16 {
                return Err("hàm trả struct >16 byte: chưa hỗ trợ".into());
            }
            // temp 16 byte (đệm đủ để codegen str x0/x1 nguyên 8-byte không đè slot khác)
            let pad = self.tt.add(Ty::Array(CHAR, 16));
            let off = self.alloc_local(String::new(), pad);
            return Ok(self.push(Node::SRet(call, off, sz), ret));
        }
        Ok(call)
    }
    fn postfix(&mut self) -> R {
        let mut e = self.primary()?;
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
        let (mt, off) = self.tt.structs[sd as usize]
            .members
            .iter()
            .find(|(n, ..)| *n == name)
            .map(|&(_, t, o)| (t, o))
            .ok_or(format!("không có member: {}", name))?;
        Ok(self.push(Node::Member(base, off), mt))
    }
    fn primary(&mut self) -> R {
        if self.eat(&Tok::Punct("(")) {
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
        } else if let Some(Tok::Str(bytes)) = self.toks.get(self.pos) {
            let mut bytes = bytes.clone();
            self.pos += 1;
            while let Some(Tok::Str(more)) = self.toks.get(self.pos) {
                bytes.extend_from_slice(more); // phase 6: nối string liền kề
                self.pos += 1;
            }
            self.strs.push(bytes);
            let i = (self.strs.len() - 1) as u32;
            let t = self.tt.add(Ty::Array(CHAR, self.strs[i as usize].len() as u32 + 1));
            Ok(self.push(Node::Str(i), t))
        } else if let Some(Tok::Ident(_)) = self.toks.get(self.pos) {
            let n = self.ident()?;
            if let Some((t, loc)) =
                self.locals.iter().rev().find(|(l, ..)| *l == n).map(|&(_, t, o)| (t, o))
            {
                return Ok(match loc {
                    Vloc::Stack(off) => self.push(Node::Var(off), t),
                    Vloc::Glob(gi) => self.push(Node::GVar(gi), t),
                });
            }
            if n == "__va_area__" {
                let t = self.tt.ptr_to(CHAR);
                return Ok(self.push(Node::VaArea(self.va_off), t));
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
}

// workaround borrow: decl_specs giữ &str từ token — copy ra ngoài vòng đời
fn n_hack(base: Option<&str>) -> &str {
    base.unwrap_or("")
}

pub fn parse(toks: &[Tok]) -> Result<Ast, String> {
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
        tags: HashMap::new(),
        typedefs: HashMap::new(),
        enums: HashMap::new(),
        switches: Vec::new(),
        fret: INT,
        va_off: 16,
    };
    let mut funcs = Vec::new();
    while p.pos < toks.len() {
        let (bt, storage) = match p.decl_specs()? {
            Some(x) => x,
            None => (INT, Storage::None), // implicit int: main() {...}
        };
        if p.eat(&Tok::Punct(";")) {
            continue; // định nghĩa struct/union/enum thuần
        }
        let (name, t) = p.declarator(bt, true)?;
        // funcdef: declarator ra kiểu Func và theo sau là "{" hoặc old-style decl list
        if let Ty::Func(fidx) = p.tt.tys[t as usize] {
            let is_def = p.peek("{")
                || matches!(p.toks.get(p.pos), Some(Tok::Ident(n)) if p.is_type_word(n) && n != "typedef");
            if is_def {
                p.fns.insert(name.clone(), t);
                let sig = p.tt.fns[fidx as usize].clone();
                p.locals.clear();
                p.cur_off = 0;
                p.fret = sig.ret;
                // old-style: parse decl list gán kiểu cho từng tên param
                let mut ptypes: HashMap<String, TypeId> = HashMap::new();
                if sig.oldstyle {
                    while !p.peek("{") {
                        let (dbt, _) = p.decl_specs()?.ok_or("cần kiểu param old-style")?;
                        loop {
                            let (dn, dt) = p.declarator(dbt, true)?;
                            let dt = match p.tt.tys[dt as usize] {
                                Ty::Array(e, _) => p.tt.ptr_to(e),
                                Ty::Func(_) => p.tt.ptr_to(dt),
                                Ty::Float => DOUBLE, // old-style promotion
                                _ => dt,
                            };
                            ptypes.insert(dn, dt);
                            if !p.eat(&Tok::Punct(",")) {
                                break;
                            }
                        }
                        p.expect(Tok::Punct(";"))?;
                    }
                }
                let mut params = Vec::new();
                for (i, pn) in sig.pnames.iter().enumerate() {
                    let pt = if sig.oldstyle {
                        ptypes.get(pn).copied().unwrap_or(INT)
                    } else {
                        sig.params[i]
                    };
                    if matches!(p.tt.tys[pt as usize], Ty::Struct(_)) && p.tt.size(pt) > 16 {
                        return Err("param struct >16 byte by value: chưa hỗ trợ".into());
                    }
                    let off = p.alloc_local(pn.clone(), pt);
                    params.push((off, pt));
                }
                // mirror bộ đếm spill của codegen: arg vô danh variadic bắt đầu
                // ngay sau các named param tràn stack (thường là [x29+16])
                let (mut gp, mut fp, mut nstk) = (0u32, 0u32, 0u32);
                for &(_, pt) in &params {
                    if matches!(p.tt.tys[pt as usize], Ty::Struct(_)) {
                        let need = if p.tt.size(pt) > 8 { 2 } else { 1 };
                        if gp + need <= 8 {
                            gp += need;
                        } else {
                            nstk += need;
                        }
                    } else if p.tt.is_float(pt) {
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
                p.va_off = 16 + 8 * nstk;
                let body = p.stmt()?;
                funcs.push(Func {
                    name,
                    params,
                    frame: (p.cur_off + 15) & !15,
                    body,
                    ret: sig.ret,
                    is_static: storage == Storage::Static,
                    variadic: sig.variadic,
                });
                continue;
            }
        }
        // không phải funcdef: chuỗi declarator "a, *b, c[2];" — cái đầu đã parse
        let mut cur = (name, t);
        loop {
            let (name, mut t) = cur;
            if storage == Storage::Typedef {
                p.typedefs.insert(name, t);
                p.eat(&Tok::Punct("=")); // typedef không init; phòng hờ
            } else if matches!(p.tt.tys[t as usize], Ty::Func(_)) {
                p.fns.insert(name, t); // prototype
            } else {
                let init = p.ginit(&mut t)?;
                let is_extern = storage == Storage::Extern && matches!(init, GInit::None);
                p.globals.push(Global {
                    name,
                    ty: t,
                    init,
                    is_static: storage == Storage::Static,
                    is_extern,
                });
            }
            if !p.eat(&Tok::Punct(",")) {
                break;
            }
            cur = p.declarator(bt, true)?;
        }
        p.expect(Tok::Punct(";"))?;
    }
    Ok(Ast {
        nodes: p.nodes,
        types: p.types,
        tt: p.tt,
        funcs,
        globals: p.globals,
        strs: p.strs,
    })
}
