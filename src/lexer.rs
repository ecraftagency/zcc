// Lexer: C source -> Vec<PTok>. PTok = Tok + metadata for the preprocessor: bol
// THEORY A1 — lexing; THEORY II-1 — the ISO C99 token and limit tables
// (first token of a logical line, eligible to introduce a '#' directive), ws
// (whitespace or comment immediately precedes it, needed for stringization),
// line (physical line, for __LINE__ and diagnostics).

// Integer-constant type per C89 (suffix + magnitude + radix); under LP64 it
// suffices to distinguish signed/unsigned x int/long.
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
    FNum(f64, u8), // floating constant; 0 = float (f/F), 1 = double, 2 = long double (l/L)
    // C99 6.4.4.2 / EXT(gcc): imaginary constant `1.0i` `1.0iF` — the embedding
    // ℝ→ℂ (b ↦ 0+bi); value + element-kind (as in FNum). The parser builds a
    // temporary complex value {re=0, im=v}.
    INum(f64, u8),
    Ident(String),
    Punct(&'static str),
    // narrow bytes (escapes already processed, excluding NUL) + (WIDTH byte,
    // wide-codepoint sequence). WIDTH: 1 narrow/u8, 2 char16 u"", 4 wchar
    // L""/char32 U"". cps separates a multibyte SOURCE character (→1 codepoint)
    // from an ESCAPE (→1 single value): when the string becomes wide,
    // "あ"→0x3042 but "\343\201\202"→3 elements (C99 5.1.1.2).
    Str(Vec<u8>, (u8, Vec<u32>)),
}

#[derive(Clone, Debug)]
pub struct PTok {
    pub tok: Tok,
    pub bol: bool,
    pub ws: bool,
    pub line: u32,
    pub file: u32,         // index into the preprocessor file table (0 = original file)
    pub hide: Vec<String>, // hideset: macros that expanded to this token (blocks re-expansion)
    pub raw: String,       // original spelling (Num/Str/Char) for # stringization; empty = spell from value
}

/// THEORY II-1 — ISO C99 §6.4.6 digraphs
// C99 6.4.6: digraphs — longest match first ("%:%:" before "%:").
const DIGRAPHS: [(&str, &str); 6] = [
    ("%:%:", "##"),
    ("%:", "#"),
    ("<:", "["),
    (":>", "]"),
    ("<%", "{"),
    ("%>", "}"),
];

/// THEORY II-1 — ISO C99 §6.4.6 punctuators
// Longer punctuators listed first so they match first ("<<=" before "<<" before "<").
const PUNCTS: [&str; 48] = [
    "...", "<<=", ">>=", "->", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "==",
    "!=", "<=", ">=", "<<", ">>", "&&", "||", "##", "<", ">", "=", "+", "-", "*", "/", "%", "(",
    ")", "{", "}", ";", ",", "&", "[", "]", ".", "!", "|", "^", "~", "?", ":", "#",
];

// Full C89 escape set: \n \t \r \a \b \f \v \\ \' \" \? \ooo \xhh.
// In: i points immediately AFTER the '\'; out: the byte value + i advanced past the escape.
// Decode one UTF-8 scalar at b[i] → (codepoint, byte count). An invalid byte → (byte, 1).
// A wide char L'Ä' = one multibyte source char → one wchar (6.4.4.4); narrow keeps the bytes.
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
            _ => return (c as u32, 1), // incomplete sequence → treat the lead byte as one char
        }
    }
    (cp, n)
}

fn escape(b: &[u8], i: &mut usize) -> Result<u32, String> {
    let c = *b.get(*i).ok_or("truncated escape sequence")?;
    *i += 1;
    Ok(match c {
        b'n' => 10,
        b't' => 9,
        b'r' => 13,
        b'a' => 7,
        b'b' => 8,
        b'f' => 12,
        b'v' => 11,
        b'e' => 27, // EXT(gcc): \e = ESC (required by chibicc test/string.c)
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
        // Undefined escape: C89 3.1.3.4 leaves it undefined behavior — following
        // gcc/clang, use identity ('\j' == 'j'; chibicc test/string.c asserts this)
        _ => c as u32,
    })
}

