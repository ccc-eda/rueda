//! 執行期值、型別，以及位寬/定點數的轉換與算術。
//!
//! 模擬器不做靜態位寬推導，而是在每個「有宣告型別的落點」
//!（let / carry / 參數 / 通道元素 / 陣列元素）做**動態範圍檢查**，
//! 對應規格 §3.3 的 E0301。

use crate::ast::BinOp;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Round {
    Trunc,
    Nearest,
    Even,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ovf {
    Wrap,
    Sat,
    Check,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RtType {
    Int { lo: i128, hi: i128 },
    Bool,
    Fixed { i: u32, f: u32, round: Round, ovf: Ovf },
    Array(Box<RtType>, usize),
    Tuple(Vec<RtType>),
    Chan(Box<RtType>, usize),
    Mem(Box<RtType>, usize),
    Any,
}

#[derive(Debug, Clone)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i128),
    Float(f64),
    /// 定點數：(原始整數, 小數位元數)；實際值 = raw / 2^frac
    Fixed(i128, u32),
    Str(String),
    Array(Vec<Value>),
    Tuple(Vec<Value>),
    /// 半開區間 [lo, hi)
    Range(i128, i128),
    Tx(usize),
    Rx(usize),
}

pub const MAX_WIDTH: u32 = 120;

pub fn pow2(n: u32) -> i128 {
    1i128 << n
}

/// 範圍 [lo, hi] 所需的最小位寬（與規格 §3.1 的 bits() 相同）
pub fn int_bits(lo: i128, hi: i128) -> u32 {
    fn clog2(x: i128) -> u32 {
        // ceil(log2(x)), x >= 1
        if x <= 1 {
            0
        } else {
            128 - ((x - 1) as u128).leading_zeros()
        }
    }
    if lo >= 0 {
        clog2(hi + 1).max(1)
    } else {
        1 + clog2((-lo).max(hi + 1))
    }
}

pub fn unsigned_type(n: u32) -> RtType {
    RtType::Int { lo: 0, hi: pow2(n) - 1 }
}
pub fn signed_type(n: u32) -> RtType {
    RtType::Int { lo: -pow2(n - 1), hi: pow2(n - 1) - 1 }
}

/// 解析 `u7` / `i12` 型別名稱
pub fn parse_int_type(name: &str) -> Option<RtType> {
    let (signed, rest) = if let Some(r) = name.strip_prefix('u') {
        (false, r)
    } else {
        let r = name.strip_prefix('i')?;
        (true, r)
    };
    if rest.is_empty() || !rest.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let n: u32 = rest.parse().ok()?;
    if n == 0 || n > MAX_WIDTH {
        return None;
    }
    Some(if signed { signed_type(n) } else { unsigned_type(n) })
}

impl fmt::Display for RtType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtType::Int { lo, hi } => {
                let w = *hi + 1;
                if *lo == 0 && w > 0 && (w & (w - 1)) == 0 {
                    write!(f, "u{}", int_bits(*lo, *hi))
                } else if *lo < 0 && *lo == -w && w > 0 && (w & (w - 1)) == 0 {
                    write!(f, "i{}", int_bits(*lo, *hi))
                } else {
                    write!(f, "int<{}..={}>", lo, hi)
                }
            }
            RtType::Bool => write!(f, "bool"),
            RtType::Fixed { i, f: fr, .. } => write!(f, "fixed<{},{}>", i, fr),
            RtType::Array(t, n) => write!(f, "[{}; {}]", t, n),
            RtType::Tuple(ts) => {
                write!(f, "(")?;
                for (k, t) in ts.iter().enumerate() {
                    if k > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
                write!(f, ")")
            }
            RtType::Chan(t, d) => write!(f, "chan<{}, depth={}>", t, d),
            RtType::Mem(t, n) => write!(f, "mem<{}, {}>", t, n),
            RtType::Any => write!(f, "_"),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => write!(f, "()"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(x) => write!(f, "{}", x),
            Value::Fixed(raw, fr) => write!(f, "{}", *raw as f64 / 2f64.powi(*fr as i32)),
            Value::Str(s) => write!(f, "{}", s),
            Value::Array(xs) => {
                write!(f, "[")?;
                for (k, x) in xs.iter().enumerate() {
                    if k > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", x)?;
                }
                write!(f, "]")
            }
            Value::Tuple(xs) => {
                write!(f, "(")?;
                for (k, x) in xs.iter().enumerate() {
                    if k > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", x)?;
                }
                write!(f, ")")
            }
            Value::Range(a, b) => write!(f, "{}..{}", a, b),
            Value::Tx(c) => write!(f, "<tx#{}>", c),
            Value::Rx(c) => write!(f, "<rx#{}>", c),
        }
    }
}

