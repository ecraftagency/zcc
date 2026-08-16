// Lexer: nguồn C → Vec<PTok>. Chỉ nhận token mà test case hiện có đòi hỏi.
// PTok = Tok + metadata cho preprocessor: bol (token đầu tiên của logical line,
// để nhận directive '#'), ws (có whitespace/comment ngay trước, để stringize),
// line (số dòng vật lý, cho __LINE__ và báo lỗi).

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Num(i64),
    Ident(String),
    Punct(&'static str),
    Str(Vec<u8>), // bytes đã xử lý escape, chưa gồm NUL cuối
}

#[derive(Clone, Debug)]
pub struct PTok {
    pub tok: Tok,
    pub bol: bool,
    pub ws: bool,
    pub line: u32,
}

// Punct nhiều ký tự đứng trước để match trước ("<=" trước "<").
const PUNCTS: [&str; 36] = [
    "...", "->", "==", "!=", "<=", ">=", "<<", ">>", "&&", "||", "##", "<", ">", "=", "+", "-",
    "*", "/", "%", "(", ")", "{", "}", ";", ",", "&", "[", "]", ".", "!", "|", "^", "~", "?",
    ":", "#",
];

pub fn lex(src: &str) -> Result<Vec<PTok>, String> {
    let b = src.as_bytes();
    let (mut i, mut toks) = (0, Vec::new());
    let (mut line, mut bol, mut ws) = (1u32, true, false);
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            bol = true;
            ws = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            ws = true;
            i += 1;
            continue;
        }
        // Nối dòng bằng backslash-newline (phase 2): không reset bol —
        // dòng logic tiếp tục, nhưng vẫn đếm dòng vật lý cho __LINE__.
        if c == b'\\' && b.get(i + 1) == Some(&b'\n') {
            line += 1;
            ws = true;
            i += 2;
            continue;
        }
        // Comment = whitespace; newline BÊN TRONG comment không ngắt dòng logic
        // (phase 3 thay comment bằng 1 space trước khi nhận directive).
        if src[i..].starts_with("/*") {
            let end = src[i + 2..].find("*/").ok_or("comment không đóng")?;
            line += b[i..i + end + 4].iter().filter(|&&x| x == b'\n').count() as u32;
            ws = true;
            i += end + 4;
            continue;
        }
        let tok = if c.is_ascii_digit() {
            let s = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            Tok::Num(src[s..i].parse().map_err(|e| format!("{e}"))?)
        } else if c == b'"' {
            i += 1;
            let mut bytes = Vec::new();
            loop {
                match *b.get(i).ok_or("string không đóng")? {
                    b'"' => break,
                    b'\\' => {
                        bytes.push(match b.get(i + 1) {
                            Some(b'n') => 10,
                            Some(b't') => 9,
                            Some(b'0') => 0,
                            Some(&e @ (b'\\' | b'"' | b'\'')) => e,
                            e => return Err(format!("escape lạ {:?}", e)),
                        });
                        i += 2;
                    }
                    e => {
                        bytes.push(e);
                        i += 1;
                    }
                }
            }
            i += 1;
            Tok::Str(bytes)
        } else if c == b'_' || c.is_ascii_alphabetic() {
            let s = i;
            while i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            Tok::Ident(src[s..i].to_string())
        } else {
            match PUNCTS.iter().find(|p| src[i..].starts_with(**p)) {
                Some(p) => {
                    i += p.len();
                    Tok::Punct(p)
                }
                None => return Err(format!("ký tự lạ '{}'", c as char)),
            }
        };
        toks.push(PTok { tok, bol, ws, line });
        bol = false;
        ws = false;
    }
    Ok(toks)
}
