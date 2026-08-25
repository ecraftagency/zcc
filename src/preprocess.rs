// C89 preprocessor (M7): token-based, running between the lexer and the parser.
// THEORY A2 — preprocessing; THEORY II-1 — ISO C99 §6.10
// In: a file path; out: a Vec<Tok> cleared of directives, ready for the parser.
// Supported constructs: #include "..." (relative to the directory of the file
// being processed), #define object/function (# stringization, ## paste,
// recursive rescan with cycle prevention via a hide-stack), #undef,
// #if/#ifdef/#ifndef/#elif/#else/#endif (constant expressions with the full C
// operator set + defined), #error; #line/#pragma are consumed silently.
// Predefined macros: __LINE__ __FILE__ __STDC__ + the GCC builtin table.
// #include <...>: consult the embedded-header table first, then the -I
// directories (filesystem). Each token carries (file, line, hideset) — the
// hideset follows the standard rule so a macro does not expand repeatedly.

use crate::lexer::{NumK, PTok, Tok, lex, lex_t};
use std::collections::HashMap;
use std::fs;

#[derive(Clone)]
enum Macro {
    Obj(Vec<PTok>),
    Fun(Vec<String>, bool, Vec<PTok>), // bool = variadic (... → __VA_ARGS__)
}
type Macros = HashMap<String, Macro>;

/// THEORY II-1 — the ISO C99 standard headers
// System headers EMBEDDED into the binary (zero dependency; the target is
// locked to arm64 ELF, so the contents are fixed). #include <...> consults this
// table, it does not read the filesystem. Only 7 COMPILER-OWNED freestanding
// headers (C99 4): the compiler must supply these itself because they declare
// its own builtins (va_list, offsetof, INT_MAX...). Library headers
// (stdio/stdlib/string/math/...) are served on ELF by the box's glibc or a musl
// sysroot — the old embedded Darwin versions were removed when Mach-O was
// dropped (embed_src blocks them via the \x01elf sentinel).
const HEADERS: [(&str, &str); 7] = [
    ("stdbool.h", include_str!("headers/stdbool.h")),
    ("stdalign.h", include_str!("headers/stdalign.h")),
    ("stdnoreturn.h", include_str!("headers/stdnoreturn.h")),
    ("float.h", include_str!("headers/float.h")),
    ("stdarg.h", include_str!("headers/stdarg.h")),
    ("stddef.h", include_str!("headers/stddef.h")),
    ("limits.h", include_str!("headers/limits.h")),
];

pub fn preprocess(
    path: &str,
    defs: &[String],
    undefs: &[String],
    incs: &[String],
) -> Result<(Vec<Tok>, Vec<(u32, u32)>, Vec<String>), String> {
    let (out, files) = preprocess_pt(path, defs, undefs, incs)?;
    let locs = out.iter().map(|t| (t.file, t.line)).collect();
    Ok((out.into_iter().map(|t| t.tok).collect(), locs, files))
}

