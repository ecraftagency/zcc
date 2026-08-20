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
    AsmOp, Ast, BOOL, CHAR, DOUBLE, FLOAT, FnSig, Func, GInit, Global, INT, LDOUBLE, LONG, Node, NodeId,
    SHORT, StructDef, SyncOp, Ty, TyTab, TypeId, UCHAR, UINT, ULONG, USHORT, VOID,
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
    S(Vec<u8>, u8), // element width byte (1 narrow, 2 char16, 4 wchar/char32)
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
    switches: Vec<(Vec<(i64, i64, NodeId)>, Option<NodeId>)>,
    fret: TypeId,              // kiểu trả về của hàm đang parse
    va_off: u32,               // offset từ x29 đến vùng arg vô danh (16 + 8*named-stack-params)
    in_fn: bool,               // đang trong body hàm (compound literal: local vs global ẩn)
    attr_aligned: Option<u32>, // aligned(n) lơ lửng từ decl_specs (member: pr23467)
    saw_inline: bool,          // EXT(gcc): decl_specs vừa gặp inline/__inline (gnu89)
    saw_thread: bool,          // EXT(gcc): decl_specs vừa gặp __thread (capture NGAY sau
    // decl_specs — init chứa cast sẽ gọi decl_specs lồng làm reset cờ)
    // EXT(gcc): tên đã có file-scope declaration KHÔNG inline — C99 6.7.4p7:
    // định nghĩa inline của tên đó vẫn là external definition (logreqres redis)
    plain_decls: std::collections::HashSet<String>,
    // C99: declarator vừa gặp mảng size không hằng (VLA) — expr của size;
    // chỉ stmt() (local decl) tiêu thụ, hạ xuống con trỏ + Alloca
    vla_size: Option<NodeId>,
    // đang parse param list (đếm vì lồng nhau): size mảng param không hằng
    // (glibc regex.h `__pmatch[__nmatch]` — __nmatch là param TRƯỚC, không có
    // trong scope) → bỏ qua expr, param decay về con trỏ nên size vô nghĩa
    in_params: u32,
    // EXT(gcc): local ghim reg `register long v __asm__("x8")` (musl syscall)
    // — key = offset stack của local, value = số reg GP
    reg_pins: HashMap<u32, u8>,
    // C99: con trỏ tới VLA `char (*p)[w]` (musl lsearch/dynlink) — key =
    // TypeId của Array(elem, 0) pointee (không intern nên unique mỗi decl),
    // value = offset local ẩn giữ SIZE BYTE runtime; mkbin scale theo nó
    vla_arrs: HashMap<TypeId, u32>,
    // C99 6.5.3.4p2: sizeof(vla local) là giá trị RUNTIME — key = offset
    // local VLA, value = offset local ẩn `.vlasz` giữ số byte (chốt một lần
    // lúc khai báo). Offset tái dụng giữa các hàm → PHẢI clear mỗi hàm.
    vla_szs: HashMap<u32, u32>,
    // C99: _Complex t (musl src/complex) hạ về struct {re, im} — layout,
    // union punning, HFA ABI (AAPCS64 coi complex như struct 2 phần tử) ăn
    // nguyên máy móc struct sẵn có. Key = elem (FLOAT/DOUBLE), value = TypeId struct.
    cplx_tys: HashMap<TypeId, TypeId>,
    fname: String, // tên hàm đang parse (label symbol cho &&label trong static init)
    asm_label: Option<String>, // EXT(gcc): __asm("_sym") vừa nuốt trong skip_attrs
    renames: HashMap<String, String>, // EXT(gcc): tên C → symbol __asm (SDK versioning)
    attr_weak: bool,            // EXT(gcc): __attribute__((weak)) (musl)
    attr_transp: bool,          // EXT(gcc): transparent_union (glibc sockaddr arg)
    attr_alias: Option<String>, // EXT(gcc): __attribute__((alias("sym"))) (musl weak_alias)
    raw_asm: Vec<String>,       // EXT(gcc): __asm__("...") cấp toàn cục (musl crt)
    aliases: Vec<(String, String, bool)>, // (mới, cũ, weak)
    // EXT(gcc): prototype mang weak (musl `extern weak hidden _DYNAMIC[]`) —
    // TU tham chiếu phải phát .weak kẻo strong undef ref làm link đòi symbol
    weak_decls: Vec<String>,
    // EXT(gcc): nested function (GNU, chỉ ELF). fn_uid = bộ đếm cấp uid; cur_uid/
    // cur_parent_uid = danh tính hàm ĐANG parse; upvar_base = mốc locals: index
    // < mốc là biến hàm bao → Upvar. nested_fns: tên nguồn → (symbol mangled,
    // parent_uid) để tham chiếu hạ về Tramp. nl_labels: (uid chủ, tên __label__)
    // cho non-local goto. pending: nested func gom được, drain sau mỗi top-level.
    fn_uid: u32,
    cur_uid: u32,
    cur_parent_uid: u32,
    upvar_base: usize,
    nested_fns: HashMap<String, (String, u32)>,
    nl_labels: Vec<(u32, String)>,
    pending: Vec<Func>,
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

