// Preprocessor C89 (M7): token-based, chạy giữa lexer và parser.
// Vào: đường dẫn file; ra: Vec<Tok> sạch directive cho parser.
// Hỗ trợ (do test đòi): #include "..." (tương đối thư mục file đang xử lý),
// #define object/function (# stringize, ## paste, rescan đệ quy có chặn vòng
// bằng hide-stack), #undef, #if/#ifdef/#ifndef/#elif/#else/#endif (constexpr
// đủ toán tử C + defined), #error; #line/#pragma nuốt im lặng.
// Macro sẵn: __LINE__ __FILE__ __STDC__ + bảng builtin GCC. #include <...>:
// tra bảng header nhúng trước, rồi các thư mục -I (filesystem). Mỗi token mang
// (file, line, hideset) — hideset theo luật chuẩn để macro không expand lặp.

use crate::lexer::{lex, NumK, PTok, Tok};
use std::collections::HashMap;
use std::fs;

#[derive(Clone)]
enum Macro {
    Obj(Vec<PTok>),
    Fun(Vec<String>, bool, Vec<PTok>), // bool = variadic (... → __VA_ARGS__)
}
type Macros = HashMap<String, Macro>;

// Header hệ thống NHÚNG vào binary (zero dependency, target lock Darwin/arm64
// nên nội dung cố định). #include <...> tra bảng này, không đọc filesystem.
const HEADERS: [(&str, &str); 17] = [
    ("wchar.h", include_str!("headers/wchar.h")),
    ("inttypes.h", include_str!("headers/inttypes.h")),
    ("sys/mman.h", include_str!("headers/sys/mman.h")),
    ("libkern/OSCacheControl.h", include_str!("headers/libkern/OSCacheControl.h")),
    ("setjmp.h", include_str!("headers/setjmp.h")),
    ("stdbool.h", include_str!("headers/stdbool.h")),
    ("stdalign.h", include_str!("headers/stdalign.h")),
    ("stdnoreturn.h", include_str!("headers/stdnoreturn.h")),
    ("assert.h", include_str!("headers/assert.h")),
    ("ctype.h", include_str!("headers/ctype.h")),
    ("float.h", include_str!("headers/float.h")),
    ("math.h", include_str!("headers/math.h")),
    ("stdarg.h", include_str!("headers/stdarg.h")),
    ("stddef.h", include_str!("headers/stddef.h")),
    ("stdio.h", include_str!("headers/stdio.h")),
    ("stdlib.h", include_str!("headers/stdlib.h")),
    ("string.h", include_str!("headers/string.h")),
];