fn preprocess_pt(
    path: &str,
    defs: &[String],
    undefs: &[String],
    incs: &[String],
) -> Result<(Vec<PTok>, Vec<String>), String> {
    let mut macros = Macros::new();
    // EXT(gcc): predefined-macro table (target arm64 ELF Linux, LP64 little-endian) —
    // __LP64__/__aarch64__/__CHAR_BIT__/__SIZEOF_*/__*_TYPE__/__builtin_* are gcc-predefined,
    // NOT ISO; only __STDC__/__STDC_VERSION__ in the table are ISO (6.10.8). By the cut test:
    // remove this table → plain C89 remains (no translation unit sees a gcc macro → standard fallback path).
    let ones = [
        "__STDC__", "__LP64__", "__arm64__", "__aarch64__",
        "__linux__",
        "__linux", // gcc/clang define the bare form too; redis setproctitle.c + config.h probe `defined __linux`
        "__gnu_linux__",
        "__unix__",
        "__unix", // symmetric: gcc defines both __unix and __unix__
        "__ELF__",
        "__CHAR_UNSIGNED__", // plain char is unsigned on Linux arm64 (AAPCS)
    ];
    for m in ones {
        macros.insert(m.into(), Macro::Obj(vec![synth(Tok::Num(1, NumK::I))]));
    }
    for (m, v) in [
        // EXT(gcc): the Darwin SDK writes the arm64 branch ONLY under #ifdef
        // __GNUC__ (the non-GNUC branch is x86-only and even lacks _OSSwapInt16),
        // so the GNUC dialect must be claimed, as clang also claims it (4.2.1).
        // __clang__ is NOT claimed (avoiding clang-specific features).
        ("__GNUC__", "4"),
        ("__GNUC_MINOR__", "2"),
        ("__GNUC_PATCHLEVEL__", "1"),
        // C99: git-compat-util.h issues #error when < 199901L — claim the C99
        // dialect, as GNUC was claimed for the SDK (M11); the current C89+ surface
        // covers the C99 features git relies on (designated/mixed-decl/long
        // long/variadic macros), patched further as gaps appear
        ("__STDC_VERSION__", "199901L"),
        ("__CHAR_BIT__", "8"),
        ("__SCHAR_MAX__", "127"),
        ("__SHRT_MAX__", "32767"),
        ("__INT_MAX__", "2147483647"),
        ("__LONG_MAX__", "9223372036854775807L"),
        ("__LONG_LONG_MAX__", "9223372036854775807L"),
        ("__SIZEOF_SHORT__", "2"),
        ("__SIZEOF_INT__", "4"),
        ("__SIZEOF_LONG__", "8"),
        ("__SIZEOF_LONG_LONG__", "8"),
        ("__SIZEOF_POINTER__", "8"),
        ("__SIZEOF_SIZE_T__", "8"),
        ("__SIZEOF_PTRDIFF_T__", "8"),
        ("__SIZEOF_DOUBLE__", "8"),
        ("__SIZEOF_FLOAT__", "4"),
        ("__ORDER_LITTLE_ENDIAN__", "1234"),
        ("__ORDER_BIG_ENDIAN__", "4321"),
        ("__ORDER_PDP_ENDIAN__", "3412"),
        ("__BYTE_ORDER__", "1234"),
        // GCC-predefined float.h constants (IEEE754): FLT_MIN=2^-126, FLT_MAX=(2-2^-23)·2^127,
        // DBL_MANT_DIG=53 mantissa bits. Only the 3 macros the torture corpus actually requires (test-first).
        ("__FLT_MIN__", "1.17549435082228750797e-38F"),
        ("__FLT_MAX__", "3.40282346638528859812e+38F"),
        ("__DBL_MANT_DIG__", "53"),
    ] {
        let toks = lex(v).map_err(|e| e.to_string())?;
        macros.insert(m.into(), Macro::Obj(toks));
    }
    for (m, v) in [
        ("__SIZE_TYPE__", "unsigned long"),
        ("__PTRDIFF_TYPE__", "long"),
        ("__WCHAR_TYPE__", "int"),
        ("__INTPTR_TYPE__", "long"),
        ("__UINTPTR_TYPE__", "unsigned long"),
        ("__INT8_TYPE__", "signed char"),
        ("__UINT8_TYPE__", "unsigned char"),
        ("__INT16_TYPE__", "short"),
        ("__UINT16_TYPE__", "unsigned short"),
        ("__INT32_TYPE__", "int"),
        ("__UINT32_TYPE__", "unsigned int"),
        ("__INT64_TYPE__", "long"),
        ("__UINT64_TYPE__", "unsigned long"),
    ] {
        let toks = lex(v).map_err(|e| e.to_string())?;
        macros.insert(m.into(), Macro::Obj(toks));
    }
    // __USER_LABEL_PREFIX__: glibc __REDIRECT/__ASMNAME stringize it into the asm
    // symbol prefix (scanf → __isoc99_scanf); if left undefined, the literal
    // "#__USER_LABEL_PREFIX__" would be pasted into the symbol name. On ELF: empty.
    macros.insert("__USER_LABEL_PREFIX__".into(), Macro::Obj(Vec::new()));
    // __builtin_va_list: va_list = an AAPCS struct (defined by stdarg.h);
    // va_start/va_arg are NOT macros — the parser lowers them to
    // Node::VaStart/VaArg. A typedef through the bare struct name is valid even
    // while incomplete (glibc __va_list.h typedefs it before anyone includes stdarg.h).
    macros.insert(
        "__builtin_va_list".into(),
        Macro::Obj(lex("struct __zcc_va_list").map_err(|e| e.to_string())?),
    );
    // inf/nan (required by musl math.h under GNUC>=3.3; if missing they become
    // bare calls): 1e10000 overflows the f64 parse → inf per the standard; nan
    // via 0/0 (folds to NaN). Function-like with 0 parameters because the code
    // calls them with parentheses: __builtin_inff().
    for (m, ps, body) in [
        ("__builtin_inf", vec![], "(1e10000)"),
        ("__builtin_inff", vec![], "(1e10000f)"),
        ("__builtin_huge_val", vec![], "(1e10000)"),
        ("__builtin_huge_valf", vec![], "(1e10000f)"),
        ("__builtin_nan", vec!["s"], "(0.0/0.0)"),
        ("__builtin_nanf", vec!["s"], "(0.0f/0.0f)"),
        ("__builtin_va_end", vec!["ap"], "((void)(ap))"),
        ("__builtin_va_copy", vec!["d", "s"], "((d) = (s))"),
        ("__builtin_constant_p", vec!["e"], "0"), // 0 is always valid per the GCC docs
        ("__builtin_unreachable", vec![], "((void)0)"),
        ("__builtin_trap", vec![], "abort()"),
        ("__builtin_return_address", vec!["n"], "((void *)0)"),
        // signbit via -0.0: 1.0/x = -inf distinguishes the sign of zero
        (
            "__builtin_signbit",
            vec!["x"],
            "((x) < 0.0 || ((x) == 0.0 && 1.0 / (x) < 0.0))",
        ),
    ] {
        let toks = lex(body).map_err(|e| e.to_string())?;
        let ps = ps.into_iter().map(String::from).collect();
        macros.insert(m.into(), Macro::Fun(ps, false, toks));
    }
    // __builtin_prefetch(addr, rw?, locality?) → no-op BUT still evaluates addr
    // (a side effect in the argument must run — GCC docs; rw/locality are constants, discardable)
    macros.insert(
        "__builtin_prefetch".into(),
        Macro::Fun(
            vec!["a".into(), "__VA_ARGS__".into()],
            true,
            lex("((void)(a))").map_err(|e| e.to_string())?,
        ),
    );
    // common GCC builtin: __builtin_expect(e, c) → (e)
    macros.insert(
        "__builtin_expect".into(),
        Macro::Fun(
            vec!["e".into(), "c".into()],
            false,
            vec![
                synth(Tok::Punct("(")),
                synth(Tok::Ident("e".into())),
                synth(Tok::Punct(")")),
            ],
        ),
    );
    // EXT(gcc): the __atomic_* family → macros routed to __sync_*; bit-manipulation builtins (ext.rs)
    for (m, ps, body) in crate::ext::ATOMIC_MACROS
        .iter()
        .chain(crate::ext::BIT_MACROS)
    {
        let toks = lex(body).map_err(|e| e.to_string())?;
        let ps = ps.iter().map(|s| s.to_string()).collect();
        macros.insert((*m).into(), Macro::Fun(ps, false, toks));
    }
    for (i, m) in crate::ext::ATOMIC_ORDERS.iter().enumerate() {
        macros.insert(
            (*m).into(),
            Macro::Obj(vec![synth(Tok::Num(i as i64, NumK::I))]),
        );
    }
    // -Dname[=val] from the CLI (default =1, like cc); -Uname removes it (including builtins)
    for d in defs {
        let (n, v) = d.split_once('=').unwrap_or((d, "1"));
        let toks = lex(v).map_err(|e| e.to_string())?;
        macros.insert(n.into(), Macro::Obj(toks));
    }
    for u in undefs {
        macros.remove(u.as_str());
    }
    let mut files = Vec::new();
    let pts = pp_file(path, &mut macros, 0, incs, &mut files, true)?; // ELF: char unsigned
    // C99: the _Pragma("...") operator — consumed like #pragma (the SDK uses it inside macros)
    let (mut out, mut i) = (Vec::with_capacity(pts.len()), 0);
    while i < pts.len() {
        if matches!(&pts[i].tok, Tok::Ident(n) if n == "_Pragma")
            && matches!(pts.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("(")))
            && matches!(pts.get(i + 2).map(|t| &t.tok), Some(Tok::Str(..)))
            && matches!(pts.get(i + 3).map(|t| &t.tok), Some(Tok::Punct(")")))
        {
            i += 4;
        } else {
            out.push(pts[i].clone());
            i += 1;
        }
    }
    Ok((out, files))
}