// A numeric constant begins at i (a digit, or '.' + digit). Returns the token + new i.
// Floating-constant suffix: fFlL (kind) + iIjJ (imaginary) in ANY ORDER (gcc: 1.0if == 1.0fi)
fn fsuffix(b: &[u8], i: &mut usize) -> (u8, bool) {
    let (mut k, mut im) = (1u8, false);
    loop {
        match b.get(*i) {
            Some(b'f' | b'F') => k = 0,
            Some(b'l' | b'L') => k = 2, // long double: Darwin=double; ELF=binary128 at the ABI boundary
            Some(b'i' | b'I' | b'j' | b'J') => im = true,
            _ => break,
        }
        *i += 1;
    }
    (k, im)
}

fn number(b: &[u8], i: &mut usize) -> Result<Tok, String> {
    let s = *i;
    // A numeric constant is all-ASCII → build a &str from the byte subrange for parse/from_str_radix
    let sv = move |x: usize, y: usize| std::str::from_utf8(&b[x..y]).unwrap();
    if b[s..].starts_with(b"0b") || b[s..].starts_with(b"0B") {
        // EXT(gcc): binary constant 0b1010
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
        // C99: hex float 0x1.8p3 = hex mantissa × 2^exponent — used heavily by
        // musl src/math (exp/log/pow constant tables). Accumulating by
        // multiply-by-16 is exact up to 2^53, sufficient for every literal of
        // ≤13 hex digits; any residual error is caught by differential vs cc.
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
                return Err("hex float requires a 'p' exponent".into());
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
            // powi(-1074) = 1/2^1074 = 1/inf = 0 — split into two steps to reach
            // the subnormal range (0x1p-1074 = musl's DBL_TRUE_MIN)
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
    // floating: contains '.' or an e/E exponent (hex-float constants do not exist in C89)
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
    // "08" is a valid pp-number, ill-formed only when USED as a constant (pcre2.h:
    // `#define PCRE2_DATE 2025-08-27` is never evaluated) — gcc lexes it through
    // and errors at conversion; zcc lexes eagerly, so demote it to decimal rather
    // than failing the whole translation unit
    let octal = octal && !b[s..*i].iter().any(|&c| c == b'8' || c == b'9');
    let v = u64::from_str_radix(sv(s, *i), if octal { 8 } else { 10 }).map_err(|e| format!("{e}"))?;
    Ok(Tok::Num(v as i64, suffix_kind(b, i, v, octal)?))
}

// Consume the u/U/l/L suffix, then select the C89 type: an unsuffixed decimal
// goes int→long; octal/hex additionally admit unsigned (int→uint→long→ulong).
fn suffix_kind(b: &[u8], i: &mut usize, v: u64, oct_hex: bool) -> Result<NumK, String> {
    let (mut u, mut l) = (false, false);
    loop {
        match b.get(*i) {
            Some(b'u' | b'U') if !u => u = true,
            Some(b'l' | b'L') if !l => {
                l = true;
                // "ll"/"LL" (long long = long under LP64): consume the second l
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

// Wrapper for synthetic sources (builtin macros, token paste) — these contain
// no character constant ≥128, so char signedness never comes into play.
pub fn lex(src: &str) -> Result<Vec<PTok>, String> {
    lex_t(src.as_bytes(), false)
}
// char_uns: plain char is UNSIGNED (Linux arm64, AAPCS64) — the only point of
// contact is the VALUE of an escape ≥ 128 in a character constant ('\377' = 255
// vs -1 on Darwin); real source files must take this path with the flag set per
// target.
// Source = RAW &[u8] (not via &str): a C string literal is a BYTE sequence in
// the source character set (5.1.1.2 / 6.4.5) — bytes ≥128 or non-UTF8 (e.g. a
// raw '\377' inside "…") must be PRESERVED; from_utf8_lossy would mangle them
// into U+FFFD (EF BF BD), inflating the string. Theorem: one source byte → one
// execution byte.
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
        // Backslash-newline line splicing (phase 2): do not reset bol — the
        // logical line continues, but still count the physical line for __LINE__.
        if c == b'\\' && b.get(i + 1) == Some(&b'\n') {
            line += 1;
            ws = true;
            i += 2;
            continue;
        }
        // A comment is whitespace; a newline INSIDE a comment does not break the
        // logical line (phase 3 replaces a comment with one space before directive
        // recognition).
        if b[i..].starts_with(b"/*") {
            let end = b[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .ok_or("unterminated comment")?;
            line += b[i..i + end + 4].iter().filter(|&&x| x == b'\n').count() as u32;
            ws = true;
            i += end + 4;
            continue;
        }
        // "//" (C99/extension, accepted by clang even under -std=c89): to end of line
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            ws = true;
            continue;
        }
        let tok_start = i;
        // Wide-literal prefix: L (wchar=int=4), u (char16=2), U (char32=4),
        // u8 (UTF-8=1) — only when immediately followed by a QUOTE, otherwise
        // u/U/L is an ordinary identifier.
        // EXT(c99)/C11 6.4.5: u/U/u8 literals. sw = element width in bytes.
        let (sw, adv) = match (c, b.get(i + 1), b.get(i + 2)) {
            (b'L' | b'U', Some(b'\'' | b'"'), _) => (4u8, 1),
            (b'u', Some(b'8'), Some(b'"')) => (1, 2),
            (b'u', Some(b'\'' | b'"'), _) => (2, 1),
            _ => (1, 0),
        };
        i += adv;
        let c = b[i];
        let wide = sw >= 2; // wide character value: do not truncate to u8
        let tok = if c.is_ascii_digit()
            || (c == b'.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            number(b, &mut i)?
        } else if c == b'\'' {
            i += 1;
            // multi-character constant 'ab' is valid C89 (3.1.3.4,
            // implementation-defined value) — fixed to match clang: pack bytes
            // toward the low end, with the first character in the high byte
            let (mut v, mut n) = (0i64, 0);
            loop {
                let e = match *b.get(i).ok_or("unterminated character constant")? {
                    b'\'' => break,
                    b'\\' => {
                        i += 1;
                        let e = escape(b, &mut i)?;
                        // char is signed on Darwin, unsigned on Linux arm64;
                        // wide preserves the value (no truncation to u8)
                        if wide {
                            e as i64
                        } else if char_uns {
                            e as u8 as i64
                        } else {
                            e as u8 as i8 as i64
                        }
                    }
                    e if wide && e >= 0x80 => {
                        // wide char: a multibyte source char → one codepoint
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
                return Err("empty character constant".into());
            }
            i += 1;
            Tok::Num(v, NumK::I)
        } else if c == b'"' {
            i += 1;
            let (mut bytes, mut cps) = (Vec::new(), Vec::<u32>::new());
            loop {
                match *b.get(i).ok_or("unterminated string literal")? {
                    b'"' => break,
                    b'\\' => {
                        i += 1;
                        // phase 2 also INSIDE a string: \<newline> splices the line, emits no byte
                        if b.get(i) == Some(&b'\n') {
                            line += 1;
                            i += 1;
                        } else {
                            let e = escape(b, &mut i)?;
                            bytes.push(e as u8);
                            cps.push(e); // an escape = one wide element, NOT UTF-8-combined
                        }
                    }
                    e if e >= 0x80 => {
                        // multibyte source character: one source char → one codepoint;
                        // narrow preserves the UTF-8 bytes (execution character set)
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
            // EXT(gcc): '$' is valid in an identifier (gcc default on every ELF/Darwin target);
            // at minimum it must survive the lexer so that #if 0 can skip it (a pp-token is only rejected at phase 7)
            let s = i;
            while i < b.len() && (b[i] == b'_' || b[i] == b'$' || b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            Tok::Ident(String::from_utf8_lossy(&b[s..i]).into_owned())
        } else if let Some((d, p)) = DIGRAPHS.iter().find(|(d, _)| b[i..].starts_with(d.as_bytes())) {
            // C99 6.4.6: a digraph is an unconditional alternative spelling
            // (phase 3), mapped to its canonical punctuator right in the lexer —
            // it must come BEFORE the PUNCTS table, since "<:" would otherwise
            // match short as "<". There is no C++-style "<::" rule: in C, "<:"
            // is ALWAYS "[".
            i += d.len();
            Tok::Punct(p)
        } else {
            match PUNCTS.iter().find(|p| b[i..].starts_with(p.as_bytes())) {
                Some(p) => {
                    i += p.len();
                    Tok::Punct(p)
                }
                None => return Err(format!("unexpected character '{}'", c as char)),
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