pub fn preprocess(
    path: &str,
    defs: &[String],
    undefs: &[String],
    incs: &[String],
) -> Result<(Vec<Tok>, Vec<(u32, u32)>, Vec<String>), String> {
    let mut macros = Macros::new();
    for m in [
        "__STDC__",
        "__LP64__",
        "__APPLE__",
        "__MACH__",
        "__arm64__",
        "__aarch64__",
        "__LITTLE_ENDIAN__", // EXT(apple): OSByteOrder.h chọn nhánh endian bằng nó
    ] {
        macros.insert(m.into(), Macro::Obj(vec![synth(Tok::Num(1, NumK::I))]));
    }
    for (m, v) in [
        // EXT(gcc): SDK Darwin CHỈ viết nhánh arm64 dưới #ifdef __GNUC__ (nhánh
        // non-GNUC là x86-only, thiếu cả _OSSwapInt16) → phải xưng GNUC dialect
        // như clang vẫn xưng (4.2.1). KHÔNG xưng __clang__ (né feature riêng).
        ("__GNUC__", "4"),
        ("__GNUC_MINOR__", "2"),
        ("__GNUC_PATCHLEVEL__", "1"),
        // EXT(apple): TargetConditionals.h chọn nhánh GNUC bằng
        // defined(__GNUC__) && defined(__APPLE_CC__) — giá trị 6000 như clang
        ("__APPLE_CC__", "6000"),
        // EXT(apple): AvailabilityMacros.h suy __MAC_OS_X_VERSION_MAX_ALLOWED
        // từ macro này (redis config.h dò MAC_OS_10_6 → dùng stat thường thay
        // struct stat64 ẩn). 110000 = macOS 11, OS đầu tiên của Apple Silicon.
        ("__ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__", "110000"),
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
    // __builtin_va_*: bản sao stdarg.h cho code gọi thẳng builtin (torture)
    macros.insert(
        "__builtin_va_list".into(),
        Macro::Obj(lex("char *").map_err(|e| e.to_string())?),
    );
    for (m, ps, body) in [
        ("__builtin_va_start", vec!["ap", "last"], "((ap) = (char *)__va_area__)"),
        (
            "__builtin_va_arg",
            vec!["ap", "t"],
            "(*(t *)(sizeof(t) > 16 ? *(char **)(((ap) += 8) - 8) \
             : (((ap) += ((sizeof(t) + 7) & ~7UL)) - ((sizeof(t) + 7) & ~7UL))))",
        ),
        ("__builtin_va_end", vec!["ap"], "((void)(ap))"),
        ("__builtin_va_copy", vec!["d", "s"], "((d) = (s))"),
        ("__builtin_constant_p", vec!["e"], "0"), // 0 luôn hợp lệ theo GCC doc
        ("__builtin_unreachable", vec![], "((void)0)"),
        ("__builtin_trap", vec![], "abort()"),
        ("__builtin_return_address", vec!["n"], "((void *)0)"),
        // signbit qua -0.0: 1.0/x = -inf phân biệt được dấu của zero
        ("__builtin_signbit", vec!["x"], "((x) < 0.0 || ((x) == 0.0 && 1.0 / (x) < 0.0))"),
    ] {
        let toks = lex(body).map_err(|e| e.to_string())?;
        let ps = ps.into_iter().map(String::from).collect();
        macros.insert(m.into(), Macro::Fun(ps, false, toks));
    }
    // __builtin_prefetch(addr, rw?, locality?) → no-op NHƯNG vẫn eval addr
    // (side effect trong arg phải chạy — GCC doc; rw/locality là hằng, bỏ được)
    macros.insert(
        "__builtin_prefetch".into(),
        Macro::Fun(
            vec!["a".into(), "__VA_ARGS__".into()],
            true,
            lex("((void)(a))").map_err(|e| e.to_string())?,
        ),
    );
    // builtin GCC hay gặp: __builtin_expect(e, c) → (e)
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
    // EXT(gcc): họ __atomic_* → macro đổ về __sync_*; builtin bit-manip (ext.rs)
    for (m, ps, body) in crate::ext::ATOMIC_MACROS.iter().chain(crate::ext::BIT_MACROS) {
        let toks = lex(body).map_err(|e| e.to_string())?;
        let ps = ps.iter().map(|s| s.to_string()).collect();
        macros.insert((*m).into(), Macro::Fun(ps, false, toks));
    }
    for (i, m) in crate::ext::ATOMIC_ORDERS.iter().enumerate() {
        macros.insert((*m).into(), Macro::Obj(vec![synth(Tok::Num(i as i64, NumK::I))]));
    }
    // -Dname[=val] từ CLI (mặc định =1, như cc); -Uname xóa (kể cả builtin)
    for d in defs {
        let (n, v) = d.split_once('=').unwrap_or((d, "1"));
        let toks = lex(v).map_err(|e| e.to_string())?;
        macros.insert(n.into(), Macro::Obj(toks));
    }
    for u in undefs {
        macros.remove(u.as_str());
    }
    let mut files = Vec::new();
    let pts = pp_file(path, &mut macros, 0, incs, &mut files)?;
    // EXT(c99): _Pragma("...") operator — nuốt như #pragma (SDK dùng trong macro)
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
    let locs = out.iter().map(|t| (t.file, t.line)).collect();
    Ok((out.into_iter().map(|t| t.tok).collect(), locs, files))
}

fn inc_find(incs: &[String], name: &str) -> Option<String> {
    incs.iter().map(|d| format!("{}/{}", d, name)).find(|p| std::path::Path::new(p).exists())
}
// -nostdinc: driver cắm sentinel \x01 đầu incs → bỏ bảng header nhúng (musl)
fn no_stdinc(incs: &[String]) -> bool {
    incs.first().is_some_and(|s| s.starts_with('\u{1}'))
}

fn synth(tok: Tok) -> PTok {
    PTok { tok, bol: false, ws: true, line: 0, file: 0, hide: Vec::new(), raw: String::new() }
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
) -> Result<Vec<PTok>, String> {
    if depth > 32 {
        return Err(format!("{}: include lồng quá sâu", path));
    }
    let bytes = fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    let src = String::from_utf8_lossy(&bytes).into_owned();
    let mut toks = lex(&src).map_err(|e| format!("{}: {}", path, e))?;
    files.push(path.to_string());
    let fid = files.len() as u32 - 1;
    for t in &mut toks {
        t.file = fid;
    }
    process(&toks, path, macros, depth, incs, files)
}

// Trạng thái một tầng #if: parent = tầng ngoài có active không; taken = đã có
// nhánh nào trúng chưa (chặn #elif sau đó); active = nhánh hiện tại đang phát.
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
) -> Result<Vec<PTok>, String> {
    let (mut out, mut i) = (Vec::new(), 0);
    let mut conds: Vec<Cond> = Vec::new();
    let mut delta: i64 = 0; // #line n → __LINE__ báo n tại dòng kế
    let mut saved: HashMap<String, Vec<Option<Macro>>> = HashMap::new(); // push_macro

    while i < toks.len() {
        let active = conds.last().map_or(true, |c| c.active);
        let t = &toks[i];
        if !(t.bol && t.tok == Tok::Punct("#")) {
            if active {
                i = expand_at(toks, i, macros, &Vec::new(), file, delta, &mut out)?;
            } else {
                i += 1;
            }
            continue;
        }
        // Directive line = từ sau '#' đến token bol kế (splice đã xoá bol giả).
        let mut j = i + 1;
        while j < toks.len() && !toks[j].bol {
            j += 1;
        }
        let (d, lno) = (&toks[i + 1..j], t.line);
        i = j;
        let kw = match d.first().map(|t| &t.tok) {
            Some(Tok::Ident(k)) => k.as_str(),
            None => continue, // "#" trơ: hợp lệ, bỏ qua
            _ if !active => continue,
            _ => return Err(err(file, lno, "directive lạ")),
        };
        match kw {
            "ifdef" | "ifndef" => {
                let v = active && {
                    let name = ident_of(d.get(1))
                        .ok_or_else(|| err(file, lno, "thiếu tên sau #ifdef/#ifndef"))?;
                    // EXT(clang): __has_include + họ __has_* "defined" sẵn
                    (macros.contains_key(name)
                        || name == "__has_include"
                        || crate::ext::has_operator_zero(name))
                        == (kw == "ifdef")
                };
                conds.push(Cond { parent: active, taken: v, active: v, in_else: false });
            }
            "if" => {
                let v = active && eval_if(&d[1..], macros, file, delta, lno, incs)?;
                conds.push(Cond { parent: active, taken: v, active: v, in_else: false });
            }
            "elif" => {
                let need = {
                    let c = conds.last().ok_or_else(|| err(file, lno, "#elif không có #if"))?;
                    if c.in_else {
                        return Err(err(file, lno, "#elif sau #else"));
                    }
                    c.parent && !c.taken
                };
                let v = need && eval_if(&d[1..], macros, file, delta, lno, incs)?;
                let c = conds.last_mut().unwrap();
                c.active = v;
                c.taken = c.taken || v;
            }
            "else" => {
                let c = conds.last_mut().ok_or_else(|| err(file, lno, "#else không có #if"))?;
                if c.in_else {
                    return Err(err(file, lno, "#else kép"));
                }
                c.active = c.parent && !c.taken;
                c.taken = true;
                c.in_else = true;
            }
            "endif" => {
                conds.pop().ok_or_else(|| err(file, lno, "#endif không có #if"))?;
            }
            _ if !active => {} // directive khác trong nhánh chết: bỏ
            "define" => {
                let name = ident_of(d.get(1))
                    .ok_or_else(|| err(file, lno, "thiếu tên sau #define"))?
                    .to_string();
                // Function-like ⟺ '(' dính SÁT tên (không whitespace) — luật C.
                let m = if matches!(d.get(2), Some(p) if p.tok == Tok::Punct("(") && !p.ws) {
                    let (mut params, mut k, mut va) = (Vec::new(), 3, false);
                    if matches!(d.get(k).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                        k += 1;
                    } else {
                        loop {
                            // "..." (C99, clang chấp nhận): phần dư → __VA_ARGS__
                            if matches!(d.get(k).map(|t| &t.tok), Some(Tok::Punct("..."))) {
                                params.push("__VA_ARGS__".to_string());
                                va = true;
                                k += 1;
                            } else {
                                let p = ident_of(d.get(k))
                                    .ok_or_else(|| err(file, lno, "tham số macro phải là ident"))?;
                                params.push(p.to_string());
                                k += 1;
                                // EXT(gcc): named variadic "#define F(args...)" —
                                // param cuối tự hứng phần dư như __VA_ARGS__
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
                                _ => return Err(err(file, lno, "thiếu ')' trong #define")),
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
                    ident_of(d.get(1)).ok_or_else(|| err(file, lno, "thiếu tên sau #undef"))?;
                macros.remove(name);
            }
            "include" => {
                // #include MACRO (computed include, C89 §3.8.2): không khớp hai
                // dạng chuẩn thì expand macro rồi mới dispatch (rax.c redis)
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
                    // không có file thật → thử bảng header nhúng ("stddef.h"...)
                    let mut path = path;
                    if !std::path::Path::new(&path).exists() {
                        if let Some((_, src)) =
                            HEADERS.iter().find(|(n, _)| *n == bare && !no_stdinc(incs))
                        {
                            let hname = format!("<{}>", bare);
                            let mut toks = lex(src).map_err(|e| format!("{}: {}", hname, e))?;
                            files.push(hname.clone());
                            let fid = files.len() as u32 - 1;
                            for t in &mut toks {
                                t.file = fid;
                            }
                            out.extend(process(&toks, &hname, macros, depth + 1, incs, files)?);
                            continue;
                        }
                        if let Some(p2) = inc_find(incs, &bare) {
                            path = p2;
                        }
                    }
                    out.extend(pp_file(&path, macros, depth + 1, incs, files)?);
                }
                Some(Tok::Punct("<")) => {
                    // tên = spelling các token đến '>' (lexer tách "stdio.h" = 3 token)
                    let mut name = String::new();
                    let mut k = 2;
                    while k < d.len() && d[k].tok != Tok::Punct(">") {
                        name.push_str(&spell(&d[k].tok));
                        k += 1;
                    }
                    // header nhúng thắng (libc stub của zcc); không có thì tra -I
                    match HEADERS
                        .iter()
                        .find(|(n, _)| *n == name && !no_stdinc(incs))
                        .map(|(_, s)| *s)
                    {
                        Some(src) => {
                            let hname = format!("<{}>", name);
                            let mut toks = lex(src).map_err(|e| format!("{}: {}", hname, e))?;
                            files.push(hname.clone());
                            let fid = files.len() as u32 - 1;
                            for t in &mut toks {
                                t.file = fid;
                            }
                            out.extend(process(&toks, &hname, macros, depth + 1, incs, files)?);
                        }
                        None => {
                            let p2 = inc_find(incs, &name).ok_or_else(|| {
                                err(file, lno, &format!("không có header nhúng <{}>", name))
                            })?;
                            out.extend(pp_file(&p2, macros, depth + 1, incs, files)?);
                        }
                    }
                }
                _ => return Err(err(file, lno, "#include cần \"file\"")),
                }
            }
            "error" => return Err(err(file, lno, &format!("#error {}", spell_seq(&d[1..])))),
            // EXT(gcc): #warning — báo rồi đi tiếp (không có trong C89)
            "warning" => eprintln!("{}", err(file, lno, &format!("#warning {}", spell_seq(&d[1..])))),
            "line" => {
                let ex = expand_seq(&d[1..], macros, &Vec::new(), file, delta)?;
                if let Some(Tok::Num(n, _)) = ex.first().map(|t| &t.tok) {
                    delta = n - (lno as i64 + 1); // dòng KẾ TIẾP directive mang số n
                }
            }
            "pragma" => {
                // #pragma push_macro("x") / pop_macro("x") — clang/gcc extension
                if let (Some(Tok::Ident(p)), Some(Tok::Str(b, _))) =
                    (d.get(1).map(|t| &t.tok), d.get(3).map(|t| &t.tok))
                {
                    let name = String::from_utf8_lossy(b).into_owned();
                    if p == "push_macro" {
                        saved.entry(name.clone()).or_default().push(macros.get(&name).cloned());
                    } else if p == "pop_macro" {
                        if let Some(m) = saved.get_mut(&name).and_then(|v| v.pop()) {
                            match m {
                                Some(m) => drop(macros.insert(name, m)),
                                None => drop(macros.remove(&name)),
                            }
                        }
                    }
                }
            }
            _ => return Err(err(file, lno, &format!("directive lạ #{}", kw))),
        }
    }
    if !conds.is_empty() {
        return Err(format!("{}: thiếu #endif", file));
    }
    Ok(out)
}

// ---- Macro expansion (rescan đệ quy; hide = stack tên đang mở để chặn vòng) ----

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
    delta: i64, // #line dịch số dòng báo ra (chỉ ảnh hưởng __LINE__)
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
        out.push(PTok { tok: Tok::Num(t.line as i64 + delta, NumK::I), ..t.clone() });
        return Ok(i + 1);
    }
    if name == "__FILE__" {
        out.push(PTok { tok: Tok::Str(file.as_bytes().to_vec(), false), ..t.clone() });
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
            // Tên macro hàm không có '(' theo sau: là ident thường.
            if !matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("("))) {
                out.push(t.clone());
                return Ok(i + 1);
            }
            let (mut args, next) = collect_args(toks, i + 2, file, t.line)?;
            if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                args.clear(); // F() = 0 đối số
            }
            if *va {
                if args.len() < params.len() {
                    args.push(Vec::new()); // F(a) với (a, ...): __VA_ARGS__ rỗng
                }
                // arg dư gộp lại thành __VA_ARGS__ (nối lại dấu phẩy đã tách)
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
                    &format!("macro {} cần {} đối số, nhận {}", name, params.len(), args.len()),
                ));
            }
            let sub = substitute(body, params, &args, macros, hide, file, delta, t.line)?;
            // luật hideset chuẩn: HS kết quả = (HS(tên) ∩ HS(")" đóng)) ∪ {tên}
            let close = &toks[next - 1];
            let callhs: Vec<String> =
                t.hide.iter().filter(|h| close.hide.contains(h)).cloned().collect();
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
    // Rescan QUA RANH GIỚI (C89 6.8.3.4): kết quả expand đuôi là tên macro hàm
    // mà '(' nằm ở stream gốc phía sau → ghép lại và expand tiếp.
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