pub fn kind(v: &Value) -> &'static str {
    match v {
        Value::Unit => "unit",
        Value::Bool(_) => "bool",
        Value::Int(_) => "integer",
        Value::Float(_) => "float literal",
        Value::Fixed(..) => "fixed-point",
        Value::Str(_) => "symbol",
        Value::Array(_) => "array",
        Value::Tuple(_) => "tuple",
        Value::Range(..) => "range",
        Value::Tx(_) => "channel tx endpoint",
        Value::Rx(_) => "channel rx endpoint",
    }
}

pub fn zero_value(t: &RtType) -> Value {
    match t {
        RtType::Int { lo, hi } => Value::Int(if *lo <= 0 && 0 <= *hi { 0 } else { *lo }),
        RtType::Bool => Value::Bool(false),
        RtType::Fixed { f, .. } => Value::Fixed(0, *f),
        RtType::Array(e, n) | RtType::Mem(e, n) => Value::Array(vec![zero_value(e); *n]),
        RtType::Tuple(ts) => Value::Tuple(ts.iter().map(zero_value).collect()),
        RtType::Chan(..) | RtType::Any => Value::Unit,
    }
}

// ---------------------------------------------------------------- 數值轉換

/// 向右移 s 位並依模式捨入
pub fn round_shift(x: i128, s: u32, r: Round) -> i128 {
    if s == 0 {
        return x;
    }
    if s >= 127 {
        return if x < 0 && r != Round::Trunc { 0 } else if x < 0 { -1 } else { 0 };
    }
    let q = x >> s; // 算術右移 = floor
    match r {
        Round::Trunc => q,
        Round::Nearest => (x + (1i128 << (s - 1))) >> s,
        Round::Even => {
            let rem = x - (q << s);
            let half = 1i128 << (s - 1);
            if rem > half {
                q + 1
            } else if rem == half {
                q + (q & 1)
            } else {
                q
            }
        }
    }
}

fn round_f64(x: f64, r: Round) -> i128 {
    match r {
        Round::Trunc => x.floor() as i128,
        Round::Nearest => (x + 0.5).floor() as i128,
        Round::Even => {
            let t = x.trunc();
            if (x - t).abs() == 0.5 {
                (2.0 * (x / 2.0).round()) as i128
            } else {
                x.round() as i128
            }
        }
    }
}

fn fixed_bounds(i: u32, f: u32) -> (i128, i128) {
    let total = i + f;
    if total == 0 {
        (0, 0)
    } else {
        (-pow2(total - 1), pow2(total - 1) - 1)
    }
}

/// 把整數 n 放進 [lo, hi]，依 ovf 模式處理溢位
fn fit(n: i128, lo: i128, hi: i128, ovf: Ovf, width: u32, tname: &str) -> Result<i128, String> {
    if n >= lo && n <= hi {
        return Ok(n);
    }
    match ovf {
        Ovf::Sat => Ok(n.clamp(lo, hi)),
        Ovf::Check => Err(format!(
            "value {} overflows {} (range {}..={})",
            n, tname, lo, hi
        )),
        Ovf::Wrap => {
            let m = pow2(width);
            let mut r = n.rem_euclid(m);
            if lo < 0 && r >= pow2(width - 1) {
                r -= m;
            }
            if r < lo || r > hi {
                Err(format!("cannot wrap {} into {}", n, tname))
            } else {
                Ok(r)
            }
        }
    }
}