const TYPE_WORDS: [&str; 25] = [
    "void",
    "char",
    "short",
    "int",
    "long",
    "signed",
    "unsigned",
    "float",
    "double",
    "struct",
    "union",
    "enum",
    "const",
    "volatile",
    "_Bool",
    "_Complex",     // C99
    "__complex__",  // EXT(gcc): bí danh _Complex
    "__complex",    // EXT(gcc): dạng không gạch đuôi (`__complex double`)
    "__const",
    "__volatile",
    "__signed",
    "__signed__",
    "__typeof__",
    "__typeof",
    "typeof", // EXT(gcc): typeof trần — kernel/coreutils/gnulib xài (rủi ro nhỏ:
              // chương trình C89 đặt biến tên `typeof` sẽ vỡ; không code thật nào làm)
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
            // typedef bị biến local CÙNG TÊN shadow (redis: quicklist *quicklist)
            // — sau dòng khai báo đó, tên là biến chứ không còn là kiểu
            // (chỉ soi shadow TRONG body — ngoài body locals là đồ thừa hàm trước)
            || (self.typedefs.contains_key(n)
                && !(self.in_fn && self.locals.iter().any(|(ln, ..)| ln == n)))
            || ["typedef", "static", "extern", "auto", "register"].contains(&n)
    }
    // keyword cứng — typedef-name KHÔNG tính (vẫn được làm tên declarator/member)
    fn is_keyword(&self, n: &str) -> bool {
        TYPE_WORDS.contains(&n) || ["typedef", "static", "extern", "auto", "register"].contains(&n)
    }

    // ---- hằng ----
    // EXT(gcc): compat cấu trúc cho __builtin_types_compatible_p — TyTab không
    // intern nên so đệ quy. Func so CHỮ KÝ (C99 6.7.5.3): return + số param +
    // từng param + cờ variadic phải khớp (chibicc builtin.c phân biệt
    // int(*)(float,double) ≠ int(*)(float), (...) ≠ (void)).
    fn ty_compat(&self, a: TypeId, b: TypeId) -> bool {
        match (&self.tt.tys[a as usize], &self.tt.tys[b as usize]) {
            (Ty::Ptr(x), Ty::Ptr(y)) => self.ty_compat(*x, *y),
            (Ty::Array(x, n), Ty::Array(y, m)) => n == m && self.ty_compat(*x, *y),
            (Ty::Struct(x), Ty::Struct(y)) => x == y,
            (Ty::Func(x), Ty::Func(y)) => {
                let (fx, fy) = (&self.tt.fns[*x as usize], &self.tt.fns[*y as usize]);
                fx.variadic == fy.variadic
                    && fx.params.len() == fy.params.len()
                    && self.ty_compat(fx.ret, fy.ret)
                    && (0..fx.params.len()).all(|i| self.ty_compat(fx.params[i], fy.params[i]))
            }
            (x, y) => std::mem::discriminant(x) == std::mem::discriminant(y),
        }
    }
    fn const_expr(&mut self) -> Result<i64, String> {
        let e = self.cond_expr()?;
        self.fold(e)
    }
    // EXT(c11): _Static_assert(const-expr[, "msg"]); — declaration ở cả
    // file-scope lẫn block (postgres18 StaticAssertStmt/Decl đòi). Đánh giá
    // NGAY lúc parse: fail = lỗi biên dịch, pass = không sinh node nào.
    // Msg optional (C23 cho bỏ — nuốt luôn cho rẻ).
    fn static_assert(&mut self) -> Result<(), String> {
        self.expect(Tok::Punct("("))?;
        let v = self.const_expr()?;
        let mut msg = String::new();
        if self.eat(&Tok::Punct(",")) {
            while let Some(Tok::Str(b, _)) = self.toks.get(self.pos) {
                msg.push_str(&String::from_utf8_lossy(b)); // chuỗi liền kề tự nối
                self.pos += 1;
            }
        }
        self.expect(Tok::Punct(")"))?;
        self.expect(Tok::Punct(";"))?;
        if v == 0 {
            return Err(format!("_Static_assert fail: {msg}"));
        }
        Ok(())
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
        // cây con kiểu NGUYÊN (musl: (double)(1 << k) trong POWF_SCALE):
        // fold nguyên trọn vẹn rồi đổi sang thực theo dấu
        if !self.tt.is_float(self.ty(id)) {
            if let Ok(v) = self.fold(id) {
                return Ok(if self.tt.is_unsigned(self.ty(id)) {
                    v as u64 as f64
                } else {
                    v as f64
                });
            }
        }
        match &self.nodes[id as usize] {
            Node::FNum(v) => Ok(*v),
            // unsigned 64-bit: 9223372036854775810ul phải thành 9.2e18 chứ không âm
            Node::Num(v) => Ok(if self.tt.is_unsigned(self.ty(id)) {
                *v as u64 as f64
            } else {
                *v as f64
            }),
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
        self.saw_inline = false;
        self.saw_thread = false;
        self.attr_weak = false;
        self.attr_transp = false;
        self.attr_alias = None;
        let mut storage = Storage::None;
        let (mut base, mut direct) = (None::<&str>, None::<TypeId>);
        let (mut uns, mut sgn, mut short, mut longs, mut any) = (false, false, false, 0u32, false);
        let mut cplx = false; // C99: _Complex
        loop {
            let n = match self.toks.get(self.pos) {
                Some(Tok::Ident(n)) => n.as_str(),
                _ => break,
            };
            match n {
                "const" | "volatile" | "auto" | "register" | "restrict" | "__restrict"
                | "__restrict__" | "__extension__" | "__volatile" | "__volatile__" | "__const"
                | "__const__" | "_Noreturn" => {}
                // EXT(gcc): __thread TLS thật (Mach-O @TLVP) — redis-tests
                // unit io-threads>=2 là chủ nợ đòi. Plain __thread trên auto
                // local (gcc cấm) rơi tự nhiên về stack — vốn đã per-thread.
                "__thread" => self.saw_thread = true,
                // EXT(gcc): inline — zcc không có inliner; definition inline sẽ hạ
                // về static (mỗi TU một bản, như nhánh "static __inline" của cdefs.h)
                // để "extern __inline" gnu89 của SDK không phát duplicate symbol
                "inline" | "__inline" | "__inline__" => self.saw_inline = true,
                // EXT(clang): nullability — no-op về ngữ nghĩa
                "_Nullable" | "_Nonnull" | "_Null_unspecified" => {}
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
                // C99: _Complex — musl src/complex; hạ về struct {re, im}.
                // EXT(gcc): __complex__ / __complex là bí danh (torture complex-*)
                "_Complex" | "__complex__" | "__complex" => cplx = true,
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
                "__typeof__" | "__typeof" | "typeof" => {
                    self.pos += 1;
                    self.expect(Tok::Punct("("))?;
                    let t = if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n))
                    {
                        self.typename()?
                    } else {
                        let e = self.expr()?; // node thành rác arena, như sizeof
                        let mut t = self.ty(e);
                        // typeof(tên hàm) = KIỂU HÀM, không decay (musl weak_alias:
                        // extern __typeof(f) g — g phải là hàm để alias đúng nhánh)
                        if let Node::FunAddr(_) = self.nodes[e as usize] {
                            if let Ty::Ptr(p) = self.tt.tys[t as usize] {
                                if matches!(self.tt.tys[p as usize], Ty::Func(_)) {
                                    t = p;
                                }
                            }
                        }
                        t
                    };
                    self.expect(Tok::Punct(")"))?;
                    direct = Some(t);
                    any = true;
                    continue;
                }
                _ => {
                    // typedef-name: chỉ khi chưa có kiểu nào khác (và không bị
                    // biến local cùng tên shadow — redis: quicklist *quicklist)
                    if base.is_none() && direct.is_none() && !uns && !sgn && !short && longs == 0 {
                        let shadowed = self.in_fn && self.locals.iter().any(|(ln, ..)| ln == n);
                        if let Some(&t) = self.typedefs.get(n).filter(|_| !shadowed) {
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
                // plain char: UNSIGNED trên Linux arm64 (AAPCS64);
                // "signed char" tường minh vẫn là CHAR
                if uns || !sgn {
                    UCHAR
                } else {
                    CHAR
                }
            }
            "float" => FLOAT,
            // C99 long double: ELF = binary128 TẠI BIÊN ABI/memory (số học vẫn
            // double, float.h khai LDBL_MANT_DIG 53)
            "double" => {
                if longs > 0 {
                    LDOUBLE
                } else {
                    DOUBLE
                }
            }
            "_Bool" => BOOL,
            _ => {
                // họ int (kể cả không có "int" tường minh)
                if short {
                    if uns { USHORT } else { SHORT }
                } else if longs > 0 {
                    if uns { ULONG } else { LONG }
                } else {
                    if uns { UINT } else { INT }
                }
            }
        };
        if cplx {
            // C99: float _Complex / double _Complex / long double _Complex
            // (long double = double); _Complex trần = double _Complex (theo gcc)
            let elem = if t == FLOAT { FLOAT } else { DOUBLE };
            return Ok(Some((self.cplx_of(elem), storage)));
        }
        Ok(Some((t, storage)))
    }
    // C99: intern struct {re, im} đại diện _Complex elem — AAPCS64 truyền
    // complex "as if" struct 2 phần tử nên HFA machinery sẵn có cho ABI đúng
    fn cplx_of(&mut self, elem: TypeId) -> TypeId {
        if let Some(&t) = self.cplx_tys.get(&elem) {
            return t;
        }
        let sz = self.tt.size(elem);
        self.tt.structs.push(crate::ast::StructDef {
            members: vec![("re".into(), elem, 0), ("im".into(), elem, sz)],
            size: 2 * sz,
            align: sz,
            is_union: false,
        });
        let t = self.tt.add(Ty::Struct((self.tt.structs.len() - 1) as u32));
        self.cplx_tys.insert(elem, t);
        t
    }
    // tra ngược: t có phải kiểu complex không → elem
    fn cplx_elem(&self, t: TypeId) -> Option<TypeId> {
        self.cplx_tys
            .iter()
            .find(|kv| *kv.1 == t)
            .map(|kv| *kv.0)
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
        let incomplete =
            tag.as_ref()
                .and_then(|g| self.tags.get(g))
                .copied()
                .filter(|&t| match self.tt.tys[t as usize] {
                    Ty::Struct(si) => self.tt.structs[si as usize].members.is_empty(),
                    _ => false,
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
        let Ty::Struct(sidx) = self.tt.tys[t as usize] else {
            unreachable!()
        };
        let mut members = Vec::new();
        // layout theo ABI Itanium (khoá clang Darwin — bitfield là impl-def C89
        // nhưng interop SDK bắt buộc khớp platform): cursor cấp phát theo BIT;
        // bitfield nhét vào bit trống kế trừ khi vắt qua biên container kiểu
        // khai báo; member thường đặt ở byte trống kế aligned. Bug unit-per-run
        // cũ (sizeof 12 vs 4 khi bitfield xen member thường) bắt bởi shape.sh.
        let (mut bits, mut mx) = (0u32, 1u32);
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
                    let o = if is_union {
                        0
                    } else {
                        bits.div_ceil(8).div_ceil(al) * al
                    };
                    members.push((String::new(), bt, o));
                    bits = if is_union {
                        bits.max(sz.wrapping_mul(8))
                    } else {
                        o.wrapping_add(sz).wrapping_mul(8)
                    };
                    mx = mx.max(al);
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
                    let w = self.const_expr()? as u32;
                    let (s, al) = (self.tt.size(mt), self.tt.align(mt));
                    let cb = s * 8; // container theo kiểu khai báo
                    if w == 0 {
                        // :0 — đẩy cursor tới biên container kế (3.5.2.1)
                        if !is_union {
                            bits = bits.div_ceil(cb) * cb;
                        }
                    } else {
                        // vắt qua biên container thì dời lên biên kế
                        let b = if is_union {
                            0
                        } else if bits % cb + w <= cb {
                            bits
                        } else {
                            bits.div_ceil(cb) * cb
                        };
                        if !mn.is_empty() {
                            let ft = self.tt.add(Ty::Bitfield(mt, b % cb, w));
                            members.push((mn, ft, b / cb * s));
                            mx = mx.max(al); // unnamed KHÔNG ảnh hưởng align (Itanium)
                        }
                        bits = if is_union { bits.max(cb) } else { b + w };
                    }
                } else {
                    // layout miền u32 (object ≤4GB — deviation-có-sổ ast.rs:116);
                    // wrapping cho khớp size() đã wrapping — huge array (2^62 short,
                    // 991014-1) wrap thay vì panic debug, KHÔNG crash trên input hợp lệ
                    let sz = self.tt.size(mt);
                    let al = if packed {
                        1
                    } else {
                        self.tt.align(mt).max(attr_al)
                    };
                    let o = if is_union {
                        0
                    } else {
                        bits.div_ceil(8).div_ceil(al) * al
                    };
                    members.push((mn, mt, o));
                    bits = if is_union {
                        bits.max(sz.wrapping_mul(8))
                    } else {
                        o.wrapping_add(sz).wrapping_mul(8)
                    };
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
        self.tt.structs[sidx as usize] = StructDef {
            members,
            size: bits.div_ceil(8).div_ceil(mx) * mx,
            align: mx,
            is_union,
        };
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
        // EXT(clang): enum tag : underlying-type (SDK malloc.h) — honor kiểu nền
        let under = if self.eat(&Tok::Punct(":")) {
            Some(self.typename()?)
        } else {
            None
        };
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
            let t = under.unwrap_or(if any_neg { INT } else { UINT });
            if !tag.is_empty() {
                self.enum_tags.insert(tag, t);
            }
            return Ok(t);
        }
        Ok(under
            .or_else(|| self.enum_tags.get(&tag).copied())
            .unwrap_or(INT))
    }
    // declarator đầy đủ C: con trỏ, nested "(...)", suffix mảng/hàm.
    // need_name=false cho abstract declarator (cast, sizeof, param không tên).
    fn declarator(&mut self, mut t: TypeId, need_name: bool) -> Result<(String, TypeId), String> {
        self.skip_attrs()?;
        while self.eat(&Tok::Punct("*")) {
            t = self.tt.ptr_to(t);
            while self.eat_kw("const")
                || self.eat_kw("volatile")
                || self.eat_kw("restrict")
                || self.eat_kw("__restrict")
                || self.eat_kw("__restrict__")
                // EXT(clang): nullability qualifier — SDK dùng trần trong FILE...
                || self.eat_kw("_Nullable")
                || self.eat_kw("_Nonnull")
                || self.eat_kw("_Null_unspecified")
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
            // đuôi sau suffix ngoài: attribute/asm-label (mach_init.h:
            // "extern int (*f)(...) __printflike(1,0);")
            self.asm_label = None;
            self.skip_attrs()?;
            if let Some(l) = self.asm_label.take() {
                if !res.0.is_empty() {
                    self.renames.insert(res.0.clone(), l);
                }
            }
            return Ok(res);
        }
        let name = match self.toks.get(self.pos) {
            // tên được phép TRÙNG typedef-name (shadow); chỉ cấm keyword cứng.
            // Không cần need_name: specifier đã bị decl_specs ăn hết trước đó,
            // abstract declarator không bao giờ chứa ident trần → ident ở đây
            // LUÔN là tên (git: param `reftable_fsck_report_fn report_fn`
            // với report_fn đồng thời là typedef toàn cục ở usage.h)
            Some(Tok::Ident(n)) if !self.is_keyword(n) => {
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
                return Err(format!(
                    "cần tên trong declarator, gặp {:?}",
                    self.toks.get(self.pos)
                ));
            }
            _ => String::new(),
        };
        let mut t = self.suffixes(t)?;
        self.asm_label = None;
        // EXT(gcc): aligned(n) SAU declarator (torture 20050215-1: `typedef
        // struct {...} V __attribute__((aligned(8)))`) — type over-aligned
        // RIÊNG (clone def, không mutate: tag gốc dùng chỗ khác giữ nguyên)
        let (_, al) = self.skip_attrs()?;
        if let (Some(a), Ty::Struct(si)) = (al, &self.tt.tys[t as usize]) {
            let mut d = self.tt.structs[*si as usize].clone();
            if d.align < a {
                d.align = a;
                d.size = d.size.div_ceil(a) * a;
                self.tt.structs.push(d);
                t = self.tt.add(Ty::Struct(self.tt.structs.len() as u32 - 1));
            }
        }
        // EXT(gcc): transparent_union — call truyền arg theo ABI của member ĐẦU
        // (gcc doc). Trong scope chỉ gặp ở PROTOTYPE glibc (bind/connect… dưới
        // _GNU_SOURCE), không ai định nghĩa hàm nhận nó → thay thẳng type =
        // member đầu là trọn ngữ nghĩa; giữ union thật sẽ đi lộn protocol
        // composite (bug redis/nginx bind EFAULT 2026-08-18).
        if self.attr_transp {
            self.attr_transp = false;
            if let Ty::Struct(si) = &self.tt.tys[t as usize] {
                let d = &self.tt.structs[*si as usize];
                if d.is_union && !d.members.is_empty() {
                    t = d.members[0].1;
                }
            }
        }
        // EXT(gcc): asm-label sau declarator → symbol thật khi emit (Call/FunAddr)
        if let Some(l) = self.asm_label.take() {
            if !name.is_empty() {
                self.renames.insert(name.clone(), l);
            }
        }
        Ok((name, t))
    }
    // EXT(gcc): tên C → symbol emit; prefix \x01 = đã đủ tên, codegen không thêm '_'
    fn funref(&self, n: String) -> String {
        match self.renames.get(&n) {
            Some(l) => format!("\x01{}", l),
            None => n,
        }
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
                        // EXT(gcc): weak/alias — bộ xương weak_alias() của musl
                        "weak" | "__weak__" => self.attr_weak = true,
                        // EXT(gcc): glibc bật union này dưới _GNU_SOURCE
                        // (__CONST_SOCKADDR_ARG của bind/connect/sendto…)
                        "transparent_union" | "__transparent_union__" => self.attr_transp = true,
                        "alias" | "__alias__" => {
                            self.expect(Tok::Punct("("))?;
                            if let Some(Tok::Str(b, _)) = self.toks.get(self.pos) {
                                self.attr_alias = Some(String::from_utf8_lossy(b).into_owned());
                                self.pos += 1;
                            }
                            self.expect(Tok::Punct(")"))?;
                        }
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
                // EXT(gcc): asm-label — "(chuỗi)" thuần là symbol thay thế cho
                // declarator (SDK versioning: __asm("_open")); còn lại nuốt bỏ
                let mut label = String::new();
                while let Some(Tok::Str(b, _)) = self.toks.get(self.pos) {
                    label.push_str(&String::from_utf8_lossy(b)); // "_" "open" ghép
                    self.pos += 1;
                }
                if !label.is_empty() && self.eat(&Tok::Punct(")")) {
                    self.asm_label = Some(label);
                    continue;
                }
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
            // C99: 6.7.5.3p21 — qualifier/static trong [] của array param
            // (SDK _regex.h: `__pmatch[ restrict n ]` khi xưng C99); no-op ở -O0
            while self.eat_kw("restrict")
                || self.eat_kw("__restrict")
                || self.eat_kw("__restrict__")
                || self.eat_kw("const")
                || self.eat_kw("volatile")
                || self.eat_kw("static")
            {}
            let n = if self.peek("]") {
                0
            } else {
                let save = self.pos;
                match self.const_expr() {
                    Ok(v) => v as u64,
                    // C99: trong param list size có thể trỏ về param trước
                    // (chưa có scope) — skip balanced đến ']', decay lo phần còn lại
                    Err(_) if self.in_params > 0 => {
                        self.pos = save;
                        let mut depth = 0u32;
                        loop {
                            match self.toks.get(self.pos) {
                                Some(Tok::Punct("[")) => depth += 1,
                                Some(Tok::Punct("]")) if depth == 0 => break,
                                Some(Tok::Punct("]")) => depth -= 1,
                                None => return Err("mảng param thiếu ']'".into()),
                                // C99 6.9.1: size expr của array param CÓ side-effect
                                // (`b[a++]`) đúng ra phải eval khi HÀM ĐƯỢC GỌI. zcc
                                // decay param về con trỏ, drop size — side-effect mất
                                // (miscompile 970217-1/pr77767 KHI hàm bị gọi). KHÔNG
                                // reject ở đây được: `bar(char a[2][(*x)++])` không bao
                                // giờ gọi thì hành vi ĐÚNG (pr22061-2) — reject sẽ phá
                                // case xanh. ⇒ HOÃN (VLA-VMT niche, charter line 31).
                                _ => {}
                            }
                            self.pos += 1;
                        }
                        0
                    }
                    // C99: size không hằng = VLA — giữ expr cho stmt() hạ
                    // xuống alloca; chỉ 1 chiều không hằng (chiều trong phải hằng)
                    Err(_) => {
                        self.pos = save;
                        let e = self.expr()?;
                        if self.vla_size.replace(e).is_some() {
                            return Err("VLA nhiều chiều không hằng: chưa hỗ trợ".into());
                        }
                        0
                    }
                }
            };
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
            return Ok(FnSig {
                oldstyle: true,
                ..empty
            }); // () — không thông tin
        }
        if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if n == "void")
            && self.toks.get(self.pos + 1) == Some(&Tok::Punct(")"))
        {
            self.pos += 2;
            return Ok(empty); // (void)
        }
        // old-style ident list: f(a, b) — nhưng __attribute__/nullability mở đầu
        // param hiện đại (SDK mig_errors.h: f(__unused T *x)), không phải tên
        if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if !self.is_type_word(n)
            && !matches!(n.as_str(), "__attribute__" | "__attribute"
                | "_Nullable" | "_Nonnull" | "_Null_unspecified"))
        {
            let mut pnames = Vec::new();
            loop {
                pnames.push(self.ident()?);
                if !self.eat(&Tok::Punct(",")) {
                    break;
                }
            }
            self.expect(Tok::Punct(")"))?;
            return Ok(FnSig {
                pnames,
                oldstyle: true,
                ..empty
            });
        }
        let (mut params, mut pnames, mut variadic) = (Vec::new(), Vec::new(), false);
        self.in_params += 1;
        loop {
            if self.eat(&Tok::Punct("...")) {
                variadic = true;
                break;
            }
            let (bt, _) = self.decl_specs()?.ok_or("cần kiểu tham số")?;
            let (nm, pt) = self.declarator(bt, false)?;
            self.vla_size = None; // C99: param VLA decay về con trỏ — bỏ size
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
        self.in_params -= 1;
        self.expect(Tok::Punct(")"))?;
        Ok(FnSig {
            ret,
            params,
            pnames,
            variadic,
            oldstyle: false,
        })
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
        // C99: chuyển đổi complex (6.3.1.6/6.3.1.7) — desugar member-wise.
        // complex → real: lấy phần thực (creal của musl CHÍNH LÀ cast này);
        // real/complex → complex: temp + gán từng phần, phần ảo = 0.
        let (se, de) = (self.cplx_elem(self.ty(e)), self.cplx_elem(to));
        if let (Some(sel), None) = (se, de) {
            if self.tt.is_integer(to) || self.tt.is_float(to) {
                let m = self.push(Node::Member(e, 0), sel);
                return self.cast(m, to);
            }
        }
        if de.is_some()
            && self.in_fn
            && (se.is_some() || self.tt.is_integer(self.ty(e)) || self.tt.is_float(self.ty(e)))
        {
            let del = de.unwrap();
            let esz = self.tt.size(del);
            let off = self.alloc_local(format!(".cplx{}", self.nodes.len()), to);
            let mut pre = None;
            let (rs, is_) = match se {
                Some(sel) => {
                    // nguồn cũng complex: chốt vào temp để không eval 2 lần
                    let st = self.ty(e);
                    let soff = self.alloc_local(format!(".cplxs{}", self.nodes.len()), st);
                    let sv = self.push(Node::Var(soff), st);
                    pre = Some(self.push(Node::Assign(sv, e), st));
                    let ssz = self.tt.size(sel);
                    let b1 = self.push(Node::Var(soff), st);
                    let m1 = self.push(Node::Member(b1, 0), sel);
                    let b2 = self.push(Node::Var(soff), st);
                    let m2 = self.push(Node::Member(b2, ssz), sel);
                    (self.cast(m1, del), self.cast(m2, del))
                }
                None => {
                    let z = self.push(Node::FNum(0.0), del);
                    (self.cast(e, del), z)
                }
            };
            let dv = self.push(Node::Var(off), to);
            let mr = self.push(Node::Member(dv, 0), del);
            let ar = self.push(Node::Assign(mr, rs), del);
            let dv2 = self.push(Node::Var(off), to);
            let mi = self.push(Node::Member(dv2, esz), del);
            let ai = self.push(Node::Assign(mi, is_), del);
            let val = self.push(Node::Var(off), to);
            let mut seq = self.push(Node::Comma(ai, val), to);
            seq = self.push(Node::Comma(ar, seq), to);
            if let Some(p) = pre {
                seq = self.push(Node::Comma(p, seq), to);
            }
            return seq;
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
            // C99 UAC: long double đứng đỉnh semilattice thực (giá trị vẫn f64
            // trong reg — cast double↔ldbl là no-op, chỉ nhãn kiểu đổi cho ABI)
            if matches!(self.tt.tys[lt as usize], Ty::LDouble)
                || matches!(self.tt.tys[rt as usize], Ty::LDouble)
            {
                return LDOUBLE;
            }
            // float+float / float+int → FLOAT (operand phải TRÒN về f32 —
            // 16777217L != (float)16777217e0 phân biệt được); còn lại double.
            // Số học chạy trong double (C89 cho phép dư precision).
            let fl = |t: TypeId| {
                matches!(self.tt.tys[t as usize], Ty::Double)
                    || !self.tt.is_integer(t) && !matches!(self.tt.tys[t as usize], Ty::Float)
            };
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
    // scale bước con trỏ: pointee VLA → nạp size byte từ local ẩn, còn lại hằng
    fn vla_scale(&mut self, e: TypeId) -> NodeId {
        match self.vla_arrs.get(&e) {
            Some(&off) => self.push(Node::Var(off), LONG),
            None => self.push(Node::Num(self.tt.size(e) as i64), LONG),
        }
    }
    // Dựng node binary op kèm chèn conversion + scale con trỏ
    fn mkbin(&mut self, op: &'static str, l: NodeId, r: NodeId) -> R {
        // C99: complex là struct — PHẢI chặn trước nhánh scalar, nếu không
        // codegen coi địa chỉ struct như giá trị 8 byte đầu → sai im lặng
        if self.cplx_elem(self.ty(l)).is_some() || self.cplx_elem(self.ty(r)).is_some() {
            return self.cplx_bin(op, l, r);
        }
        let (lp, rp) = (self.tt.pointee(self.ty(l)), self.tt.pointee(self.ty(r)));
        match (op, lp, rp) {
            ("+", None, Some(_)) => self.mkbin("+", r, l), // int + ptr: giao hoán
            ("+" | "-", Some(e), None) => {
                let r = self.cast(r, LONG);
                let sz = self.vla_scale(e);
                let r = self.push(Node::Bin("*", r, sz), LONG);
                let t = self.tt.ptr_to(e);
                Ok(self.push(Node::Bin(op, l, r), t))
            }
            ("-", Some(e), Some(_)) => {
                let d = self.push(Node::Bin("-", l, r), LONG);
                let sz = self.vla_scale(e);
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
    // C99: đại số complex hạ member-wise qua temp (6.3.1.8 UAC + phép toán
    // trên trường số phức). Nhân/chia dùng công thức đại số thẳng — KHÔNG Annex G
    // NaN-fixup/scaling (tương đương gcc -fcx-limited-range; deviation tuyên bố,
    // musl src/complex tự decompose creal/cimag ở các đường biên nên ít đụng).
    // đọc hằng phức từ node const: Bin("__ci",re,im) | scalar thực → (re,im)
    fn ccval(&self, id: NodeId) -> Option<(f64, f64)> {
        match &self.nodes[id as usize] {
            Node::Bin("__ci", a, b) => Some((self.fold_f(*a).ok()?, self.fold_f(*b).ok()?)),
            _ => self.fold_f(id).ok().map(|v| (v, 0.0)),
        }
    }
    // hằng ảo → temp complex {re=0, im=v} (phép nhúng ℝ→ℂ).
    // !in_fn (static init): dựng sentinel Bin("__ci",0,v) để ginit fold ra bytes.
    fn cplx_imag(&mut self, im: f64, elem: TypeId) -> NodeId {
        let ct = self.cplx_of(elem);
        if !self.in_fn {
            let (z, v) = (self.push(Node::FNum(0.0), elem), self.push(Node::FNum(im), elem));
            return self.push(Node::Bin("__ci", z, v), ct);
        }
        let esz = self.tt.size(elem);
        let o = self.alloc_local(format!(".ci{}", self.nodes.len()), ct);
        let vr = self.push(Node::Var(o), ct);
        let mre = self.push(Node::Member(vr, 0), elem);
        let zero = self.push(Node::FNum(0.0), elem);
        let are = self.push(Node::Assign(mre, zero), elem);
        let vi = self.push(Node::Var(o), ct);
        let mim = self.push(Node::Member(vi, esz), elem);
        let imv = self.push(Node::FNum(im), elem);
        let aim = self.push(Node::Assign(mim, imv), elem);
        let val = self.push(Node::Var(o), ct);
        let seq = self.push(Node::Comma(aim, val), ct);
        self.push(Node::Comma(are, seq), ct)
    }
    // π₁/π₂: __real__ z / __imag__ z — phép chiếu ℂ→ℝ (Member re@0 / im@esz)
    fn cplx_proj(&mut self, e: NodeId, imag: bool) -> R {
        if let Some(el) = self.cplx_elem(self.ty(e)) {
            let off = if imag { self.tt.size(el) } else { 0 };
            return Ok(self.push(Node::Member(e, off), el));
        }
        // scalar thực: __real__ x = x; __imag__ x = 0 (cùng kiểu)
        if imag {
            let t = self.ty(e);
            return Ok(if self.tt.is_float(t) {
                self.push(Node::FNum(0.0), t)
            } else {
                self.push(Node::Num(0), t)
            });
        }
        Ok(e)
    }
    fn cplx_bin(&mut self, op: &'static str, l: NodeId, r: NodeId) -> R {
        let lf = self.cplx_elem(self.ty(l)).unwrap_or(self.ty(l)) == FLOAT;
        let rf = self.cplx_elem(self.ty(r)).unwrap_or(self.ty(r)) == FLOAT;
        let elem = if lf && rf { FLOAT } else { DOUBLE };
        let ct = self.cplx_of(elem);
        let esz = self.tt.size(elem);
        // static init: fold hằng ℂ → sentinel Bin("__ci",re,im) (không temp runtime)
        if !self.in_fn {
            let ((a, b), (c, d)) = (
                self.ccval(l).ok_or("toán tử complex trên hạng không hằng")?,
                self.ccval(r).ok_or("toán tử complex trên hạng không hằng")?,
            );
            let (re, im) = match op {
                "+" => (a + c, b + d),
                "-" => (a - c, b - d),
                "*" => (a * c - b * d, a * d + b * c),
                "/" => {
                    let den = c * c + d * d;
                    ((a * c + b * d) / den, (b * c - a * d) / den)
                }
                _ => return Err(format!("op '{op}' trên hằng complex")),
            };
            let (rn, in_) = (self.push(Node::FNum(re), elem), self.push(Node::FNum(im), elem));
            return Ok(self.push(Node::Bin("__ci", rn, in_), ct));
        }
        let (lv, rv) = (self.cast(l, ct), self.cast(r, ct));
        let ao = self.alloc_local(format!(".ca{}", self.nodes.len()), ct);
        let av = self.push(Node::Var(ao), ct);
        let p1 = self.push(Node::Assign(av, lv), ct);
        let bo = self.alloc_local(format!(".cb{}", self.nodes.len()), ct);
        let bv = self.push(Node::Var(bo), ct);
        let p2 = self.push(Node::Assign(bv, rv), ct);
        // đọc member tươi mỗi lần dùng (temp nên không có side effect)
        let m = |s: &mut Self, o: u32, k: u32| {
            let v = s.push(Node::Var(o), ct);
            s.push(Node::Member(v, k), elem)
        };
        if matches!(op, "==" | "!=") {
            let (a, b) = (m(self, ao, 0), m(self, bo, 0));
            let e1 = self.mkbin("==", a, b)?;
            let (a, b) = (m(self, ao, esz), m(self, bo, esz));
            let e2 = self.mkbin("==", a, b)?;
            let mut val = self.mkbin("&", e1, e2)?;
            if op == "!=" {
                let z = self.push(Node::Num(0), INT);
                val = self.mkbin("==", val, z)?;
            }
            let s2 = self.push(Node::Comma(p2, val), INT);
            return Ok(self.push(Node::Comma(p1, s2), INT));
        }
        let mut p3 = None;
        let (rre, rim) = match op {
            "+" | "-" => {
                let (a, b) = (m(self, ao, 0), m(self, bo, 0));
                let re = self.mkbin(op, a, b)?;
                let (a, b) = (m(self, ao, esz), m(self, bo, esz));
                (re, self.mkbin(op, a, b)?)
            }
            "*" => {
                let (a, b) = (m(self, ao, 0), m(self, bo, 0));
                let t1 = self.mkbin("*", a, b)?;
                let (a, b) = (m(self, ao, esz), m(self, bo, esz));
                let t2 = self.mkbin("*", a, b)?;
                let re = self.mkbin("-", t1, t2)?;
                let (a, b) = (m(self, ao, 0), m(self, bo, esz));
                let t3 = self.mkbin("*", a, b)?;
                let (a, b) = (m(self, ao, esz), m(self, bo, 0));
                let t4 = self.mkbin("*", a, b)?;
                (re, self.mkbin("+", t3, t4)?)
            }
            "/" => {
                let (x, y) = (m(self, bo, 0), m(self, bo, 0));
                let d1 = self.mkbin("*", x, y)?;
                let (x, y) = (m(self, bo, esz), m(self, bo, esz));
                let d2 = self.mkbin("*", x, y)?;
                let d = self.mkbin("+", d1, d2)?;
                let dof = self.alloc_local(format!(".cd{}", self.nodes.len()), elem);
                let dv = self.push(Node::Var(dof), elem);
                p3 = Some(self.push(Node::Assign(dv, d), elem));
                let (a, b) = (m(self, ao, 0), m(self, bo, 0));
                let t1 = self.mkbin("*", a, b)?;
                let (a, b) = (m(self, ao, esz), m(self, bo, esz));
                let t2 = self.mkbin("*", a, b)?;
                let nre = self.mkbin("+", t1, t2)?;
                let dr = self.push(Node::Var(dof), elem);
                let re = self.mkbin("/", nre, dr)?;
                let (a, b) = (m(self, ao, esz), m(self, bo, 0));
                let t3 = self.mkbin("*", a, b)?;
                let (a, b) = (m(self, ao, 0), m(self, bo, esz));
                let t4 = self.mkbin("*", a, b)?;
                let nim = self.mkbin("-", t3, t4)?;
                let di = self.push(Node::Var(dof), elem);
                (re, self.mkbin("/", nim, di)?)
            }
            _ => return Err(format!("op '{}' trên complex", op)),
        };
        let mut seq = self.cplx_pack(rre, rim, elem);
        if let Some(p) = p3 {
            seq = self.push(Node::Comma(p, seq), ct);
        }
        seq = self.push(Node::Comma(p2, seq), ct);
        Ok(self.push(Node::Comma(p1, seq), ct))
    }
    // gói (re,im) → temp complex, trả về Var(temp) (cần in_fn)
    fn cplx_pack(&mut self, rre: NodeId, rim: NodeId, elem: TypeId) -> NodeId {
        let ct = self.cplx_of(elem);
        let esz = self.tt.size(elem);
        let ro = self.alloc_local(format!(".cr{}", self.nodes.len()), ct);
        let rv1 = self.push(Node::Var(ro), ct);
        let mr = self.push(Node::Member(rv1, 0), elem);
        let ar = self.push(Node::Assign(mr, rre), elem);
        let rv2 = self.push(Node::Var(ro), ct);
        let mi = self.push(Node::Member(rv2, esz), elem);
        let ai = self.push(Node::Assign(mi, rim), elem);
        let val = self.push(Node::Var(ro), ct);
        let seq = self.push(Node::Comma(ai, val), ct);
        self.push(Node::Comma(ar, seq), ct)
    }
    // EXT(gcc): ~z trên complex = liên hợp (re, −im). Dùng Neg (FNEG lật sign
    // bit) chứ KHÔNG 0−im: liên hợp của +0 là −0 (signed zero, khớp cc bit-đối-bit).
    fn cplx_conj(&mut self, e: NodeId) -> R {
        let el = self.cplx_elem(self.ty(e)).ok_or("~ trên không phải complex")?;
        let esz = self.tt.size(el);
        let re = self.push(Node::Member(e, 0), el);
        let im0 = self.push(Node::Member(e, esz), el);
        let im = self.push(Node::Neg(im0), el);
        Ok(self.cplx_pack(re, im, el))
    }

    // ---- local/scope ----
    fn alloc_local(&mut self, name: String, t: TypeId) -> u32 {
        let (sz, al) = (self.tt.size(t), self.tt.align(t));
        self.cur_off = (self.cur_off + sz).div_ceil(al) * al;
        self.locals.push((name, t, Vloc::Stack(self.cur_off)));
        self.cur_off
    }
    // cấp slot param + mirror thuật toán spill codegen (PHẢI khớp từng byte):
    // stack args pack — scalar natural align, composite align max(8,align) size
    // tròn 8, tràn khóa gp=8 (C.11). Vùng arg vô danh variadic bắt đầu sau named
    // tròn 8. Trả (params, sret slot). Dùng chung top-level & nested funcdef.
    fn setup_params(
        &mut self,
        sig: &crate::ast::FnSig,
        ptypes: &HashMap<String, TypeId>,
    ) -> (Vec<(u32, TypeId)>, u32) {
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
        let alup = |o: u32, a: u32| (o + a - 1) & !(a - 1);
        let (mut gp, mut fp, mut boff) = (0u32, 0u32, 0u32);
        for &(_, pt) in &params {
            if let Some((_, n)) = self.tt.hfa(pt) {
                if fp + n <= 8 {
                    fp += n;
                } else {
                    fp = 8; // AAPCS: HFA tràn thì khóa luôn v-reg còn lại
                    let o = alup(boff, self.tt.align(pt).max(8));
                    boff = o + self.tt.size(pt).div_ceil(8) * 8;
                }
            } else if matches!(self.tt.tys[pt as usize], Ty::Struct(_)) {
                let sz = self.tt.size(pt);
                if sz > 16 {
                    if gp < 8 {
                        gp += 1;
                    } else {
                        boff = alup(boff, 8) + 8;
                    }
                } else {
                    let need = if sz > 8 { 2 } else { 1 };
                    if gp + need <= 8 {
                        gp += need;
                    } else {
                        let o = alup(boff, self.tt.align(pt).max(8));
                        boff = o + 8 * need;
                        gp = 8; // AAPCS C.11
                    }
                }
            } else {
                let (fl, sz) = (self.tt.is_float(pt), self.tt.size(pt));
                if fl && fp < 8 {
                    fp += 1;
                } else if !fl && gp < 8 {
                    gp += 1;
                } else {
                    boff = alup(boff, sz) + sz;
                }
            }
        }
        self.va_off = 16 + ((boff + 7) & !7);
        let sret = if matches!(self.tt.tys[sig.ret as usize], Ty::Struct(_))
            && self.tt.size(sig.ret) > 16
            && self.tt.hfa(sig.ret).is_none()
        {
            self.alloc_local(String::new(), ULONG)
        } else {
            0
        };
        (params, sret)
    }

    // EXT(gcc): parse ĐỊNH NGHĨA nested function (GNU, chỉ ELF). Đang giữa thân
    // hàm bao (state live) → lưu/khôi phục toàn bộ frame-state; locals hàm bao
    // GIỮ NGUYÊN để resolve upvar, chỉ truncate phần riêng của nested sau. Symbol
    // mangled "{name}.{uid}" tránh đụng. Kết quả đẩy vào self.pending.
    fn nested_funcdef(&mut self, name: String, t: TypeId, fidx: u32) -> Result<(), String> {
        // ELF-only: trampoline đòi executable stack (GNU extension).
        let uid = self.fn_uid;
        self.fn_uid += 1;
        let sig = self.tt.fns[fidx as usize].clone();
        let mangled = format!("{}.{}", name, uid);
        self.fns.insert(name.clone(), t); // gọi/tham chiếu trong scope
        self.nested_fns
            .insert(name.clone(), (mangled.clone(), self.cur_uid));
        // lưu state hàm bao (nó đang parse dở)
        let (s_off, s_ret, s_fname, s_va, s_uid, s_puid, s_ubase) = (
            self.cur_off,
            self.fret,
            std::mem::take(&mut self.fname),
            self.va_off,
            self.cur_uid,
            self.cur_parent_uid,
            self.upvar_base,
        );
        let s_pins = std::mem::take(&mut self.reg_pins);
        let s_vsz = std::mem::take(&mut self.vla_szs);
        let base = self.locals.len(); // locals < base = biến hàm bao → Upvar
        self.cur_parent_uid = self.cur_uid;
        self.cur_uid = uid;
        self.upvar_base = base;
        self.cur_off = 0;
        self.fret = sig.ret;
        self.fname = mangled.clone();
        // slot đầu tiên = static chain (x18 lưu trong prologue); offset ≠ 0
        let chain = self.alloc_local(".chain".into(), ULONG);
        let (params, sret) = self.setup_params(&sig, &HashMap::new());
        let body = self.stmt()?;
        self.pending.push(Func {
            name: mangled,
            params,
            frame: (self.cur_off + 15) & !15,
            body,
            ret: sig.ret,
            is_static: true,
            is_inline: false,
            is_weak: false,
            variadic: sig.variadic,
            sret,
            uid,
            parent_uid: self.cur_parent_uid,
            chain,
            has_vla: !self.vla_szs.is_empty(),
        });
        // khôi phục state hàm bao
        self.locals.truncate(base);
        self.cur_off = s_off;
        self.fret = s_ret;
        self.fname = s_fname;
        self.va_off = s_va;
        self.cur_uid = s_uid;
        self.cur_parent_uid = s_puid;
        self.upvar_base = s_ubase;
        self.reg_pins = s_pins;
        self.vla_szs = s_vsz;
        Ok(())
    }

    // nhánh bị DCE có chứa label không (goto từ ngoài vào thì không được bỏ)
    fn has_label(&self, id: NodeId) -> bool {
        match &self.nodes[id as usize] {
            // Case cũng là jump target (bảng switch tham chiếu LC{id})
            Node::Label(..) | Node::Case(..) => true,
            Node::If(a, b, c) => {
                self.has_label(*a) || self.has_label(*b) || c.is_some_and(|x| self.has_label(x))
            }
            Node::While(a, b)
            | Node::Do(a, b)
            | Node::Comma(a, b)
            | Node::Assign(a, b)
            | Node::Bin(_, a, b) => self.has_label(*a) || self.has_label(*b),
            Node::For(a, b, c, d) => {
                [a, b, c]
                    .iter()
                    .any(|x| x.is_some_and(|x| self.has_label(x)))
                    || self.has_label(*d)
            }
            Node::Switch(a, b, ..) => self.has_label(*a) || self.has_label(*b),
            Node::Block(v) => v.iter().any(|&x| self.has_label(x)),
            Node::Cond(a, b, c) => self.has_label(*a) || self.has_label(*b) || self.has_label(*c),
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
            Node::Call(_, args, _) | Node::Sync(_, args, _) => {
                args.iter().any(|&x| self.has_label(x))
            }
            Node::Overflow(_, a, b, c) => {
                self.has_label(*a) || self.has_label(*b) || self.has_label(*c)
            }
            Node::Asm(_, ops) => ops.iter().any(|o| self.has_label(o.e)),
            Node::CallPtr(f, args, _) => {
                self.has_label(*f) || args.iter().any(|&x| self.has_label(x))
            }
            _ => false,
        }
    }
    fn check_lval(&self, l: NodeId) -> Result<(), String> {
        if matches!(
            self.nodes[l as usize],
            Node::Var(_) | Node::Upvar(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..)
        ) {
            Ok(())
        } else {
            Err("cần lvalue".into())
        }
    }

    // ---- statement ----
    fn stmt(&mut self) -> R {
        // EXT(gcc): __extension__ đầu stmt là no-op nhưng nằm trong type-word
        // → phải bóc TRƯỚC khi phân loại decl/expr, kẻo `__extension__ ({...});`
        // (obstack_blank của git) bị ăn nhầm thành declaration
        if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if n == "__extension__")
            && matches!(self.toks.get(self.pos + 1), Some(Tok::Punct("(")))
        {
            self.pos += 1;
        }
        if self.eat_kw("_Static_assert") {
            // EXT(c11): declaration rỗng về mặt codegen — trả block trống
            self.static_assert()?;
            return Ok(self.push(Node::Block(Vec::new()), INT));
        }
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
            // EXT(gcc): inline asm SUBSET (xxhash M14; musl M17 nới): template
            // phát verbatim; constraint xem AsmOp bên ast.rs. Clobber bỏ qua —
            // vô hại ở -O0 vì mọi statement reload từ memory, chỉ sp/x29/x30
            // là bất khả xâm phạm. KHÔNG [tên], không asm goto — lỗi rõ ràng
            // để không im lặng sinh code sai.
            while self.eat_kw("volatile")
                || self.eat_kw("__volatile__")
                || self.eat_kw("__volatile")
            {}
            self.expect(Tok::Punct("("))?;
            let mut tpl = String::new();
            while let Some(Tok::Str(b, _)) = self.toks.get(self.pos) {
                tpl.push_str(&String::from_utf8_lossy(b));
                self.pos += 1;
            }
            let mut ops: Vec<AsmOp> = Vec::new();
            for sect in 0..3 {
                if !self.eat(&Tok::Punct(":")) {
                    break;
                }
                while let Some(Tok::Str(c, _)) = self.toks.get(self.pos) {
                    let c = String::from_utf8_lossy(c).into_owned();
                    self.pos += 1;
                    if sect < 2 {
                        self.expect(Tok::Punct("("))?;
                        let e = self.assign()?;
                        self.expect(Tok::Punct(")"))?;
                        let mut op = AsmOp {
                            e,
                            out: sect == 0,
                            rw: c.starts_with('+'),
                            mem: false,
                            fp: false,
                            tied: None,
                            pin: None,
                        };
                        // "&" (early-clobber) vô hại: pool mỗi operand một reg riêng
                        match c.trim_start_matches(['=', '+', '&']) {
                            "r" => {}
                            "w" => op.fp = true,
                            "Q" | "m" => op.mem = true,
                            d if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) => {
                                op.tied = Some(d.parse().map_err(|e| format!("{e}"))?)
                            }
                            _ => return Err(format!("asm: constraint \"{c}\" chưa hỗ trợ")),
                        }
                        if let Node::Var(off) = self.nodes[e as usize] {
                            op.pin = self.reg_pins.get(&off).copied();
                        }
                        if op.out {
                            self.check_lval(e)?;
                        }
                        ops.push(op);
                    }
                    if !self.eat(&Tok::Punct(",")) {
                        break;
                    }
                }
            }
            // pool tách GP/FP; pin và tied không tốn pool
            let pool = |fp: bool| {
                ops.iter()
                    .filter(|o| o.fp == fp && o.pin.is_none() && o.tied.is_none())
                    .count()
            };
            if pool(false) > 7 || pool(true) > 7 {
                return Err("asm: tối đa 7 operand mỗi pool (x9-x15 / v16-v22)".into());
            }
            self.expect(Tok::Punct(")"))?;
            self.expect(Tok::Punct(";"))?;
            return Ok(self.push(Node::Asm(tpl, ops), INT));
        }
        if self.eat_kw("__label__") {
            // GNU local label declaration. Ghi (uid chủ, tên) để nested function
            // goto vào label hàm bao hạ về NlGoto (non-local goto qua static chain).
            loop {
                if let Some(Tok::Ident(l)) = self.toks.get(self.pos) {
                    self.nl_labels.push((self.cur_uid, l.clone()));
                }
                self.pos += 1;
                if self.eat(&Tok::Punct(";")) {
                    break;
                }
            }
            return Ok(self.push(Node::Block(Vec::new()), INT));
        }
        if self.eat_kw("return") {
            if self.eat(&Tok::Punct(";")) {
                return Ok(self.push(Node::Ret(None), INT));
            }
            let e = self.expr()?;
            let e = if self.fret == VOID {
                e
            } else {
                self.cast(e, self.fret)
            };
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Ret(Some(e)), INT))
        } else if self.eat_kw("if") {
            self.expect(Tok::Punct("("))?;
            let c = self.expr()?;
            let c = self.truthy(c)?;
            self.expect(Tok::Punct(")"))?;
            let t = self.stmt()?;
            let e = if self.eat_kw("else") {
                Some(self.stmt()?)
            } else {
                None
            };
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
            // C99 6.8.5.3: for-init declaration được scope TỚI HẾT for (init+cond+
            // incr+body), phải đóng sau body kẻo leak shadow ra ngoài. Chỉ cho auto/
            // register object nên chỉ cần locals (không tag/typedef).
            let scope = self.locals.len();
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
            self.locals.truncate(scope);
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
                let narrow = |v: i64| if uns { v as u32 as i64 } else { v as i32 as i64 };
                for (lo, hi, _) in &mut cases {
                    *lo = narrow(*lo);
                    *hi = narrow(*hi);
                }
            }
            Ok(self.push(Node::Switch(c, b, cases, def), INT))
        } else if self.eat_kw("case") {
            let lo = self.const_expr()?;
            // EXT(gcc): case lo ... hi — nhãn cho MỌI giá trị trong [lo,hi]
            let hi = if self.eat(&Tok::Punct("...")) {
                self.const_expr()?
            } else {
                lo
            };
            self.expect(Tok::Punct(":"))?;
            let st = self.stmt()?;
            let id = self.push(Node::Case(st), INT);
            self.switches
                .last_mut()
                .ok_or("case ngoài switch")?
                .0
                .push((lo, hi, id));
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
            // EXT(gcc): goto tới __label__ của HÀM BAO (owner uid ≠ hàm hiện tại)
            // → non-local goto qua static chain.
            if let Some(&(owner, _)) = self
                .nl_labels
                .iter()
                .find(|(u, l)| *l == n && *u != self.cur_uid)
            {
                return Ok(self.push(Node::NlGoto(owner, n), INT));
            }
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
            let tls = self.saw_thread;
            let mut stmts = Vec::new();
            if !self.eat(&Tok::Punct(";")) {
                loop {
                    let (name, mut t) = self.declarator(bt, true)?;
                    let vla = self.vla_size.take(); // C99
                    // EXT(gcc): asm-label trên LOCAL = ghim reg (musl syscall
                    // `register long x8 __asm__("x8")`); gỡ khỏi renames vì
                    // local không có symbol để rename
                    let pin = self.renames.remove(&name).and_then(|l| {
                        l.strip_prefix('x')
                            .or_else(|| l.strip_prefix('w'))
                            .and_then(|d| d.parse::<u8>().ok())
                            .filter(|&r| r < 29)
                    });
                    // EXT(gcc): nested function definition — declarator ra Func và
                    // "{" theo sau (không phải prototype). Là declarator DUY NHẤT.
                    if let Ty::Func(fidx) = self.tt.tys[t as usize] {
                        if self.peek("{") {
                            self.nested_funcdef(name, t, fidx)?;
                            return Ok(self.push(Node::Block(stmts), INT)); // không có ";"
                        }
                    }
                    match storage {
                        Storage::Typedef => {
                            // C99 6.7.7: typedef của variably-modified type
                            // (`typedef int c[i+2]`) — sizeof phải eval runtime.
                            // zcc chỉ lower VLA thành Alloca cho local object, không
                            // có chỗ treo size-expr cho typedef ⇒ TỪ CHỐI sạch thay
                            // vì trả sizeof=0 (miscompile: 20040411-1).
                            if vla.is_some() {
                                return Err("variably-modified typedef: chưa hỗ trợ (C99 6.7.7)".into());
                            }
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
                                is_tls: tls,
                                is_weak: false,
                            });
                            self.locals
                                .push((name, t, Vloc::Glob(self.globals.len() as u32 - 1)));
                        }
                        Storage::Extern => {
                            // C99 6.2.2p4: block-scope extern có prior declaration
                            // cùng tên (file-scope) → GIỮ nguyên linkage cũ, trỏ về
                            // ĐÚNG global đó. Chỉ push extern mới khi tên chưa từng
                            // khai báo trong TU (nếu không: extern int b của
                            // `static int b=20` alias nhầm sang global khác).
                            let gi = self
                                .globals
                                .iter()
                                .position(|g| g.name == name)
                                .unwrap_or_else(|| {
                                    self.globals.push(Global {
                                        name: name.clone(),
                                        ty: t,
                                        init: GInit::None,
                                        is_static: false,
                                        is_extern: true,
                                        is_tls: tls,
                                        is_weak: false,
                                    });
                                    self.globals.len() - 1
                                });
                            self.locals.push((name, t, Vloc::Glob(gi as u32)));
                        }
                        // C99: VLA local → con trỏ + alloca(n*sizeof(elem)).
                        // Hệ quả biết trước: sizeof(vla) = 8 (size con trỏ), sai
                        // spec nhưng redis không đụng; epilogue mov sp,x29 thu hồi.
                        _ if vla.is_some() => {
                            // con trỏ tới VLA `char (*p)[w]` (musl): KHÔNG cấp
                            // phát — chỉ ghi size byte pointee vào local ẩn để
                            // mkbin scale runtime; init scalar bình thường
                            if let Ty::Ptr(inner) = self.tt.tys[t as usize] {
                                let Ty::Array(elem, 0) = self.tt.tys[inner as usize] else {
                                    return Err(
                                        "size không hằng sau con trỏ: chỉ hỗ trợ (*p)[w]".into()
                                    );
                                };
                                let w = self.cast(vla.unwrap(), ULONG);
                                let esz = self.push(Node::Num(self.tt.size(elem) as i64), ULONG);
                                let bytes = self.push(Node::Bin("*", w, esz), ULONG);
                                let hid = self.alloc_local(format!("{name}.vlasz"), ULONG);
                                let hv = self.push(Node::Var(hid), ULONG);
                                stmts.push(self.mkassign(hv, bytes)?);
                                self.vla_arrs.insert(inner, hid);
                                let off = self.alloc_local(name, t);
                                if let Some(r) = pin {
                                    self.reg_pins.insert(off, r);
                                }
                                if self.eat(&Tok::Punct("=")) {
                                    let e = self.assign()?;
                                    let v = self.push(Node::Var(off), t);
                                    stmts.push(self.mkassign(v, e)?);
                                }
                            } else {
                                if self.peek("=") {
                                    return Err("VLA có initializer".into());
                                }
                                let elem = match self.tt.tys[t as usize] {
                                    Ty::Array(e, _) => e,
                                    _ => return Err("size không hằng ngoài mảng".into()),
                                };
                                let pt = self.tt.ptr_to(elem);
                                let off = self.alloc_local(name.clone(), pt);
                                let n = self.cast(vla.unwrap(), ULONG);
                                let esz = self.tt.size(elem);
                                let sz = self.push(Node::Num(esz as i64), ULONG);
                                let bytes = self.push(Node::Bin("*", n, sz), ULONG);
                                // C99 6.5.3.4p2: sizeof(vla) = runtime — chốt
                                // số byte vào local ẩn (size expr eval đúng 1
                                // lần), sizeof + alloca cùng đọc lại từ đó
                                let hid = self.alloc_local(format!("{name}.vlasz"), ULONG);
                                let hv = self.push(Node::Var(hid), ULONG);
                                stmts.push(self.mkassign(hv, bytes)?);
                                self.vla_szs.insert(off, hid);
                                let hv2 = self.push(Node::Var(hid), ULONG);
                                let al = self.push(Node::Alloca(hv2), pt);
                                let v = self.push(Node::Var(off), pt);
                                stmts.push(self.mkassign(v, al)?);
                            }
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
                                if let Some(r) = pin {
                                    self.reg_pins.insert(off, r);
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
                                                let bv =
                                                    self.push(Node::Member(v, o + bi as u32), CHAR);
                                                let num = self.push(Node::Num(byte as i64), INT);
                                                stmts.push(self.mkassign(bv, num)?);
                                            }
                                        }
                                    }
                                }
                            } else {
                                let off = self.alloc_local(name, t);
                                if let Some(r) = pin {
                                    self.reg_pins.insert(off, r);
                                }
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
            let (mut b, mut w) = (b.clone(), w.0);
            self.pos += 1;
            while let Some(Tok::Str(m, w2)) = self.toks.get(self.pos) {
                b.extend_from_slice(m);
                w = w.max(w2.0); // nối chuỗi: prefix rộng nhất thắng (C11 6.4.5p5)
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
        let init = self.unbrace_str(t, init);
        match init {
            Init::S(mut b, w) => {
                if let Ty::Array(e, n) = self.tt.tys[t as usize] {
                    let es = self.tt.size(e);
                    if w > 1 && es == w as u32 {
                        // wchar_t/char16_t ws[] = L".."/u"..": mỗi codepoint → es
                        // byte LE + NUL es byte (khớp width phần tử mảng)
                        let cps = wchars(&b);
                        let ew = es as usize;
                        let mut wb = Vec::with_capacity((cps.len() + 1) * ew);
                        for c in cps {
                            wb.extend_from_slice(&c.to_le_bytes()[..ew]);
                        }
                        wb.extend(std::iter::repeat(0).take(ew));
                        if n > 0 && wb.len() as u64 > n * es as u64 {
                            wb.truncate((n * es as u64) as usize);
                        }
                        out.push((base, t, FlatItem::B(wb)));
                        return Ok(());
                    }
                    if es == 1 && w == 1 {
                        b.push(0);
                        if n > 0 && b.len() as u64 > n {
                            b.truncate(n as usize); // char s[3] = "abc" hợp lệ C89
                        }
                        out.push((base, t, FlatItem::B(b)));
                        return Ok(());
                    }
                }
                // char *p = "str" / wchar_t *p = L"str" / char16_t *p = u"str"
                let (n, st) = if w > 1 {
                    let cps = wchars(&b);
                    let (n, ew) = (cps.len() as u32, w as usize);
                    let mut wb = Vec::with_capacity((cps.len() + 1) * ew - 1);
                    for c in cps {
                        wb.extend_from_slice(&c.to_le_bytes()[..ew]);
                    }
                    wb.extend(std::iter::repeat(0).take(ew - 1)); // .asciz bù NUL cuối
                    b = wb;
                    (n + 1, if w == 2 { USHORT } else { INT })
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
                // complex FLOATING (init HẰNG tĩnh, !in_fn) là scalar leaf: gitem fold
                // (re,im) ra bytes. Local runtime (in_fn) PHẢI giữ đường cũ — assign
                // scalar vào struct{re,im} 16-byte sẽ sai/crash; descend vào .re.
                if !self.in_fn && self.cplx_elem(t).is_some_and(|el| self.tt.is_float(el)) {
                    out.push((base, t, FlatItem::E(e)));
                    return Ok(());
                }
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
                while let Some((d, _)) = it.peek_mut() {
                    match d {
                        // quay lui index TRƯỚC khi xét đầy; desig của tầng NÀY dùng
                        // xong phải XÓA — elision descend giữ chung iterator, tầng
                        // trong thấy lại sẽ apply nhầm (hoặc lặp vô hạn với Mem)
                        Desig::Idx(k) => {
                            i = *k;
                            *d = Desig::No;
                        }
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
                        let nm = nm.clone();
                        i = match members.iter().position(|(mn, ..)| *mn == nm) {
                            Some(k) => {
                                // desig tầng NÀY đã dùng: xóa kẻo tầng trong lặp vô hạn
                                it.peek_mut().unwrap().0 = Desig::No;
                                k
                            }
                            None => match members.iter().position(|(mn, mt2, _)| {
                                mn.is_empty()
                                    && matches!(self.tt.tys[*mt2 as usize], Ty::Struct(s2)
                                        if self.find_member(s2, &nm).is_some())
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
    // C89 3.5.7: string literal init mảng char/wchar được bọc {} tùy chọn
    // (luac.c: static char Output[]={ "luac.out" }) — bóc ngoặc ra Init::S
    fn unbrace_str(&self, t: TypeId, init: Init) -> Init {
        if let Ty::Array(e, _) = self.tt.tys[t as usize] {
            let esz = self.tt.size(e);
            let hit = matches!(&init, Init::L(v) if v.len() == 1
                && matches!(&v[0], (Desig::No, Init::S(_, w)) if esz == 1 || (*w > 1 && esz == *w as u32)));
            if hit {
                if let Init::L(v) = init {
                    return v.into_iter().next().unwrap().1;
                }
            }
        }
        init
    }
    fn flat_init(
        &mut self,
        t: &mut TypeId,
    ) -> Result<(Vec<(u32, TypeId, FlatItem)>, bool), String> {
        let init = self.parse_init()?;
        let init = self.unbrace_str(*t, init);
        let agg = matches!(init, Init::L(_) | Init::S(..));
        let mut flat = Vec::new();
        match (self.tt.tys[*t as usize], init) {
            (Ty::Array(e, 0), Init::L(v)) => {
                let mut it = v.into_iter().peekable();
                let n = self.fill_list(*t, 0, &mut it, &mut flat)?;
                *t = self.tt.add(Ty::Array(e, n.max(1) as u64));
            }
            (Ty::Array(e, 0), Init::S(b, w)) => {
                // wide L"..": độ dài = số CODEPOINT decode (6.4.5), không phải
                // byte thô — "Ä" = 1 wchar (U+00C4) chứ không 2 (bug wchar_t-1)
                let len = if w > 1 && self.tt.size(e) == w as u32 {
                    wchars(&b).len() as u64
                } else {
                    b.len() as u64
                };
                *t = self.tt.add(Ty::Array(e, len + 1));
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
        let Node::Bin("-", l, r) = self.nodes[e as usize] else {
            return None;
        };
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
                // decay MEMBER kiểu mảng: int *p = g.a.arr (C89 3.4 address
                // constant qua path . -> [] — chibicc initializer.c đòi)
                Node::Member(..) if matches!(self.tt.tys[self.ty(e) as usize], Ty::Array(..)) => {
                    return self.glval(e);
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
        // static init complex FLOATING: (re,im) → hai ô float liền kề {re,im}
        if let Some(el) = self.cplx_elem(t).filter(|&el| self.tt.is_float(el)) {
            let (re, im) = self.ccval(e).ok_or("init complex không hằng")?;
            let esz = self.tt.size(el);
            let bits =
                |v: f64| if esz == 4 { (v as f32).to_bits() as i64 } else { v.to_bits() as i64 };
            return Ok(GInit::List(vec![
                (0, esz, GInit::Num(bits(re))),
                (esz, esz, GInit::Num(bits(im))),
            ]));
        }
        if self.tt.is_float(t) {
            let v = self.fold_f(e0)?;
            if matches!(self.tt.tys[t as usize], Ty::LDouble) {
                // memory long double ELF = binary128: nới f64 → f128 (exact)
                return Ok(GInit::Bytes(f128_bytes(v).to_vec()));
            }
            let bits = if self.tt.size(t) == 4 {
                (v as f32).to_bits() as i64
            } else {
                v.to_bits() as i64
            };
            return Ok(GInit::Num(bits));
        }
        // hằng nguyên từ biểu thức thực: (int)1.9 v.v. — fold_f rồi truncate
        if self.tt.is_float(self.ty(e0)) || self.tt.is_float(self.ty(e)) {
            if let Ok(v) = self.fold_f(e) {
                // cast Rust saturate: unsigned phải đi đường u64 kẻo 1.8e19 → i64::MAX
                let n = if self.tt.is_unsigned(t) {
                    v as u64 as i64
                } else {
                    v as i64
                };
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
    fn flat_to_ginit(&mut self, flat: Vec<(u32, TypeId, FlatItem)>) -> Result<GInit, String> {
        let mut list: Vec<(u32, u32, GInit)> = Vec::new();
        let mut bfs: Vec<(u32, u32, u128)> = Vec::new(); // bitfield: (byte đầu, byte sau cuối, ảnh bit)
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
                    // bitfield: chỉ phát ĐÚNG dải byte field chiếm — Itanium cho
                    // member thường chen vào byte trống của container, phát trọn
                    // container sẽ đè/trôi hàng xóm (gdata List không quay lui)
                    if let Ty::Bitfield(_, boff, w) = self.tt.tys[mt as usize] {
                        let mask = !0u64 >> (64 - w); // w ≥ 1 (w=0 không có tên field)
                        let v = (self.fold(e)? as u64) & mask;
                        let (s0, s1) = (off + boff / 8, off + (boff + w).div_ceil(8));
                        bfs.push((s0, s1, (v as u128) << (boff % 8)));
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
        // các field giao byte nhau (chung container) OR thành một dải Bytes
        bfs.sort_by_key(|x| x.0);
        let mut runs: Vec<(u32, u32, u128)> = Vec::new();
        for (s0, s1, img) in bfs {
            match runs.last_mut() {
                Some(r) if s0 < r.1 => {
                    r.2 |= img << ((s0 - r.0) * 8);
                    r.1 = r.1.max(s1);
                }
                _ => runs.push((s0, s1, img)),
            }
        }
        for (s0, s1, img) in runs {
            let bytes: Vec<u8> = (0..s1 - s0).map(|k| (img >> (8 * k)) as u8).collect();
            list.push((s0, s1 - s0, GInit::Bytes(bytes)));
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
            // C99 6.5.17: vế phải qua lvalue conversion → array decay
            let t = self.arr_decay(self.ty(r));
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
            let t = self.arr_decay(self.ty(c));
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
            // C99 6.5.15: operand mảng decay về con trỏ — giữ type mảng thì arg
            // variadic tràn stack bị store_narrow cắt theo size mảng (git diff.c
            // `? " " : ""` → strh 2 byte → con trỏ rác, segv theo layout)
            let rt = self.arr_decay(tt_);
            Ok(self.push(Node::Cond(c, t, e), rt))
        }
    }
    // lvalue conversion C99 6.3.2.1p3: array → con trỏ phần tử (value = địa chỉ,
    // codegen array-expr vốn đã trả địa chỉ nên chỉ cần đổi type)
    fn arr_decay(&mut self, t: TypeId) -> TypeId {
        if let Ty::Array(e, _) = self.tt.tys[t as usize] {
            self.tt.ptr_to(e)
        } else {
            t
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
        // EXT(gcc): __real__/__imag__ z — phép chiếu ℂ→ℝ (C99 dùng creal/cimag)
        if self.eat_kw("__real__") || self.eat_kw("__real") {
            let e = self.unary()?;
            return self.cplx_proj(e, false);
        }
        if self.eat_kw("__imag__") || self.eat_kw("__imag") {
            let e = self.unary()?;
            return self.cplx_proj(e, true);
        }
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
                                is_tls: false,
                                is_weak: false,
                            });
                            let g = self.push(Node::GVar(self.globals.len() as u32 - 1), t);
                            return self.postfix_ops(g);
                        }
                        let (flat, _) = self.flat_init(&mut t)?;
                        let off = self.alloc_local(String::new(), t);
                        let v = self.push(Node::Var(off), t);
                        let mut acc =
                            if matches!(self.tt.tys[t as usize], Ty::Array(..) | Ty::Struct(_)) {
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
                                        let bv = self.push(Node::Member(v, o + bi as u32), CHAR);
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
            // C99: -z trên complex → 0 - z (0.0 giữ elem để không lên double)
            if let Some(el) = self.cplx_elem(self.ty(e)) {
                let z = self.push(Node::FNum(0.0), el);
                return self.mkbin("-", z, e);
            }
            let t = self.ty(e);
            let t = if self.tt.is_float(t) {
                t
            } else {
                self.promote(t)
            };
            let e = if self.tt.is_float(t) {
                e
            } else {
                self.cast(e, t)
            };
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
            if self.cplx_elem(self.ty(e)).is_some() {
                return self.cplx_conj(e); // ~z = liên hợp
            }
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
                // C99 6.5.3.4p2: sizeof(int[n]) với n không hằng → runtime
                if let Some(w) = self.vla_size.take() {
                    let Ty::Array(elem, 0) = self.tt.tys[t as usize] else {
                        return Err("size không hằng ngoài mảng".into());
                    };
                    let n = self.cast(w, ULONG);
                    let esz = self.push(Node::Num(self.tt.size(elem) as i64), ULONG);
                    return Ok(self.push(Node::Bin("*", n, esz), ULONG));
                }
                self.tt.size64(t)
            } else {
                let e = self.unary()?; // node toán hạng thành rác arena, chấp nhận
                // C99 6.5.3.4p2: toán hạng VLA → sizeof runtime, đọc local ẩn
                // .vlasz: `sizeof a` (local VLA, đã decay con trỏ trong rep)
                // qua vla_szs; `sizeof *p` (p = con trỏ tới VLA) qua vla_arrs
                if let Node::Var(off) = self.nodes[e as usize] {
                    if let Some(&hid) = self.vla_szs.get(&off) {
                        return Ok(self.push(Node::Var(hid), ULONG));
                    }
                }
                let t = self.ty(e);
                if let Some(&hid) = self.vla_arrs.get(&t) {
                    return Ok(self.push(Node::Var(hid), ULONG));
                }
                self.tt.size64(t)
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
        let (ret, params, variadic, oldstyle) = sig.ok_or("gọi thứ không phải hàm/con trỏ hàm")?;
        for (i, a) in args.iter_mut().enumerate() {
            if i < params.len() && !oldstyle {
                *a = self.cast(*a, params[i]);
            } else {
                // default argument promotions (+ decay mảng C99 6.5.2.2p6 —
                // thiếu thì slot stack cắt theo size mảng, xem arr_decay)
                let t = self.ty(*a);
                let pt = if matches!(self.tt.tys[t as usize], Ty::LDouble) {
                    t // C99 6.5.2.2p6: promotion chỉ nâng float→double, long double GIỮ NGUYÊN
                } else if self.tt.is_float(t) {
                    DOUBLE
                } else if matches!(self.tt.tys[t as usize], Ty::Array(..)) {
                    self.arr_decay(t)
                } else {
                    self.promote(t)
                };
                *a = self.cast(*a, pt);
            }
        }
        let nreg = if variadic {
            params.len() as u32
        } else {
            args.len() as u32
        };
        // struct >16B by value: ABI truyền GIÁN TIẾP — copy vào temp, đưa con trỏ.
        // Ngoại lệ: HFA đi by value (AAPCS B.4) — ELF kể cả anonymous (gcc pr92904
        // f7: d0-d3)
        for a in args.iter_mut() {
            let t = self.ty(*a);
            if matches!(self.tt.tys[t as usize], Ty::Struct(_))
                && self.tt.size(t) > 16
                && self.tt.hfa(t).is_none()
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
                let t = self
                    .tt
                    .pointee(self.ty(sum))
                    .ok_or("index thứ không phải mảng/con trỏ")?;
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
                let t = self
                    .tt
                    .pointee(self.ty(e))
                    .ok_or("-> trên thứ không phải con trỏ")?;
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
        let (mt, off) = self
            .find_member(sd, &name)
            .ok_or(format!("không có member: {}", name))?;
        Ok(self.push(Node::Member(base, off), mt))
    }
    // attr packed/aligned đứng SAU thân: tính lại layout tại chỗ
    fn repack(&mut self, t: TypeId, packed: bool, aligned: Option<u32>) {
        let Ty::Struct(si) = self.tt.tys[t as usize] else {
            return;
        };
        let sd = &self.tt.structs[si as usize];
        if sd.is_union
            || sd
                .members
                .iter()
                .any(|m| matches!(self.tt.tys[m.1 as usize], Ty::Bitfield(..)))
        {
            return;
        }
        let mut members = sd.members.clone();
        // packed: align hạ về 1 NHƯNG giữ phần aligned tường minh trước đó
        // (sd.align vượt align tự nhiên của member ⟺ có aligned(n) đứng trước)
        let natural = sd
            .members
            .iter()
            .map(|m| self.tt.align(m.1))
            .max()
            .unwrap_or(1);
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
        // EXT(gcc): __extension__ trước expr = no-op tắt cảnh báo pedantic
        // (obstack.h của git: __extension__ ({ ... }))
        while self.eat_kw("__extension__") {}
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
        } else if let Some(&Tok::FNum(v, k)) = self.toks.get(self.pos) {
            self.pos += 1;
            let v = if k != 0 { v } else { v as f32 as f64 };
            let t = match k {
                0 => FLOAT,
                2 => LDOUBLE, // suffix L (ELF: binary128 tại biên ABI)
                _ => DOUBLE,
            };
            Ok(self.push(Node::FNum(v), t))
        } else if let Some(&Tok::INum(v, k)) = self.toks.get(self.pos) {
            // C99 6.4.4.2: hằng ảo `v i` = 0 + v·i (phép nhúng ℝ→ℂ)
            self.pos += 1;
            let elem = if k == 0 { FLOAT } else { DOUBLE };
            let v = if k == 0 { v as f32 as f64 } else { v };
            Ok(self.cplx_imag(v, elem))
        } else if let Some(Tok::Str(bytes, w)) = self.toks.get(self.pos) {
            let (mut bytes, mut wid, mut cps) = (bytes.clone(), w.0, w.1.clone());
            self.pos += 1;
            while let Some(Tok::Str(more, w2)) = self.toks.get(self.pos) {
                bytes.extend_from_slice(more); // phase 6: nối string liền kề
                wid = wid.max(w2.0); // prefix rộng nhất thắng (C11 6.4.5p5)
                cps.extend_from_slice(&w2.1);
                self.pos += 1;
            }
            if wid > 1 {
                // wide/char16: mỗi codepoint → wid byte little-endian; .asciz thêm
                // 1 NUL nên tự pad wid-1 — đủ terminator wid byte. u""→USHORT(2),
                // L""/U""→INT(4). Dùng cps (ký tự nguồn đã tách khỏi escape).
                let (n, ew) = (cps.len() as u32, wid as usize);
                let mut wb = Vec::with_capacity((cps.len() + 1) * ew - 1);
                for c in &cps {
                    wb.extend_from_slice(&c.to_le_bytes()[..ew]);
                }
                wb.extend(std::iter::repeat(0).take(ew - 1));
                self.strs.push(wb);
                let i = (self.strs.len() - 1) as u32;
                let elem = if wid == 2 { USHORT } else { INT };
                let t = self.tt.add(Ty::Array(elem, n as u64 + 1));
                return Ok(self.push(Node::Str(i), t));
            }
            self.strs.push(bytes);
            let i = (self.strs.len() - 1) as u32;
            let t = self
                .tt
                .add(Ty::Array(CHAR, self.strs[i as usize].len() as u64 + 1));
            Ok(self.push(Node::Str(i), t))
        } else if let Some(Tok::Ident(_)) = self.toks.get(self.pos) {
            let n = self.ident()?;
            // ngoài body hàm locals là đồ thừa hàm TRƯỚC (không dọn để rẻ) —
            // cấm tra, kẻo ginit toàn cục &x ăn nhầm param cùng tên (git rm.c:
            // param index_only của check_local_mod vs global index_only)
            if let Some(idx) = self.locals.iter().rposition(|(l, ..)| self.in_fn && *l == n) {
                let (t, loc) = (self.locals[idx].1, self.locals[idx].2);
                match loc {
                    // EXT(gcc): index < upvar_base ⟹ biến automatic của hàm bao →
                    // Upvar (đọc qua static chain [x18 - off]), không phải Var.
                    Vloc::Stack(off) if idx < self.upvar_base => {
                        return Ok(self.push(Node::Upvar(off), t));
                    }
                    Vloc::Stack(off) => return Ok(self.push(Node::Var(off), t)),
                    Vloc::Glob(gi) => return Ok(self.push(Node::GVar(gi), t)),
                    Vloc::Fn => {} // rơi xuống nhánh tra self.fns phía dưới
                }
            }
            if n == "__va_area__" {
                let t = self.tt.ptr_to(CHAR);
                return Ok(self.push(Node::VaArea(self.va_off), t));
            }
            // ELF: stdarg.h nhánh AAPCS đổ về 2 builtin thật (Darwin: macro che tên)
            if n == "__builtin_va_start" {
                self.expect(Tok::Punct("("))?;
                let ap = self.assign()?;
                self.expect(Tok::Punct(","))?;
                let mark = self.nodes.len();
                let _ = self.assign()?; // last: chỉ cần tên, không eval
                self.nodes.truncate(mark);
                self.types.truncate(mark);
                self.expect(Tok::Punct(")"))?;
                return Ok(self.push(Node::VaStart(ap), VOID));
            }
            if n == "__builtin_va_arg" {
                self.expect(Tok::Punct("("))?;
                let ap = self.assign()?;
                self.expect(Tok::Punct(","))?;
                let ty = self.typename()?;
                self.expect(Tok::Punct(")"))?;
                // struct: cấp scratch — backend cần vùng liên tục khi gather HFA
                let tmp = if matches!(self.tt.tys[ty as usize], Ty::Struct(_)) {
                    self.alloc_local(format!(".vaarg{}", self.nodes.len()), ty)
                } else {
                    0
                };
                return Ok(self.push(Node::VaArg(ap, ty, tmp), ty));
            }
            if n == "__func__" || n == "__FUNCTION__" || n == "__PRETTY_FUNCTION__" {
                let bytes = self.fname.clone().into_bytes();
                let ln = bytes.len() as u32;
                self.strs.push(bytes);
                let i = (self.strs.len() - 1) as u32;
                let t = self.tt.add(Ty::Array(CHAR, ln as u64 + 1));
                return Ok(self.push(Node::Str(i), t));
            }
            if n == "__builtin_types_compatible_p" {
                // EXT(gcc): fold hằng 0/1 so kiểu cấu trúc — git ARRAY_SIZE
                // (BUILD_ASSERT_OR_ZERO: sizeof(char[1-2*!(cond)])) cần nó
                // trong constant expression; array ≠ pointer là điểm ăn tiền
                self.expect(Tok::Punct("("))?;
                let a = self.typename()?;
                self.expect(Tok::Punct(","))?;
                let b = self.typename()?;
                self.expect(Tok::Punct(")"))?;
                let v = self.ty_compat(a, b) as i64;
                return Ok(self.push(Node::Num(v), INT));
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
            // EXT(gcc): __builtin_{add,sub,mul}_overflow(a, b, &res) — hạ xuống
            // Node::Overflow; codegen phát chuỗi 128-bit (xem ext::overflow_emit).
            // KHÔNG cast toán hạng: giữ kiểu gốc (dấu/width) đúng ngữ nghĩa GCC.
            if let Some(oop) = match n.as_str() {
                "__builtin_add_overflow" => Some(0u8),
                "__builtin_sub_overflow" => Some(1u8),
                "__builtin_mul_overflow" => Some(2u8),
                _ => None,
            } {
                self.expect(Tok::Punct("("))?;
                let a = self.assign()?;
                self.expect(Tok::Punct(","))?;
                let b = self.assign()?;
                self.expect(Tok::Punct(","))?;
                let rp = self.assign()?;
                self.expect(Tok::Punct(")"))?;
                return Ok(self.push(Node::Overflow(oop, a, b, rp), INT));
            }
            // EXT(gcc): atomics __sync_* (M12) — không phải symbol libc, hạ thẳng
            // xuống Node::Sync cho codegen phát ldaxr/stlxr; bảng tên ở ext.rs
            if let Some((op, arity)) = crate::ext::sync_op(&n) {
                if self.peek("(") {
                    self.pos += 1;
                    let mut args = Vec::new();
                    for k in 0..arity {
                        if k > 0 {
                            self.expect(Tok::Punct(","))?;
                        }
                        args.push(self.assign()?);
                    }
                    self.expect(Tok::Punct(")"))?;
                    // Barrier không có ptr; còn lại kiểu + size = pointee của arg đầu
                    let (et, sz) = if arity == 0 {
                        (VOID, 0)
                    } else {
                        let et = self
                            .tt
                            .pointee(self.ty(args[0]))
                            .ok_or_else(|| format!("{n}: arg đầu phải là con trỏ"))?;
                        let ok = self.tt.is_integer(et)
                            || matches!(self.tt.tys[et as usize], Ty::Ptr(_));
                        let sz = self.tt.size(et);
                        if !ok || (sz != 4 && sz != 8) {
                            return Err(format!(
                                "{n}: mới hỗ trợ operand integer/pointer 4|8 byte"
                            ));
                        }
                        (et, sz)
                    };
                    for k in 1..args.len() {
                        args[k] = self.cast(args[k], et); // value arg về đúng độ rộng operand
                    }
                    let ret = match op {
                        SyncOp::BoolCas => INT,
                        SyncOp::Release | SyncOp::Barrier => VOID,
                        _ => et,
                    };
                    return Ok(self.push(Node::Sync(op, args, sz), ret));
                }
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
                // EXT(gcc): tham chiếu tên nested function (gọi/truyền) → dựng
                // trampoline trên frame hiện tại; giá trị = địa chỉ trampoline.
                if let Some((sym, _)) = self.nested_fns.get(&n).cloned() {
                    let tt = self.tt.add(Ty::Array(ULONG, 5)); // 40B slot
                    let slot = self.alloc_local(format!(".tramp{}", self.nodes.len()), tt);
                    return Ok(self.push(Node::Tramp(sym, slot), pt));
                }
                let n = self.funref(n); // EXT(gcc): asm-label rename
                return Ok(self.push(Node::FunAddr(n), pt));
            }
            if self.peek("(") {
                // __builtin_abort... → abort (GCC builtin đổ về libc)
                let n = n
                    .strip_prefix("__builtin_")
                    .map(str::to_string)
                    .unwrap_or(n);
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
                    let n = self.funref(n); // EXT(gcc): asm-label rename
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
        let mut ranges: Vec<(u32, u32)> = Vec::new(); // body [n0,n1) từng func
        while self.pos < self.toks.len() {
            // EXT(gcc): __asm__("...") cấp toàn cục → phát verbatim (musl
            // crt_arch.h định nghĩa _start; PHẢI bắt trước decl_specs vì
            // skip_attrs sẽ nuốt nhầm thành asm-label)
            if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if n == "__asm__" || n == "__asm" || n == "asm")
                && self.toks.get(self.pos + 1) == Some(&Tok::Punct("("))
            {
                self.pos += 2;
                let mut s = String::new();
                while let Some(Tok::Str(b, _)) = self.toks.get(self.pos) {
                    s.push_str(&String::from_utf8_lossy(b));
                    self.pos += 1;
                }
                self.expect(Tok::Punct(")"))?;
                self.expect(Tok::Punct(";"))?;
                self.raw_asm.push(s);
                continue;
            }
            if self.eat_kw("_Static_assert") {
                self.static_assert()?; // EXT(c11): file-scope
                continue;
            }
            let (bt, storage) = match self.decl_specs()? {
                Some(x) => x,
                None => (INT, Storage::None), // implicit int: main() {...}
            };
            // chốt NGAY: declarator/param/body sẽ gọi decl_specs và reset flag
            let inline_fn = self.saw_inline;
            let tls = self.saw_thread;
            let weak_fn = self.attr_weak; // EXT(gcc): weak đứng trước declarator
            if self.eat(&Tok::Punct(";")) {
                continue; // định nghĩa struct/union/enum thuần
            }
            let (name, t) = self.declarator(bt, true)?;
            if self.vla_size.take().is_some() {
                return Err("VLA chỉ được là biến local".into()); // C99
            }
            // funcdef: declarator ra kiểu Func và theo sau là "{" hoặc old-style decl list
            if let Ty::Func(fidx) = self.tt.tys[t as usize] {
                let is_def = self.peek("{")
                    || matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n) && n != "typedef");
                if is_def {
                    self.fns.insert(name.clone(), t);
                    let sig = self.tt.fns[fidx as usize].clone();
                    self.locals.clear();
                    self.reg_pins.clear(); // key = offset stack — hàm mới trùng offset là dính pin ma
                    self.vla_szs.clear(); // cùng lý do: key offset stack
                    self.cur_off = 0;
                    self.fret = sig.ret;
                    self.fname = name.clone();
                    // EXT(gcc): danh tính top-level (nested def trong body đọc làm
                    // parent_uid); nested_fns/nl_labels reset theo từng top-level.
                    let uid = self.fn_uid;
                    self.fn_uid += 1;
                    self.cur_uid = uid;
                    self.cur_parent_uid = u32::MAX;
                    self.upvar_base = 0;
                    self.nested_fns.clear();
                    self.nl_labels.clear();
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
                    let (params, sret) = self.setup_params(&sig, &ptypes);
                    self.in_fn = true;
                    let n0 = self.nodes.len() as u32; // range node của body (cho DCE weak)
                    let body = self.stmt()?;
                    let n1 = self.nodes.len() as u32;
                    self.in_fn = false;
                    let is_static = storage == Storage::Static || self.static_fns.contains(&name);
                    // EXT(gcc): inline (kể cả static inline) không có declaration
                    // trần (C99 6.7.4p7) → ứng viên DCE; codegen phát weak nếu
                    // non-static để các TU cùng phát coalesce được
                    let is_inline = inline_fn && !self.plain_decls.contains(&name);
                    funcs.push(Func {
                        name,
                        params,
                        frame: (self.cur_off + 15) & !15,
                        body,
                        ret: sig.ret,
                        is_static,
                        is_inline,
                        is_weak: weak_fn || self.attr_weak,
                        variadic: sig.variadic,
                        sret,
                        uid,
                        parent_uid: u32::MAX,
                        chain: 0,
                        has_vla: !self.vla_szs.is_empty(),
                    });
                    ranges.push((n0, n1));
                    // EXT(gcc): nested func gom trong body → xả vào funcs (static,
                    // không DCE; range (0,0) vì is_inline=false không đọc tới).
                    for nf in std::mem::take(&mut self.pending) {
                        funcs.push(nf);
                        ranges.push((0, 0));
                    }
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
                    if !inline_fn {
                        self.plain_decls.insert(name.clone()); // C99 6.7.4p7
                    }
                    // EXT(gcc): weak_alias musl — declaration hàm mang alias("old")
                    if let Some(old) = self.attr_alias.take() {
                        self.aliases
                            .push((name.clone(), old, weak_fn || self.attr_weak));
                    } else if weak_fn || self.attr_weak {
                        self.weak_decls.push(name.clone()); // weak proto trần
                    }
                    self.fns.insert(name, t); // prototype
                } else {
                    // C89 6.1.2.1: tên vào scope từ CUỐI declarator, trước init
                    // → push trước để initializer tự tham chiếu được
                    // (git LIST_HEAD: static struct x = { &x, &x })
                    // tentative definition: int x; int x = 3; int x; → MỘT symbol
                    // EXT(gcc): alias trên object (musl weak_alias data) — chỉ
                    // phát .set, không phát storage; vẫn đăng ký extern để TU
                    // này tham chiếu tên mới được
                    let obj_alias = self.attr_alias.take();
                    if let Some(old) = &obj_alias {
                        self.aliases
                            .push((name.clone(), old.clone(), weak_fn || self.attr_weak));
                    }
                    let gi = if let Some(gi) = self.globals.iter().position(|g| g.name == name) {
                        gi
                    } else {
                        self.globals.push(Global {
                            name,
                            ty: t,
                            init: GInit::None,
                            is_static: storage == Storage::Static,
                            is_extern: true, // tạm; chốt sau khi biết có init
                            is_tls: tls,
                            is_weak: false,
                        });
                        self.globals.len() - 1
                    };
                    let init = self.ginit(&mut t)?;
                    let is_extern = (storage == Storage::Extern || obj_alias.is_some())
                        && matches!(init, GInit::None);
                    let bigger = self.tt.size(t) > self.tt.size(self.globals[gi].ty);
                    let g = &mut self.globals[gi];
                    if bigger {
                        g.ty = t; // int a[]; → int a[3]; hoàn thiện kiểu
                    }
                    if !matches!(init, GInit::None) {
                        g.init = init;
                    }
                    g.is_extern = g.is_extern && is_extern;
                    g.is_tls = g.is_tls || tls;
                    g.is_weak = g.is_weak || weak_fn || self.attr_weak;
                }
                if !self.eat(&Tok::Punct(",")) {
                    break;
                }
                cur = self.declarator(bt, true)?;
            }
            self.expect(Tok::Punct(";"))?;
        }
        // EXT(gcc): DCE hàm weak (inline) không ai với tới — như clang không
        // phát inline chưa dùng. Bắt buộc: body inline trong header (server.h
        // redis) tham chiếu symbol mà TU khác (redis-cli) không link.
        // Root = mọi tham chiếu NGOÀI body các hàm weak (hàm thường, global
        // init); lan truyền qua đồ thị gọi giữa các hàm weak.
        let weak_idx: HashMap<&str, usize> = funcs
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_inline)
            .map(|(i, f)| (f.name.as_str(), i))
            .collect();
        if !weak_idx.is_empty() {
            let mut in_weak = vec![false; self.nodes.len()];
            for (i, f) in funcs.iter().enumerate() {
                if f.is_inline {
                    in_weak[ranges[i].0 as usize..ranges[i].1 as usize].fill(true);
                }
            }
            fn refname(n: &Node) -> Option<&str> {
                match n {
                    Node::Call(nm, ..) | Node::FunAddr(nm) => Some(nm.as_str()),
                    _ => None,
                }
            }
            let mut used = vec![false; funcs.len()];
            let mut queue: Vec<usize> = Vec::new();
            for (k, n) in self.nodes.iter().enumerate() {
                if !in_weak[k] {
                    if let Some(&i) = refname(n).and_then(|nm| weak_idx.get(nm)) {
                        if !used[i] {
                            used[i] = true;
                            queue.push(i);
                        }
                    }
                }
            }
            while let Some(i) = queue.pop() {
                for k in ranges[i].0..ranges[i].1 {
                    if let Some(&j) =
                        refname(&self.nodes[k as usize]).and_then(|nm| weak_idx.get(nm))
                    {
                        if !used[j] {
                            used[j] = true;
                            queue.push(j);
                        }
                    }
                }
            }
            let mut i = 0;
            funcs.retain(|f| {
                let keep = !f.is_inline || used[i];
                i += 1;
                keep
            });
        }
        Ok(funcs)
    }
}

// workaround borrow: decl_specs giữ &str từ token — copy ra ngoài vòng đời
fn n_hack(base: Option<&str>) -> &str {
    base.unwrap_or("")
}

pub fn parse(
    toks: &[Tok],
    locs: &[(u32, u32)],
    files: &[String],
) -> Result<Ast, String> {
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
        plain_decls: std::collections::HashSet::new(),
        tags: HashMap::new(),
        typedefs: HashMap::new(),
        enums: HashMap::new(),
        enum_tags: HashMap::new(),
        switches: Vec::new(),
        fret: INT,
        va_off: 16,
        in_fn: false,
        attr_aligned: None,
        saw_inline: false,
        saw_thread: false,
        vla_size: None,
        in_params: 0,
        reg_pins: HashMap::new(),
        vla_arrs: HashMap::new(),
        vla_szs: HashMap::new(),
        cplx_tys: HashMap::new(),
        asm_label: None,
        renames: HashMap::new(),
        fname: String::new(),
        attr_weak: false,
        attr_transp: false,
        attr_alias: None,
        raw_asm: Vec::new(),
        aliases: Vec::new(),
        weak_decls: Vec::new(),
        fn_uid: 0,
        cur_uid: u32::MAX,
        cur_parent_uid: u32::MAX,
        upvar_base: 0,
        nested_fns: HashMap::new(),
        nl_labels: Vec::new(),
        pending: Vec::new(),
    };
    // EXT(gcc): __uint128_t/__int128_t — CHỈ storage 16 byte align 16 (SDK mach
    // NEON state trong mcontext cần layout đúng); arithmetic không hỗ trợ.
    {
        p.tt.structs.push(crate::ast::StructDef {
            members: vec![("__lo".into(), ULONG, 0), ("__hi".into(), ULONG, 8)],
            size: 16,
            align: 16,
            is_union: false,
        });
        let t = p.tt.add(Ty::Struct(p.tt.structs.len() as u32 - 1));
        p.typedefs.insert("__uint128_t".into(), t);
        p.typedefs.insert("__int128_t".into(), t);
    }
    // ELF: seed struct __zcc_va_list COMPLETE — header ngoài (musl stdarg.h)
    // chỉ thấy `__builtin_va_list` = tên tag này, không có thân; thiếu seed thì
    // va_list local size 0 → va_start ghi 32 byte đè slot hàng xóm (vfprintf
    // musl nát con trỏ fmt). stdarg.h nhúng định nghĩa lại chỉ shadow, vô hại.
    {
        let pv = p.tt.ptr_to(VOID);
        p.tt.structs.push(crate::ast::StructDef {
            members: vec![
                ("__stack".into(), pv, 0),
                ("__gr_top".into(), pv, 8),
                ("__vr_top".into(), pv, 16),
                ("__gr_offs".into(), INT, 24),
                ("__vr_offs".into(), INT, 28),
            ],
            size: 32,
            align: 8,
            is_union: false,
        });
        let t = p.tt.add(Ty::Struct(p.tt.structs.len() as u32 - 1));
        p.tags.insert("__zcc_va_list".into(), t);
    }
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
        for f in [
            "copysign", "fabs", "sqrt", "floor", "ceil", "fmod", "pow", "atan2",
        ] {
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
        raw_asm: p.raw_asm,
        aliases: p.aliases,
        pic: false,
        weak_decls: p.weak_decls,
    })
}

// L"..": nguồn là UTF-8 → giải mã ra code point cho wchar_t (byte escape lẻ
// >127 không phải UTF-8 hợp lệ sẽ thành U+FFFD — chấp nhận)
fn wchars(b: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(b)
        .chars()
        .map(|c| c as u32)
        .collect()
}

// f64 → binary128 little-endian (nới EXACT, không rounding): sign giữ nguyên,
// exponent rebias 1023→16383, mantissa 52 bit dồn lên đỉnh field 112 bit;
// subnormal double normalize lại (f128 range dư sức chứa), inf/nan giữ dạng.
fn f128_bytes(v: f64) -> [u8; 16] {
    let b = v.to_bits();
    let (sg, e, m) = (b >> 63, (b >> 52) & 0x7ff, b & ((1u64 << 52) - 1));
    let (e2, m2): (u128, u128) = match e {
        0 if m == 0 => (0, 0),
        0 => {
            let sh = m.leading_zeros() - 11; // đưa bit dẫn về vị trí 52 (hidden)
            (
                16383 - 1022 - sh as u128,
                ((((m as u128) << sh) & ((1 << 52) - 1)) as u128) << 60,
            )
        }
        0x7ff => (0x7fff, (m as u128) << 60),
        _ => (e as u128 - 1023 + 16383, (m as u128) << 60),
    };
    (((sg as u128) << 127) | (e2 << 112) | m2).to_le_bytes()
}
