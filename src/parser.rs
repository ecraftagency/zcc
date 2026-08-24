// Parser: &[Tok] → AST arena (Vec<Node> + NodeId u32) + type arena (TyTab).
// C89 grammar (missing: initializer {..}, bitfield, struct by value — later passes):
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
// Type conversion: the parser inserts Node::Cast at every convergence point (usual
// arithmetic conversions, assignment, argument by prototype, return) — codegen only
// inspects the type to select instructions.
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
    Fn, // a prototype inside a block shadows an outer variable — look further in self.fns
}

// initializer tree: expr / list {..} / string literal
#[derive(Clone)]
enum Init {
    E(NodeId),
    L(Vec<(Desig, Init)>),
    S(Vec<u8>, u8), // element width in bytes (1 narrow, 2 char16, 4 wchar/char32)
}
// C99 designator ([i] = / .m =) — clang accepts it under -std=c89
#[derive(Clone)]
enum Desig {
    No,
    Idx(u32),
    Rng(u32, u32), // EXT(gcc): [lo ... hi] range designator
    Mem(String),
}
// leaf after flattening an initializer: an expr or a byte string (string into a char array)
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
    locals: Vec<(String, TypeId, Vloc)>, // scope = truncate when a block closes
    cur_off: u32,
    /// (offset, size) of every stack object of the function being parsed
    cur_objs: Vec<(u32, u32)>,
    globals: Vec<Global>,
    strs: Vec<Vec<u8>>,
    fns: HashMap<String, TypeId>, // function name → TypeId (Ty::Func)
    static_fns: std::collections::HashSet<String>, // declared static → definition keeps internal linkage
    tags: HashMap<String, TypeId>,
    typedefs: HashMap<String, TypeId>,
    enums: HashMap<String, i64>,
    enum_tags: HashMap<String, TypeId>, // tag → underlying (INT | UINT, matching clang)
    switches: Vec<(Vec<(i64, i64, NodeId)>, Option<NodeId>)>,
    fret: TypeId,              // return type of the function currently being parsed
    va_off: u32,               // offset from x29 to the anonymous-argument area (16 + 8*named-stack-params)
    in_fn: bool,               // inside a function body (compound literal: local vs implicit global)
    attr_aligned: Option<u32>, // aligned(n) pending from decl_specs (member: pr23467)
    saw_inline: bool,          // EXT(gcc): decl_specs just saw inline/__inline (gnu89)
    saw_thread: bool,          // EXT(gcc): decl_specs just saw __thread (captured IMMEDIATELY after
    // decl_specs — an init containing a cast calls decl_specs recursively and would reset the flag)
    // EXT(gcc): a name with a NON-inline file-scope declaration — C99 6.7.4p7:
    // an inline definition of that name is still an external definition
    plain_decls: std::collections::HashSet<String>,
    // C99: declarator just saw an array with a non-constant size (VLA) — the size expr;
    // consumed only by stmt() (local decl), lowered to pointer + Alloca
    vla_size: Option<NodeId>,
    // C99 6.7.6.2: multidimensional VLA `int M[d0][d1]` — vla_size holds the OUTER
    // dimension (d0), vla_inner holds the INNER dimensions (d1, ...) in outer→inner
    // order. Consumed only by stmt() (2D-only for now; ≥3 dimensions are rejected cleanly).
    vla_inner: Vec<NodeId>,
    // C99 6.7.6.2/6.9.1: an array-parameter dimension WITH a side effect (`array[i++]`)
    // is evaluated on entry — the parameter decays to a pointer, but the side effect of
    // the size expr MUST run when the function is called. Store the (start,end) token range
    // of each dimension, since the parameter is not yet in scope while parsing the declarator;
    // re-parse in the prologue after setup_params (parameters are now locals). Drained on
    // entering a funcdef; cleared for a non-definition.
    param_vla_dims: Vec<(usize, usize)>,
    // currently parsing a parameter list (counted because of nesting): a non-constant
    // parameter-array size (glibc regex.h `__pmatch[__nmatch]` — __nmatch is a PRIOR
    // parameter, not in scope) → skip the expr; the parameter decays to a pointer, so the
    // size is meaningless
    in_params: u32,
    // EXT(gcc): a local pinned to a register `register long v __asm__("x8")` (musl syscall)
    // — key = the local's stack offset, value = GP register number
    reg_pins: HashMap<u32, u8>,
    // C99: pointer to a VLA `char (*p)[w]` (musl lsearch/dynlink) — key = the TypeId of
    // the Array(elem, 0) pointee (not interned, so unique per decl), value = offset of the
    // hidden local holding the runtime SIZE IN BYTES; mkbin scales by it
    vla_arrs: HashMap<TypeId, u32>,
    // C99 6.5.3.4p2: sizeof(local vla) is a RUNTIME value — key = offset of the VLA local,
    // value = offset of the hidden local `.vlasz` holding the byte count (fixed once at
    // declaration). Offsets are reused across functions → MUST be cleared per function.
    vla_szs: HashMap<u32, u32>,
    // C99 6.7.7 + 6.5.3.4p2: typedef of a variably-modified array (`typedef int c[i+2]`)
    // — the size is fixed at RUNTIME at the declaration, and sizeof(c) reads it back. Key =
    // TypeId Array(elem,0) (unique per decl), value = offset of the hidden local holding the
    // byte count. Cleared per function like vla_szs (frame-local offset).
    vm_typedef_sz: HashMap<TypeId, u32>,
    // C99: _Complex t (musl src/complex) is lowered to struct {re, im} — layout, union
    // punning, and HFA ABI (AAPCS64 treats a complex as a 2-element struct) all reuse the
    // existing struct machinery verbatim. Key = elem (FLOAT/DOUBLE), value = TypeId of the struct.
    cplx_tys: HashMap<TypeId, TypeId>,
    fname: String, // name of the function being parsed (label symbol for &&label in static init)
    asm_label: Option<String>, // EXT(gcc): __asm("_sym") just consumed in skip_attrs
    renames: HashMap<String, String>, // EXT(gcc): C name → __asm symbol (SDK versioning)
    attr_weak: bool,            // EXT(gcc): __attribute__((weak)) (musl)
    attr_transp: bool,          // EXT(gcc): transparent_union (glibc sockaddr arg)
    attr_alias: Option<String>, // EXT(gcc): __attribute__((alias("sym"))) (musl weak_alias)
    attr_mode: Option<(u32, bool)>, // EXT(gcc): mode(M) → (width-in-bytes, is_float); remap type
    raw_asm: Vec<String>,       // EXT(gcc): global-level __asm__("...") (musl crt)
    aliases: Vec<(String, String, bool)>, // (new, old, weak)
    // EXT(gcc): a prototype carrying weak (musl `extern weak hidden _DYNAMIC[]`) —
    // a referencing TU must emit .weak, otherwise a strong undefined reference makes the
    // link demand the symbol
    weak_decls: Vec<String>,
}

type R = Result<NodeId, String>;

