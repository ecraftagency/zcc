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
    FNum(f64, u8), // hằng thực; 0 = float (f/F), 1 = double, 2 = long double (l/L)
    // C99 6.4.4.2 / EXT(gcc): hằng ẢO `1.0i` `1.0iF` — phép nhúng ℝ→ℂ (b ↦ 0+bi);
    // giá trị + elem-kind (như FNum). Parser dựng temp complex {re=0, im=v}.
    INum(f64, u8),
    Ident(String),
    Punct(&'static str),
    // narrow bytes (escape đã xử lý, chưa gồm NUL) + (WIDTH byte, wide-codepoint
    // seq). WIDTH: 1 narrow/u8, 2 char16 u"", 4 wchar L""/char32 U"". cps tách
    // ký tự NGUỒN multibyte (→1 codepoint) khỏi ESCAPE (→1 giá trị đơn), vì khi
    // chuỗi thành wide, "あ"→0x3042 nhưng "\343\201\202"→3 phần tử (C99 5.1.1.2).
    Str(Vec<u8>, (u8, Vec<u32>)),
}

#[derive(Clone, Debug)]
pub struct PTok {
    pub tok: Tok,
    pub bol: bool,
    pub ws: bool,
    pub line: u32,
    pub file: u32,         // id vào bảng file của preprocess (0 = file gốc)
    pub hide: Vec<String>, // hideset: macro đã expand ra token này (chặn expand lại)
    pub raw: String,       // spelling gốc (Num/Str/Char) cho # stringize; rỗng = spell từ giá trị
}

// C99 6.4.6: digraphs — dài trước ngắn ("%:%:" trước "%:").
const DIGRAPHS: [(&str, &str); 6] = [
    ("%:%:", "##"),
    ("%:", "#"),
    ("<:", "["),
    (":>", "]"),
    ("<%", "{"),
    ("%>", "}"),
];

// Punct dài đứng trước để match trước ("<<=" trước "<<" trước "<").
const PUNCTS: [&str; 48] = [
    "...", "<<=", ">>=", "->", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "==",
    "!=", "<=", ">=", "<<", ">>", "&&", "||", "##", "<", ">", "=", "+", "-", "*", "/", "%", "(",
    ")", "{", "}", ";", ",", "&", "[", "]", ".", "!", "|", "^", "~", "?", ":", "#",
];

// Escape C89 đầy đủ: \n \t \r \a \b \f \v \\ \' \" \? \ooo \xhh.
// Vào: i trỏ ngay SAU dấu '\', ra: byte giá trị + i đã nhảy qua escape.
// Decode 1 scalar UTF-8 tại b[i] → (codepoint, số byte). Byte lỗi → (byte, 1).
// Wide char L'Ä' = 1 multibyte source char → 1 wchar (6.4.4.4); narrow giữ byte.
fn utf8_cp(b: &[u8], i: usize) -> (u32, usize) {
    let c = b[i];
    let (n, mut cp) = match c {
        0x00..=0x7f => return (c as u32, 1),
        0xc0..=0xdf => (2, (c & 0x1f) as u32),
        0xe0..=0xef => (3, (c & 0x0f) as u32),
        0xf0..=0xf7 => (4, (c & 0x07) as u32),
        _ => return (c as u32, 1),
    };
    for k in 1..n {
        match b.get(i + k) {
            Some(&d @ 0x80..=0xbf) => cp = (cp << 6) | (d & 0x3f) as u32,
            _ => return (c as u32, 1), // dở dang → coi byte đầu là 1 char
        }
    }
    (cp, n)
}

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
        b'e' => 27, // EXT(gcc): \e = ESC (chibicc test/string.c đòi)
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
        // escape không định nghĩa: C89 3.1.3.4 để UB — theo gcc/clang cho
        // identity ('\j' == 'j'; chibicc test/string.c assert đúng điều này)
        _ => c as u32,
    })
}

// Hằng số bắt đầu tại i (digit, hoặc '.' + digit). Trả token + i mới.
// hậu tố hằng thực: fFlL (kind) + iIjJ (ảo) theo THỨ TỰ BẤT KỲ (gcc: 1.0if == 1.0fi)
fn fsuffix(b: &[u8], i: &mut usize) -> (u8, bool) {
    let (mut k, mut im) = (1u8, false);
    loop {
        match b.get(*i) {
            Some(b'f' | b'F') => k = 0,
            Some(b'l' | b'L') => k = 2, // long double: Darwin=double; ELF=binary128 tại biên ABI
            Some(b'i' | b'I' | b'j' | b'J') => im = true,
            _ => break,
        }
        *i += 1;
    }
    (k, im)
}