// EXT(gcc): a .S file (musl memset.S aarch64) = assembly that MUST pass through
// the C preprocessor. Re-spelling from PTok needs ws/raw: "v0.16B" splits into
// 3 C tokens ("v0" ".16" "B") that must be rejoined without a space; a token on
// a different source line starts a new line (one instruction per line).
pub fn preprocess_asm(
    path: &str,
    defs: &[String],
    undefs: &[String],
    incs: &[String],
) -> Result<String, String> {
    let (out, _) = preprocess_pt(path, defs, undefs, incs)?;
    let mut s = String::new();
    let mut cur = (u32::MAX, u32::MAX);
    for t in &out {
        if (t.file, t.line) != cur {
            s.push('\n');
            cur = (t.file, t.line);
        } else if t.ws {
            s.push(' ');
        }
        if t.raw.is_empty() {
            s.push_str(&spell(&t.tok));
        } else {
            s.push_str(&t.raw);
        }
    }
    s.push('\n');
    Ok(s)
}

fn inc_find(incs: &[String], name: &str) -> Option<String> {
    incs.iter()
        .map(|d| format!("{}/{}", d, name))
        .find(|p| std::path::Path::new(p).exists())
}
// Look up the embedded-header table. A \x01 sentinel at the head of incs is
// injected by the driver:
// \x01nostdinc (musl sysroot / -nostdinc) — disables the whole table;
// \x01elf — embeds only COMPILER-OWNED headers (the gcc model: stdarg/stddef/...),
// leaving libc headers to the box's glibc — the embedded contents carry Darwin
// values (MAP_ANON 0x1000 vs Linux 0x20, a differently laid-out jmp_buf) → wrong runtime ABI.
fn embed_src(incs: &[String], name: &str, skip: bool) -> Option<&'static str> {
    let sent = incs.first().map(String::as_str);
    if skip || sent == Some("\u{1}nostdinc") {
        return None;
    }
    if sent == Some("\u{1}elf")
        && !matches!(
            name,
            "stdarg.h" | "stddef.h" | "stdbool.h" | "stdalign.h" | "stdnoreturn.h" | "float.h"
                | "limits.h"
        )
    {
        return None;
    }
    HEADERS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

fn synth(tok: Tok) -> PTok {
    PTok {
        tok,
        bol: false,
        ws: true,
        line: 0,
        file: 0,
        hide: Vec::new(),
        raw: String::new(),
    }
}

fn err(file: &str, line: u32, msg: &str) -> String {
    format!("{}:{}: {}", file, line, msg)
}

fn ident_of(t: Option<&PTok>) -> Option<&str> {
    match t.map(|t| &t.tok) {
        Some(Tok::Ident(n)) => Some(n),
        _ => None,
    }
}

fn pp_file(
    path: &str,
    macros: &mut Macros,
    depth: u32,
    incs: &[String],
    files: &mut Vec<String>,
    cu: bool, // char_uns per target — passed into lex_t for every real source file
) -> Result<Vec<PTok>, String> {
    if depth > 32 {
        return Err(format!("{}: include nesting too deep", path));
    }
    let bytes = fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    // lex DIRECTLY over the raw bytes — preserving non-UTF8 bytes in string literals
    let mut toks = lex_t(&bytes, cu).map_err(|e| format!("{}: {}", path, e))?;
    files.push(path.to_string());
    let fid = files.len() as u32 - 1;
    for t in &mut toks {
        t.file = fid;
    }
    process(&toks, path, macros, depth, incs, files, cu)
}

// State of one #if level: parent = whether the enclosing level is active; taken
// = whether some branch has already matched (blocks a later #elif); active =
// whether the current branch is being emitted.
struct Cond {
    parent: bool,
    taken: bool,
    active: bool,
    in_else: bool,
}

