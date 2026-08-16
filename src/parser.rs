// Parser: &[Tok] → AST arena (Vec<Node> + NodeId u32) + type arena (TyTab: Vec<Ty> +
// TypeId u32, struct/union member ở bảng structs riêng vì Ty phải Copy).
// Grammar hiện tại (M0–M7 + 2a):
//   program    = ("typedef" typespec declarator ";" | typespec ";" | typespec "*"* ident
//                 ("(" params ")" (";" | block) | ("[" const "]")? ("=" hằng)? ";"))*
//   typespec   = "int" | "char" | "long" | tên typedef
//              | ("struct" | "union") tag? ("{" (typespec declarator ";")* "}")?
//              | "enum" tag? ("{" ident ("=" const)? ("," ...)* "}")?
//   stmt       = "return" expr ";" | if/while/for/do-while/switch/case/default
//              | break/continue/goto/label | "{" stmt* "}" (mở scope)
//              | typespec declarator ("=" assign)? ";" | ";" | expr ";"
//   expr       = assign ("," assign)*
//   assign     = cond (("=" | "+=" | "-=" | ... ) assign)?   (vế trái lvalue)
//   cond       = lor ("?" expr ":" cond)?
//   lor/land   = || && — desugar thành Cond lồng nhau (short-circuit + chuẩn hóa 0/1)
//   bitor..and = "|" "^" "&" từng tầng; equality → relational → shift → add → mul
//   unary      = ("-" "+" "*" "&" "!" "~" "++" "--" "sizeof") unary | postfix
//   postfix    = primary ("[" expr "]" | "." id | "->" id | "++" | "--")*
//   primary    = num | string | ident | call | "(" expr ")"
// Tên hàm trong call KHÔNG kiểm chứng — codegen phát `bl _name`, ld64 lo phần còn lại.
use crate::ast::{Ast, Func, GInit, Global, Node, NodeId, StructDef, Ty, TyTab, TypeId, CHAR, INT, LONG};
use crate::lexer::{NumK, Tok};
use std::collections::HashMap;