/// 明確轉換（`as`、`.to`、`.sat`、`.wrap`、`.quantize`）
pub fn convert(v: &Value, t: &RtType, round: Round, ovf: Ovf) -> Result<Value, String> {
    match t {
        RtType::Any => Ok(v.clone()),
        RtType::Int { lo, hi } => {
            let n: i128 = match v {
                Value::Int(n) => *n,
                Value::Bool(b) => *b as i128,
                Value::Fixed(raw, f) => round_shift(*raw, *f, round),
                Value::Float(x) => round_f64(*x, round),
                other => return Err(format!("cannot convert {} to {}", kind(other), t)),
            };
            let w = int_bits(*lo, *hi);
            Ok(Value::Int(fit(n, *lo, *hi, ovf, w, &t.to_string())?))
        }
        RtType::Fixed { i, f, .. } => {
            let raw: i128 = match v {
                Value::Int(n) => n
                    .checked_mul(pow2(*f))
                    .ok_or_else(|| "fixed-point overflow".to_string())?,
                Value::Bool(b) => (*b as i128) << f,
                Value::Fixed(r, fs) => {
                    if f >= fs {
                        r.checked_mul(pow2(f - fs))
                            .ok_or_else(|| "fixed-point overflow".to_string())?
                    } else {
                        round_shift(*r, fs - f, round)
                    }
                }
                Value::Float(x) => round_f64(x * 2f64.powi(*f as i32), round),
                other => return Err(format!("cannot convert {} to {}", kind(other), t)),
            };
            let (mn, mx) = fixed_bounds(*i, *f);
            let w = (*i + *f).max(1);
            Ok(Value::Fixed(fit(raw, mn, mx, ovf, w, &t.to_string())?, *f))
        }
        RtType::Bool => match v {
            Value::Bool(_) => Ok(v.clone()),
            Value::Int(n) => Ok(Value::Bool(*n != 0)),
            other => Err(format!("cannot convert {} to bool", kind(other))),
        },
        RtType::Array(e, _) | RtType::Mem(e, _) => match v {
            Value::Array(xs) => Ok(Value::Array(
                xs.iter().map(|x| convert(x, e, round, ovf)).collect::<Result<_, _>>()?,
            )),
            other => Err(format!("cannot convert {} to {}", kind(other), t)),
        },
        RtType::Tuple(ts) => match v {
            Value::Tuple(xs) if xs.len() == ts.len() => Ok(Value::Tuple(
                xs.iter().zip(ts).map(|(x, t)| convert(x, t, round, ovf)).collect::<Result<_, _>>()?,
            )),
            other => Err(format!("cannot convert {} to {}", kind(other), t)),
        },
        RtType::Chan(..) => Ok(v.clone()),
    }
}