fn process(
    toks: &[PTok],
    file: &str,
    macros: &mut Macros,
    depth: u32,
    incs: &[String],
    files: &mut Vec<String>,
    cu: bool, // char_uns per target
) -> Result<Vec<PTok>, String> {
    let (mut out, mut i) = (Vec::new(), 0);
    let mut conds: Vec<Cond> = Vec::new();
    let mut delta: i64 = 0; // #line n → __LINE__ reports n on the next line
    let mut saved: HashMap<String, Vec<Option<Macro>>> = HashMap::new(); // push_macro

    while i < toks.len() {
        let active = conds.last().is_none_or(|c| c.active);
        let t = &toks[i];
        if !(t.bol && t.tok == Tok::Punct("#")) {
            if active {
                i = expand_at(toks, i, macros, &Vec::new(), file, delta, &mut out)?;
            } else {
                i += 1;
            }
            continue;
        }
        // A directive line runs from just after '#' to the next bol token (splicing has already cleared spurious bol flags).
        let mut j = i + 1;
        while j < toks.len() && !toks[j].bol {
            j += 1;
        }
        let (d, lno) = (&toks[i + 1..j], t.line);
        i = j;
        let kw = match d.first().map(|t| &t.tok) {
            Some(Tok::Ident(k)) => k.as_str(),
            None => continue, // a bare "#": valid, ignored
            _ if !active => continue,
            _ => return Err(err(file, lno, "unknown directive")),
        };
        match kw {
            "ifdef" | "ifndef" => {
                let v = active && {
                    let name = ident_of(d.get(1))
                        .ok_or_else(|| err(file, lno, "missing name after #ifdef/#ifndef"))?;
                    // EXT(clang): __has_include + the __has_* family are "defined" builtins
                    (macros.contains_key(name)
                        || name == "__has_include"
                        || crate::ext::has_operator_zero(name))
                        == (kw == "ifdef")
                };
                conds.push(Cond {
                    parent: active,
                    taken: v,
                    active: v,
                    in_else: false,
                });
            }
            "if" => {
                let v = active && eval_if(&d[1..], macros, file, delta, lno, incs)?;
                conds.push(Cond {
                    parent: active,
                    taken: v,
                    active: v,
                    in_else: false,
                });
            }
            "elif" => {
                let need = {
                    let c = conds
                        .last()
                        .ok_or_else(|| err(file, lno, "#elif without #if"))?;
                    if c.in_else {
                        return Err(err(file, lno, "#elif after #else"));
                    }
                    c.parent && !c.taken
                };
                let v = need && eval_if(&d[1..], macros, file, delta, lno, incs)?;
                let c = conds.last_mut().unwrap();
                c.active = v;
                c.taken = c.taken || v;
            }
            "else" => {
                let c = conds
                    .last_mut()
                    .ok_or_else(|| err(file, lno, "#else without #if"))?;
                if c.in_else {
                    return Err(err(file, lno, "duplicate #else"));
                }
                c.active = c.parent && !c.taken;
                c.taken = true;
                c.in_else = true;
            }
            "endif" => {
                conds
                    .pop()
                    .ok_or_else(|| err(file, lno, "#endif without #if"))?;
            }
            _ if !active => {} // any other directive in a dead branch: discard
            "define" => {
                let name = ident_of(d.get(1))
                    .ok_or_else(|| err(file, lno, "missing name after #define"))?
                    .to_string();
                // Function-like ⟺ '(' immediately follows the name (no whitespace) — the C rule.
                let m = if matches!(d.get(2), Some(p) if p.tok == Tok::Punct("(") && !p.ws) {
                    let (mut params, mut k, mut va) = (Vec::new(), 3, false);
                    if matches!(d.get(k).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                        k += 1;
                    } else {
                        loop {
                            // "..." (C99, accepted by clang): the remainder → __VA_ARGS__
                            if matches!(d.get(k).map(|t| &t.tok), Some(Tok::Punct("..."))) {
                                params.push("__VA_ARGS__".to_string());
                                va = true;
                                k += 1;
                            } else {
                                let p = ident_of(d.get(k))
                                    .ok_or_else(|| err(file, lno, "macro parameter must be an identifier"))?;
                                params.push(p.to_string());
                                k += 1;
                                // EXT(gcc): named variadic "#define F(args...)" —
                                // the last parameter absorbs the remainder like __VA_ARGS__
                                if matches!(d.get(k).map(|t| &t.tok), Some(Tok::Punct("..."))) {
                                    va = true;
                                    k += 1;
                                }
                            }
                            match d.get(k).map(|t| &t.tok) {
                                Some(Tok::Punct(",")) if !va => k += 1,
                                Some(Tok::Punct(")")) => {
                                    k += 1;
                                    break;
                                }
                                _ => return Err(err(file, lno, "missing ')' in #define")),
                            }
                        }
                    }
                    Macro::Fun(params, va, d[k..].to_vec())
                } else {
                    Macro::Obj(d.get(2..).unwrap_or(&[]).to_vec())
                };
                macros.insert(name, m);
            }
            "undef" => {
                let name =
                    ident_of(d.get(1)).ok_or_else(|| err(file, lno, "missing name after #undef"))?;
                macros.remove(name);
            }
            // EXT(gcc): #include_next — a subset: like #include <..> but SKIPPING
            // the embedded table (sufficient for its sole role: zcc's embedded
            // header plays "the compiler's header" and then forwards down to the
            // real libc, as clang does; glibc's include_next never fails because
            // of the _GCC_LIMITS_H_ guard)
            "include" | "include_next" => {
                let skip_embedded = kw == "include_next";
                // #include MACRO (computed include, C89 §3.8.2): when neither
                // standard form matches, expand the macro before dispatching (redis rax.c)
                let nd: Vec<PTok>;
                let d: &[PTok] = if matches!(d.get(1).map(|t| &t.tok), Some(Tok::Ident(_))) {
                    let mut v = vec![d[0].clone()];
                    v.extend(expand_seq(&d[1..], macros, &Vec::new(), file, delta)?);
                    nd = v;
                    &nd
                } else {
                    d
                };
                match d.get(1).map(|t| &t.tok) {
                    Some(Tok::Str(b, _)) => {
                        let name = String::from_utf8_lossy(b).into_owned();
                        let bare = name.clone();
                        let path = if name.starts_with('/') {
                            name
                        } else {
                            match file.rsplit_once('/') {
                                Some((dir, _)) => format!("{}/{}", dir, name),
                                None => name,
                            }
                        };
                        // no real file → try the embedded-header table ("stddef.h"...)
                        let mut path = path;
                        if !std::path::Path::new(&path).exists() {
                            if let Some(src) = embed_src(incs, &bare, skip_embedded) {
                                let hname = format!("<{}>", bare);
                                let mut toks = lex_t(src.as_bytes(), cu).map_err(|e| format!("{}: {}", hname, e))?;
                                files.push(hname.clone());
                                let fid = files.len() as u32 - 1;
                                for t in &mut toks {
                                    t.file = fid;
                                }
                                out.extend(process(&toks, &hname, macros, depth + 1, incs, files, cu)?);
                                continue;
                            }
                            if let Some(p2) = inc_find(incs, &bare) {
                                path = p2;
                            }
                        }
                        out.extend(pp_file(&path, macros, depth + 1, incs, files, cu)?);
                    }
                    Some(Tok::Punct("<")) => {
                        // the name = the spelling of the tokens up to '>' (the lexer splits "stdio.h" into 3 tokens)
                        let mut name = String::new();
                        let mut k = 2;
                        while k < d.len() && d[k].tok != Tok::Punct(">") {
                            name.push_str(&spell(&d[k].tok));
                            k += 1;
                        }
                        // the embedded header wins (zcc's libc stub); otherwise consult -I
                        match embed_src(incs, &name, skip_embedded) {
                            Some(src) => {
                                let hname = format!("<{}>", name);
                                let mut toks = lex_t(src.as_bytes(), cu).map_err(|e| format!("{}: {}", hname, e))?;
                                files.push(hname.clone());
                                let fid = files.len() as u32 - 1;
                                for t in &mut toks {
                                    t.file = fid;
                                }
                                out.extend(process(&toks, &hname, macros, depth + 1, incs, files, cu)?);
                            }
                            None => {
                                let p2 = inc_find(incs, &name).ok_or_else(|| {
                                    err(file, lno, &format!("no embedded header <{}>", name))
                                })?;
                                out.extend(pp_file(&p2, macros, depth + 1, incs, files, cu)?);
                            }
                        }
                    }
                    _ => return Err(err(file, lno, "#include expects \"file\"")),
                }
            }
            "error" => return Err(err(file, lno, &format!("#error {}", spell_seq(&d[1..])))),
            // EXT(gcc): #warning — report and continue (not present in C89)
            "warning" => eprintln!(
                "{}",
                err(file, lno, &format!("#warning {}", spell_seq(&d[1..])))
            ),
            "line" => {
                let ex = expand_seq(&d[1..], macros, &Vec::new(), file, delta)?;
                if let Some(Tok::Num(n, _)) = ex.first().map(|t| &t.tok) {
                    delta = n - (lno as i64 + 1); // the line FOLLOWING the directive takes number n
                }
            }
            "pragma" => {
                // EXT(gcc): #pragma push_macro("x") / pop_macro("x")
                if let (Some(Tok::Ident(p)), Some(Tok::Str(b, _))) =
                    (d.get(1).map(|t| &t.tok), d.get(3).map(|t| &t.tok))
                {
                    let name = String::from_utf8_lossy(b).into_owned();
                    if p == "push_macro" {
                        saved
                            .entry(name.clone())
                            .or_default()
                            .push(macros.get(&name).cloned());
                    } else if p == "pop_macro"
                        && let Some(m) = saved.get_mut(&name).and_then(|v| v.pop()) {
                            match m {
                                Some(m) => drop(macros.insert(name, m)),
                                None => drop(macros.remove(&name)),
                            }
                        }
                }
            }
            _ => return Err(err(file, lno, &format!("unknown directive #{}", kw))),
        }
    }
    if !conds.is_empty() {
        return Err(format!("{}: missing #endif", file));
    }
    Ok(out)
}

