// Preprocessor C89 (M7): token-based, chạy giữa lexer và parser.
// Vào: đường dẫn file; ra: Vec<Tok> sạch directive cho parser.
// Hỗ trợ (do test đòi): #include "..." (tương đối thư mục file đang xử lý),
// #define object/function (# stringize, ## paste, rescan đệ quy có chặn vòng
// bằng hide-stack), #undef, #if/#ifdef/#ifndef/#elif/#else/#endif (constexpr
// đủ toán tử C + defined), #error; #line/#pragma nuốt im lặng.
// Macro sẵn: __LINE__ __FILE__ __STDC__. Chưa có: #include <...> (cần search
// path hệ thống — để dành khi test/M8 đòi), macro variadic (C89 không có).

use crate::lexer::{lex, PTok, Tok};
use std::collections::HashMap;
use std::fs;

enum Macro {
    Obj(Vec<PTok>),
    Fun(Vec<String>, Vec<PTok>),
}
type Macros = HashMap<String, Macro>;

pub fn preprocess(path: &str) -> Result<Vec<Tok>, String> {
    let mut macros = Macros::new();
    macros.insert("__STDC__".into(), Macro::Obj(vec![synth(Tok::Num(1))]));
    Ok(pp_file(path, &mut macros, 0)?.into_iter().map(|t| t.tok).collect())
}

fn synth(tok: Tok) -> PTok {
    PTok { tok, bol: false, ws: true, line: 0 }
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

fn pp_file(path: &str, macros: &mut Macros, depth: u32) -> Result<Vec<PTok>, String> {
    if depth > 32 {
        return Err(format!("{}: include lồng quá sâu", path));
    }
    let src = fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    let toks = lex(&src).map_err(|e| format!("{}: {}", path, e))?;
    process(&toks, path, macros, depth)
}

// Trạng thái một tầng #if: parent = tầng ngoài có active không; taken = đã có
// nhánh nào trúng chưa (chặn #elif sau đó); active = nhánh hiện tại đang phát.
struct Cond {
    parent: bool,
    taken: bool,
    active: bool,
    in_else: bool,
}

fn process(toks: &[PTok], file: &str, macros: &mut Macros, depth: u32) -> Result<Vec<PTok>, String> {
    let (mut out, mut i) = (Vec::new(), 0);
    let mut conds: Vec<Cond> = Vec::new();
    while i < toks.len() {
        let active = conds.last().map_or(true, |c| c.active);
        let t = &toks[i];
        if !(t.bol && t.tok == Tok::Punct("#")) {
            if active {
                i = expand_at(toks, i, macros, &Vec::new(), file, &mut out)?;
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
                    macros.contains_key(name) == (kw == "ifdef")
                };
                conds.push(Cond { parent: active, taken: v, active: v, in_else: false });
            }
            "if" => {
                let v = active && eval_if(&d[1..], macros, file, lno)?;
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
                let v = need && eval_if(&d[1..], macros, file, lno)?;
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
                    let (mut params, mut k) = (Vec::new(), 3);
                    if matches!(d.get(k).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                        k += 1;
                    } else {
                        loop {
                            let p = ident_of(d.get(k))
                                .ok_or_else(|| err(file, lno, "tham số macro phải là ident"))?;
                            params.push(p.to_string());
                            k += 1;
                            match d.get(k).map(|t| &t.tok) {
                                Some(Tok::Punct(",")) => k += 1,
                                Some(Tok::Punct(")")) => {
                                    k += 1;
                                    break;
                                }
                                _ => return Err(err(file, lno, "thiếu ')' trong #define")),
                            }
                        }
                    }
                    Macro::Fun(params, d[k..].to_vec())
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
            "include" => match d.get(1).map(|t| &t.tok) {
                Some(Tok::Str(b)) => {
                    let name = String::from_utf8_lossy(b).into_owned();
                    let path = if name.starts_with('/') {
                        name
                    } else {
                        match file.rsplit_once('/') {
                            Some((dir, _)) => format!("{}/{}", dir, name),
                            None => name,
                        }
                    };
                    out.extend(pp_file(&path, macros, depth + 1)?);
                }
                Some(Tok::Punct("<")) => {
                    return Err(err(file, lno, "#include <...>: chưa có search path hệ thống"));
                }
                _ => return Err(err(file, lno, "#include cần \"file\"")),
            },
            "error" => return Err(err(file, lno, &format!("#error {}", spell_seq(&d[1..])))),
            "line" | "pragma" => {}
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
) -> Result<Vec<PTok>, String> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < toks.len() {
        i = expand_at(toks, i, macros, hide, file, &mut out)?;
    }
    Ok(out)
}

fn expand_at(
    toks: &[PTok],
    i: usize,
    macros: &Macros,
    hide: &Vec<String>,
    file: &str,
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
        out.push(PTok { tok: Tok::Num(t.line as i64), ..t.clone() });
        return Ok(i + 1);
    }
    if name == "__FILE__" {
        out.push(PTok { tok: Tok::Str(file.as_bytes().to_vec()), ..t.clone() });
        return Ok(i + 1);
    }
    if hide.contains(&name) {
        out.push(t.clone());
        return Ok(i + 1);
    }
    match macros.get(&name) {
        None => {
            out.push(t.clone());
            Ok(i + 1)
        }
        Some(Macro::Obj(body)) => {
            let body = retag(body, t);
            let mut h = hide.clone();
            h.push(name);
            out.extend(expand_seq(&body, macros, &h, file)?);
            Ok(i + 1)
        }
        Some(Macro::Fun(params, body)) => {
            // Tên macro hàm không có '(' theo sau: là ident thường.
            if !matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("("))) {
                out.push(t.clone());
                return Ok(i + 1);
            }
            let (mut args, next) = collect_args(toks, i + 2, file, t.line)?;
            if params.is_empty() && args.len() == 1 && args[0].is_empty() {
                args.clear(); // F() = 0 đối số
            }
            if args.len() != params.len() {
                return Err(err(
                    file,
                    t.line,
                    &format!("macro {} cần {} đối số, nhận {}", name, params.len(), args.len()),
                ));
            }
            let sub = substitute(body, params, &args, macros, hide, file, t.line)?;
            let sub = retag(&sub, t);
            let mut h = hide.clone();
            h.push(name);
            out.extend(expand_seq(&sub, macros, &h, file)?);
            Ok(next)
        }
    }
}

