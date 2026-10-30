//! 詞法分析：byte-based，語法部分只用 ASCII，註解與字串可含任意 UTF-8。

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i128),
    /// 定寬字面量 `8'hFF`：(寬度, 值)
    Sized(u32, i128),
    Float(f64),
    Str(String),
    P(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    pub col: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct FxError {
    pub line: u32,
    pub col: u32,
    pub msg: String,
}

impl fmt::Display for FxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

pub type Res<T> = Result<T, FxError>;

const PUNCT3: &[&str] = &["..=", "<<=", ">>="];
const PUNCT2: &[&str] = &[
    "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "+=", "-=", "*=", "/=", "&=",
    "|=", "^=", "..", "+%", "-%", "*%", ";;",
];
const PUNCT1: &[&str] = &[
    "+", "-", "*", "/", "%", "&", "|", "^", "~", "!", "<", ">", "=", "(", ")", "{", "}", "[", "]",
    ",", ";", ":", ".", "@", "#",
];

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

pub fn lex(src: &str) -> Res<Vec<Token>> {
    lex_from(src, 0)
}

/// `line_base`：多檔案載入時，把檔案編號編進行號（file_idx * FILE_STRIDE），錯誤訊息再還原
pub fn lex_from(src: &str, line_base: u32) -> Res<Vec<Token>> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut line = line_base + 1;
    let mut line_start = 0usize;
    let mut out: Vec<Token> = Vec::new();

    macro_rules! err {
        ($i:expr, $($arg:tt)*) => {
            return Err(FxError { line, col: ($i - line_start + 1) as u32, msg: format!($($arg)*) })
        };
    }

    while i < b.len() {
        let c = b[i];
        // 空白
        if c == b'\n' {
            line += 1;
            i += 1;
            line_start = i;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        // 註解
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let mut depth = 1;
            let start = i;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                err!(start, "unterminated block comment");
            }
            continue;
        }

        let start = i;
        let col = (i - line_start + 1) as u32;
        let tline = line;
        macro_rules! push {
            ($out:expr, $tok:expr, $end:expr) => {
                $out.push(Token { tok: $tok, line: tline, col, start, end: $end })
            };
        }

        // 識別字
        if is_ident_start(c) {
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            push!(out, Tok::Ident(src[start..i].to_string()), i);
            continue;
        }

        // 數字
        if c.is_ascii_digit() {
            let after_dot = matches!(out.last(), Some(Token { tok: Tok::P("."), .. }));
            // 0x / 0b
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                i += 2;
                let s = i;
                while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
                    i += 1;
                }
                let txt: String = src[s..i].chars().filter(|&c| c != '_').collect();
                match i128::from_str_radix(&txt, 16) {
                    Ok(v) => push!(out, Tok::Int(v), i),
                    Err(_) => err!(start, "invalid hex literal"),
                }
                continue;
            }
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'b' || b[i + 1] == b'B') {
                i += 2;
                let s = i;
                while i < b.len() && (b[i] == b'0' || b[i] == b'1' || b[i] == b'_') {
                    i += 1;
                }
                let txt: String = src[s..i].chars().filter(|&c| c != '_').collect();
                match i128::from_str_radix(&txt, 2) {
                    Ok(v) => push!(out, Tok::Int(v), i),
                    Err(_) => err!(start, "invalid binary literal"),
                }
                continue;
            }
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            let txt: String = src[start..i].chars().filter(|&c| c != '_').collect();
            // 定寬：8'hFF
            if i + 2 < b.len()
                && b[i] == b'\''
                && matches!(b[i + 1], b'b' | b'B' | b'd' | b'D' | b'h' | b'H')
            {
                let width: u32 = match txt.parse() {
                    Ok(w) => w,
                    Err(_) => err!(start, "invalid literal width"),
                };
                let radix = match b[i + 1] {
                    b'b' | b'B' => 2,
                    b'd' | b'D' => 10,
                    _ => 16,
                };
                i += 2;
                let s = i;
                while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
                    i += 1;
                }
                let digits: String = src[s..i].chars().filter(|&c| c != '_').collect();
                match i128::from_str_radix(&digits, radix) {
                    Ok(v) => push!(out, Tok::Sized(width, v), i),
                    Err(_) => err!(start, "invalid sized literal"),
                }
                continue;
            }
            // 浮點
            if !after_dot
                && i + 1 < b.len()
                && b[i] == b'.'
                && b[i + 1].is_ascii_digit()
            {
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                    let save = i;
                    i += 1;
                    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                        i += 1;
                    }
                    if i < b.len() && b[i].is_ascii_digit() {
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                    } else {
                        i = save;
                    }
                }
                let t: String = src[start..i].chars().filter(|&c| c != '_').collect();
                match t.parse::<f64>() {
                    Ok(v) => push!(out, Tok::Float(v), i),
                    Err(_) => err!(start, "invalid float literal"),
                }
                continue;
            }
            match txt.parse::<i128>() {
                Ok(v) => push!(out, Tok::Int(v), i),
                Err(_) => err!(start, "integer literal too large"),
            }
            continue;
        }

        // 字串
        if c == b'"' {
            i += 1;
            let mut buf: Vec<u8> = Vec::new();
            loop {
                if i >= b.len() {
                    err!(start, "unterminated string literal");
                }
                match b[i] {
                    b'"' => {
                        i += 1;
                        break;
                    }
                    b'\\' if i + 1 < b.len() => {
                        buf.push(match b[i + 1] {
                            b'n' => b'\n',
                            b't' => b'\t',
                            o => o,
                        });
                        i += 2;
                    }
                    b'\n' => {
                        line += 1;
                        line_start = i + 1;
                        buf.push(b'\n');
                        i += 1;
                    }
                    o => {
                        buf.push(o);
                        i += 1;
                    }
                }
            }
            push!(out, Tok::Str(String::from_utf8_lossy(&buf).into_owned()), i);
            continue;
        }

        // 標點（最長匹配）
        let rest = &src[i..];
        let mut matched: Option<&'static str> = None;
        for p in PUNCT3.iter().chain(PUNCT2.iter()).chain(PUNCT1.iter()) {
            if rest.starts_with(p) {
                matched = Some(p);
                break;
            }
        }
        match matched {
            Some(p) => {
                i += p.len();
                push!(out, Tok::P(p), i);
            }
            None => {
                let ch = rest.chars().next().unwrap_or('?');
                err!(i, "unexpected character `{}`", ch);
            }
        }
    }
    out.push(Token { tok: Tok::Eof, line, col: 1, start: b.len(), end: b.len() });
    Ok(out)
}