// ---- Macro expansion (recursive rescan; hide = stack of names currently open, to prevent cycles) ----

fn expand_seq(
    toks: &[PTok],
    macros: &Macros,
    hide: &Vec<String>,
    file: &str,
    delta: i64,
) -> Result<Vec<PTok>, String> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < toks.len() {
        i = expand_at(toks, i, macros, hide, file, delta, &mut out)?;
    }
    Ok(out)
}

fn expand_at(
    toks: &[PTok],
    i: usize,
    macros: &Macros,
    hide: &Vec<String>,
    file: &str,
    delta: i64, // #line shifts the reported line number (affects __LINE__ only)
    out: &mut Vec<PTok>,
) -> Result<usize, String> {
    let t = &toks[i];
    let name = match &t.tok {
        Tok::Ident(n) => n.clone(),
        _ => {
            out.push(t.clone());
            return Ok(i + 1);
        }
    };
    if name == "__LINE__" {
        out.push(PTok {
            tok: Tok::Num(t.line as i64 + delta, NumK::I),
            ..t.clone()
        });
        return Ok(i + 1);
    }
    if name == "__FILE__" {
        let fb = file.as_bytes().to_vec();
        let cps = fb.iter().map(|&x| x as u32).collect();
        out.push(PTok {
            tok: Tok::Str(fb, (1, cps)),
            ..t.clone()
        });
        return Ok(i + 1);
    }
    if hide.contains(&name) || t.hide.contains(&name) {
        out.push(t.clone());
        return Ok(i + 1);
    }
    let mut next = match macros.get(&name) {
        None => {
            out.push(t.clone());
            return Ok(i + 1);
        }
        Some(Macro::Obj(body)) => {
            let body = retag(body, t, &t.hide);
            let mut h = hide.clone();
            h.push(name.clone());
            let mut ex = expand_seq(&body, macros, &h, file, delta)?;
            for e in &mut ex {
                e.hide.push(name.clone());
            }
            out.extend(ex);
            i + 1
        }
        Some(Macro::Fun(params, va, body)) => {
            // A function-like macro name not followed by '(': an ordinary identifier.
            if !matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("("))) {
                out.push(t.clone());
                return Ok(i + 1);
            }
            let (mut args, next) = collect_args(toks, i + 2, file, t.line)?;
            if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                args.clear(); // F() = 0 arguments
            }
            if *va {
                if args.len() < params.len() {
                    args.push(Vec::new()); // F(a) with (a, ...): __VA_ARGS__ empty
                }
                // surplus arguments are merged into __VA_ARGS__ (rejoining the commas that were split)
                let extra = args.split_off(params.len().min(args.len()));
                if let Some(last) = args.last_mut() {
                    for e in extra {
                        last.push(synth(Tok::Punct(",")));
                        last.extend(e);
                    }
                }
            }
            if args.len() != params.len() {
                return Err(err(
                    file,
                    t.line,
                    &format!(
                        "macro {} expects {} arguments, got {}",
                        name,
                        params.len(),
                        args.len()
                    ),
                ));
            }
            let sub = substitute(body, params, &args, macros, hide, file, delta, t.line)?;
            // standard hideset rule: result HS = (HS(name) ∩ HS(closing ")")) ∪ {name}
            let close = &toks[next - 1];
            let callhs: Vec<String> = t
                .hide
                .iter()
                .filter(|h| close.hide.contains(h))
                .cloned()
                .collect();
            let sub = retag(&sub, t, &callhs);
            let mut h = hide.clone();
            h.push(name.clone());
            let mut ex = expand_seq(&sub, macros, &h, file, delta)?;
            for e in &mut ex {
                e.hide.push(name.clone());
            }
            out.extend(ex);
            next
        }
    };
    // Rescan ACROSS THE BOUNDARY (C89 6.8.3.4): the tail of the expansion is a
    // function-like macro name whose '(' lies further along in the original
    // stream → rejoin and continue expanding.
    loop {
        let is_fun = match out.last().map(|l| &l.tok) {
            Some(Tok::Ident(n)) => {
                !hide.contains(n)
                    && !out.last().unwrap().hide.contains(n)
                    && matches!(macros.get(n), Some(Macro::Fun(..)))
            }
            _ => false,
        };
        if !is_fun || !matches!(toks.get(next).map(|t| &t.tok), Some(Tok::Punct("("))) {
            break;
        }
        let head = out.pop().unwrap();
        let mut stream = vec![head];
        stream.extend_from_slice(&toks[next..]);
        let used = expand_at(&stream, 0, macros, hide, file, delta, out)?;
        next += used - 1;
    }
    Ok(next)
}