struct P<'a> {
    toks: &'a [Tok],
    pos: usize,
    nodes: Vec<Node>,
    types: Vec<TypeId>,
    tt: TyTab,
    locals: Vec<(String, TypeId, u32)>, // (tên, kiểu, offset); scope = truncate khi đóng block
    cur_off: u32,
    globals: Vec<Global>,
    strs: Vec<Vec<u8>>,
    fns: HashMap<String, (u32, bool)>, // tên → (số param đặt tên, variadic?)
    tags: HashMap<String, TypeId>,     // tag struct/union
    typedefs: HashMap<String, TypeId>,
    enums: HashMap<String, i64>,                       // hằng enum
    switches: Vec<(Vec<(i64, NodeId)>, Option<NodeId>)>, // stack switch đang mở
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
    // constant-expression: parse như cond rồi fold — node xây dở thành rác arena
    fn const_expr(&mut self) -> Result<i64, String> {
        let e = self.cond_expr()?;
        self.fold(e)
    }
    fn fold(&self, id: NodeId) -> Result<i64, String> {
        match &self.nodes[id as usize] {
            Node::Num(v) => Ok(*v),
            Node::Neg(e) => Ok(self.fold(*e)?.wrapping_neg()),
            Node::Cond(c, t, e) => {
                if self.fold(*c)? != 0 {
                    self.fold(*t)
                } else {
                    self.fold(*e)
                }
            }
            Node::Comma(_, r) => self.fold(*r),
            Node::Bin(op, l, r) => {
                let (l, r) = (self.fold(*l)?, self.fold(*r)?);
                Ok(match *op {
                    "+" => l.wrapping_add(r),
                    "-" => l.wrapping_sub(r),
                    "*" => l.wrapping_mul(r),
                    "/" | "%" if r == 0 => return Err("chia 0 trong hằng".into()),
                    "/" => l.wrapping_div(r),
                    "%" => l.wrapping_rem(r),
                    "&" => l & r,
                    "|" => l | r,
                    "^" => l ^ r,
                    "<<" => l.wrapping_shl(r as u32),
                    ">>" => l.wrapping_shr(r as u32),
                    "==" => (l == r) as i64,
                    "!=" => (l != r) as i64,
                    "<" => (l < r) as i64,
                    "<=" => (l <= r) as i64,
                    ">" => (l > r) as i64,
                    ">=" => (l >= r) as i64,
                    _ => return Err("op không dùng được trong hằng".into()),
                })
            }
            _ => Err("cần biểu thức hằng".into()),
        }
    }
    // None = token hiện tại không mở đầu một kiểu (→ caller thử hướng khác)
    fn typespec(&mut self) -> Result<Option<TypeId>, String> {
        for (kw, t) in [("int", INT), ("char", CHAR), ("long", LONG)] {
            if self.eat_kw(kw) {
                return Ok(Some(t));
            }
        }
        for (kw, is_union) in [("struct", false), ("union", true)] {
            if self.eat_kw(kw) {
                return self.struct_union(is_union).map(Some);
            }
        }
        if self.eat_kw("enum") {
            return self.enum_spec().map(Some);
        }
        if let Some(Tok::Ident(n)) = self.toks.get(self.pos) {
            if let Some(&t) = self.typedefs.get(n) {
                self.pos += 1;
                return Ok(Some(t));
            }
        }
        Ok(None)
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
            return self.tags.get(&tag).copied().ok_or(format!("tag chưa định nghĩa: {}", tag));
        }
        let mut members = Vec::new();
        let (mut off, mut mx) = (0u32, 1u32);
        while !self.eat(&Tok::Punct("}")) {
            let bt = self.typespec()?.ok_or("cần kiểu member")?;
            let (mn, mt) = self.declarator(bt)?;
            let (sz, al) = (self.tt.size(mt), self.tt.align(mt));
            let o = if is_union { 0 } else { off.div_ceil(al) * al };
            members.push((mn, mt, o));
            off = if is_union { off.max(sz) } else { o + sz };
            mx = mx.max(al);
            self.expect(Tok::Punct(";"))?;
        }
        self.tt.structs.push(StructDef { members, size: off.div_ceil(mx) * mx, align: mx });
        let t = self.tt.add(Ty::Struct(self.tt.structs.len() as u32 - 1));
        if let Some(tag) = tag {
            self.tags.insert(tag, t);
        }
        Ok(t)
    }
    // enum = int; thân chỉ sinh hằng vào bảng enums
    fn enum_spec(&mut self) -> Result<TypeId, String> {
        if let Some(Tok::Ident(_)) = self.toks.get(self.pos) {
            self.pos += 1; // tag: nuốt, không cần bảng riêng
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
    fn declarator(&mut self, mut t: TypeId) -> Result<(String, TypeId), String> {
        while self.eat(&Tok::Punct("*")) {
            t = self.tt.ptr_to(t);
        }
        let name = self.ident()?;
        if self.eat(&Tok::Punct("[")) {
            let n = self.const_expr()?;
            self.expect(Tok::Punct("]"))?;
            t = self.tt.add(Ty::Array(t, n as u32));
        }
        Ok((name, t))
    }
    // Cấp slot trên stack: offset đi xuống từ x29, thẳng hàng theo align của kiểu
    fn alloc_local(&mut self, name: String, t: TypeId) -> u32 {
        let (sz, al) = (self.tt.size(t), self.tt.align(t));
        self.cur_off = (self.cur_off + sz).div_ceil(al) * al;
        self.locals.push((name, t, self.cur_off));
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
    fn stmt(&mut self) -> R {
        // label: ident ":" — phải bắt trước expr; case/default đã ăn bởi eat_kw phía dưới
        if let (Some(Tok::Ident(n)), Some(Tok::Punct(":"))) =
            (self.toks.get(self.pos), self.toks.get(self.pos + 1))
        {
            if !["case", "default"].contains(&n.as_str()) && !self.typedefs.contains_key(n) {
                let n = n.clone();
                self.pos += 2;
                let st = self.stmt()?;
                return Ok(self.push(Node::Label(n, st), INT));
            }
        }
        if self.eat_kw("return") {
            let e = self.expr()?;
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Ret(e), INT))
        } else if self.eat_kw("if") {
            self.expect(Tok::Punct("("))?;
            let c = self.expr()?;
            self.expect(Tok::Punct(")"))?;
            let t = self.stmt()?;
            let e = if self.eat_kw("else") { Some(self.stmt()?) } else { None };
            Ok(self.push(Node::If(c, t, e), INT))
        } else if self.eat_kw("while") {
            self.expect(Tok::Punct("("))?;
            let c = self.expr()?;
            self.expect(Tok::Punct(")"))?;
            let b = self.stmt()?;
            Ok(self.push(Node::While(c, b), INT))
        } else if self.eat_kw("for") {
            self.expect(Tok::Punct("("))?;
            let i = self.opt_expr(";")?;
            let c = self.opt_expr(";")?;
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
        } else if let Some(bt) = self.typespec()? {
            let (name, t) = self.declarator(bt)?;
            let off = self.alloc_local(name, t);
            let s = if self.eat(&Tok::Punct("=")) {
                let v = self.push(Node::Var(off), t);
                let e = self.assign()?;
                self.push(Node::Assign(v, e), t)
            } else {
                self.push(Node::Block(Vec::new()), INT) // khai báo trần: không sinh code
            };
            self.expect(Tok::Punct(";"))?;
            Ok(s)
        } else {
            let e = self.expr()?;
            self.expect(Tok::Punct(";"))?;
            Ok(e)
        }
    }
    // expr? kết thúc bằng `end` (cho 3 khoang của for), nuốt luôn `end`
    fn opt_expr(&mut self, end: &'static str) -> Result<Option<NodeId>, String> {
        if self.eat(&Tok::Punct(end)) {
            return Ok(None);
        }
        let e = self.expr()?;
        self.expect(Tok::Punct(end))?;
        Ok(Some(e))
    }
    // Dựng node binary op kèm typing + scale con trỏ (p±n → p±n*sizeof(elem), p-p → /sizeof)
    fn mkbin(&mut self, op: &'static str, l: NodeId, r: NodeId) -> NodeId {
        let (lp, rp) = (self.tt.pointee(self.ty(l)), self.tt.pointee(self.ty(r)));
        match (op, lp, rp) {
            ("+", None, Some(_)) => self.mkbin("+", r, l), // int + ptr: giao hoán
            ("+" | "-", Some(e), None) => {
                let sz = self.push(Node::Num(self.tt.size(e) as i64), LONG);
                let r = self.push(Node::Bin("*", r, sz), LONG);
                let t = self.tt.ptr_to(e);
                self.push(Node::Bin(op, l, r), t)
            }
            ("-", Some(e), Some(_)) => {
                let d = self.push(Node::Bin("-", l, r), LONG);
                let sz = self.push(Node::Num(self.tt.size(e) as i64), LONG);
                self.push(Node::Bin("/", d, sz), LONG)
            }
            _ => {
                let t = match op {
                    "==" | "!=" | "<" | "<=" | ">" | ">=" => INT,
                    "<<" | ">>" => self.ty(l), // kiểu = vế trái (promoted)
                    _ if self.tt.size(self.ty(l)).max(self.tt.size(self.ty(r))) == 8 => LONG,
                    _ => INT,
                };
                self.push(Node::Bin(op, l, r), t)
            }
        }
    }
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
                let r = self.assign()?; // kết hợp phải
                let r = if bop.is_empty() { r } else { self.mkbin(bop, l, r) };
                let t = self.ty(l);
                return Ok(self.push(Node::Assign(l, r), t));
            }
        }
        Ok(l)
    }
    fn cond_expr(&mut self) -> R {
        let c = self.lor()?;
        if !self.eat(&Tok::Punct("?")) {
            return Ok(c);
        }
        let t = self.expr()?;
        self.expect(Tok::Punct(":"))?;
        let e = self.cond_expr()?;
        let ty = if self.tt.size(self.ty(t)) >= self.tt.size(self.ty(e)) {
            self.ty(t)
        } else {
            self.ty(e)
        };
        Ok(self.push(Node::Cond(c, t, e), ty))
    }
    // a || b → a ? 1 : (b ? 1 : 0); a && b → a ? (b ? 1 : 0) : 0 — short-circuit + 0/1
    fn lor(&mut self) -> R {
        let mut l = self.land()?;
        while self.eat(&Tok::Punct("||")) {
            let r = self.land()?;
            let one = self.push(Node::Num(1), INT);
            let zero = self.push(Node::Num(0), INT);
            let rb = self.push(Node::Cond(r, one, zero), INT);
            l = self.push(Node::Cond(l, one, rb), INT);
        }
        Ok(l)
    }
    fn land(&mut self) -> R {
        let mut l = self.bitor()?;
        while self.eat(&Tok::Punct("&&")) {
            let r = self.bitor()?;
            let one = self.push(Node::Num(1), INT);
            let zero = self.push(Node::Num(0), INT);
            let rb = self.push(Node::Cond(r, one, zero), INT);
            l = self.push(Node::Cond(l, rb, zero), INT);
        }
        Ok(l)
    }
    // Một tầng binary op trái-kết-hợp: next (op next)*
    fn bin(&mut self, ops: &[&'static str], next: fn(&mut Self) -> R) -> R {
        let mut l = next(self)?;
        'again: loop {
            for &op in ops {
                if self.eat(&Tok::Punct(op)) {
                    let r = next(self)?;
                    l = self.mkbin(op, l, r);
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
    // ++x → x = x + 1 (mkbin tự scale con trỏ)
    fn incdec_pre(&mut self, op: &'static str) -> R {
        let e = self.unary()?;
        self.check_lval(e)?;
        let one = self.push(Node::Num(1), INT);
        let v = self.mkbin(op, e, one);
        let t = self.ty(e);
        Ok(self.push(Node::Assign(e, v), t))
    }
    fn unary(&mut self) -> R {
        if self.eat(&Tok::Punct("-")) {
            let e = self.unary()?;
            let t = self.ty(e);
            Ok(self.push(Node::Neg(e), t))
        } else if self.eat(&Tok::Punct("+")) {
            self.unary()
        } else if self.eat(&Tok::Punct("!")) {
            let e = self.unary()?;
            let z = self.push(Node::Num(0), INT);
            Ok(self.mkbin("==", e, z))
        } else if self.eat(&Tok::Punct("~")) {
            let e = self.unary()?;
            let m = self.push(Node::Num(-1), INT);
            Ok(self.mkbin("^", e, m))
        } else if self.eat(&Tok::Punct("++")) {
            self.incdec_pre("+")
        } else if self.eat(&Tok::Punct("--")) {
            self.incdec_pre("-")
        } else if self.eat(&Tok::Punct("*")) {
            let e = self.unary()?;
            let t = self.tt.pointee(self.ty(e)).ok_or("deref thứ không phải con trỏ")?;
            Ok(self.push(Node::Deref(e), t))
        } else if self.eat(&Tok::Punct("&")) {
            let e = self.unary()?;
            let t = self.tt.ptr_to(self.ty(e));
            Ok(self.push(Node::Addr(e), t))
        } else if self.eat_kw("sizeof") {
            let e = self.unary()?; // node toán hạng thành rác trong arena, chấp nhận
            let sz = self.tt.size(self.ty(e));
            Ok(self.push(Node::Num(sz as i64), LONG))
        } else {
            self.postfix()
        }
    }
    fn post_incdec(&mut self, op: &'static str, e: NodeId) -> R {
        self.check_lval(e)?;
        let t = self.ty(e);
        let delta = self.tt.pointee(t).map_or(1, |p| self.tt.size(p) as i64);
        Ok(self.push(Node::Post(op, e, delta), t))
    }
    fn postfix(&mut self) -> R {
        let mut e = self.primary()?;
        loop {
            if self.eat(&Tok::Punct("[")) {
                let i = self.expr()?;
                self.expect(Tok::Punct("]"))?;
                let sum = self.mkbin("+", e, i);
                let t =
                    self.tt.pointee(self.ty(sum)).ok_or("index thứ không phải mảng/con trỏ")?;
                e = self.push(Node::Deref(sum), t);
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
                NumK::I | NumK::U => INT,
                NumK::L | NumK::UL => LONG,
            };
            Ok(self.push(Node::Num(v), t))
        } else if let Some(Tok::Str(bytes)) = self.toks.get(self.pos) {
            let mut bytes = bytes.clone();
            self.pos += 1;
            // phase 6: string literal liền kề nối lại
            while let Some(Tok::Str(more)) = self.toks.get(self.pos) {
                bytes.extend_from_slice(more);
                self.pos += 1;
            }
            self.strs.push(bytes);
            let i = (self.strs.len() - 1) as u32;
            let t = self.tt.add(Ty::Array(CHAR, self.strs[i as usize].len() as u32 + 1));
            Ok(self.push(Node::Str(i), t))
        } else if let Some(Tok::Ident(_)) = self.toks.get(self.pos) {
            let n = self.ident()?;
            if self.eat(&Tok::Punct("(")) {
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
                // variadic: chỉ số param đặt tên đi thanh ghi; hàm chưa khai báo coi như thường
                let nreg = match self.fns.get(&n) {
                    Some(&(nnamed, true)) => nnamed,
                    _ => args.len() as u32,
                };
                return Ok(self.push(Node::Call(n, args, nreg), INT)); // hàm trả int (đến 2b)
            }
            if let Some((t, off)) =
                self.locals.iter().rev().find(|(l, ..)| *l == n).map(|&(_, t, o)| (t, o))
            {
                return Ok(self.push(Node::Var(off), t));
            }
            if let Some(&v) = self.enums.get(&n) {
                return Ok(self.push(Node::Num(v), INT));
            }
            if let Some(gi) = self.globals.iter().position(|g| g.name == n) {
                let t = self.globals[gi].ty;
                return Ok(self.push(Node::GVar(gi as u32), t));
            }
            Err(format!("biến chưa khai báo: {}", n))
        } else {
            Err(format!("cần expr, gặp {:?}", self.toks.get(self.pos)))
        }
    }
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
    };
    let mut funcs = Vec::new();
    while p.pos < toks.len() {
        if p.eat_kw("typedef") {
            let bt = p.typespec()?.ok_or("cần kiểu sau typedef")?;
            let (name, t) = p.declarator(bt)?;
            p.typedefs.insert(name, t);
            p.expect(Tok::Punct(";"))?;
            continue;
        }
        let bt = p.typespec()?.ok_or("cần kiểu")?;
        if p.eat(&Tok::Punct(";")) {
            continue; // định nghĩa struct/union/enum thuần
        }
        let mut t = bt;
        while p.eat(&Tok::Punct("*")) {
            t = p.tt.ptr_to(t);
        }
        let name = p.ident()?;
        if p.eat(&Tok::Punct("(")) {
            // funcdef hoặc prototype; t = kiểu trả về, chưa dùng đến
            p.locals.clear();
            p.cur_off = 0;
            let (mut params, mut variadic) = (Vec::new(), false);
            if !p.eat(&Tok::Punct(")")) {
                loop {
                    let pbt = p.typespec()?.ok_or("cần kiểu tham số")?;
                    let (pn, pt) = p.declarator(pbt)?;
                    let off = p.alloc_local(pn, pt);
                    params.push((off, p.tt.size(pt)));
                    if !p.eat(&Tok::Punct(",")) {
                        break;
                    }
                    if p.eat(&Tok::Punct("...")) {
                        variadic = true;
                        break;
                    }
                }
                p.expect(Tok::Punct(")"))?;
            }
            p.fns.insert(name.clone(), (params.len() as u32, variadic));
            if p.eat(&Tok::Punct(";")) {
                continue; // prototype thuần: chỉ ghi sổ
            }
            let body = p.stmt()?;
            funcs.push(Func { name, params, frame: (p.cur_off + 15) & !15, body });
        } else {
            // biến global; init chỉ nhận hằng (const expr / chuỗi)
            if p.eat(&Tok::Punct("[")) {
                let n = p.const_expr()?;
                p.expect(Tok::Punct("]"))?;
                t = p.tt.add(Ty::Array(t, n as u32));
            }
            let init = if p.eat(&Tok::Punct("=")) {
                if let Some(Tok::Str(bytes)) = p.toks.get(p.pos) {
                    p.strs.push(bytes.clone());
                    p.pos += 1;
                    GInit::Str((p.strs.len() - 1) as u32)
                } else {
                    GInit::Num(p.const_expr()?)
                }
            } else {
                GInit::None
            };
            p.expect(Tok::Punct(";"))?;
            p.globals.push(Global { name, ty: t, init });
        }
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