/// 隱式強制轉換（賦值落點）：只允許「不丟資訊」的放大，其餘需顯式轉換
pub fn coerce(v: Value, t: &RtType) -> Result<Value, String> {
    match t {
        RtType::Any => Ok(v),
        RtType::Int { lo, hi } => match v {
            Value::Int(n) => {
                if n >= *lo && n <= *hi {
                    Ok(Value::Int(n))
                } else {
                    Err(format!(
                        "value {} does not fit {} (range {}..={})",
                        n, t, lo, hi
                    ))
                }
            }
            Value::Bool(b) if *lo <= 0 && *hi >= 1 => Ok(Value::Int(b as i128)),
            other => Err(format!(
                "cannot implicitly convert {} to {}; use `as`, `.to::<T>()`, `.sat::<T>()` or `.wrap::<T>()`",
                kind(&other),
                t
            )),
        },
        RtType::Bool => match v {
            Value::Bool(_) => Ok(v),
            other => Err(format!("expected bool, found {}", kind(&other))),
        },
        RtType::Fixed { i, f, .. } => {
            let raw = match &v {
                Value::Int(n) => n
                    .checked_mul(pow2(*f))
                    .ok_or_else(|| "fixed-point overflow".to_string())?,
                Value::Float(x) => round_f64(x * 2f64.powi(*f as i32), Round::Nearest),
                Value::Fixed(r, fs) => {
                    if f >= fs {
                        r.checked_mul(pow2(f - fs))
                            .ok_or_else(|| "fixed-point overflow".to_string())?
                    } else {
                        let sh = fs - f;
                        if r.rem_euclid(pow2(sh)) != 0 {
                            return Err(format!(
                                "cannot implicitly narrow fixed<_,{}> to {}: fractional bits would be lost; use `.to::<T>(round = ..)`",
                                fs, t
                            ));
                        }
                        r >> sh
                    }
                }
                other => {
                    return Err(format!("cannot implicitly convert {} to {}", kind(other), t))
                }
            };
            let (mn, mx) = fixed_bounds(*i, *f);
            if raw < mn || raw > mx {
                return Err(format!(
                    "value {} does not fit {}",
                    Value::Fixed(raw, *f),
                    t
                ));
            }
            Ok(Value::Fixed(raw, *f))
        }
        RtType::Array(e, n) | RtType::Mem(e, n) => match v {
            Value::Array(xs) => {
                if xs.len() != *n {
                    return Err(format!(
                        "array length mismatch: expected {}, found {}",
                        n,
                        xs.len()
                    ));
                }
                let mut out = Vec::with_capacity(xs.len());
                for (k, x) in xs.into_iter().enumerate() {
                    out.push(coerce(x, e).map_err(|m| format!("element {}: {}", k, m))?);
                }
                Ok(Value::Array(out))
            }
            other => Err(format!("expected {}, found {}", t, kind(&other))),
        },
        RtType::Tuple(ts) => match v {
            Value::Tuple(xs) if xs.len() == ts.len() => {
                let mut out = Vec::new();
                for (x, t) in xs.into_iter().zip(ts) {
                    out.push(coerce(x, t)?);
                }
                Ok(Value::Tuple(out))
            }
            other => Err(format!("expected {}, found {}", t, kind(&other))),
        },
        RtType::Chan(..) => match v {
            Value::Tx(_) | Value::Rx(_) => Ok(v),
            other => Err(format!("expected channel endpoint, found {}", kind(&other))),
        },
    }
}

// ---------------------------------------------------------------- 算術

pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        Value::Fixed(r, f) => Some(*r as f64 / 2f64.powi(*f as i32)),
        _ => None,
    }
}

/// 轉成 (raw, frac)：整數視為 frac = 0 的定點數
fn as_fixed(v: &Value) -> Option<(i128, u32)> {
    match v {
        Value::Int(n) => Some((*n, 0)),
        Value::Fixed(r, f) => Some((*r, *f)),
        _ => None,
    }
}

fn align(a: (i128, u32), b: (i128, u32)) -> Result<(i128, i128, u32), String> {
    let f = a.1.max(b.1);
    let sa = a.0.checked_mul(pow2(f - a.1)).ok_or("fixed-point overflow")?;
    let sb = b.0.checked_mul(pow2(f - b.1)).ok_or("fixed-point overflow")?;
    Ok((sa, sb, f))
}

fn cmp_result(op: BinOp, ord: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        BinOp::Eq => ord == Equal,
        BinOp::Ne => ord != Equal,
        BinOp::Lt => ord == Less,
        BinOp::Le => ord != Greater,
        BinOp::Gt => ord == Greater,
        BinOp::Ge => ord != Less,
        _ => unreachable!(),
    }
}

pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Array(x), Value::Array(y)) | (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| values_equal(p, q))
        }
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(x), Value::Int(y)) | (Value::Int(y), Value::Bool(x)) => (*x as i128) == *y,
        _ => match (as_fixed(a), as_fixed(b)) {
            (Some(p), Some(q)) => match align(p, q) {
                Ok((x, y, _)) => x == y,
                Err(_) => false,
            },
            _ => match (as_f64(a), as_f64(b)) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            },
        },
    }
}