// Macro-body tokens carry the line of the CALL SITE (per the __LINE__ rule); the
// first token's ws is inherited from the call site so nested stringization keeps
// reasonable spacing.
fn retag(body: &[PTok], at: &PTok, callhs: &[String]) -> Vec<PTok> {
    body.iter()
        .enumerate()
        .map(|(k, b)| {
            let mut hide = b.hide.clone();
            hide.extend(callhs.iter().cloned());
            PTok {
                tok: b.tok.clone(),
                bol: false,
                ws: if k == 0 { at.ws } else { b.ws },
                line: at.line,
                file: at.file,
                hide,
                raw: b.raw.clone(),
            }
        })
        .collect()
}

// Collect arguments after '(': split on ',' at depth 0, keeping nested parentheses intact.
fn collect_args(
    toks: &[PTok],
    mut i: usize,
    file: &str,
    lno: u32,
) -> Result<(Vec<Vec<PTok>>, usize), String> {
    let (mut args, mut cur, mut depth) = (Vec::new(), Vec::new(), 0u32);
    loop {
        let t = toks
            .get(i)
            .ok_or_else(|| err(file, lno, "missing ')' to close macro arguments"))?;
        match &t.tok {
            Tok::Punct("(") => {
                depth += 1;
                cur.push(t.clone());
            }
            Tok::Punct(")") if depth == 0 => {
                args.push(cur);
                return Ok((args, i + 1));
            }
            Tok::Punct(")") => {
                depth -= 1;
                cur.push(t.clone());
            }
            Tok::Punct(",") if depth == 0 => args.push(std::mem::take(&mut cur)),
            _ => cur.push(t.clone()),
        }
        i += 1;
    }
}

fn param_of(t: Option<&PTok>, params: &[String]) -> Option<usize> {
    ident_of(t).and_then(|n| params.iter().position(|p| p == n))
}

