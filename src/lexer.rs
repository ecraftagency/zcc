// Lexer: nguồn C → Vec<PTok>. PTok = Tok + metadata cho preprocessor: bol
// (token đầu logical line, nhận directive '#'), ws (có whitespace/comment ngay
// trước, cho stringize), line (dòng vật lý, cho __LINE__ và báo lỗi).

// Kiểu của hằng nguyên theo C89 (suffix + độ lớn + cơ số); LP64 nên chỉ cần
// phân biệt signed/unsigned × int/long.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumK {
    I,
    U,
    L,
    UL,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Num(i64, NumK),
    FNum(f64, bool), // hằng thực; bool = double (không suffix f/F)
    Ident(String),
    Punct(&'static str),
    Str(Vec<u8>, bool), // (bytes đã xử lý escape chưa gồm NUL cuối, wide L"..")
}

#[derive(Clone, Debug)]
pub struct PTok {
    pub tok: Tok,
    pub bol: bool,
    pub ws: bool,
    pub line: u32,
    pub file: u32, // id vào bảng file của preprocess (0 = file gốc)
    pub hide: Vec<String>, // hideset: macro đã expand ra token này (chặn expand lại)
    pub raw: String, // spelling gốc (Num/Str/Char) cho # stringize; rỗng = spell từ giá trị
}

// Punct dài đứng trước để match trước ("<<=" trước "<<" trước "<").
const PUNCTS: [&str; 48] = [
    "...", "<<=", ">>=", "->", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "==",
    "!=", "<=", ">=", "<<", ">>", "&&", "||", "##", "<", ">", "=", "+", "-", "*", "/", "%", "(",
    ")", "{", "}", ";", ",", "&", "[", "]", ".", "!", "|", "^", "~", "?", ":", "#",
];

// Escape C89 đầy đủ: \n \t \r \a \b \f \v \\ \' \" \? \ooo \xhh.
// Vào: i trỏ ngay SAU dấu '\', ra: byte giá trị + i đã nhảy qua escape.
fn escape(b: &[u8], i: &mut usize) -> Result<u32, String> {
    let c = *b.get(*i).ok_or("escape cụt")?;
    *i += 1;
    Ok(match c {
        b'n' => 10,
        b't' => 9,
        b'r' => 13,
        b'a' => 7,
        b'b' => 8,
        b'f' => 12,
        b'v' => 11,
        b'\\' | b'\'' | b'"' | b'?' => c as u32,
        b'x' => {
            let mut v = 0u32;
            while let Some(d) = b.get(*i).and_then(|c| (*c as char).to_digit(16)) {
                v = v * 16 + d;
                *i += 1;
            }
            v
        }
        b'0'..=b'7' => {
            let mut v = (c - b'0') as u32;
            for _ in 0..2 {
                match b.get(*i) {
                    Some(&d @ b'0'..=b'7') => {
                        v = v * 8 + (d - b'0') as u32;
                        *i += 1;
                    }
                    _ => break,
                }
            }
            v
        }
        _ => return Err(format!("escape lạ '\\{}'", c as char)),
    })
}

// Hằng số bắt đầu tại i (digit, hoặc '.' + digit). Trả token + i mới.
fn number(src: &str, b: &[u8], i: &mut usize) -> Result<Tok, String> {
    let s = *i;
    if src[s..].starts_with("0b") || src[s..].starts_with("0B") {
        // GNU: hằng nhị phân 0b1010
        *i += 2;
        let d = *i;
        while matches!(b.get(*i), Some(b'0' | b'1')) {
            *i += 1;
        }
        let v = u64::from_str_radix(&src[d..*i], 2).map_err(|e| format!("{e}"))?;
        return Ok(Tok::Num(v as i64, suffix_kind(b, i, v, true)?));
    }
    if src[s..].starts_with("0x") || src[s..].starts_with("0X") {
        *i += 2;
        let d = *i;
        while b.get(*i).is_some_and(|c| c.is_ascii_hexdigit()) {
            *i += 1;
        }
        let v = u64::from_str_radix(&src[d..*i], 16).map_err(|e| format!("{e}"))?;
        return Ok(Tok::Num(v as i64, suffix_kind(b, i, v, true)?));
    }
    while b.get(*i).is_some_and(|c| c.is_ascii_digit()) {
        *i += 1;
    }
    // thực: có '.' hoặc mũ e/E (hằng hex-float không tồn tại trong C89)
    let dot = b.get(*i) == Some(&b'.');
    let exp = matches!(b.get(*i), Some(b'e' | b'E'));
    if dot || exp {
        if dot {
            *i += 1;
            while b.get(*i).is_some_and(|c| c.is_ascii_digit()) {
                *i += 1;
            }
        }
        if matches!(b.get(*i), Some(b'e' | b'E')) {
            *i += 1;
            if matches!(b.get(*i), Some(b'+' | b'-')) {
                *i += 1;
            }
            while b.get(*i).is_some_and(|c| c.is_ascii_digit()) {
                *i += 1;
            }
        }
        let v: f64 = src[s..*i].parse().map_err(|e| format!("{e}"))?;
        let mut dbl = true;
        if matches!(b.get(*i), Some(b'f' | b'F')) {
            *i += 1;
            dbl = false;
        } else if matches!(b.get(*i), Some(b'l' | b'L')) {
            *i += 1; // long double = double trên arm64 Darwin
        }
        return Ok(Tok::FNum(v, dbl));
    }
    let octal = b[s] == b'0' && *i > s + 1;
    let v = u64::from_str_radix(&src[s..*i], if octal { 8 } else { 10 })
        .map_err(|e| format!("{e}"))?;
    Ok(Tok::Num(v as i64, suffix_kind(b, i, v, octal)?))
}