pub fn binop(op: BinOp, a: &Value, b: &Value) -> Result<Value, String> {
    use BinOp::*;
    match op {
        And | Or => {
            let (x, y) = (truthy(a)?, truthy(b)?);
            return Ok(Value::Bool(if op == And { x && y } else { x || y }));
        }
        Eq => return Ok(Value::Bool(values_equal(a, b))),
        Ne => return Ok(Value::Bool(!values_equal(a, b))),
        Lt | Le | Gt | Ge => {
            let ord = match (as_fixed(a), as_fixed(b)) {
                (Some(p), Some(q)) => {
                    let (x, y, _) = align(p, q)?;
                    x.cmp(&y)
                }
                _ => match (as_f64(a), as_f64(b)) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).ok_or("NaN comparison")?,
                    _ => {
                        return Err(format!(
                            "cannot compare {} with {}",
                            kind(a),
                            kind(b)
                        ))
                    }
                },
            };
            return Ok(Value::Bool(cmp_result(op, ord)));
        }
        _ => {}
    }

    // Bool 的位元運算
    if let (Value::Bool(x), Value::Bool(y)) = (a, b) {
        return match op {
            BitAnd => Ok(Value::Bool(*x & *y)),
            BitOr => Ok(Value::Bool(*x | *y)),
            BitXor => Ok(Value::Bool(*x ^ *y)),
            _ => Err("arithmetic on bool".to_string()),
        };
    }

    match (a, b) {
        (Value::Int(x), Value::Int(y)) => int_op(op, *x, *y).map(Value::Int),
        (Value::Float(_), _) | (_, Value::Float(_)) => {
            let (x, y) = (
                as_f64(a).ok_or_else(|| format!("bad operand {}", kind(a)))?,
                as_f64(b).ok_or_else(|| format!("bad operand {}", kind(b)))?,
            );
            Ok(Value::Float(match op {
                Add | AddW => x + y,
                Sub | SubW => x - y,
                Mul | MulW => x * y,
                Div => x / y,
                _ => return Err("unsupported operator on float literal".to_string()),
            }))
        }
        (Value::Fixed(..), _) | (_, Value::Fixed(..)) => {
            let (p, q) = match (as_fixed(a), as_fixed(b)) {
                (Some(p), Some(q)) => (p, q),
                _ => {
                    return Err(format!(
                        "cannot apply operator to {} and {}",
                        kind(a),
                        kind(b)
                    ))
                }
            };
            match op {
                Add | AddW | Sub | SubW => {
                    let (x, y, f) = align(p, q)?;
                    let r = if matches!(op, Add | AddW) { x.checked_add(y) } else { x.checked_sub(y) };
                    Ok(Value::Fixed(r.ok_or("fixed-point overflow")?, f))
                }
                Mul | MulW => Ok(Value::Fixed(
                    p.0.checked_mul(q.0).ok_or("fixed-point overflow")?,
                    p.1 + q.1,
                )),
                _ => Err("unsupported operator on fixed-point values (use .to::<T>() first)".to_string()),
            }
        }
        _ => Err(format!(
            "cannot apply operator to {} and {}",
            kind(a),
            kind(b)
        )),
    }
}

fn int_op(op: BinOp, x: i128, y: i128) -> Result<i128, String> {
    use BinOp::*;
    let ov = || "integer overflow in simulator (value exceeds 120 bits)".to_string();
    Ok(match op {
        Add | AddW => x.checked_add(y).ok_or_else(ov)?,
        Sub | SubW => x.checked_sub(y).ok_or_else(ov)?,
        Mul | MulW => x.checked_mul(y).ok_or_else(ov)?,
        Div => {
            if y == 0 {
                return Err("division by zero".to_string());
            }
            x / y
        }
        Rem => {
            if y == 0 {
                return Err("modulo by zero".to_string());
            }
            x % y
        }
        Shl => {
            if y < 0 {
                return Err(format!("invalid shift amount {}", y));
            }
            // 0 << n 對任何 n 都是 0；其餘超過精確整數寬度的移位必然溢位
            if x == 0 {
                0
            } else if y >= 120 {
                return Err(ov());
            } else {
                x.checked_mul(pow2(y as u32)).ok_or_else(ov)?
            }
        }
        Shr => {
            if y < 0 {
                return Err(format!("invalid shift amount {}", y));
            }
            // 算術右移 = floor(x / 2^y)：移位量超過寬度時結果是 0 或 -1
            x >> y.min(127)
        }
        BitAnd => x & y,
        BitOr => x | y,
        BitXor => x ^ y,
        _ => unreachable!(),
    })
}

pub fn truthy(v: &Value) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Int(0) => Ok(false),
        Value::Int(1) => Ok(true),
        other => Err(format!("expected bool or bit, found {}", kind(other))),
    }
}