// Substitute parameters into the macro body: #p = stringize the RAW argument, p
// adjacent to ## = the raw argument, an ordinary p = the fully expanded
// argument; ## pastes the last token of the left onto the first token of the right.
fn substitute(
    body: &[PTok],
    params: &[String],
    args: &[Vec<PTok>],
    macros: &Macros,
    hide: &Vec<String>,
    file: &str,
    delta: i64,
    lno: u32,
) -> Result<Vec<PTok>, String> {
    let (mut out, mut i) = (Vec::<PTok>::new(), 0);
    while i < body.len() {
        let t = &body[i];
        if t.tok == Tok::Punct("#") {
            let p = param_of(body.get(i + 1), params)
                .ok_or_else(|| err(file, lno, "# must be followed by a macro parameter"))?;
            let sb = stringize(&args[p]);
            let cps = sb.iter().map(|&x| x as u32).collect();
            out.push(PTok {
                tok: Tok::Str(sb, (1, cps)),
                bol: false,
                ws: t.ws,
                line: lno,
                file: t.file,
                hide: Vec::new(),
                raw: String::new(),
            });
            i += 2;
        } else if t.tok == Tok::Punct("##") {
            let r = body
                .get(i + 1)
                .ok_or_else(|| err(file, lno, "## at end of macro body"))?;
            // EXT(gcc): ", ## __VA_ARGS__" — when the argument is empty, DROP the
            // comma; when it is present, keep the comma + args (no actual paste;
            // rescan expands it later)
            if let Some(p) = param_of(Some(r), params)
                && matches!(out.last().map(|x| &x.tok), Some(Tok::Punct(","))) {
                    if args[p].is_empty() {
                        out.pop();
                    } else {
                        out.extend(args[p].iter().cloned());
                    }
                    i += 2;
                    continue;
                }
            let rhs = match param_of(Some(r), params) {
                Some(p) => args[p].clone(),
                None => vec![r.clone()],
            };
            let l = out
                .pop()
                .ok_or_else(|| err(file, lno, "## at start of macro body"))?;
            if rhs.is_empty() {
                out.push(l);
            } else {
                // use the ORIGINAL (raw) spelling when available — spell() from the
                // value drops a numeric suffix (199506L → "199506", making cdefs.h
                // paste an incorrect macro name)
                let sp = |p: &PTok| {
                    if p.raw.is_empty() {
                        spell(&p.tok)
                    } else {
                        p.raw.clone()
                    }
                };
                let s = format!("{}{}", sp(&l), sp(&rhs[0]));
                let one = lex(&s)
                    .ok()
                    .and_then(|mut v| {
                        if v.len() == 1 {
                            Some(v.remove(0).tok)
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| err(file, lno, &format!("## formed an invalid token '{}'", s)))?;
                out.push(PTok {
                    tok: one,
                    bol: false,
                    ws: l.ws,
                    line: lno,
                    file: l.file,
                    hide: Vec::new(),
                    raw: String::new(),
                });
                out.extend(rhs[1..].iter().cloned());
            }
            i += 2;
        } else if let Some(p) = param_of(Some(t), params) {
            let raw = matches!(body.get(i + 1).map(|n| &n.tok), Some(Tok::Punct("##")));
            let mut rep = if raw {
                args[p].clone()
            } else {
                expand_seq(&args[p], macros, hide, file, delta)?
            };
            if let Some(first) = rep.first_mut() {
                first.ws = t.ws;
                first.bol = false;
            }
            out.extend(rep);
            i += 1;
        } else {
            out.push(t.clone());
            i += 1;
        }
    }
    Ok(out)
}

// ---- Spelling (for # stringization, ## paste, #error) ----

pub fn spell(t: &Tok) -> String {
    match t {
        Tok::Num(n, NumK::U | NumK::UL) => (*n as u64).to_string(),
        Tok::Num(n, _) => n.to_string(),
        Tok::FNum(v, _) => format!("{v:?}"),
        Tok::INum(v, _) => format!("{v:?}i"),
        Tok::Ident(s) => s.clone(),
        Tok::Punct(p) => p.to_string(),
        Tok::Str(b, _) => {
            let mut s = String::from("\"");
            for &c in b {
                match c {
                    b'"' => s.push_str("\\\""),
                    b'\\' => s.push_str("\\\\"),
                    10 => s.push_str("\\n"),
                    9 => s.push_str("\\t"),
                    0 => s.push_str("\\0"),
                    c => s.push(c as char),
                }
            }
            s.push('"');
            s
        }
    }
}

// Join spellings; any whitespace between two tokens → exactly one space (the C89 # rule).
fn spell_seq(ts: &[PTok]) -> String {
    let mut s = String::new();
    for (k, t) in ts.iter().enumerate() {
        if k > 0 && t.ws {
            s.push(' ');
        }
        if t.raw.is_empty() {
            s.push_str(&spell(&t.tok));
        } else {
            s.push_str(&t.raw); // original spelling: 0xff, 'a', "s\n"
        }
    }
    s
}

fn stringize(arg: &[PTok]) -> Vec<u8> {
    spell_seq(arg).into_bytes()
}

// ---- #if constant expression: resolve defined first, expand, remaining identifiers → 0, then eval as i64 ----

fn eval_if(
    d: &[PTok],
    macros: &Macros,
    file: &str,
    delta: i64,
    lno: u32,
    incs: &[String],
) -> Result<bool, String> {
    let pre = if_resolve(d, macros, file, lno, incs)?;
    let ex = expand_seq(&pre, macros, &Vec::new(), file, delta)?;
    // a macro may expand INTO defined(...)/__has_include (SDK pthread.h) → resolve a second time
    let ex = if_resolve(&ex, macros, file, lno, incs)?;
    let ts: Vec<Tok> = ex
        .into_iter()
        .map(|t| match t.tok {
            Tok::Ident(_) => Tok::Num(0, NumK::I), // a non-macro identifier → 0 (the C rule)
            tok => tok,
        })
        .collect();
    let mut p = 0;
    let v = ternary(&ts, &mut p).map_err(|e| err(file, lno, &e))?;
    if p != ts.len() {
        return Err(err(file, lno, "extra tokens in #if expression"));
    }
    Ok(v.0 != 0)
}

// resolve defined(X) / __has_include(...) to 0|1 within an #if expression
fn if_resolve(
    d: &[PTok],
    macros: &Macros,
    file: &str,
    lno: u32,
    incs: &[String],
) -> Result<Vec<PTok>, String> {
    let (mut pre, mut i) = (Vec::new(), 0);
    while i < d.len() {
        if matches!(&d[i].tok, Tok::Ident(n) if n == "defined") {
            let paren = matches!(d.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("(")));
            let at = if paren { i + 2 } else { i + 1 };
            let name =
                ident_of(d.get(at)).ok_or_else(|| err(file, lno, "defined requires a macro name"))?;
            // EXT(clang): __has_include + the __has_* family are builtin operators — "defined" = 1
            let def = macros.contains_key(name)
                || name == "__has_include"
                || crate::ext::has_operator_zero(name);
            pre.push(synth(Tok::Num(def as i64, NumK::I)));
            i = at + 1;
            if paren {
                if !matches!(d.get(i).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                    return Err(err(file, lno, "defined( missing ')'"));
                }
                i += 1;
            }
        } else if matches!(&d[i].tok, Tok::Ident(n) if crate::ext::has_operator_zero(n))
            && matches!(d.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("(")))
        {
            // EXT(clang): __has_feature/extension/builtin/attribute(...) → 0,
            // consuming balanced parentheses (the argument is only an identifier, but guard against nesting anyway)
            i += 2;
            let mut depth = 1;
            while depth > 0 {
                match d.get(i).map(|t| &t.tok) {
                    Some(Tok::Punct("(")) => depth += 1,
                    Some(Tok::Punct(")")) => depth -= 1,
                    None => return Err(err(file, lno, "__has_*( missing ')'")),
                    _ => {}
                }
                i += 1;
            }
            pre.push(synth(Tok::Num(0, NumK::I)));
        } else if matches!(&d[i].tok, Tok::Ident(n) if n == "__has_include" || n == "__has_include_next")
        {
            // EXT(clang): __has_include(<h> | "h") — evaluated BEFORE expansion (a
            // header name is not a macro); __has_include_next is always 0 (there is no include stack)
            let next = matches!(&d[i].tok, Tok::Ident(n) if n == "__has_include_next");
            if !matches!(d.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("("))) {
                return Err(err(file, lno, "__has_include requires '('"));
            }
            i += 2;
            let name = match d.get(i).map(|t| &t.tok) {
                Some(Tok::Str(b, _)) => {
                    i += 1;
                    String::from_utf8_lossy(b).into_owned()
                }
                Some(Tok::Punct("<")) => {
                    i += 1;
                    let mut s = String::new();
                    while !matches!(d.get(i).map(|t| &t.tok), Some(Tok::Punct(">")) | None) {
                        s.push_str(&spell(&d[i].tok));
                        i += 1;
                    }
                    i += 1; // '>'
                    s
                }
                _ => return Err(err(file, lno, "__has_include requires <h> or \"h\"")),
            };
            if !matches!(d.get(i).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                return Err(err(file, lno, "__has_include( missing ')'"));
            }
            i += 1;
            let found = !next
                && (embed_src(incs, &name, false).is_some() || inc_find(incs, &name).is_some());
            pre.push(synth(Tok::Num(found as i64, NumK::I)));
        } else {
            pre.push(d[i].clone());
            i += 1;
        }
    }
    Ok(pre)
}