// Token thân macro mang line của CHỖ GỌI (đúng luật __LINE__), ws token đầu
// thừa kế chỗ gọi để stringize lồng nhau giữ khoảng cách hợp lý.
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

// Gom đối số từ sau '(': tách bởi ',' ở depth 0, ngoặc lồng giữ nguyên.
fn collect_args(
    toks: &[PTok],
    mut i: usize,
    file: &str,
    lno: u32,
) -> Result<(Vec<Vec<PTok>>, usize), String> {
    let (mut args, mut cur, mut depth) = (Vec::new(), Vec::new(), 0u32);
    loop {
        let t = toks.get(i).ok_or_else(|| err(file, lno, "thiếu ')' đóng đối số macro"))?;
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

// Thay tham số vào thân macro: #p = stringize arg THÔ, p cạnh ## = arg thô,
// p thường = arg đã expand đầy đủ; ## dán token cuối trái với token đầu phải.
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
                .ok_or_else(|| err(file, lno, "# phải đứng trước tham số macro"))?;
            out.push(PTok {
                tok: Tok::Str(stringize(&args[p]), false),
                bol: false,
                ws: t.ws,
                line: lno,
                file: t.file,
                hide: Vec::new(),
                raw: String::new(),
            });
            i += 2;
        } else if t.tok == Tok::Punct("##") {
            let r = body.get(i + 1).ok_or_else(|| err(file, lno, "## ở cuối thân macro"))?;
            // EXT(gcc): ", ## __VA_ARGS__" — arg rỗng thì XÓA dấu phẩy, có arg
            // thì giữ nguyên phẩy + args (không paste thật; rescan expand sau)
            if let Some(p) = param_of(Some(r), params) {
                if matches!(out.last().map(|x| &x.tok), Some(Tok::Punct(","))) {
                    if args[p].is_empty() {
                        out.pop();
                    } else {
                        out.extend(args[p].iter().cloned());
                    }
                    i += 2;
                    continue;
                }
            }
            let rhs = match param_of(Some(r), params) {
                Some(p) => args[p].clone(),
                None => vec![r.clone()],
            };
            let l = out.pop().ok_or_else(|| err(file, lno, "## ở đầu thân macro"))?;
            if rhs.is_empty() {
                out.push(l);
            } else {
                // spelling GỐC (raw) khi có — spell() từ giá trị làm rớt suffix
                // số (199506L → "199506", cdefs.h paste ra sai tên macro)
                let sp = |p: &PTok| {
                    if p.raw.is_empty() { spell(&p.tok) } else { p.raw.clone() }
                };
                let s = format!("{}{}", sp(&l), sp(&rhs[0]));
                let one = lex(&s)
                    .ok()
                    .and_then(|mut v| if v.len() == 1 { Some(v.remove(0).tok) } else { None })
                    .ok_or_else(|| {
                        err(file, lno, &format!("## tạo token không hợp lệ '{}'", s))
                    })?;
                out.push(PTok { tok: one, bol: false, ws: l.ws, line: lno, file: l.file, hide: Vec::new(), raw: String::new() });
                out.extend(rhs[1..].iter().cloned());
            }
            i += 2;
        } else if let Some(p) = param_of(Some(t), params) {
            let raw = matches!(body.get(i + 1).map(|n| &n.tok), Some(Tok::Punct("##")));
            let mut rep =
                if raw { args[p].clone() } else { expand_seq(&args[p], macros, hide, file, delta)? };
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

// ---- Spelling (cho # stringize, ## paste, #error) ----

pub fn spell(t: &Tok) -> String {
    match t {
        Tok::Num(n, NumK::U | NumK::UL) => (*n as u64).to_string(),
        Tok::Num(n, _) => n.to_string(),
        Tok::FNum(v, _) => format!("{v:?}"),
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

// Ghép spelling, whitespace bất kỳ giữa 2 token → đúng 1 space (luật # C89).
fn spell_seq(ts: &[PTok]) -> String {
    let mut s = String::new();
    for (k, t) in ts.iter().enumerate() {
        if k > 0 && t.ws {
            s.push(' ');
        }
        if t.raw.is_empty() {
            s.push_str(&spell(&t.tok));
        } else {
            s.push_str(&t.raw); // spelling gốc: 0xff, 'a', "s\n"
        }
    }
    s
}

fn stringize(arg: &[PTok]) -> Vec<u8> {
    spell_seq(arg).into_bytes()
}

// ---- #if constexpr: defined trước, expand, ident sót → 0, rồi eval i64 ----

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
    // macro có thể expand RA defined(...)/__has_include (SDK pthread.h) → resolve lần 2
    let ex = if_resolve(&ex, macros, file, lno, incs)?;
    let ts: Vec<Tok> = ex
        .into_iter()
        .map(|t| match t.tok {
            Tok::Ident(_) => Tok::Num(0, NumK::I), // ident không phải macro → 0 (luật C)
            tok => tok,
        })
        .collect();
    let mut p = 0;
    let v = ternary(&ts, &mut p).map_err(|e| err(file, lno, &e))?;
    if p != ts.len() {
        return Err(err(file, lno, "token thừa trong biểu thức #if"));
    }
    Ok(v.0 != 0)
}

// resolve defined(X) / __has_include(...) thành 0|1 trong biểu thức #if
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
                ident_of(d.get(at)).ok_or_else(|| err(file, lno, "defined cần tên macro"))?;
            // EXT(clang): __has_include + họ __has_* là operator builtin — "defined" = 1
            let def = macros.contains_key(name)
                || name == "__has_include"
                || crate::ext::has_operator_zero(name);
            pre.push(synth(Tok::Num(def as i64, NumK::I)));
            i = at + 1;
            if paren {
                if !matches!(d.get(i).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                    return Err(err(file, lno, "defined( thiếu ')'"));
                }
                i += 1;
            }
        } else if matches!(&d[i].tok, Tok::Ident(n) if crate::ext::has_operator_zero(n))
            && matches!(d.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("(")))
        {
            // EXT(clang): __has_feature/extension/builtin/attribute(...) → 0,
            // nuốt ngoặc cân bằng (arg chỉ là ident nhưng cứ đề phòng lồng nhau)
            i += 2;
            let mut depth = 1;
            while depth > 0 {
                match d.get(i).map(|t| &t.tok) {
                    Some(Tok::Punct("(")) => depth += 1,
                    Some(Tok::Punct(")")) => depth -= 1,
                    None => return Err(err(file, lno, "__has_*( thiếu ')'")),
                    _ => {}
                }
                i += 1;
            }
            pre.push(synth(Tok::Num(0, NumK::I)));
        } else if matches!(&d[i].tok, Tok::Ident(n) if n == "__has_include" || n == "__has_include_next")
        {
            // EXT(clang): __has_include(<h> | "h") — eval TRƯỚC expand (tên header
            // không phải macro); __has_include_next luôn 0 (không có include stack)
            let next = matches!(&d[i].tok, Tok::Ident(n) if n == "__has_include_next");
            if !matches!(d.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("("))) {
                return Err(err(file, lno, "__has_include cần '('"));
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
                _ => return Err(err(file, lno, "__has_include cần <h> hoặc \"h\"")),
            };
            if !matches!(d.get(i).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                return Err(err(file, lno, "__has_include( thiếu ')'"));
            }
            i += 1;
            let found = !next
                && ((!no_stdinc(incs) && HEADERS.iter().any(|(n, _)| *n == name))
                    || inc_find(incs, &name).is_some());
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

// Giá trị #if = (bit, unsigned): C89 3.8.1 — mọi số học trong #if tính bằng
// long/unsigned long, UAC thu về 1 bit "có toán hạng unsigned không". Bỏ bit
// này (eval thuần i64) là sai / % >> so-sánh khi trộn dấu: -1L < 0xFF..FFUL
// phải bằng 0. Bug bắt bởi tests/cpp.sh (họ F, vét cạn #if).
type PV = (i64, bool);

fn ternary(ts: &[Tok], p: &mut usize) -> Result<PV, String> {
    let c = binlv(ts, p, 0)?;
    if !eat(ts, p, "?") {
        return Ok(c);
    }
    let a = ternary(ts, p)?;
    if !eat(ts, p, ":") {
        return Err("thiếu ':' của '?'".into());
    }
    let b = ternary(ts, p)?;
    // kiểu chung của ?: là UAC hai nhánh, giá trị lấy theo nhánh chọn
    Ok((if c.0 != 0 { a.0 } else { b.0 }, a.1 | b.1))
}

// Bậc ưu tiên nhị phân C, thấp → cao.
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
        let u = l.1 | r.1; // UAC: một bên unsigned → phép toán unsigned
        let b = |x: bool| (x as i64, false);
        // wrapping: overflow là UB bên C, đừng panic bên Rust
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
            // shift: kiểu kết quả = kiểu VẾ TRÁI (3.3.7 — không UAC)
            "<<" => (l.0.wrapping_shl(r.0 as u32), l.1),
            ">>" if l.1 => (((l.0 as u64).wrapping_shr(r.0 as u32)) as i64, true),
            ">>" => (l.0.wrapping_shr(r.0 as u32), l.1),
            "+" => (l.0.wrapping_add(r.0), u),
            "-" => (l.0.wrapping_sub(r.0), u),
            "*" => (l.0.wrapping_mul(r.0), u),
            // chia 0 trong nhánh chết của && || phải vô hại (eval eager) → cho 0
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
            return Err("thiếu ')' trong #if".into());
        }
        Ok(v)
    } else if let Some(&Tok::Num(n, k)) = ts.get(*p) {
        *p += 1;
        Ok((n, matches!(k, NumK::U | NumK::UL)))
    } else {
        Err("biểu thức #if hỏng".into())
    }
}