// splice a GInit (base offset `off`) into a flat list (offset, size, item)
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
    "__complex__",  // EXT(gcc): alias for _Complex
    "__complex",    // EXT(gcc): form without trailing underscores (`__complex double`)
    "__const",
    "__volatile",
    "__signed",
    "__signed__",
    "__typeof__",
    "__typeof",
    "typeof", // EXT(gcc): bare typeof — used by kernel/coreutils/gnulib (minor risk:
              // a C89 program that names a variable `typeof` would break; no real code does)
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
            Err(format!("expected {:?}, found {:?}", want, self.toks.get(self.pos)))
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        if let Some(Tok::Ident(n)) = self.toks.get(self.pos) {
            self.pos += 1;
            Ok(n.clone())
        } else {
            Err(format!("expected identifier, found {:?}", self.toks.get(self.pos)))
        }
    }
    fn is_type_word(&self, n: &str) -> bool {
        TYPE_WORDS.contains(&n)
            // a typedef shadowed by a local variable of the SAME NAME (redis: quicklist *quicklist)
            // — after that declaration line the name is a variable, no longer a type
            // (only check shadowing INSIDE a body — outside a body, locals are leftovers from a prior function)
            || (self.typedefs.contains_key(n)
                && !(self.in_fn && self.locals.iter().any(|(ln, ..)| ln == n)))
            || ["typedef", "static", "extern", "auto", "register"].contains(&n)
    }
    // hard keyword — a typedef-name does NOT count (it may still be a declarator/member name)
    fn is_keyword(&self, n: &str) -> bool {
        TYPE_WORDS.contains(&n) || ["typedef", "static", "extern", "auto", "register"].contains(&n)
    }

    // ---- constants ----
    // EXT(gcc): structural compatibility for __builtin_types_compatible_p — TyTab does
    // not intern, so compare recursively. Functions compare by SIGNATURE (C99 6.7.5.3):
    // return type + parameter count + each parameter + the variadic flag must match
    // (chibicc builtin.c distinguishes int(*)(float,double) ≠ int(*)(float), (...) ≠ (void)).
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
    // EXT(c11): _Static_assert(const-expr[, "msg"]); — a declaration at both file scope
    // and block scope (required by postgres18 StaticAssertStmt/Decl). Evaluated
    // IMMEDIATELY at parse time: failure = a compile error, success = no node emitted.
    // The message is optional (C23 allows omitting it — accepted here for simplicity).
    fn static_assert(&mut self) -> Result<(), String> {
        self.expect(Tok::Punct("("))?;
        let v = self.const_expr()?;
        let mut msg = String::new();
        if self.eat(&Tok::Punct(",")) {
            while let Some(Tok::Str(b, _)) = self.toks.get(self.pos) {
                msg.push_str(&String::from_utf8_lossy(b)); // adjacent strings are concatenated
                self.pos += 1;
            }
        }
        self.expect(Tok::Punct(")"))?;
        self.expect(Tok::Punct(";"))?;
        if v == 0 {
            return Err(format!("_Static_assert failed: {msg}"));
        }
        Ok(())
    }
    fn fold(&self, id: NodeId) -> Result<i64, String> {
        match &self.nodes[id as usize] {
            Node::Num(v) => Ok(*v),
            Node::Neg(e) => Ok(self.fold(*e)?.wrapping_neg()),
            // &((T *)K)->m / &((T *)K)->a[i]: classic offsetof — an integer constant
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
                        _ => return Err("constant expression required".into()),
                    }
                }
            }
            Node::Cast(e) => {
                // narrow to the target type so that (char)300 etc. is correct
                let v = self.fold(*e)?;
                let t = self.ty(id);
                // _Bool does NOT narrow modularly: C99 6.3.1.2 = (value != 0). size 1 +
                // unsigned collides with the `v as u8` arm → (_Bool)0x100 would yield 0 (wrong). Handle first.
                if matches!(self.tt.tys[t as usize], Ty::Bool) {
                    return Ok((v != 0) as i64);
                }
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
            // both sides must fold — a Comma with a side effect is not a constant
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
                    "/" | "%" if r == 0 => return Err("division by zero in constant expression".into()),
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
                    _ => return Err("operator not usable in constant expression".into()),
                })
            }
            _ => Err("constant expression required".into()),
        }
    }
    // floating constant for a float-typed global initializer
    fn fold_f(&self, id: NodeId) -> Result<f64, String> {
        // an INTEGER-typed subtree (musl: (double)(1 << k) in POWF_SCALE):
        // fold it fully as an integer, then convert to floating according to signedness
        if !self.tt.is_float(self.ty(id))
            && let Ok(v) = self.fold(id) {
                return Ok(if self.tt.is_unsigned(self.ty(id)) {
                    v as u64 as f64
                } else {
                    v as f64
                });
            }
        match &self.nodes[id as usize] {
            Node::FNum(v) => Ok(*v),
            // unsigned 64-bit: 9223372036854775810ul must become 9.2e18, not negative
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
                    _ => return Err("operator not usable in floating constant expression".into()),
                })
            }
            _ => Err("floating constant required".into()),
        }
    }

    // ---- types ----
    // None = the current token does not begin a declaration
    fn decl_specs(&mut self) -> Result<Option<(TypeId, Storage)>, String> {
        self.attr_aligned = None; // belongs to the previous declaration; do not carry over
        self.saw_inline = false;
        self.saw_thread = false;
        self.attr_weak = false;
        self.attr_transp = false;
        self.attr_alias = None;
        self.attr_mode = None;
        let mut storage = Storage::None;
        let (mut base, mut direct) = (None::<&str>, None::<TypeId>);
        let (mut uns, mut sgn, mut short, mut longs, mut any) = (false, false, false, 0u32, false);
        let mut cplx = false; // C99: _Complex
        let mut vol = false; // C99 6.7.3: a `volatile` qualifier in this specifier-list
        loop {
            let n = match self.toks.get(self.pos) {
                Some(Tok::Ident(n)) => n.as_str(),
                _ => break,
            };
            match n {
                // C99 6.7.3 — volatile is captured (it changes access semantics);
                // the other qualifiers stay no-ops at this ABI.
                "volatile" | "__volatile" | "__volatile__" => vol = true,
                "const" | "auto" | "register" | "restrict" | "__restrict"
                | "__restrict__" | "__extension__" | "__const"
                | "__const__" | "_Noreturn" => {}
                // EXT(gcc): real __thread TLS (Mach-O @TLVP) — required by the redis-tests
                // io-threads>=2 unit. A plain __thread on an auto local (which gcc forbids)
                // naturally falls back to the stack — already per-thread.
                "__thread" => self.saw_thread = true,
                // EXT(gcc): inline — zcc has no inliner; an inline definition is lowered
                // to static (one copy per TU, like the "static __inline" branch of cdefs.h)
                // so that the SDK's gnu89 "extern __inline" does not emit a duplicate symbol
                "inline" | "__inline" | "__inline__" => self.saw_inline = true,
                // EXT(clang): nullability — semantically a no-op
                "_Nullable" | "_Nonnull" | "_Null_unspecified" => {}
                "__attribute__" | "__asm__" | "__asm" => {
                    let (pk, al) = self.skip_attrs()?;
                    if pk || al.is_some() {
                        // "struct {...} __attribute__((packed / aligned))" suffix
                        if let Some(t) = direct {
                            self.repack(t, pk, al);
                        } else if let Some(a) = al {
                            // "int __attribute__((aligned(8))) x" — hold it for the
                            // declarator/member to use (pr23467)
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
                // C99: _Complex — musl src/complex; lowered to struct {re, im}.
                // EXT(gcc): __complex__ / __complex are aliases (torture complex-*)
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
                // EXT(gcc): __typeof__(expr | typename) acts as a type-specifier
                "__typeof__" | "__typeof" | "typeof" => {
                    self.pos += 1;
                    self.expect(Tok::Punct("("))?;
                    let t = if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n))
                    {
                        self.typename()?
                    } else {
                        let e = self.expr()?; // the node becomes arena garbage, like sizeof
                        let mut t = self.ty(e);
                        // typeof(function name) = the FUNCTION TYPE, no decay (musl weak_alias:
                        // extern __typeof(f) g — g must be a function so the alias hits the right branch)
                        if let Node::FunAddr(_) = self.nodes[e as usize]
                            && let Ty::Ptr(p) = self.tt.tys[t as usize]
                                && matches!(self.tt.tys[p as usize], Ty::Func(_)) {
                                    t = p;
                                }
                        t
                    };
                    self.expect(Tok::Punct(")"))?;
                    direct = Some(t);
                    any = true;
                    continue;
                }
                _ => {
                    // typedef-name: only when no other type is present yet (and not
                    // shadowed by a local variable of the same name — redis: quicklist *quicklist)
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
        let n = n_hack(base); // avoid a borrow; see below
        if !any {
            return Ok(None);
        }
        if let Some(t) = direct {
            let t = if vol { self.tt.volatile_of(t) } else { t };
            return Ok(Some((t, storage)));
        }
        let t = match n {
            "void" => VOID,
            "char" => {
                // plain char: UNSIGNED on Linux arm64 (AAPCS64);
                // an explicit "signed char" is still CHAR
                if uns || !sgn {
                    UCHAR
                } else {
                    CHAR
                }
            }
            "float" => FLOAT,
            // C99 long double: ELF = binary128 AT the ABI/memory boundary (the arithmetic
            // is still double, float.h declares LDBL_MANT_DIG 53)
            "double" => {
                if longs > 0 {
                    LDOUBLE
                } else {
                    DOUBLE
                }
            }
            "_Bool" => BOOL,
            _ => {
                // the int family (even without an explicit "int")
                if short {
                    if uns { USHORT } else { SHORT }
                } else if longs > 0 {
                    if uns { ULONG } else { LONG }
                } else {
                    if uns { UINT } else { INT }
                }
            }
        };
        let t = self.apply_mode(t)?; // EXT(gcc): mode(M) at specifier position
        if cplx {
            // C99 6.2.5: complex is ONLY float/double/long double. `_Complex int/long/
            // char/unsigned…` = INTEGER complex (GNU ext, outside C99) → reject CLEANLY
            // instead of silently coercing to double complex (mis-size: complex int 8B vs
            // complex double 16B → miscompile, pr56837). Bare `_Complex` (no base
            // keyword) = double complex (gcc default). long double = double (documented).
            let elem = if t == FLOAT {
                FLOAT
            } else if t == DOUBLE || t == LDOUBLE {
                DOUBLE
            } else if base.is_some() || short || longs > 0 || uns || sgn {
                return Err(
                    "integer _Complex (GNU extension outside C99 6.2.5): not supported".into(),
                );
            } else {
                DOUBLE // bare _Complex = double _Complex
            };
            let ct = self.cplx_of(elem);
            let ct = if vol { self.tt.volatile_of(ct) } else { ct };
            return Ok(Some((ct, storage)));
        }
        let t = if vol { self.tt.volatile_of(t) } else { t };
        Ok(Some((t, storage)))
    }
    // C99: intern a struct {re, im} representing _Complex elem — AAPCS64 passes a
    // complex "as if" it were a 2-element struct, so the existing HFA machinery gives the correct ABI
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
    // reverse lookup: is t a complex type → elem. Volatile-agnostic: a complex type is
    // identified by its (unique) struct index, so a `volatile`-qualified complex — a distinct
    // TypeId carrying the SAME Ty::Struct — still resolves (C99 complex-7: volatile _Complex).
    fn cplx_elem(&self, t: TypeId) -> Option<TypeId> {
        let Ty::Struct(si) = self.tt.tys[t as usize] else {
            return None;
        };
        self.cplx_tys
            .iter()
            .find(|kv| matches!(self.tt.tys[*kv.1 as usize], Ty::Struct(s) if s == si))
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
            let tag = tag.ok_or("struct/union requires a tag or a body")?;
            // forward reference: create an incomplete placeholder; a later definition overwrites it
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
        // with a body: if the tag already has an INCOMPLETE placeholder, define into that
        // same slot (allowing self-reference); if the tag is already complete → a NEW definition
        // shadows it (inner scope)
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
        // layout per the Itanium ABI (locked to clang on Darwin — bitfields are impl-defined
        // in C89, but SDK interop requires matching the platform): the cursor allocates in
        // BITS; a bitfield is packed into the next free bit unless it would straddle the
        // container boundary of the declared type; an ordinary member is placed at the next
        // free aligned byte. An old bug (sizeof 12 vs 4 when a bitfield is interleaved with an
        // ordinary member) is caught by shape.sh.
        let (mut bits, mut mx) = (0u32, 1u32);
        while !self.eat(&Tok::Punct("}")) {
            let (bt, _) = self.decl_specs()?.ok_or("member type required")?;
            let attr_al = self.attr_aligned.take().unwrap_or(1);
            // no declarator: an anonymous struct/union (C11, allowed by clang) → hoist its
            // members into this level; any other type (a tag definition) → ignore
            if self.peek(";") {
                if let Ty::Struct(_) = self.tt.tys[bt as usize] {
                    // a SINGLE empty-named member (keeps the init cursor correct); accessed
                    // through it via recursive find_member
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
                // unnamed bitfield: "int : 3;" — no declarator
                let (mn, mt) = if self.peek(":") {
                    (String::new(), bt)
                } else {
                    self.declarator(bt, true)?
                };
                // C99 6.7.2.1: an array member must have a constant size — a VLA-in-struct
                // is a GNU extension (clang -pedantic-errors: "will never be supported").
                // Reject cleanly, and clear VLA state so it does not leak into the enclosing variable.
                if self.vla_size.take().is_some() {
                    self.vla_inner.clear();
                    return Err("VLA in struct/union (GNU) not supported".into());
                }
                if self.eat(&Tok::Punct(":")) {
                    let w = self.const_expr()? as u32;
                    let (s, al) = (self.tt.size(mt), self.tt.align(mt));
                    let cb = s * 8; // container per the declared type
                    if w == 0 {
                        // :0 — push the cursor to the next container boundary (3.5.2.1)
                        if !is_union {
                            bits = bits.div_ceil(cb) * cb;
                        }
                    } else {
                        // if it straddles a container boundary, move up to the next boundary
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
                            mx = mx.max(al); // unnamed does NOT affect alignment (Itanium)
                        }
                        bits = if is_union { bits.max(cb) } else { b + w };
                    }
                } else {
                    // layout in the u32 domain (object ≤4GB — documented deviation, ast.rs:116);
                    // wrapping to match size(), which already wraps — a huge array (2^62 short,
                    // 991014-1) wraps instead of a debug panic, and does NOT crash on valid input
                    let sz = self.tt.size(mt);
                    // an object >4GB is outside zcc's limit (layout size u32 — documented
                    // deviation, ast.rs:116). Accepting it wraps → sizeof/offset WRONG (991014-1). Reject CLEANLY.
                    if self.tt.size64(mt) > u32::MAX as u64 {
                        return Err(format!(
                            "member '{mn}' >4GB: object exceeds zcc's u32 size limit"
                        ));
                    }
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
        if let Some(Tok::Ident(n)) = self.toks.get(self.pos)
            && (!self.is_type_word(n) && self.toks.get(self.pos + 1) != Some(&Tok::Punct("{"))
                || self.toks.get(self.pos + 1) == Some(&Tok::Punct("{")))
            {
                tag = n.clone();
                self.pos += 1;
            }
        // EXT(clang): enum tag : underlying-type (SDK malloc.h) — honor the underlying type
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
            // match clang: if every enumerator is non-negative → underlying unsigned int
            // (important for an enum-typed bitfield: it must zero-extend)
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
    // full C declarator: pointer, nested "(...)", array/function suffixes.
    // need_name=false for an abstract declarator (cast, sizeof, unnamed parameter).
    fn declarator(&mut self, mut t: TypeId, need_name: bool) -> Result<(String, TypeId), String> {
        self.skip_attrs()?;
        while self.eat(&Tok::Punct("*")) {
            t = self.tt.ptr_to(t);
            // C99 6.7.5.1 — a qualifier right of `*` qualifies the POINTER itself
            // (`int * volatile p`: p is a volatile pointer to int), distinct from a
            // qualifier in the specifier-list, which qualifies the pointee.
            let mut pvol = false;
            loop {
                if self.eat_kw("volatile") || self.eat_kw("__volatile") || self.eat_kw("__volatile__")
                {
                    pvol = true;
                } else if self.eat_kw("const")
                    || self.eat_kw("restrict")
                    || self.eat_kw("__restrict")
                    || self.eat_kw("__restrict__")
                    // EXT(clang): nullability qualifier — the SDK uses it bare in FILE...
                    || self.eat_kw("_Nullable")
                    || self.eat_kw("_Nonnull")
                    || self.eat_kw("_Null_unspecified")
                {
                } else {
                    break;
                }
            }
            if pvol {
                t = self.tt.volatile_of(t);
            }
            self.skip_attrs()?; // "void *__attribute__((noinline)) f(...)"
        }
        if self.nested_ahead() {
            self.pos += 1; // '('
            let save = self.pos;
            self.declarator(VOID, false)?; // trial parse to find ')' — the type produced is arena garbage
            self.expect(Tok::Punct(")"))?;
            let outer = self.suffixes(t)?; // the OUTER suffix applies first (inside-out rule)
            let end = self.pos;
            self.pos = save;
            let res = self.declarator(outer, need_name)?;
            self.pos = end;
            // tail after the outer suffix: attribute/asm-label (mach_init.h:
            // "extern int (*f)(...) __printflike(1,0);")
            self.asm_label = None;
            self.skip_attrs()?;
            if let Some(l) = self.asm_label.take()
                && !res.0.is_empty() {
                    self.renames.insert(res.0.clone(), l);
                }
            return Ok(res);
        }
        let name = match self.toks.get(self.pos) {
            // the name MAY coincide with a typedef-name (shadow); only hard keywords are forbidden.
            // No need for need_name: the specifiers were already consumed by decl_specs, and an
            // abstract declarator never contains a bare ident → an ident here is ALWAYS a name
            // (git: parameter `reftable_fsck_report_fn report_fn`, where report_fn is also a
            // global typedef in usage.h)
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
                    "identifier required in declarator, found {:?}",
                    self.toks.get(self.pos)
                ));
            }
            _ => String::new(),
        };
        let mut t = self.suffixes(t)?;
        self.asm_label = None;
        // EXT(gcc): aligned(n) AFTER the declarator (torture 20050215-1: `typedef
        // struct {...} V __attribute__((aligned(8)))`) — a SEPARATE over-aligned type
        // (clone the def, do not mutate: the original tag used elsewhere stays unchanged)
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
        t = self.apply_mode(t)?; // EXT(gcc): trailing mode(M) (glibc register_t)
        // EXT(gcc): transparent_union — a call passes the argument per the ABI of the FIRST
        // member (gcc docs). Within scope this appears only in glibc PROTOTYPES (bind/connect…
        // under _GNU_SOURCE); nobody defines a function taking it → replacing the type directly
        // with the first member is fully faithful; keeping the real union would route through the
        // wrong composite protocol (redis/nginx bind EFAULT bug).
        if self.attr_transp {
            self.attr_transp = false;
            if let Ty::Struct(si) = &self.tt.tys[t as usize] {
                let d = &self.tt.structs[*si as usize];
                if d.is_union && !d.members.is_empty() {
                    t = d.members[0].1;
                }
            }
        }
        // EXT(gcc): asm-label after the declarator → the real symbol when emitting (Call/FunAddr)
        if let Some(l) = self.asm_label.take()
            && !name.is_empty() {
                self.renames.insert(name.clone(), l);
            }
        Ok((name, t))
    }
    // EXT(gcc): C name → emitted symbol; prefix \x01 = the name is complete, codegen adds no '_'
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
    // consume __attribute__((...)) / __asm__("..") — an extension, no semantic effect
    // consume __attribute__/__asm__; understand packed + aligned(n)
    // EXT(gcc): apply the mode(M) recorded in attr_mode — remap t to the type of width M,
    // preserving signedness/floatness (int mode(word)→long; unsigned int mode(QI)→uchar).
    // Width table from Part II (gcc machmode.def). No-op if there is no mode.
    fn apply_mode(&mut self, t: TypeId) -> Result<TypeId, String> {
        let (w, is_f) = match self.attr_mode.take() {
            Some(m) => m,
            None => return Ok(t),
        };
        Ok(if is_f || self.tt.is_float(t) {
            match w {
                4 => FLOAT,
                8 => DOUBLE,
                16 => LDOUBLE,
                _ => return Err(format!("mode float width {w} not supported")),
            }
        } else {
            let uns = self.tt.is_unsigned(t);
            match w {
                1 => if uns { UCHAR } else { CHAR },
                2 => if uns { USHORT } else { SHORT },
                4 => if uns { UINT } else { INT },
                8 => if uns { ULONG } else { LONG },
                _ => return Err(format!("mode int width {w} not supported")),
            }
        })
    }
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
                        // EXT(gcc): vector_size = a SIMD type (GNU vector). zcc does NOT
                        // implement it — silently swallowing the attribute would turn a vector
                        // typedef into a scalar and then miscompile arithmetic/initialization.
                        // 2-fact rule: reject CLEANLY.
                        "vector_size" | "__vector_size__" => {
                            return Err(
                                "__attribute__((vector_size)): SIMD vector type (GNU) not supported"
                                    .into(),
                            );
                        }
                        // EXT(gcc): scalar_storage_order reverses the byte order of each scalar
                        // in a struct. zcc does not implement it — silently swallowing it would give
                        // the wrong byte layout → miscompile. 2-fact rule: reject CLEANLY.
                        "scalar_storage_order" | "__scalar_storage_order__" => {
                            return Err(
                                "__attribute__((scalar_storage_order)): byte-order reversal (GNU) not supported"
                                    .into(),
                            );
                        }
                        // EXT(gcc): mode(M) forces the type width per a machine-mode (gcc
                        // machmode.def). glibc `register_t __mode__(word)` + the int8_t family.
                        // Silently swallowing it keeps the declared width → wrong sizeof (measured: mode(QI) 1→4).
                        // Width table from Part II; the remap preserves signedness in decl_specs.
                        "mode" | "__mode__" => {
                            self.expect(Tok::Punct("("))?;
                            let m = self.ident()?;
                            self.expect(Tok::Punct(")"))?;
                            self.attr_mode = Some(match m.trim_matches('_') {
                                "QI" | "byte" => (1, false),
                                "HI" => (2, false),
                                "SI" => (4, false),
                                "DI" | "word" | "pointer" | "Pmode" => (8, false),
                                "SF" => (4, true),
                                "DF" => (8, true),
                                "TF" => (16, true),
                                // TI(int128)/XF(x87-80b)/vector: no zcc type → reject CLEANLY
                                _ => return Err(format!(
                                    "__attribute__((mode({m}))): machine-mode not supported"
                                )),
                            });
                        }
                        // EXT(gcc): weak/alias — the skeleton of musl's weak_alias()
                        "weak" | "__weak__" => self.attr_weak = true,
                        // EXT(gcc): glibc enables this union under _GNU_SOURCE
                        // (__CONST_SOCKADDR_ARG of bind/connect/sendto…)
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
                                aligned = Some(16); // GCC: bare aligned = 16
                            }
                        }
                        _ => {
                            // unknown attribute (may carry (args)): consume balanced
                            if self.eat(&Tok::Punct("(")) {
                                let mut depth = 1u32;
                                while depth > 0 {
                                    match self.toks.get(self.pos) {
                                        Some(Tok::Punct("(")) => depth += 1,
                                        Some(Tok::Punct(")")) => depth -= 1,
                                        None => return Err("unterminated __attribute__".into()),
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
                // EXT(gcc): asm-label — a plain "(string)" is a replacement symbol for the
                // declarator (SDK versioning: __asm("_open")); anything else is discarded
                let mut label = String::new();
                while let Some(Tok::Str(b, _)) = self.toks.get(self.pos) {
                    label.push_str(&String::from_utf8_lossy(b)); // "_" "open" concatenated
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
                        None => return Err("unterminated __asm__".into()),
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
            // C99 6.7.5.3p21 — qualifier/static inside the [] of an array parameter
            // (SDK _regex.h: `__pmatch[ restrict n ]` when declared C99); no-op at -O0
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
                    // C99: inside a parameter list the size may refer to a prior parameter
                    // (not yet in scope) — skip balanced to ']', and let decay handle the rest
                    Err(_) if self.in_params > 0 => {
                        self.pos = save;
                        let mut depth = 0u32;
                        let mut side_effect = false;
                        loop {
                            match self.toks.get(self.pos) {
                                Some(Tok::Punct("[")) => depth += 1,
                                Some(Tok::Punct("]")) if depth == 0 => break,
                                Some(Tok::Punct("]")) => depth -= 1,
                                None => return Err("array parameter missing ']'".into()),
                                // C99 6.9.1: an array-parameter size expr WITH a side effect (`b[a++]`)
                                // should really be evaluated WHEN THE FUNCTION IS CALLED (prologue). zcc
                                // decays the parameter to a pointer and drops the size — if the size only
                                // reads (`__pmatch[n]`) the drop is HARMLESS (it is a pointer either way).
                                // But a dimension WITH ++/-- makes the drop LOSE a side effect → miscompile
                                // on call (970217-1/pr77767): at a function DEFINITION (top-level, !in_fn)
                                // store the token range and re-evaluate it in the prologue (C99 6.9.1). Inside
                                // a nested prototype (in_fn) the dimension is NOT evaluated (6.7.6.2p5)
                                // → dropping is correct. The parameter still decays to a pointer as usual.
                                Some(Tok::Punct("++")) | Some(Tok::Punct("--")) => {
                                    side_effect = true
                                }
                                _ => {}
                            }
                            self.pos += 1;
                        }
                        if side_effect && !self.in_fn {
                            self.param_vla_dims.push((save, self.pos)); // [save, ']')
                        }
                        0
                    }
                    // C99: a non-constant size = a VLA — keep the expr for stmt() to lower to
                    // alloca. The OUTER dimension (seen first) goes into vla_size, the INNER
                    // dimensions (recursive suffixes) accumulate into vla_inner in outer→inner order.
                    Err(_) => {
                        self.pos = save;
                        let e = self.expr()?;
                        if self.vla_size.is_none() {
                            self.vla_size = Some(e);
                        } else {
                            self.vla_inner.push(e);
                        }
                        0
                    }
                }
            };
            self.expect(Tok::Punct("]"))?;
            let inner = self.suffixes(t)?; // multidimensional: int a[2][3] = array 2 of array 3
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
            }); // () — no information
        }
        if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if n == "void")
            && self.toks.get(self.pos + 1) == Some(&Tok::Punct(")"))
        {
            self.pos += 2;
            return Ok(empty); // (void)
        }
        // old-style identifier list: f(a, b) — but __attribute__/nullability begins a
        // modern parameter (SDK mig_errors.h: f(__unused T *x)), not a name
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
            let (bt, _) = self.decl_specs()?.ok_or("parameter type required")?;
            let (nm, pt) = self.declarator(bt, false)?;
            self.vla_size = None; // C99: a VLA parameter decays to a pointer — drop the size
            self.vla_inner.clear(); // 2D parameter unsupported: drop (decays to a pointer)
            // adjust the parameter type: array → pointer, function → function pointer
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
        let (bt, _) = self.decl_specs()?.ok_or("type name required")?;
        let (_, t) = self.declarator(bt, false)?;
        Ok(t)
    }

    // ---- type conversion ----
    fn cast(&mut self, e: NodeId, to: TypeId) -> NodeId {
        if self.ty(e) == to {
            return e;
        }
        // C99: complex conversion (6.3.1.6/6.3.1.7) — desugar member-wise.
        // complex → real: take the real part (musl's creal IS exactly this cast);
        // real/complex → complex: temp + assign each part, imaginary part = 0.
        let (se, de) = (self.cplx_elem(self.ty(e)), self.cplx_elem(to));
        if let (Some(sel), None) = (se, de)
            && (self.tt.is_integer(to) || self.tt.is_float(to)) {
                let m = self.push(Node::Member(e, 0), sel);
                return self.cast(m, to);
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
                    // the source is also complex: bind it to a temp to avoid evaluating it twice
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
            Ty::UChar | Ty::UShort | Ty::Bool => INT, // all three fit in int (LP64)
            Ty::Float => DOUBLE,
            // Bitfield: promote to int if the range fits int, unsigned int if it fits uint,
            // wider than 32 bits follows the base (ANSI leaves this undefined; follow gcc)
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
            // C99 UAC: long double sits at the top of the real semilattice (the value is still
            // f64 in a register — a double↔ldbl cast is a no-op, only the type label changes for the ABI)
            if matches!(self.tt.tys[lt as usize], Ty::LDouble)
                || matches!(self.tt.tys[rt as usize], Ty::LDouble)
            {
                return LDOUBLE;
            }
            // float+float / float+int → FLOAT (the operand must be ROUNDED to f32 —
            // 16777217L != (float)16777217e0 is distinguishable); otherwise double.
            // Arithmetic runs in double (C89 permits excess precision).
            let fl = |t: TypeId| {
                matches!(self.tt.tys[t as usize], Ty::Double)
                    || !self.tt.is_integer(t) && !matches!(self.tt.tys[t as usize], Ty::Float)
            };
            return if fl(lt) || fl(rt) { DOUBLE } else { FLOAT };
        }
        if !self.tt.is_integer(lt) || !self.tt.is_integer(rt) {
            return ULONG; // pointers etc.: 64-bit unsigned comparison/arithmetic
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
    // A lvalue whose address is a pure constant offset from a local/global — no Deref, no
    // array-index, no side effect. `L` may then appear twice (load + store) WITHOUT an address
    // temp: re-lowering it is free and side-effect-free. This is also the SROA precondition —
    // a Member field-chain kept out of a pointer temp stays a bare Lea(Local) the pass can split.
    fn pure_lval_addr(&self, n: NodeId) -> bool {
        match self.nodes[n as usize] {
            Node::Var(_) | Node::GVar(_) => true,
            Node::Member(b, _) => self.pure_lval_addr(b),
            _ => false, // Deref (p->x), array-index (a[i].x): may have a side effect → keep temp
        }
    }
    // L op= R (and ++L/--L): L appears twice in the tree (load + store) — if the
    // address has a side effect (a[*s++] |= 1) it must be held in a temp and evaluated once:
    // (tmp = &L, *tmp = *tmp op R)
    fn opassign(&mut self, l: NodeId, bop: &'static str, r: NodeId) -> R {
        match self.nodes[l as usize] {
            _ if self.pure_lval_addr(l) => {
                // static / pure-offset address (local, global, or their struct fields): no temp
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
        // complex is Ty::Struct → scalar()=false, but an assignment `c = x` (scalar→complex)
        // MUST go through cast() to build a temp {re=x, im=0}; otherwise Assign treats an int
        // RHS as an ADDRESS and struct-copies 16B from there → segfault (960512-1). cast()
        // returns identity when the types match, so a same-type complex↔complex is still a struct-copy.
        let r = if self.scalar(lt) || self.cplx_elem(lt).is_some() {
            self.cast(r, lt)
        } else {
            r
        };
        Ok(self.push(Node::Assign(l, r), lt))
    }
    // condition: a float must be compared != 0.0 (cbz on the bit pattern would be wrong for -0.0)
    fn truthy(&mut self, e: NodeId) -> R {
        // C99 6.3.1.2: complex nonzero ⟺ re≠0 || im≠0. e is Ty::Struct, so cbz would look
        // at the struct address (always ≠0) → WRONG (960512-1). Bind a temp (e may have a
        // side effect: `if(c=f())`) then OR the two parts != 0.
        if let Some(el) = self.cplx_elem(self.ty(e)) {
            let ct = self.ty(e);
            let off = self.alloc_local(String::new(), ct);
            let tv = self.push(Node::Var(off), ct);
            let save = self.push(Node::Assign(tv, e), ct);
            let t1 = self.push(Node::Var(off), ct);
            let re = self.cplx_proj(t1, false)?;
            let z1 = self.push(Node::FNum(0.0), el);
            let rne = self.mkbin("!=", re, z1)?;
            let t2 = self.push(Node::Var(off), ct);
            let im = self.cplx_proj(t2, true)?;
            let z2 = self.push(Node::FNum(0.0), el);
            let ine = self.mkbin("!=", im, z2)?;
            let or = self.mkbin("|", rne, ine)?; // rne,ine ∈ {0,1} → bitwise = logical
            let ot = self.ty(or);
            return Ok(self.push(Node::Comma(save, or), ot));
        }
        if self.tt.is_float(self.ty(e)) {
            let z = self.push(Node::FNum(0.0), DOUBLE);
            self.mkbin("!=", e, z)
        } else {
            Ok(e)
        }
    }
    // pointer-step scale: a VLA pointee → load the byte size from a hidden local, otherwise a constant
    fn vla_scale(&mut self, e: TypeId) -> NodeId {
        match self.vla_arrs.get(&e) {
            Some(&off) => self.push(Node::Var(off), LONG),
            None => self.push(Node::Num(self.tt.size(e) as i64), LONG),
        }
    }
    // Build a binary-op node, inserting conversions + pointer scaling
    fn mkbin(&mut self, op: &'static str, l: NodeId, r: NodeId) -> R {
        // C99: complex is a struct — MUST be handled before the scalar branch, otherwise
        // codegen treats the struct address as its first 8-byte value → silent error
        if self.cplx_elem(self.ty(l)).is_some() || self.cplx_elem(self.ty(r)).is_some() {
            return self.cplx_bin(op, l, r);
        }
        let (lp, rp) = (self.tt.pointee(self.ty(l)), self.tt.pointee(self.ty(r)));
        match (op, lp, rp) {
            ("+", None, Some(_)) => self.mkbin("+", r, l), // int + ptr: commutative
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
                        return Err("shift on floating-point operand".into());
                    }
                    let l = self.cast(l, lt);
                    Ok(self.push(Node::Bin(op, l, r), lt))
                }
                _ => {
                    let ct = self.common_ty(self.ty(l), self.ty(r));
                    if self.tt.is_float(ct) && matches!(op, "%" | "&" | "|" | "^") {
                        return Err(format!("operator '{}' on floating-point operand", op));
                    }
                    let (l, r) = (self.cast(l, ct), self.cast(r, ct));
                    Ok(self.push(Node::Bin(op, l, r), ct))
                }
            },
        }
    }
    // C99: complex algebra lowered member-wise through temps (6.3.1.8 UAC + operations
    // on the field of complex numbers). Multiplication/division use the direct algebraic
    // formulas — NO Annex G NaN-fixup/scaling (equivalent to gcc -fcx-limited-range; a
    // declared deviation, and musl src/complex decomposes creal/cimag at the boundaries, so it is rarely hit).
    // read a complex constant from a const node: Bin("__ci",re,im) | real scalar → (re,im)
    fn ccval(&self, id: NodeId) -> Option<(f64, f64)> {
        match &self.nodes[id as usize] {
            Node::Bin("__ci", a, b) => Some((self.fold_f(*a).ok()?, self.fold_f(*b).ok()?)),
            _ => self.fold_f(id).ok().map(|v| (v, 0.0)),
        }
    }
    // an imaginary constant → a complex temp {re=0, im=v} (the embedding ℝ→ℂ).
    // !in_fn (static init): build the sentinel Bin("__ci",0,v) so ginit folds it to bytes.
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
    // π₁/π₂: __real__ z / __imag__ z — the projection ℂ→ℝ (Member re@0 / im@esz)
    fn cplx_proj(&mut self, e: NodeId, imag: bool) -> R {
        if let Some(el) = self.cplx_elem(self.ty(e)) {
            let off = if imag { self.tt.size(el) } else { 0 };
            return Ok(self.push(Node::Member(e, off), el));
        }
        // real scalar: __real__ x = x; __imag__ x = 0 (same type)
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
        // "is the element FLOAT (vs DOUBLE)" — via the underlying Ty so a `volatile` qualifier
        // on either the complex or a bare scalar float operand does not misroute to double.
        let elemf = |s: &Self, n| {
            let e = s.cplx_elem(s.ty(n)).unwrap_or(s.ty(n));
            matches!(s.tt.tys[e as usize], Ty::Float)
        };
        let lf = elemf(self, l);
        let rf = elemf(self, r);
        let elem = if lf && rf { FLOAT } else { DOUBLE };
        let ct = self.cplx_of(elem);
        let esz = self.tt.size(elem);
        // static init: fold the ℂ constant → sentinel Bin("__ci",re,im) (no runtime temp)
        if !self.in_fn {
            let ((a, b), (c, d)) = (
                self.ccval(l).ok_or("complex operator on non-constant operand")?,
                self.ccval(r).ok_or("complex operator on non-constant operand")?,
            );
            let (re, im) = match op {
                "+" => (a + c, b + d),
                "-" => (a - c, b - d),
                "*" => (a * c - b * d, a * d + b * c),
                "/" => {
                    let den = c * c + d * d;
                    ((a * c + b * d) / den, (b * c - a * d) / den)
                }
                _ => return Err(format!("operator '{op}' on complex constant")),
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
        // read the member fresh each use (a temp, so no side effect)
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
            _ => return Err(format!("operator '{}' on complex operand", op)),
        };
        let mut seq = self.cplx_pack(rre, rim, elem);
        if let Some(p) = p3 {
            seq = self.push(Node::Comma(p, seq), ct);
        }
        seq = self.push(Node::Comma(p2, seq), ct);
        Ok(self.push(Node::Comma(p1, seq), ct))
    }
    // pack (re,im) → a complex temp, returning Var(temp) (requires in_fn)
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
    // EXT(gcc): ~z on a complex = the conjugate (re, −im). Use Neg (FNEG flips the sign
    // bit), NOT 0−im: the conjugate of +0 is −0 (signed zero, matching cc bit-for-bit).
    fn cplx_conj(&mut self, e: NodeId) -> R {
        let el = self.cplx_elem(self.ty(e)).ok_or("~ on non-complex operand")?;
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
        // the object's extent, for `ast::Func::objs`
        self.cur_objs.push((self.cur_off, sz));
        self.cur_off
    }
    // allocate parameter slots + mirror the codegen spill algorithm (MUST match byte for byte):
    // stack args pack — scalar at natural alignment, composite at max(8,align) with size
    // rounded to 8, overflow locks gp=8 (C.11). The variadic anonymous-argument area starts
    // after the named args, rounded to 8. Returns (params, sret slot). Shared by top-level & nested funcdef.
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
                    fp = 8; // AAPCS: when an HFA overflows, lock the remaining v-registers
                    // over-alignment is ignored (see isel/abi.rs, torture pr92904)
                    let o = alup(boff, 8);
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
                        let o = alup(boff, 8);
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
                    // AAPCS64 C.13/C.14 + C.16: the NSAA is rounded to the larger
                    // of 8 and the natural alignment, and an argument narrower
                    // than 8 bytes still OCCUPIES 8 (verified against gcc: char,
                    // short, int, char at [sp,0], [sp,8], [sp,16], [sp,24]).
                    // Byte-identical with `isel/abi.rs` — edit both (Article E).
                    boff = alup(boff, if sz > 8 { 16 } else { 8 }) + sz.max(8);
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


    // does a DCE-eliminated branch contain a label (a goto from outside must not drop it)
    fn has_label(&self, id: NodeId) -> bool {
        match &self.nodes[id as usize] {
            // a Case is also a jump target (the switch table references LC{id})
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
            Node::Var(_) | Node::GVar(_) | Node::Deref(_) | Node::Member(..)
        ) {
            Ok(())
        } else {
            Err("lvalue required".into())
        }
    }

    // ---- statement ----
    fn stmt(&mut self) -> R {
        // EXT(gcc): __extension__ at the start of a statement is a no-op but is a type-word
        // → it must be stripped BEFORE classifying decl/expr, otherwise `__extension__ ({...});`
        // (git's obstack_blank) is mistaken for a declaration
        if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if n == "__extension__")
            && matches!(self.toks.get(self.pos + 1), Some(Tok::Punct("(")))
        {
            self.pos += 1;
        }
        if self.eat_kw("_Static_assert") {
            // EXT(c11): a declaration that is empty for codegen — return an empty block
            self.static_assert()?;
            return Ok(self.push(Node::Block(Vec::new()), INT));
        }
        if let (Some(Tok::Ident(n)), Some(Tok::Punct(":"))) =
            (self.toks.get(self.pos), self.toks.get(self.pos + 1))
        {
            // typedef-name + ":" is still a label (a declaration cannot begin with ":")
            if !["case", "default"].contains(&n.as_str()) {
                let n = n.clone();
                self.pos += 2;
                let st = self.stmt()?;
                return Ok(self.push(Node::Label(n, st), INT));
            }
        }
        if self.eat_kw("asm") || self.eat_kw("__asm__") || self.eat_kw("__asm") {
            // EXT(gcc): inline asm SUBSET (xxhash M14; extended for musl at M17): the template
            // is emitted verbatim; for constraints see AsmOp in ast.rs. Clobbers are ignored —
            // harmless at -O0 because every statement reloads from memory; only sp/x29/x30
            // are inviolable. NO [name], no asm goto — a clear error so incorrect code is
            // never silently produced.
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
                        // "&" (early-clobber) is harmless: the pool gives each operand its own register
                        match c.trim_start_matches(['=', '+', '&']) {
                            "r" => {}
                            "w" => op.fp = true,
                            "Q" | "m" => op.mem = true,
                            d if !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) => {
                                op.tied = Some(d.parse().map_err(|e| format!("{e}"))?)
                            }
                            _ => return Err(format!("asm: constraint \"{c}\" not supported")),
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
            // the pool is split GP/FP; pinned and tied operands do not consume the pool
            let pool = |fp: bool| {
                ops.iter()
                    .filter(|o| o.fp == fp && o.pin.is_none() && o.tied.is_none())
                    .count()
            };
            if pool(false) > 7 || pool(true) > 7 {
                return Err("asm: at most 7 operands per pool (x9-x15 / v16-v22)".into());
            }
            self.expect(Tok::Punct(")"))?;
            self.expect(Tok::Punct(";"))?;
            return Ok(self.push(Node::Asm(tpl, ops), INT));
        }
        if self.eat_kw("__label__") {
            // EXT(gcc): __label__ local label — no-op: a same-function label resolves through the
            // ordinary Goto; non-local goto (nested func) has been dropped, so nothing extra is tracked.
            loop {
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
            // constant condition → keep the correct branch (minimal DCE — clang -O0 does this
            // too, and torture link_error relies on it); skip if the dropped branch contains a label
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
            // C99 6.8.5.3: a for-init declaration is scoped to the WHOLE for (init+cond+
            // incr+body) and must be closed after the body, otherwise the shadow leaks out. Only
            // auto/register objects are allowed, so only locals are needed (no tag/typedef).
            let scope = self.locals.len();
            // C99 (allowed by clang -std=c89): "for (int i = 0; ...)" — the init is a declaration
            let i = if matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n))
            {
                Some(self.stmt()?) // a decl-stmt consumes its own ";"
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
                return Err("do requires while".into());
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
            // C89 6.6.4.2: a case constant is converted to the (promoted) type of the control expr
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
            // EXT(gcc): case lo ... hi — a label for EVERY value in [lo,hi]
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
                .ok_or("case outside switch")?
                .0
                .push((lo, hi, id));
            Ok(id)
        } else if self.eat_kw("default") {
            self.expect(Tok::Punct(":"))?;
            let st = self.stmt()?;
            let id = self.push(Node::Case(st), INT);
            self.switches.last_mut().ok_or("default outside switch")?.1 = Some(id);
            Ok(id)
        } else if self.eat_kw("break") {
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Break, INT))
        } else if self.eat_kw("continue") {
            self.expect(Tok::Punct(";"))?;
            Ok(self.push(Node::Continue, INT))
        } else if self.eat_kw("goto") {
            if self.eat(&Tok::Punct("*")) {
                // EXT(gcc): computed goto "goto *e"
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
            // tag/typedef/enum are also block-scoped (shadow, then restore)
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
            // local declaration (multiple declarators, with init); typedef/static/extern handled separately
            let tls = self.saw_thread;
            let mut stmts = Vec::new();
            if !self.eat(&Tok::Punct(";")) {
                loop {
                    let (name, mut t) = self.declarator(bt, true)?;
                    let vla = self.vla_size.take(); // C99
                    let vla_in = std::mem::take(&mut self.vla_inner); // inner dimensions (2D)
                    // EXT(gcc): an asm-label on a LOCAL = pin to a register (musl syscall
                    // `register long x8 __asm__("x8")`); remove it from renames because
                    // a local has no symbol to rename
                    let pin = self.renames.remove(&name).and_then(|l| {
                        l.strip_prefix('x')
                            .or_else(|| l.strip_prefix('w'))
                            .and_then(|d| d.parse::<u8>().ok())
                            .filter(|&r| r < 29)
                    });
                    // nested function (GNU) = vendor lock-in, DROPPED (clang/MSVC lack it,
                    // no app in the corpus needs it, and it requires an executable stack): reject cleanly.
                    if let Ty::Func(_) = self.tt.tys[t as usize]
                        && self.peek("{") {
                            return Err("nested function (GNU) not supported".into());
                        }
                    match storage {
                        Storage::Typedef => {
                            // C99 6.7.7 + 6.5.3.4p2: typedef of a variably-modified array
                            // (`typedef int c[i+2]`) — the size expr is evaluated ONCE at the
                            // declaration, fixing the byte count into a hidden local; sizeof(c)
                            // reads it back (not returning 0, which would be miscompile 20040411-1).
                            if let Some(w) = vla {
                                let Ty::Array(elem, 0) = self.tt.tys[t as usize] else {
                                    return Err("variably-modified typedef outside array: not supported".into());
                                };
                                let esz = self.tt.size(elem);
                                if esz == 0 {
                                    return Err("multidimensional variably-modified typedef: not supported".into());
                                }
                                let n = self.cast(w, ULONG);
                                let szn = self.push(Node::Num(esz as i64), ULONG);
                                let bytes = self.push(Node::Bin("*", n, szn), ULONG);
                                let hid = self.alloc_local(format!("{name}.vmtsz"), ULONG);
                                let hv = self.push(Node::Var(hid), ULONG);
                                stmts.push(self.mkassign(hv, bytes)?);
                                self.vm_typedef_sz.insert(t, hid);
                            }
                            self.typedefs.insert(name, t);
                        }
                        _ if matches!(self.tt.tys[t as usize], Ty::Func(_)) => {
                            self.fns.insert(name.clone(), t); // prototype inside a function
                            self.locals.push((name, t, Vloc::Fn)); // shadow a variable of the same name
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
                            // C99 6.2.2p4: a block-scope extern with a prior declaration of the
                            // same name (file-scope) → KEEP the existing linkage and point to
                            // THAT exact global. Only push a new extern when the name was never
                            // declared in the TU (otherwise: the `extern int b` of
                            // `static int b=20` would alias to the wrong global).
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
                        // C99: a local VLA → pointer + alloca(n*sizeof(elem)).
                        // Known consequence: sizeof(vla) = 8 (pointer size), which is off-spec
                        // but redis does not hit it; the epilogue mov sp,x29 reclaims it.
                        // C99 6.7.6.2: a 2D VLA `int M[d0][d1]` → pointer to a VLA ROW `int[d1]`
                        // + alloca(d0*d1*elem). Reuse the (*p)[w] mechanism verbatim: register
                        // the row-type in vla_arrs so mkbin scales the runtime row step (d1*elem);
                        // indexing M[i][j] then works via the Deref-Array decay (867-875 arm64_elf).
                        // ≥3 dimensions or an inner VLA-nested dimension → reject cleanly (unsupported).
                        _ if vla.is_some() && !vla_in.is_empty() => {
                            if vla_in.len() != 1 {
                                return Err("VLA with more than 2 dimensions: not supported".into());
                            }
                            let Ty::Array(row, 0) = self.tt.tys[t as usize] else {
                                return Err("multidimensional VLA outside array".into());
                            };
                            let Ty::Array(elem, 0) = self.tt.tys[row as usize] else {
                                return Err("multidimensional VLA: inner dimension is not an array".into());
                            };
                            let esz = self.tt.size(elem);
                            if esz == 0 {
                                return Err("VLA with more than 2 dimensions: not supported".into());
                            }
                            if self.peek("=") {
                                return Err("VLA with initializer".into());
                            }
                            // row step = d1 * sizeof(elem) (runtime) → hidden local
                            let d1 = self.cast(vla_in[0], ULONG);
                            let eszn = self.push(Node::Num(esz as i64), ULONG);
                            let rbytes = self.push(Node::Bin("*", d1, eszn), ULONG);
                            let hrow = self.alloc_local(format!("{name}.rowsz"), ULONG);
                            let hrv = self.push(Node::Var(hrow), ULONG);
                            stmts.push(self.mkassign(hrv, rbytes)?);
                            self.vla_arrs.insert(row, hrow);
                            // total bytes = d0 * row step → hidden local (sizeof + alloca)
                            let d0 = self.cast(vla.unwrap(), ULONG);
                            let hrv2 = self.push(Node::Var(hrow), ULONG);
                            let total = self.push(Node::Bin("*", d0, hrv2), ULONG);
                            let htot = self.alloc_local(format!("{name}.vlasz"), ULONG);
                            let htv = self.push(Node::Var(htot), ULONG);
                            stmts.push(self.mkassign(htv, total)?);
                            let pt = self.tt.ptr_to(row);
                            let off = self.alloc_local(name.clone(), pt);
                            self.vla_szs.insert(off, htot);
                            let htv2 = self.push(Node::Var(htot), ULONG);
                            let al = self.push(Node::Alloca(htv2), pt);
                            let v = self.push(Node::Var(off), pt);
                            stmts.push(self.mkassign(v, al)?);
                        }
                        _ if vla.is_some() => {
                            // pointer to a VLA `char (*p)[w]` (musl): NO allocation —
                            // only record the pointee byte size in a hidden local so
                            // mkbin can scale at runtime; scalar init as usual
                            if let Ty::Ptr(inner) = self.tt.tys[t as usize] {
                                let Ty::Array(elem, 0) = self.tt.tys[inner as usize] else {
                                    return Err(
                                        "non-constant size after pointer: only (*p)[w] supported".into()
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
                                    return Err("VLA with initializer".into());
                                }
                                let elem = match self.tt.tys[t as usize] {
                                    Ty::Array(e, _) => e,
                                    _ => return Err("non-constant size outside array".into()),
                                };
                                let pt = self.tt.ptr_to(elem);
                                let off = self.alloc_local(name.clone(), pt);
                                let n = self.cast(vla.unwrap(), ULONG);
                                let esz = self.tt.size(elem);
                                let sz = self.push(Node::Num(esz as i64), ULONG);
                                let bytes = self.push(Node::Bin("*", n, sz), ULONG);
                                // C99 6.5.3.4p2: sizeof(vla) = runtime — fix the byte count
                                // into a hidden local (the size expr is evaluated exactly once),
                                // and both sizeof and alloca read it back from there
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
                                // the name's scope begins AFTER the declarator — the init may
                                // reference itself (sizeof *p). Only an array [] whose size is
                                // not yet fixed must be flattened before allocating the slot.
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
                                // aggregate {..}/"..": zero-fill first (partial init)
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
    // The init tree is parsed first, the [] array size is fixed, and only then is it lowered
    // to assignments (local) or flattened to (offset, size, item) (global/static).
    fn parse_init(&mut self) -> Result<Init, String> {
        if self.eat(&Tok::Punct("{")) {
            let mut v = Vec::new();
            if !self.eat(&Tok::Punct("}")) {
                loop {
                    // designator chain: .a.j / [2].x / .m[1] — desugar the tail into
                    // a nested init: ".a.j = v" ≡ ".a = { .j = v }"
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
                    // old EXT(gcc): "a : 'A'" ≡ ".a = 'A'" (element head, not confused with ?:)
                    if steps.is_empty()
                        && let (Some(Tok::Ident(n)), Some(Tok::Punct(":"))) =
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
                w = w.max(w2.0); // string concatenation: the widest prefix wins (C11 6.4.5p5)
                self.pos += 1;
            }
            // only a bare string is Init::S; "abc" + 10 is an expression
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
    // ---- initializer emission (cursor model — C89-standard brace elision) ----
    // fill_obj: one COMPLETE initializer (braced/string/expr) into an object (t, base).
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
                        // wchar_t/char16_t ws[] = L".."/u"..": each codepoint → es
                        // bytes LE + an es-byte NUL (matching the array element width)
                        let cps = wchars(&b);
                        let ew = es as usize;
                        let mut wb = Vec::with_capacity((cps.len() + 1) * ew);
                        for c in cps {
                            wb.extend_from_slice(&c.to_le_bytes()[..ew]);
                        }
                        wb.extend(std::iter::repeat_n(0, ew));
                        if n > 0 && wb.len() as u64 > n * es as u64 {
                            wb.truncate((n * es as u64) as usize);
                        }
                        out.push((base, t, FlatItem::B(wb)));
                        return Ok(());
                    }
                    if es == 1 && w == 1 {
                        b.push(0);
                        if n > 0 && b.len() as u64 > n {
                            b.truncate(n as usize); // char s[3] = "abc" is valid C89
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
                    wb.extend(std::iter::repeat_n(0, ew - 1)); // .asciz supplies the trailing NUL
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
                // FLOATING complex (a static CONSTANT init, !in_fn) is a scalar leaf: gitem folds
                // (re,im) to bytes. A runtime local (in_fn) MUST keep the old path — assigning a
                // scalar into a 16-byte struct{re,im} would be wrong/crash; descend into .re.
                if !self.in_fn && self.cplx_elem(t).is_some_and(|el| self.tt.is_float(el)) {
                    out.push((base, t, FlatItem::E(e)));
                    return Ok(());
                }
                // a scalar "flows" into the first leaf when t is an aggregate and the expr has a different type
                let (mut t, mut base) = (t, base);
                while !self.scalar(t) && self.ty(e) != t {
                    match self.tt.tys[t as usize] {
                        Ty::Array(el, _) => t = el,
                        Ty::Struct(si) => {
                            let m = self.tt.structs[si as usize]
                                .members
                                .first()
                                .ok_or("empty struct initializer")?;
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
    // fill_list: drain the iterator (SHARED across levels = elision) into aggregate t.
    // Returns the number of array elements touched (to infer the size of T x[]).
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
                        // reposition the index BEFORE checking fullness; a designator at THIS level
                        // must be CLEARED after use — elision descent shares the iterator, and an
                        // inner level seeing it again would apply it wrongly (or loop forever with Mem)
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
                        // a designator repositions the cursor BEFORE checking fullness; if not found
                        // directly, point at the anonymous member containing it (descent finds it again)
                        let nm = nm.clone();
                        i = match members.iter().position(|(mn, ..)| *mn == nm) {
                            Some(k) => {
                                // the designator at THIS level is used: clear it, otherwise an inner level loops forever
                                it.peek_mut().unwrap().0 = Desig::No;
                                k
                            }
                            None => match members.iter().position(|(mn, mt2, _)| {
                                mn.is_empty()
                                    && matches!(self.tt.tys[*mt2 as usize], Ty::Struct(s2)
                                        if self.find_member(s2, &nm).is_some())
                            }) {
                                Some(k) => k,
                                None => break, // a member of an OUTER level → return the cursor to the caller
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
                let init = it.next().ok_or("empty initializer")?.1;
                self.fill_obj(t, base, init, out)?;
                Ok(1)
            }
        }
    }
    // One sub-object: if the next element is braced/string → consume it whole; a scalar expr
    // over an aggregate sub-object → elision: descend with the SAME iterator.
    fn fill_one(
        &mut self,
        t: TypeId,
        base: u32,
        it: &mut ItInit,
        out: &mut Vec<(u32, TypeId, FlatItem)>,
    ) -> Result<(), String> {
        // a string is "atomic" for array/char*; for a struct, elision descends
        let braced = match it.peek() {
            Some((_, Init::L(_))) => true,
            Some((_, Init::S(..))) => !matches!(self.tt.tys[t as usize], Ty::Struct(_)),
            _ => false,
        };
        // a struct-typed expr that MATCHES the member → whole init, no elision descent
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
    // parse init + flatten + fix the [] array size. Returns (flat, whether the init is {..}/"..").
    // C89 3.5.7: a string-literal init of a char/wchar array may be optionally wrapped in {}
    // (luac.c: static char Output[]={ "luac.out" }) — unwrap the braces to Init::S
    fn unbrace_str(&self, t: TypeId, init: Init) -> Init {
        if let Ty::Array(e, _) = self.tt.tys[t as usize] {
            let esz = self.tt.size(e);
            let hit = matches!(&init, Init::L(v) if v.len() == 1
                && matches!(&v[0], (Desig::No, Init::S(_, w)) if esz == 1 || (*w > 1 && esz == *w as u32)));
            if hit
                && let Init::L(v) = init {
                    return v.into_iter().next().unwrap().1;
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
                // wide L"..": the length = the number of decoded CODEPOINTS (6.4.5), not
                // raw bytes — "Ä" = 1 wchar (U+00C4), not 2 (bug wchar_t-1)
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
    // a constant item: number / float bits / symbol address / string
    // recognize (&&a - &&b) — including the ((a-b)/1) form from ptr-diff — return the symbol pair
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
    // a constant pointer value: symbol + byte offset (mkbin already scaled the ptr arithmetic to bytes)
    fn gaddr(&self, mut e: NodeId) -> Option<(String, i64)> {
        loop {
            match &self.nodes[e as usize] {
                Node::Cast(i) => e = *i,
                Node::FunAddr(n) => return Some((n.clone(), 0)),
                Node::Addr(x) => return self.glval(*x),
                // only array/function decay is a pointer value; a scalar must go through &
                Node::GVar(gi) => {
                    let g = &self.globals[*gi as usize];
                    return matches!(self.tt.tys[g.ty as usize], Ty::Array(..) | Ty::Func(_))
                        .then(|| (g.name.clone(), 0));
                }
                // decay an array-typed MEMBER: int *p = g.a.arr (C89 3.4 address
                // constant through a . -> [] path — required by chibicc initializer.c)
                Node::Member(..) if matches!(self.tt.tys[self.ty(e) as usize], Ty::Array(..)) => {
                    return self.glval(e);
                }
                // C99 6.6p9: a multidimensional array — `*(a+i)` of array type (int[9]) decays
                // to an address = the pointer value `a+i`; &a[i][j] nests Deref this way.
                Node::Deref(p) if matches!(self.tt.tys[self.ty(e) as usize], Ty::Array(..)) => {
                    e = *p
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
    // a constant lvalue path → symbol + byte offset
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
        // strip casts ONLY to recognize the address pattern (str/&g/decay); folding a number
        // must use the original node, otherwise the truncation of (unsigned int)-4 is lost (pr39240)
        let mut e = e0;
        while let Node::Cast(inner) = self.nodes[e as usize] {
            e = inner;
        }
        // "abc" + k / &"abc"[k] → an address into the middle of a string
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
                if let Node::Deref(x) = self.nodes[*inner as usize]
                    && let Some((i, k)) = stroff(self, x) {
                        return Ok(GInit::StrOff(i, k));
                    }
            }
            Node::LabelAddr(n) => {
                // a symbol matching codegen's label convention (no leading underscore)
                return Ok(GInit::Addr(format!("\x01lg_{}.{}", self.fname, n), 0));
            }
            // EXT(gcc): &&a - &&b, the difference of two labels (static jump table); a void*
            // ptr-diff is wrapped in "/1" by mkbin, so it must be stripped
            Node::Bin("/" | "-", ..) => {
                if let Some((a, b)) = self.label_diff(e) {
                    return Ok(GInit::Diff(a, b));
                }
            }
            _ => {}
        }
        // a general address constant: &g.m, &a[i], (arr+1)->m... → symbol + offset
        if let Some((s, k)) = self.gaddr(e) {
            return Ok(GInit::Addr(s, k));
        }
        // static init of a FLOATING complex: (re,im) → two adjacent float cells {re,im}
        if let Some(el) = self.cplx_elem(t).filter(|&el| self.tt.is_float(el)) {
            let (re, im) = self.ccval(e).ok_or("non-constant complex initializer")?;
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
                // in-memory long double on ELF = binary128: widen f64 → f128 (exact)
                return Ok(GInit::Bytes(f128_bytes(v).to_vec()));
            }
            let bits = if self.tt.size(t) == 4 {
                (v as f32).to_bits() as i64
            } else {
                v.to_bits() as i64
            };
            return Ok(GInit::Num(bits));
        }
        // an integer constant from a floating expression: (int)1.9 etc. — fold_f then truncate
        if (self.tt.is_float(self.ty(e0)) || self.tt.is_float(self.ty(e)))
            && let Ok(v) = self.fold_f(e) {
                // Rust cast saturates: unsigned must go through u64, otherwise 1.8e19 → i64::MAX
                let n = if self.tt.is_unsigned(t) {
                    v as u64 as i64
                } else {
                    v as i64
                };
                return Ok(GInit::Num(n));
            }
        Ok(GInit::Num(self.fold(e0)?))
    }
    // global/static init: return the GInit + fix the [] array size
    fn ginit(&mut self, t: &mut TypeId) -> Result<GInit, String> {
        if !self.eat(&Tok::Punct("=")) {
            return Ok(GInit::None);
        }
        let (flat, _) = self.flat_init(t)?;
        self.flat_to_ginit(flat)
    }
    fn flat_to_ginit(&mut self, flat: Vec<(u32, TypeId, FlatItem)>) -> Result<GInit, String> {
        let mut list: Vec<(u32, u32, GInit)> = Vec::new();
        let mut bfs: Vec<(u32, u32, u128)> = Vec::new(); // bitfield: (first byte, one-past-last byte, bit image)
        for (off, mt, item) in flat {
            match item {
                FlatItem::E(e) => {
                    if self.tt.size(mt) == 0 {
                        continue; // EXT(gcc): empty struct — no data
                    }
                    // an aggregate = an implicit compound literal (static GVar) → splice its init in here
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
                    // bitfield: emit ONLY the exact byte range the field occupies — Itanium lets
                    // an ordinary member slot into the container's free bytes, so emitting the whole
                    // container would overwrite/shift the neighbor (the gdata List does not backtrack)
                    if let Ty::Bitfield(_, boff, w) = self.tt.tys[mt as usize] {
                        let mask = !0u64 >> (64 - w); // w ≥ 1 (w=0 has no named field)
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
        // fields that overlap bytes (share a container) are OR-ed into one Bytes run
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
        // a designator may be out of order / overwrite: stable sort + keep the LAST version
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
            // C99 6.5.17: the right operand undergoes lvalue conversion → array decay
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
        // EXT(gcc): elvis "a ?: b" — the middle operand = the cond itself, NOT re-evaluated
        // (codegen recognizes tb==cond to preserve x0)
        if self.eat(&Tok::Punct(":")) {
            let e = self.cond_expr()?;
            let t = self.arr_decay(self.ty(c));
            return Ok(self.push(Node::Cond(c, c, e), t));
        }
        let t = self.expr()?;
        self.expect(Tok::Punct(":"))?;
        let e = self.cond_expr()?;
        // the two operands converge to a common type (scalar); struct/ptr keeps the left operand
        let (tt_, te) = (self.ty(t), self.ty(e));
        if self.scalar(tt_) && self.scalar(te) && self.tt.is_integer(tt_) | self.tt.is_float(tt_) {
            let ct = self.common_ty(tt_, te);
            let (t, e) = (self.cast(t, ct), self.cast(e, ct));
            Ok(self.push(Node::Cond(c, t, e), ct))
        } else {
            // C99 6.5.15: an array operand decays to a pointer — keeping the array type would
            // make a variadic argument spilled to the stack be store_narrow-ed to the array size
            // (git diff.c `? " " : ""` → strh 2 bytes → a garbage pointer, segv per layout)
            let rt = self.arr_decay(tt_);
            Ok(self.push(Node::Cond(c, t, e), rt))
        }
    }
    // lvalue conversion C99 6.3.2.1p3: array → pointer-to-element (value = address;
    // codegen for an array expr already returns the address, so only the type changes)
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
        // EXT(gcc): __real__/__imag__ z — the projection ℂ→ℝ (C99 uses creal/cimag)
        if self.eat_kw("__real__") || self.eat_kw("__real") {
            let e = self.unary()?;
            return self.cplx_proj(e, false);
        }
        if self.eat_kw("__imag__") || self.eat_kw("__imag") {
            let e = self.unary()?;
            return self.cplx_proj(e, true);
        }
        // cast: "(" typename ")"
        if self.peek("(")
            && let Some(Tok::Ident(n)) = self.toks.get(self.pos + 1)
                && (self.is_type_word(n) || n == "__attribute__") {
                    self.pos += 1;
                    let ty = self.typename()?;
                    self.expect(Tok::Punct(")"))?;
                    // compound literal (C99, accepted by clang under -std=c89): "(T){...}"
                    if self.peek("{") {
                        let mut t = ty;
                        if !self.in_fn {
                            // global scope: an implicit static object + constant init
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
                        // make it an lvalue: Deref(Comma(inits, &temp)) — assignable/&-able as in C99
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
        if self.eat(&Tok::Punct("-")) {
            let e = self.unary()?;
            // C99: -z on a complex → 0 - z (0.0 keeps elem so it does not promote to double)
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
                return self.cplx_conj(e); // ~z = conjugate
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
                Ty::Func(_) => self.ty(e), // *f on a function = itself
                _ => return Err("dereference of non-pointer".into()),
            };
            Ok(self.push(Node::Deref(e), t))
        } else if self.eat(&Tok::Punct("&&")) {
            // EXT(gcc): "&&label" — && in prefix position cannot be a logical-and
            let n = self.ident()?;
            let t = self.tt.ptr_to(VOID);
            Ok(self.push(Node::LabelAddr(n), t))
        } else if self.eat(&Tok::Punct("&")) {
            let e = self.unary()?;
            if matches!(self.nodes[e as usize], Node::FunAddr(_)) {
                return Ok(e); // &f = f for a function
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
                // EXT(gcc): __alignof__(func) = the alignment set by __attribute__((aligned))
                // on a FUNCTION (align-3 requires 256). A C99 function has no alignment; zcc
                // does not track it → the func decays to a pointer, align()=8 ≠ 256 → miscompile. Reject CLEANLY.
                let is_fn = matches!(self.nodes[e as usize], Node::FunAddr(_));
                let t = self.ty(e);
                self.nodes.truncate(save);
                self.types.truncate(save);
                if is_fn {
                    return Err(
                        "__alignof__ of function (GNU function-alignment) not supported".into(),
                    );
                }
                self.tt.align(t)
            };
            Ok(self.push(Node::Num(al as i64), ULONG))
        } else if self.eat_kw("sizeof") {
            // sizeof(typename) | sizeof unary
            let sz = if self.peek("(")
                && matches!(self.toks.get(self.pos + 1), Some(Tok::Ident(n)) if self.is_type_word(n))
            {
                self.pos += 1;
                let t = self.typename()?;
                self.expect(Tok::Punct(")"))?;
                // C99 6.5.3.4p2: sizeof(int[n]) with a non-constant n → runtime
                if let Some(w) = self.vla_size.take() {
                    let Ty::Array(elem, 0) = self.tt.tys[t as usize] else {
                        return Err("non-constant size outside array".into());
                    };
                    let n = self.cast(w, ULONG);
                    let esz = self.push(Node::Num(self.tt.size(elem) as i64), ULONG);
                    return Ok(self.push(Node::Bin("*", n, esz), ULONG));
                }
                // C99 6.7.7: sizeof(a VM-array typedef name) → read the fixed hidden local
                if let Some(&hid) = self.vm_typedef_sz.get(&t) {
                    return Ok(self.push(Node::Var(hid), ULONG));
                }
                self.tt.size64(t)
            } else {
                let e = self.unary()?; // the operand node becomes arena garbage, accepted
                // C99 6.5.3.4p2: a VLA operand → runtime sizeof, read the hidden local
                // .vlasz: `sizeof a` (a local VLA, already decayed to a pointer in the rep)
                // via vla_szs; `sizeof *p` (p = pointer to a VLA) via vla_arrs
                if let Node::Var(off) = self.nodes[e as usize]
                    && let Some(&hid) = self.vla_szs.get(&off) {
                        return Ok(self.push(Node::Var(hid), ULONG));
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
            // rare: desugar (x op= 1) op⁻¹ 1 — accept the theoretical rounding discrepancy
            let one = self.push(Node::FNum(1.0), DOUBLE);
            let v = self.mkbin(op, e, one)?;
            let a = self.mkassign(e, v)?;
            let one2 = self.push(Node::FNum(1.0), DOUBLE);
            return self.mkbin(if op == "+" { "-" } else { "+" }, a, one2);
        }
        let delta = self.tt.pointee(t).map_or(1, |p| self.tt.size(p) as i64);
        Ok(self.push(Node::Post(op, e, delta), t))
    }
    // finish a call: insert casts per the prototype + default promotions
    fn finish_call(&mut self, callee: NodeId, mut args: Vec<NodeId>) -> R {
        let sig = self
            .tt
            .fnsig(self.ty(callee))
            .map(|s| (s.ret, s.params.clone(), s.variadic, s.oldstyle));
        let (ret, params, variadic, oldstyle) = sig.ok_or("call of non-function/function-pointer")?;
        for (i, a) in args.iter_mut().enumerate() {
            if i < params.len() && !oldstyle {
                *a = self.cast(*a, params[i]);
            } else {
                // default argument promotions (+ array decay C99 6.5.2.2p6 —
                // without it the stack slot is cut to the array size, see arr_decay)
                let t = self.ty(*a);
                let pt = if matches!(self.tt.tys[t as usize], Ty::LDouble) {
                    t // C99 6.5.2.2p6: promotion only raises float→double, long double is UNCHANGED
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
        // struct >16B by value: the ABI passes it INDIRECTLY — copy into a temp, pass a pointer.
        // Exception: an HFA is passed by value (AAPCS B.4) — on ELF even anonymous (gcc pr92904
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
        // returning a struct: ≤16B lowers x0/x1 into a temp; >16B the callee writes the temp directly via x8
        if matches!(self.tt.tys[ret as usize], Ty::Struct(_)) {
            let sz = self.tt.size(ret);
            // ≤16B: pad to 16 bytes so a codegen 8-byte str does not overwrite another slot
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
    // the postfix loop is separate so a compound literal can also chain: (int[]){..}[i]
    fn postfix_ops(&mut self, mut e: NodeId) -> R {
        loop {
            if self.eat(&Tok::Punct("[")) {
                let i = self.expr()?;
                self.expect(Tok::Punct("]"))?;
                let sum = self.mkbin("+", e, i)?;
                let t = self
                    .tt
                    .pointee(self.ty(sum))
                    .ok_or("subscript of non-array/pointer")?;
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
                    .ok_or("-> on non-pointer")?;
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
            return Err("member access on non-struct/union".into());
        };
        let (mt, off) = self
            .find_member(sd, &name)
            .ok_or(format!("no such member: {}", name))?;
        Ok(self.push(Node::Member(base, off), mt))
    }
    // packed/aligned attribute AFTER the body: recompute the layout in place
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
        // packed: alignment drops to 1 BUT keeps any prior explicit aligned
        // (sd.align exceeds the members' natural alignment ⟺ there was a preceding aligned(n))
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
    // find a member by name, through anonymous struct/union (empty name)
    fn find_member(&self, sd: u32, name: &str) -> Option<(TypeId, u32)> {
        for (n, t, o) in &self.tt.structs[sd as usize].members {
            if n == name {
                return Some((*t, *o));
            }
            if n.is_empty()
                && let Ty::Struct(si2) = self.tt.tys[*t as usize]
                    && let Some((mt, mo)) = self.find_member(si2, name) {
                        return Some((mt, o + mo));
                    }
        }
        None
    }
    fn primary(&mut self) -> R {
        // EXT(gcc): __extension__ before an expr = a no-op that suppresses pedantic warnings
        // (git's obstack.h: __extension__ ({ ... }))
        while self.eat_kw("__extension__") {}
        if self.eat(&Tok::Punct("(")) {
            // EXT(gcc): statement expression ({ ...; expr; }) — value = the last statement
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
                2 => LDOUBLE, // suffix L (ELF: binary128 at the ABI boundary)
                _ => DOUBLE,
            };
            Ok(self.push(Node::FNum(v), t))
        } else if let Some(&Tok::INum(v, k)) = self.toks.get(self.pos) {
            // C99 6.4.4.2: imaginary constant `v i` = 0 + v·i (the embedding ℝ→ℂ)
            self.pos += 1;
            let elem = if k == 0 { FLOAT } else { DOUBLE };
            let v = if k == 0 { v as f32 as f64 } else { v };
            Ok(self.cplx_imag(v, elem))
        } else if let Some(Tok::Str(bytes, w)) = self.toks.get(self.pos) {
            let (mut bytes, mut wid, mut cps) = (bytes.clone(), w.0, w.1.clone());
            self.pos += 1;
            while let Some(Tok::Str(more, w2)) = self.toks.get(self.pos) {
                bytes.extend_from_slice(more); // phase 6: concatenate adjacent strings
                wid = wid.max(w2.0); // the widest prefix wins (C11 6.4.5p5)
                cps.extend_from_slice(&w2.1);
                self.pos += 1;
            }
            if wid > 1 {
                // wide/char16: each codepoint → wid bytes little-endian; .asciz adds
                // 1 NUL, so pad wid-1 more — for a full wid-byte terminator. u""→USHORT(2),
                // L""/U""→INT(4). Use cps (source characters already separated from escapes).
                let (n, ew) = (cps.len() as u32, wid as usize);
                let mut wb = Vec::with_capacity((cps.len() + 1) * ew - 1);
                for c in &cps {
                    wb.extend_from_slice(&c.to_le_bytes()[..ew]);
                }
                wb.extend(std::iter::repeat_n(0, ew - 1));
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
            // outside a function body, locals are leftovers from a PRIOR function (not cleared,
            // for cheapness) — do not look them up, otherwise a global ginit &x mistakes a
            // parameter of the same name (git rm.c: the index_only parameter of check_local_mod vs the global index_only)
            if let Some(idx) = self.locals.iter().rposition(|(l, ..)| self.in_fn && *l == n) {
                let (t, loc) = (self.locals[idx].1, self.locals[idx].2);
                match loc {
                    Vloc::Stack(off) => return Ok(self.push(Node::Var(off), t)),
                    Vloc::Glob(gi) => return Ok(self.push(Node::GVar(gi), t)),
                    Vloc::Fn => {} // fall through to the self.fns lookup below
                }
            }
            if n == "__va_area__" {
                let t = self.tt.ptr_to(CHAR);
                return Ok(self.push(Node::VaArea(self.va_off), t));
            }
            // ELF: the AAPCS branch of stdarg.h lowers to 2 real builtins (Darwin: a macro hides the name)
            if n == "__builtin_va_start" {
                self.expect(Tok::Punct("("))?;
                let ap = self.assign()?;
                self.expect(Tok::Punct(","))?;
                let mark = self.nodes.len();
                let _ = self.assign()?; // last: only the name is needed, not evaluated
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
                // struct: allocate scratch — the backend needs a contiguous area when gathering an HFA
                let tmp = if matches!(self.tt.tys[ty as usize], Ty::Struct(_)) {
                    self.alloc_local(format!(".vaarg{}", self.nodes.len()), ty)
                } else {
                    0
                };
                return Ok(self.push(Node::VaArg(ap, ty, tmp), ty));
            }
            // __func__ = C99 6.4.2.2; EXT(gcc): __FUNCTION__/__PRETTY_FUNCTION__ aliases
            if n == "__func__" || n == "__FUNCTION__" || n == "__PRETTY_FUNCTION__" {
                let bytes = self.fname.clone().into_bytes();
                let ln = bytes.len() as u32;
                self.strs.push(bytes);
                let i = (self.strs.len() - 1) as u32;
                let t = self.tt.add(Ty::Array(CHAR, ln as u64 + 1));
                return Ok(self.push(Node::Str(i), t));
            }
            if n == "__builtin_types_compatible_p" {
                // EXT(gcc): fold to a 0/1 constant comparing types structurally — git ARRAY_SIZE
                // (BUILD_ASSERT_OR_ZERO: sizeof(char[1-2*!(cond)])) needs it
                // in a constant expression; array ≠ pointer is the decisive case
                self.expect(Tok::Punct("("))?;
                let a = self.typename()?;
                self.expect(Tok::Punct(","))?;
                let b = self.typename()?;
                self.expect(Tok::Punct(")"))?;
                let v = self.ty_compat(a, b) as i64;
                return Ok(self.push(Node::Num(v), INT));
            }
            if n == "__builtin_classify_type" {
                // EXT(gcc): the constant class of the argument's type (not evaluated), coded per
                // gcc/typeclass.h: void=0 int=1 ptr=5 real=8 struct=12 union=13 — enough for torture
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
                // member-designator: ident ("." ident | "[" constant "]")*
                let (mut t, mut off) = (ty, 0i64);
                loop {
                    let name = self.ident()?;
                    let Ty::Struct(si) = self.tt.tys[t as usize] else {
                        return Err("offsetof on non-struct".into());
                    };
                    let (mt, mo) = self
                        .find_member(si, &name)
                        .ok_or_else(|| format!("offsetof: no such member {name}"))?;
                    t = mt;
                    off += mo as i64;
                    loop {
                        if self.eat(&Tok::Punct("[")) {
                            let i = self.const_expr()?;
                            self.expect(Tok::Punct("]"))?;
                            let e = self.tt.pointee(t).ok_or("offsetof: subscript on non-array")?;
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
            // EXT(gcc): __builtin_{add,sub,mul}_overflow(a, b, &res) — lowered to
            // Node::Overflow; codegen emits a 128-bit sequence (see ext::overflow_emit).
            // Do NOT cast the operands: keep the original type (signedness/width) per GCC semantics.
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
            // EXT(gcc): atomics __sync_* (M12) — not a libc symbol, lowered directly to
            // Node::Sync so codegen emits ldaxr/stlxr; the name table is in ext.rs
            if let Some((op, arity)) = crate::ext::sync_op(&n)
                && self.peek("(") {
                    self.pos += 1;
                    let mut args = Vec::new();
                    for k in 0..arity {
                        if k > 0 {
                            self.expect(Tok::Punct(","))?;
                        }
                        args.push(self.assign()?);
                    }
                    self.expect(Tok::Punct(")"))?;
                    // Barrier has no ptr; otherwise the type + size = the pointee of the first argument
                    let (et, sz) = if arity == 0 {
                        (VOID, 0)
                    } else {
                        let et = self
                            .tt
                            .pointee(self.ty(args[0]))
                            .ok_or_else(|| format!("{n}: first argument must be a pointer"))?;
                        let ok = self.tt.is_integer(et)
                            || matches!(self.tt.tys[et as usize], Ty::Ptr(_));
                        let sz = self.tt.size(et);
                        if !ok || (sz != 4 && sz != 8) {
                            return Err(format!(
                                "{n}: only 4/8-byte integer/pointer operands supported"
                            ));
                        }
                        (et, sz)
                    };
                    for k in 1..args.len() {
                        args[k] = self.cast(args[k], et); // value argument to the operand width
                    }
                    let ret = match op {
                        SyncOp::BoolCas => INT,
                        SyncOp::Release | SyncOp::Barrier => VOID,
                        _ => et,
                    };
                    return Ok(self.push(Node::Sync(op, args, sz), ret));
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
                let n = self.funref(n); // EXT(gcc): asm-label rename
                return Ok(self.push(Node::FunAddr(n), pt));
            }
            if self.peek("(") {
                // __builtin_abort… → abort: GCC lowers a builtin to a libc symbol. But
                // ONLY when <f> really is a library function (whitelist ext::builtin_is_libc).
                // A pure builtin intrinsic (clrsb/parity/frame_address/apply/
                // va_arg_pack/mul_overflow_p…) has NO symbol → stripping would emit a garbage
                // call → as/ld chokes. 2-fact rule: reject CLEANLY instead of swallow-then-emit.
                let n = if let Some(f) = n.strip_prefix("__builtin_") {
                    if f == "alloca" || crate::ext::builtin_is_libc(f) {
                        f.to_string()
                    } else {
                        return Err(format!("__builtin_{f}: builtin not supported"));
                    }
                } else {
                    n
                };
                if n == "alloca" {
                    // no libc symbol; sub sp directly (the epilogue mov sp,x29 reclaims it)
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
                // call to an undeclared function: implicit int, old-style
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
            Err(format!("undeclared identifier: {}", n))
        } else {
            Err(format!("expected expression, found {:?}", self.toks.get(self.pos)))
        }
    }
    fn program(&mut self) -> Result<Vec<Func>, String> {
        let mut funcs = Vec::new();
        let mut ranges: Vec<(u32, u32)> = Vec::new(); // body [n0,n1) per func
        while self.pos < self.toks.len() {
            // EXT(gcc): global-level __asm__("...") → emitted verbatim (musl
            // crt_arch.h defines _start; MUST be caught before decl_specs because
            // skip_attrs would mistakenly consume it as an asm-label)
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
            // capture NOW: declarator/param/body will call decl_specs and reset the flags
            let inline_fn = self.saw_inline;
            let tls = self.saw_thread;
            let weak_fn = self.attr_weak; // EXT(gcc): weak before the declarator
            if self.eat(&Tok::Punct(";")) {
                continue; // a bare struct/union/enum definition
            }
            let (name, t) = self.declarator(bt, true)?;
            if self.vla_size.take().is_some() {
                self.vla_inner.clear();
                return Err("a VLA may only be a local variable".into()); // C99
            }
            self.vla_inner.clear();
            // C99 6.9.1: a side-effecting param-VLA dimension is evaluated on entry; drain the
            // captured ranges (meaningful only for a funcdef — dropped for a prototype).
            let pdims = std::mem::take(&mut self.param_vla_dims);
            // funcdef: the declarator yields a Func type and is followed by "{" or an old-style decl list
            if let Ty::Func(fidx) = self.tt.tys[t as usize] {
                let is_def = self.peek("{")
                    || matches!(self.toks.get(self.pos), Some(Tok::Ident(n)) if self.is_type_word(n) && n != "typedef");
                if is_def {
                    self.fns.insert(name.clone(), t);
                    let sig = self.tt.fns[fidx as usize].clone();
                    self.locals.clear();
                    self.reg_pins.clear(); // key = stack offset — a new function reusing an offset would inherit a phantom pin
                    self.vla_szs.clear(); // same reason: key is a stack offset
                    self.vm_typedef_sz.clear(); // key is a TypeId but the value = a stack offset
                    self.cur_off = 0;
                    self.cur_objs.clear();
                    self.fret = sig.ret;
                    self.fname = name.clone();
                    // old-style: parse the decl list assigning a type to each parameter name
                    let mut ptypes: HashMap<String, TypeId> = HashMap::new();
                    if sig.oldstyle {
                        while !self.peek("{") {
                            let (dbt, _) = self.decl_specs()?.ok_or("old-style parameter type required")?;
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
                    let n0 = self.nodes.len() as u32; // node range of the body (for weak DCE)
                    // C99 6.9.1: evaluate the side-effecting param-VLA dimensions in the prologue
                    // (parameters are now in scope). Re-parse the token range, wrapped as the first body statement.
                    let mut prologue = Vec::new();
                    if !pdims.is_empty() {
                        let bodypos = self.pos;
                        for &(s, e) in &pdims {
                            self.pos = s;
                            prologue.push(self.expr()?);
                            debug_assert!(self.pos == e);
                        }
                        self.pos = bodypos;
                    }
                    let body = if prologue.is_empty() {
                        self.stmt()?
                    } else {
                        let b = self.stmt()?;
                        prologue.push(b);
                        self.push(Node::Block(prologue), INT)
                    };
                    let n1 = self.nodes.len() as u32;
                    self.in_fn = false;
                    let is_static = storage == Storage::Static || self.static_fns.contains(&name);
                    // EXT(gcc): inline (including static inline) with no bare declaration
                    // (C99 6.7.4p7) → a DCE candidate; codegen emits weak if non-static so that
                    // copies from multiple TUs can coalesce
                    let is_inline = inline_fn && !self.plain_decls.contains(&name);
                    // C99 6.7.3 — TYPE-accurate: does any lvalue in this function have a
                    // volatile-qualified type? Every volatile access lowers from a node whose
                    // type carries the bit (or from a volatile parameter slot), so scanning the
                    // body's node range [n0,n1) + the param types catches the access wherever the
                    // qualifier originated — a local decl, a pointee, a member, or a file-scope
                    // typedef/object used here (which token-span detection could not see). If any
                    // is volatile the function keeps the -O0 path (opt is proven only volatile-free).
                    let has_volatile = params.iter().any(|&(_, pt)| self.tt.is_volatile(pt))
                        || (n0..n1).any(|i| self.tt.is_volatile(self.types[i as usize]));
                    funcs.push(Func {
                        name,
                        params,
                        frame: (self.cur_off + 15) & !15,
                        objs: std::mem::take(&mut self.cur_objs),
                        body,
                        ret: sig.ret,
                        is_static,
                        is_inline,
                        is_weak: weak_fn || self.attr_weak,
                        variadic: sig.variadic,
                        sret,
                        has_vla: !self.vla_szs.is_empty(),
                        has_volatile,
                    });
                    ranges.push((n0, n1));
                    continue;
                }
            }
            // not a funcdef: a declarator chain "a, *b, c[2];" — the first is already parsed
            let mut cur = (name, t);
            loop {
                let (name, mut t) = cur;
                if storage == Storage::Typedef {
                    self.typedefs.insert(name, t);
                    self.eat(&Tok::Punct("=")); // a typedef has no init; defensive
                } else if matches!(self.tt.tys[t as usize], Ty::Func(_)) {
                    if storage == Storage::Static {
                        self.static_fns.insert(name.clone());
                    }
                    if !inline_fn {
                        self.plain_decls.insert(name.clone()); // C99 6.7.4p7
                    }
                    // EXT(gcc): musl weak_alias — a function declaration carrying alias("old")
                    if let Some(old) = self.attr_alias.take() {
                        self.aliases
                            .push((name.clone(), old, weak_fn || self.attr_weak));
                    } else if weak_fn || self.attr_weak {
                        self.weak_decls.push(name.clone()); // bare weak prototype
                    }
                    self.fns.insert(name, t); // prototype
                } else {
                    // C89 6.1.2.1: the name enters scope from the END of the declarator, before the init
                    // → push it first so the initializer can reference itself
                    // (git LIST_HEAD: static struct x = { &x, &x })
                    // tentative definition: int x; int x = 3; int x; → ONE symbol
                    // EXT(gcc): alias on an object (musl weak_alias data) — emit only .set,
                    // no storage; still register the extern so this TU can reference the new name
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
                            is_extern: true, // provisional; finalized once init presence is known
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
                        g.ty = t; // int a[]; → int a[3]; completes the type
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
        // EXT(gcc): DCE for weak (inline) functions nobody reaches — like clang not
        // emitting an unused inline. Required: an inline body in a header (redis server.h)
        // references a symbol that another TU (redis-cli) does not link.
        // Roots = all references OUTSIDE the bodies of weak functions (ordinary functions,
        // global inits); propagate through the call graph among the weak functions.
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
                if !in_weak[k]
                    && let Some(&i) = refname(n).and_then(|nm| weak_idx.get(nm))
                        && !used[i] {
                            used[i] = true;
                            queue.push(i);
                        }
            }
            while let Some(i) = queue.pop() {
                for k in ranges[i].0..ranges[i].1 {
                    if let Some(&j) =
                        refname(&self.nodes[k as usize]).and_then(|nm| weak_idx.get(nm))
                        && !used[j] {
                            used[j] = true;
                            queue.push(j);
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

// borrow workaround: decl_specs holds a &str from a token — copy it out of the lifetime
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
        cur_objs: Vec::new(),
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
        vla_inner: Vec::new(),
        vm_typedef_sz: HashMap::new(),
        param_vla_dims: Vec::new(),
        in_params: 0,
        reg_pins: HashMap::new(),
        vla_arrs: HashMap::new(),
        vla_szs: HashMap::new(),
        cplx_tys: HashMap::new(),
        asm_label: None,
        renames: HashMap::new(),
        fname: String::new(),
        attr_weak: false,
        attr_mode: None,
        attr_transp: false,
        attr_alias: None,
        raw_asm: Vec::new(),
        aliases: Vec::new(),
        weak_decls: Vec::new(),
    };
    // EXT(gcc): __uint128_t/__int128_t — ONLY 16-byte storage, align 16 (SDK mach
    // NEON state in mcontext needs the correct layout); arithmetic is not supported.
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
    // ELF: seed struct __zcc_va_list as COMPLETE — the external header (musl stdarg.h)
    // only sees `__builtin_va_list` = this tag name, with no body; without the seed a
    // va_list local has size 0 → va_start writes 32 bytes over the neighboring slot (musl
    // vfprintf corrupts the fmt pointer). A redefinition embedded in stdarg.h merely shadows, harmlessly.
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
    // libc functions returning a pointer: calling them without a prototype (common in torture)
    // and leaving them implicit int makes sxtw cut off the high half of a heap address → seed the correct return type.
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
        // libm functions returning double (implicit int would read x0 instead of d0)
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
        // the variadic printf family: anonymous arguments must go ON THE STACK (Apple) — oldstyle
        // (all "named") would place them in registers and libc would read garbage
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
        // error position = the parser's current token → file:line from preprocessing
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

// L"..": the source is UTF-8 → decode to code points for wchar_t (a stray escape byte
// >127 that is not valid UTF-8 becomes U+FFFD — accepted)
fn wchars(b: &[u8]) -> Vec<u32> {
    String::from_utf8_lossy(b)
        .chars()
        .map(|c| c as u32)
        .collect()
}

// f64 → binary128 little-endian (EXACT widening, no rounding): the sign is preserved,
// the exponent is rebiased 1023→16383, the 52-bit mantissa is packed at the top of the 112-bit
// field; a subnormal double is renormalized (the f128 range easily contains it), inf/nan keep their form.
fn f128_bytes(v: f64) -> [u8; 16] {
    let b = v.to_bits();
    let (sg, e, m) = (b >> 63, (b >> 52) & 0x7ff, b & ((1u64 << 52) - 1));
    let (e2, m2): (u128, u128) = match e {
        0 if m == 0 => (0, 0),
        0 => {
            let sh = m.leading_zeros() - 11; // move the leading bit to position 52 (hidden)
            (
                16383 - 1022 - sh as u128,
                ((((m as u128) << sh) & ((1 << 52) - 1))) << 60,
            )
        }
        0x7ff => (0x7fff, (m as u128) << 60),
        _ => (e as u128 - 1023 + 16383, (m as u128) << 60),
    };
    (((sg as u128) << 127) | (e2 << 112) | m2).to_le_bytes()
}