// Nuốt suffix u/U/l/L rồi chọn kiểu C89: decimal không suffix đi int→long;
// octal/hex chen thêm unsigned (int→uint→long→ulong).
fn suffix_kind(b: &[u8], i: &mut usize, v: u64, oct_hex: bool) -> Result<NumK, String> {
    let (mut u, mut l) = (false, false);
    loop {
        match b.get(*i) {
            Some(b'u' | b'U') if !u => u = true,
            Some(b'l' | b'L') if !l => {
                l = true;
                // "ll"/"LL" (long long = long trên LP64): nuốt chữ l thứ hai
                if matches!(b.get(*i + 1), Some(b'l' | b'L')) {
                    *i += 1;
                }
            }
            _ => break,
        }
        *i += 1;
    }
    Ok(match (u, l) {
        (true, true) => NumK::UL,
        (true, false) => {
            if v <= u32::MAX as u64 {
                NumK::U
            } else {
                NumK::UL
            }
        }
        (false, true) => {
            if v <= i64::MAX as u64 {
                NumK::L
            } else {
                NumK::UL
            }
        }
        (false, false) => {
            if v <= i32::MAX as u64 {
                NumK::I
            } else if oct_hex && v <= u32::MAX as u64 {
                NumK::U
            } else if v <= i64::MAX as u64 {
                NumK::L
            } else {
                NumK::UL
            }
        }
    })
}

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
        // Nối dòng backslash-newline (phase 2): không reset bol — dòng logic
        // tiếp tục, nhưng vẫn đếm dòng vật lý cho __LINE__.
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
        // "//" (C99/extension, clang chấp nhận cả trong -std=c89): đến hết dòng
        if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            ws = true;
            continue;
        }
        let tok_start = i;
        // wide literal L'x' / L"s": wchar = int (LP64 Darwin)
        let wide = c == b'L' && matches!(b.get(i + 1), Some(b'\'' | b'"'));
        let c = if wide {
            i += 1;
            b[i]
        } else {
            c
        };
        let tok = if c.is_ascii_digit()
            || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            number(src, b, &mut i)?
        } else if c == b'\'' {
            i += 1;
            // multi-char constant 'ab' hợp lệ C89 (3.1.3.4, giá trị impl-def) —
            // khoá theo clang: dồn byte về phía thấp, ký tự đầu ở byte cao
            let (mut v, mut n) = (0i64, 0);
            loop {
                let e = match *b.get(i).ok_or("hằng ký tự không đóng")? {
                    b'\'' => break,
                    b'\\' => {
                        i += 1;
                        let e = escape(b, &mut i)?;
                        if wide { e as i64 } else { e as u8 as i8 as i64 } // char signed trên Darwin
                    }
                    e => {
                        i += 1;
                        e as i64
                    }
                };
                v = if n == 0 { e } else { (v << 8) | (e & 0xff) };
                n += 1;
            }
            if n == 0 {
                return Err("hằng ký tự rỗng".into());
            }
            i += 1;
            Tok::Num(v, NumK::I)
        } else if c == b'"' {
            i += 1;
            let mut bytes = Vec::new();
            loop {
                match *b.get(i).ok_or("string không đóng")? {
                    b'"' => break,
                    b'\\' => {
                        i += 1;
                        // phase 2 cả TRONG string: \<newline> nối dòng, không ra byte
                        if b.get(i) == Some(&b'\n') {
                            line += 1;
                            i += 1;
                        } else {
                            bytes.push(escape(b, &mut i)? as u8);
                        }
                    }
                    e => {
                        bytes.push(e);
                        i += 1;
                    }
                }
            }
            i += 1;
            Tok::Str(bytes, wide)
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
        let raw = match tok {
            Tok::Num(..) | Tok::FNum(..) | Tok::Str(..) => src[tok_start..i].to_string(),
            _ => String::new(),
        };
        toks.push(PTok { tok, bol, ws, line, file: 0, hide: Vec::new(), raw });
        bol = false;
        ws = false;
    }
    Ok(toks)
}