fn number(b: &[u8], i: &mut usize) -> Result<Tok, String> {
    let s = *i;
    // hằng số toàn ASCII → dựng &str từ subrange byte cho parse/from_str_radix
    let sv = move |x: usize, y: usize| std::str::from_utf8(&b[x..y]).unwrap();
    if b[s..].starts_with(b"0b") || b[s..].starts_with(b"0B") {
        // EXT(gcc): hằng nhị phân 0b1010
        *i += 2;
        let d = *i;
        while matches!(b.get(*i), Some(b'0' | b'1')) {
            *i += 1;
        }
        let v = u64::from_str_radix(sv(d, *i), 2).map_err(|e| format!("{e}"))?;
        return Ok(Tok::Num(v as i64, suffix_kind(b, i, v, true)?));
    }
    if b[s..].starts_with(b"0x") || b[s..].starts_with(b"0X") {
        *i += 2;
        let d = *i;
        while b.get(*i).is_some_and(|c| c.is_ascii_hexdigit()) {
            *i += 1;
        }
        // C99: hex float 0x1.8p3 = mantissa hex × 2^mũ — musl src/math
        // dùng dày đặc (bảng hằng exp/log/pow). Tích lũy nhân-16 chính xác
        // tới 2^53, đủ cho mọi literal ≤13 hex digit; sai lệch còn lại bị
        // differential vs cc bắt.
        if matches!(b.get(*i), Some(b'.' | b'p' | b'P')) {
            let mut v: f64 = 0.0;
            for &c in &b[d..*i] {
                v = v * 16.0 + (c as char).to_digit(16).unwrap() as f64;
            }
            if b.get(*i) == Some(&b'.') {
                *i += 1;
                let mut scale = 1.0f64 / 16.0;
                while let Some(c) = b.get(*i).filter(|c| c.is_ascii_hexdigit()) {
                    v += (*c as char).to_digit(16).unwrap() as f64 * scale;
                    scale /= 16.0;
                    *i += 1;
                }
            }
            if !matches!(b.get(*i), Some(b'p' | b'P')) {
                return Err("hex float cần mũ p".into());
            }
            *i += 1;
            let neg = match b.get(*i) {
                Some(b'-') => {
                    *i += 1;
                    true
                }
                Some(b'+') => {
                    *i += 1;
                    false
                }
                _ => false,
            };
            let e0 = *i;
            while b.get(*i).is_some_and(|c| c.is_ascii_digit()) {
                *i += 1;
            }
            let mut exp: i32 = sv(e0, *i).parse().map_err(|e| format!("{e}"))?;
            if neg {
                exp = -exp;
            }
            // powi(-1074) = 1/2^1074 = 1/inf = 0 — tách 2 bước để vào được
            // vùng subnormal (0x1p-1074 = DBL_TRUE_MIN của musl)
            let v = if exp >= -1022 {
                v * 2.0f64.powi(exp)
            } else {
                v * 2.0f64.powi(-1022) * 2.0f64.powi(exp + 1022)
            };
            let (k, im) = fsuffix(b, i);
            return Ok(if im { Tok::INum(v, k) } else { Tok::FNum(v, k) });
        }
        let v = u64::from_str_radix(sv(d, *i), 16).map_err(|e| format!("{e}"))?;
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
        let v: f64 = sv(s, *i).parse().map_err(|e| format!("{e}"))?;
        let (k, im) = fsuffix(b, i);
        return Ok(if im { Tok::INum(v, k) } else { Tok::FNum(v, k) });
    }
    let octal = b[s] == b'0' && *i > s + 1;
    // "08" là pp-number hợp lệ, chỉ ill-formed khi DÙNG làm hằng (pcre2.h:
    // `#define PCRE2_DATE 2025-08-27` không ai eval) — gcc lex qua, lỗi lúc
    // convert; zcc lex eager nên hạ xuống decimal thay vì chết cả TU
    let octal = octal && !b[s..*i].iter().any(|&c| c == b'8' || c == b'9');
    let v = u64::from_str_radix(sv(s, *i), if octal { 8 } else { 10 }).map_err(|e| format!("{e}"))?;
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

// Wrapper cho nguồn synthetic (macro builtin, token paste) — không chứa hằng
// ký tự ≥128 nên char signedness không chạm tới.
pub fn lex(src: &str) -> Result<Vec<PTok>, String> {
    lex_t(src.as_bytes(), false)
}
// char_uns: plain char UNSIGNED (Linux arm64, AAPCS64) — điểm chạm duy nhất
// là GIÁ TRỊ escape ≥ 128 trong hằng ký tự ('\377' = 255 vs -1 Darwin);
// nguồn file thật phải đi đường này với cờ theo target.
// Nguồn = &[u8] THÔ (không qua &str): string literal C là chuỗi BYTE của
// source char set (5.1.1.2 / 6.4.5) — byte ≥128 hoặc non-UTF8 (vd '\377' raw
// trong "…") phải giữ NGUYÊN; from_utf8_lossy mangle chúng thành U+FFFD (EF BF
// BD) → phình chuỗi (bug 20000227-1). Định lý: byte source → 1 byte exec.
pub fn lex_t(b: &[u8], char_uns: bool) -> Result<Vec<PTok>, String> {
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
        if b[i..].starts_with(b"/*") {
            let end = b[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .ok_or("comment không đóng")?;
            line += b[i..i + end + 4].iter().filter(|&&x| x == b'\n').count() as u32;
            ws = true;
            i += end + 4;
            continue;
        }
        // "//" (C99/extension, clang chấp nhận cả trong -std=c89): đến hết dòng
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            ws = true;
            continue;
        }
        let tok_start = i;
        // Prefix literal rộng: L (wchar=int=4), u (char16=2), U (char32=4),
        // u8 (UTF-8=1) — chỉ khi liền QUOTE, nếu không u/U/L là identifier thường.
        // EXT(c99)/C11 6.4.5: u/U/u8 literal. sw = element width byte.
        let (sw, adv) = match (c, b.get(i + 1), b.get(i + 2)) {
            (b'L' | b'U', Some(b'\'' | b'"'), _) => (4u8, 1),
            (b'u', Some(b'8'), Some(b'"')) => (1, 2),
            (b'u', Some(b'\'' | b'"'), _) => (2, 1),
            _ => (1, 0),
        };
        i += adv;
        let c = b[i];
        let wide = sw >= 2; // giá trị char rộng: không cắt về u8
        let tok = if c.is_ascii_digit()
            || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            number(b, &mut i)?
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
                        // char signed trên Darwin, unsigned trên Linux arm64;
                        // wide giữ nguyên giá trị (không cắt u8)
                        if wide {
                            e as i64
                        } else if char_uns {
                            e as u8 as i64
                        } else {
                            e as u8 as i8 as i64
                        }
                    }
                    e if wide && e >= 0x80 => {
                        // wide char: multibyte source char → 1 codepoint
                        let (cp, len) = utf8_cp(b, i);
                        i += len;
                        cp as i64
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
            let (mut bytes, mut cps) = (Vec::new(), Vec::<u32>::new());
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
                            let e = escape(b, &mut i)?;
                            bytes.push(e as u8);
                            cps.push(e); // escape = 1 phần tử wide, KHÔNG UTF-8-ghép
                        }
                    }
                    e if e >= 0x80 => {
                        // ký tự nguồn multibyte: 1 char nguồn → 1 codepoint;
                        // narrow giữ nguyên byte UTF-8 (execution char set)
                        let (cp, len) = utf8_cp(b, i);
                        bytes.extend_from_slice(&b[i..i + len]);
                        cps.push(cp);
                        i += len;
                    }
                    e => {
                        bytes.push(e);
                        cps.push(e as u32);
                        i += 1;
                    }
                }
            }
            i += 1;
            Tok::Str(bytes, (sw, cps))
        } else if c == b'_' || c == b'$' || c.is_ascii_alphabetic() {
            // EXT(gcc): '$' hợp lệ trong identifier (mặc định gcc mọi target ELF/Darwin);
            // tối thiểu phải sống qua lexer để #if 0 skip được (pp-token chỉ chết ở phase 7)
            let s = i;
            while i < b.len() && (b[i] == b'_' || b[i] == b'$' || b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            Tok::Ident(String::from_utf8_lossy(&b[s..i]).into_owned())
        } else if let Some((d, p)) = DIGRAPHS.iter().find(|(d, _)| b[i..].starts_with(d.as_bytes())) {
            // C99 6.4.6: digraph = spelling thay thế vô điều kiện (phase 3),
            // ánh xạ về punct chính tắc ngay tại lexer — phải đứng TRƯỚC bảng
            // PUNCTS vì "<:" sẽ bị match ngắn thành "<". Không có luật "<::"
            // kiểu C++: trong C, "<:" LUÔN là "[".
            i += d.len();
            Tok::Punct(p)
        } else {
            match PUNCTS.iter().find(|p| b[i..].starts_with(p.as_bytes())) {
                Some(p) => {
                    i += p.len();
                    Tok::Punct(p)
                }
                None => return Err(format!("ký tự lạ '{}'", c as char)),
            }
        };
        let raw = match tok {
            Tok::Num(..) | Tok::FNum(..) | Tok::INum(..) | Tok::Str(..) => {
                String::from_utf8_lossy(&b[tok_start..i]).into_owned()
            }
            _ => String::new(),
        };
        toks.push(PTok {
            tok,
            bol,
            ws,
            line,
            file: 0,
            hide: Vec::new(),
            raw,
        });
        bol = false;
        ws = false;
    }
    Ok(toks)
}
