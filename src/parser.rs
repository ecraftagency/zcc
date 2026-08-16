// Parser: &[Tok] → AST arena (Vec<Node> + NodeId u32) + type arena (TyTab: Vec<Ty> +
// TypeId u32, struct/union member ở bảng structs riêng vì Ty phải Copy).
// Grammar hiện tại (M0–M6):
//   program    = ("typedef" typespec declarator ";" | typespec ";" | typespec "*"* ident
//                 ("(" params ")" (";" | block) | ("[" num "]")? ("=" hằng)? ";"))*
//   typespec   = "int" | "char" | "long" | tên typedef
//              | ("struct" | "union") tag? ("{" (typespec declarator ";")* "}")?
//              | "enum" tag? ("{" ident ("=" num)? ("," ...)* "}")?
//   declarator = "*"* ident ("[" num "]")?
//   stmt       = "return" expr ";" | "if" "(" expr ")" stmt ("else" stmt)?
//              | "while" "(" expr ")" stmt | "for" "(" expr? ";" expr? ";" expr? ")" stmt
//              | "{" stmt* "}" | typespec declarator ("=" expr)? ";" | expr ";"
//   expr       = equality ("=" expr)?        (vế trái phải là lvalue)
//   equality   = relational (("==" | "!=") relational)*
//   relational = add (("<" | "<=" | ">" | ">=") add)*
//   add        = mul (("+" | "-") mul)*      (con trỏ ± int: parser tự chèn *sizeof(elem))
//   mul        = unary (("*" | "/" | "%") unary)*
//   unary      = ("-" | "+" | "*" | "&" | "sizeof") unary | postfix
//   postfix    = primary ("[" expr "]" | "." ident | "->" ident)*
//   primary    = num | string | ident | ident "(" (expr ("," expr)*)? ")" | "(" expr ")"
// Tên hàm trong call KHÔNG kiểm chứng — codegen phát `bl _name`, ld64 lo phần còn lại
// (nhờ vậy gọi được hàm định nghĩa sau và libc miễn phí).
use crate::ast::{Ast, Func, GInit, Global, Node, NodeId, StructDef, Ty, TyTab, TypeId, CHAR, INT, LONG};
use crate::lexer::Tok;
use std::collections::HashMap;

struct P<'a> {
    toks: &'a [Tok],
    pos: usize,
    nodes: Vec<Node>,
    types: Vec<TypeId>,
    tt: TyTab,
    locals: Vec<(String, TypeId, u32)>, // (tên, kiểu, offset)
    cur_off: u32,
    globals: Vec<Global>,
    strs: Vec<Vec<u8>>,
    fns: HashMap<String, (u32, bool)>,  // tên → (số param đặt tên, variadic?)
    tags: HashMap<String, TypeId>,      // tag struct/union
    typedefs: HashMap<String, TypeId>,
    enums: HashMap<String, i64>,        // hằng enum
}

type R = Result<NodeId, String>;

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
    fn num(&mut self) -> Result<i64, String> {
        if let Some(&Tok::Num(v)) = self.toks.get(self.pos) {
            self.pos += 1;
            Ok(v)
        } else {
            Err(format!("cần hằng số, gặp {:?}", self.toks.get(self.pos)))
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
                    val = self.num()?;
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
            let n = self.num()?;
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
    fn stmt(&mut self) -> R {
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
        } else if self.eat(&Tok::Punct("{")) {
            let mut v = Vec::new();
            while !self.eat(&Tok::Punct("}")) {
                v.push(self.stmt()?);
            }
            Ok(self.push(Node::Block(v), INT))
        } else if let Some(bt) = self.typespec()? {
            let (name, t) = self.declarator(bt)?;
            let off = self.alloc_local(name, t);
            let s = if self.eat(&Tok::Punct("=")) {
                let v = self.push(Node::Var(off), t);
                let e = self.expr()?;
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
                    _ if self.tt.size(self.ty(l)).max(self.tt.size(self.ty(r))) == 8 => LONG,
                    _ => INT,
                };
                self.push(Node::Bin(op, l, r), t)
            }
        }
    }
    fn expr(&mut self) -> R {
        let l = self.equality()?;
        if self.eat(&Tok::Punct("=")) {
            if !matches!(
                self.nodes[l as usize],
                Node::Var(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..)
            ) {
                return Err("vế trái '=' không phải lvalue".into());
            }
            let r = self.expr()?; // '=' kết hợp phải
            let t = self.ty(l);
            return Ok(self.push(Node::Assign(l, r), t));
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
    fn equality(&mut self) -> R {
        self.bin(&["==", "!="], Self::relational)
    }
    fn relational(&mut self) -> R {
        self.bin(&["<", "<=", ">", ">="], Self::add)
    }
    fn add(&mut self) -> R {
        self.bin(&["+", "-"], Self::mul)
    }
    fn mul(&mut self) -> R {
        self.bin(&["*", "/", "%"], Self::unary)
    }
    fn unary(&mut self) -> R {
        if self.eat(&Tok::Punct("-")) {
            let e = self.unary()?;
            let t = self.ty(e);
            Ok(self.push(Node::Neg(e), t))
        } else if self.eat(&Tok::Punct("+")) {
            self.unary()
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
        } else if let Some(&Tok::Num(v)) = self.toks.get(self.pos) {
            self.pos += 1;
            Ok(self.push(Node::Num(v), INT))
        } else if let Some(Tok::Str(bytes)) = self.toks.get(self.pos) {
            self.strs.push(bytes.clone());
            let i = (self.strs.len() - 1) as u32;
            self.pos += 1;
            let t = self.tt.add(Ty::Array(CHAR, self.strs[i as usize].len() as u32 + 1));
            Ok(self.push(Node::Str(i), t))
        } else if let Some(Tok::Ident(_)) = self.toks.get(self.pos) {
            let n = self.ident()?;
            if self.eat(&Tok::Punct("(")) {
                let mut args = Vec::new();
                if !self.eat(&Tok::Punct(")")) {
                    loop {
                        args.push(self.expr()?);
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
                return Ok(self.push(Node::Call(n, args, nreg), INT)); // hàm trả int (đủ tới M6)
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
            // biến global; init chỉ nhận hằng (num / chuỗi)
            if p.eat(&Tok::Punct("[")) {
                let n = p.num()?;
                p.expect(Tok::Punct("]"))?;
                t = p.tt.add(Ty::Array(t, n as u32));
            }
            let init = if p.eat(&Tok::Punct("=")) {
                if let Some(Tok::Str(bytes)) = p.toks.get(p.pos) {
                    p.strs.push(bytes.clone());
                    p.pos += 1;
                    GInit::Str((p.strs.len() - 1) as u32)
                } else {
                    let neg = p.eat(&Tok::Punct("-"));
                    let v = p.num()?;
                    GInit::Num(if neg { -v } else { v })
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