// Token thân macro mang line của CHỖ GỌI (đúng luật __LINE__), ws token đầu
// thừa kế chỗ gọi để stringize lồng nhau giữ khoảng cách hợp lý.
fn retag(body: &[PTok], at: &PTok) -> Vec<PTok> {
    body.iter()
        .enumerate()
        .map(|(k, b)| PTok {
            tok: b.tok.clone(),
            bol: false,
            ws: if k == 0 { at.ws } else { b.ws },
            line: at.line,
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
    lno: u32,
) -> Result<Vec<PTok>, String> {
    let (mut out, mut i) = (Vec::<PTok>::new(), 0);
    while i < body.len() {
        let t = &body[i];
        if t.tok == Tok::Punct("#") {
            let p = param_of(body.get(i + 1), params)
                .ok_or_else(|| err(file, lno, "# phải đứng trước tham số macro"))?;
            out.push(PTok { tok: Tok::Str(stringize(&args[p])), bol: false, ws: t.ws, line: lno });
            i += 2;
        } else if t.tok == Tok::Punct("##") {
            let r = body.get(i + 1).ok_or_else(|| err(file, lno, "## ở cuối thân macro"))?;
            let rhs = match param_of(Some(r), params) {
                Some(p) => args[p].clone(),
                None => vec![r.clone()],
            };
            let l = out.pop().ok_or_else(|| err(file, lno, "## ở đầu thân macro"))?;
            if rhs.is_empty() {
                out.push(l);
            } else {
                let s = format!("{}{}", spell(&l.tok), spell(&rhs[0].tok));
                let one = lex(&s)
                    .ok()
                    .and_then(|mut v| if v.len() == 1 { Some(v.remove(0).tok) } else { None })
                    .ok_or_else(|| {
                        err(file, lno, &format!("## tạo token không hợp lệ '{}'", s))
                    })?;
                out.push(PTok { tok: one, bol: false, ws: l.ws, line: lno });
                out.extend(rhs[1..].iter().cloned());
            }
            i += 2;
        } else if let Some(p) = param_of(Some(t), params) {
            let raw = matches!(body.get(i + 1).map(|n| &n.tok), Some(Tok::Punct("##")));
            let mut rep =
                if raw { args[p].clone() } else { expand_seq(&args[p], macros, hide, file)? };
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

fn spell(t: &Tok) -> String {
    match t {
        Tok::Num(n) => n.to_string(),
        Tok::Ident(s) => s.clone(),
        Tok::Punct(p) => p.to_string(),
        Tok::Str(b) => {
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
        s.push_str(&spell(&t.tok));
    }
    s
}

fn stringize(arg: &[PTok]) -> Vec<u8> {
    spell_seq(arg).into_bytes()
}

// ---- #if constexpr: defined trước, expand, ident sót → 0, rồi eval i64 ----

fn eval_if(d: &[PTok], macros: &Macros, file: &str, lno: u32) -> Result<bool, String> {
    let (mut pre, mut i) = (Vec::new(), 0);
    while i < d.len() {
        if matches!(&d[i].tok, Tok::Ident(n) if n == "defined") {
            let paren = matches!(d.get(i + 1).map(|t| &t.tok), Some(Tok::Punct("(")));
            let at = if paren { i + 2 } else { i + 1 };
            let name =
                ident_of(d.get(at)).ok_or_else(|| err(file, lno, "defined cần tên macro"))?;
            pre.push(synth(Tok::Num(macros.contains_key(name) as i64)));
            i = at + 1;
            if paren {
                if !matches!(d.get(i).map(|t| &t.tok), Some(Tok::Punct(")"))) {
                    return Err(err(file, lno, "defined( thiếu ')'"));
                }
                i += 1;
            }
        } else {
            pre.push(d[i].clone());
            i += 1;
        }
    }
    let ex = expand_seq(&pre, macros, &Vec::new(), file)?;
    let ts: Vec<Tok> = ex
        .into_iter()
        .map(|t| match t.tok {
            Tok::Ident(_) => Tok::Num(0), // ident không phải macro → 0 (luật C)
            tok => tok,
        })
        .collect();
    let mut p = 0;
    let v = ternary(&ts, &mut p).map_err(|e| err(file, lno, &e))?;
    if p != ts.len() {
        return Err(err(file, lno, "token thừa trong biểu thức #if"));
    }
    Ok(v != 0)
}

fn eat(ts: &[Tok], p: &mut usize, op: &str) -> bool {
    if matches!(ts.get(*p), Some(Tok::Punct(x)) if *x == op) {
        *p += 1;
        true
    } else {
        false
    }
}

fn ternary(ts: &[Tok], p: &mut usize) -> Result<i64, String> {
    let c = binlv(ts, p, 0)?;
    if !eat(ts, p, "?") {
        return Ok(c);
    }
    let a = ternary(ts, p)?;
    if !eat(ts, p, ":") {
        return Err("thiếu ':' của '?'".into());
    }
    let b = ternary(ts, p)?;
    Ok(if c != 0 { a } else { b })
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

fn binlv(ts: &[Tok], p: &mut usize, lv: usize) -> Result<i64, String> {
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
        // wrapping: overflow là UB bên C, đừng panic bên Rust
        l = match op {
            "||" => (l != 0 || r != 0) as i64,
            "&&" => (l != 0 && r != 0) as i64,
            "|" => l | r,
            "^" => l ^ r,
            "&" => l & r,
            "==" => (l == r) as i64,
            "!=" => (l != r) as i64,
            "<" => (l < r) as i64,
            ">" => (l > r) as i64,
            "<=" => (l <= r) as i64,
            ">=" => (l >= r) as i64,
            "<<" => l.wrapping_shl(r as u32),
            ">>" => l.wrapping_shr(r as u32),
            "+" => l.wrapping_add(r),
            "-" => l.wrapping_sub(r),
            "*" => l.wrapping_mul(r),
            "/" | "%" if r == 0 => return Err("chia 0 trong #if".into()),
            "/" => l.wrapping_div(r),
            "%" => l.wrapping_rem(r),
            _ => unreachable!(),
        };
    }
}

fn unary(ts: &[Tok], p: &mut usize) -> Result<i64, String> {
    if eat(ts, p, "!") {
        Ok((unary(ts, p)? == 0) as i64)
    } else if eat(ts, p, "~") {
        Ok(!unary(ts, p)?)
    } else if eat(ts, p, "-") {
        Ok(unary(ts, p)?.wrapping_neg())
    } else if eat(ts, p, "+") {
        unary(ts, p)
    } else if eat(ts, p, "(") {
        let v = ternary(ts, p)?;
        if !eat(ts, p, ")") {
            return Err("thiếu ')' trong #if".into());
        }
        Ok(v)
    } else if let Some(Tok::Num(n)) = ts.get(*p) {
        *p += 1;
        Ok(*n)
    } else {
        Err("biểu thức #if hỏng".into())
    }
}