fn eat(ts: &[Tok], p: &mut usize, op: &str) -> bool {
    if matches!(ts.get(*p), Some(Tok::Punct(x)) if *x == op) {
        *p += 1;
        true
    } else {
        false
    }
}

// An #if value = (bits, unsigned): C89 3.8.1 — all arithmetic in #if is computed
// in long/unsigned long, and the usual arithmetic conversions reduce to one bit
// "is there an unsigned operand". Dropping this bit (pure i64 eval) gives wrong
// / % >> comparisons on mixed signedness: -1L < 0xFF..FFUL must be 0. This
// defect is caught by tests/cpp.sh (the F family, exhausting #if).
type PV = (i64, bool);

fn ternary(ts: &[Tok], p: &mut usize) -> Result<PV, String> {
    let c = binlv(ts, p, 0)?;
    if !eat(ts, p, "?") {
        return Ok(c);
    }
    let a = ternary(ts, p)?;
    if !eat(ts, p, ":") {
        return Err("missing ':' for '?'".into());
    }
    let b = ternary(ts, p)?;
    // the common type of ?: is the usual arithmetic conversion of both branches; the value follows the selected branch
    Ok((if c.0 != 0 { a.0 } else { b.0 }, a.1 | b.1))
}

/// THEORY II-1 — ISO C99 §5.2.4.1 nesting limits
// C binary-operator precedence levels, lowest → highest.
const LEVELS: [&[&str]; 10] = [
    &["||"],
    &["&&"],
    &["|"],
    &["^"],
    &["&"],
    &["==", "!="],
    &["<", ">", "<=", ">="],
    &["<<", ">>"],
    &["+", "-"],
    &["*", "/", "%"],
];

fn binlv(ts: &[Tok], p: &mut usize, lv: usize) -> Result<PV, String> {
    if lv == LEVELS.len() {
        return unary(ts, p);
    }
    let mut l = binlv(ts, p, lv + 1)?;
    loop {
        let op = match ts.get(*p) {
            Some(Tok::Punct(x)) if LEVELS[lv].contains(x) => *x,
            _ => return Ok(l),
        };
        *p += 1;
        let r = binlv(ts, p, lv + 1)?;
        let u = l.1 | r.1; // usual arithmetic conversions: one unsigned side → unsigned operation
        let b = |x: bool| (x as i64, false);
        // wrapping: overflow is UB in C, so do not panic in Rust
        l = match op {
            "||" => b(l.0 != 0 || r.0 != 0),
            "&&" => b(l.0 != 0 && r.0 != 0),
            "|" => (l.0 | r.0, u),
            "^" => (l.0 ^ r.0, u),
            "&" => (l.0 & r.0, u),
            "==" => b(l.0 == r.0),
            "!=" => b(l.0 != r.0),
            "<" if u => b((l.0 as u64) < (r.0 as u64)),
            ">" if u => b((l.0 as u64) > (r.0 as u64)),
            "<=" if u => b((l.0 as u64) <= (r.0 as u64)),
            ">=" if u => b((l.0 as u64) >= (r.0 as u64)),
            "<" => b(l.0 < r.0),
            ">" => b(l.0 > r.0),
            "<=" => b(l.0 <= r.0),
            ">=" => b(l.0 >= r.0),
            // shift: the result type = the type of the LEFT operand (3.3.7 — no usual arithmetic conversions)
            "<<" => (l.0.wrapping_shl(r.0 as u32), l.1),
            ">>" if l.1 => (((l.0 as u64).wrapping_shr(r.0 as u32)) as i64, true),
            ">>" => (l.0.wrapping_shr(r.0 as u32), l.1),
            "+" => (l.0.wrapping_add(r.0), u),
            "-" => (l.0.wrapping_sub(r.0), u),
            "*" => (l.0.wrapping_mul(r.0), u),
            // division by 0 in a dead branch of && || must be harmless (eager eval) → yield 0
            "/" | "%" if r.0 == 0 => (0, u),
            "/" if u => (((l.0 as u64) / (r.0 as u64)) as i64, true),
            "%" if u => (((l.0 as u64) % (r.0 as u64)) as i64, true),
            "/" => (l.0.wrapping_div(r.0), false),
            "%" => (l.0.wrapping_rem(r.0), false),
            _ => unreachable!(),
        };
    }
}

fn unary(ts: &[Tok], p: &mut usize) -> Result<PV, String> {
    if eat(ts, p, "!") {
        Ok(((unary(ts, p)?.0 == 0) as i64, false))
    } else if eat(ts, p, "~") {
        let v = unary(ts, p)?;
        Ok((!v.0, v.1))
    } else if eat(ts, p, "-") {
        let v = unary(ts, p)?;
        Ok((v.0.wrapping_neg(), v.1))
    } else if eat(ts, p, "+") {
        unary(ts, p)
    } else if eat(ts, p, "(") {
        let v = ternary(ts, p)?;
        if !eat(ts, p, ")") {
            return Err("missing ')' in #if".into());
        }
        Ok(v)
    } else if let Some(&Tok::Num(n, k)) = ts.get(*p) {
        *p += 1;
        Ok((n, matches!(k, NumK::U | NumK::UL)))
    } else {
        Err("malformed #if expression".into())
    }
}
