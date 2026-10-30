//! Hardware values: bits plus the exact integer range they hold (spec §3.2). The width of a
//! value is the fewest bits its range needs (two's complement when it can be negative), so
//! arithmetic never overflows: `u8 + u8` is `int<0..=510>`, 9 bits.

use eda::netlist::{const_bits, pad, sext, Bit, Builder};
use fluxa::value::{int_bits, RtType};

pub type R<T> = Result<T, String>;

/// a type that has hardware: integers (and bits, bool) for now
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ty {
    Int { lo: i128, hi: i128 },
    Bool,
}

impl Ty {
    pub fn width(self) -> usize {
        match self {
            Ty::Int { lo, hi } => int_bits(lo, hi) as usize,
            Ty::Bool => 1,
        }
    }
    pub fn range(self) -> (i128, i128) {
        match self {
            Ty::Int { lo, hi } => (lo, hi),
            Ty::Bool => (0, 1),
        }
    }
    pub fn of_rt(t: &RtType) -> R<Ty> {
        match t {
            RtType::Int { lo, hi } => Ok(Ty::Int { lo: *lo, hi: *hi }),
            RtType::Bool => Ok(Ty::Bool),
            RtType::Fixed { .. } => Err("fixed-point types are not synthesized yet".into()),
            RtType::Array(..) => Err("arrays are not synthesized yet".into()),
            RtType::Tuple(..) => Err("tuples are not synthesized yet".into()),
            RtType::Chan(..) => Err("a channel is not a value".into()),
            RtType::Mem(..) => Err("mem is not synthesized yet".into()),
            RtType::Any => Err("this value needs a type for synthesis".into()),
        }
    }
}

/// a value in hardware
#[derive(Clone, Debug)]
pub struct SVal {
    pub bits: Vec<Bit>,
    pub lo: i128,
    pub hi: i128,
    pub boolean: bool,
}

impl SVal {
    pub fn signed(&self) -> bool { self.lo < 0 }
    pub fn ty(&self) -> Ty { if self.boolean { Ty::Bool } else { Ty::Int { lo: self.lo, hi: self.hi } } }

    pub fn konst(n: i128) -> SVal {
        let w = int_bits(n, n) as u32;
        SVal { bits: const_bits(n as u128, w), lo: n, hi: n, boolean: false }
    }
    pub fn bool_const(b: bool) -> SVal { SVal { bits: vec![Bit::of(b)], lo: 0, hi: 1, boolean: true } }
    pub fn bit(b: Bit) -> SVal { SVal { bits: vec![b], lo: 0, hi: 1, boolean: true } }
    /// `bits` holding a value of type `t`
    pub fn of_ty(bits: Vec<Bit>, t: Ty) -> SVal {
        let (lo, hi) = t.range();
        SVal { bits, lo, hi, boolean: t == Ty::Bool }
    }
    /// the value as `w` bits (sign- or zero-extended; never narrower than it is)
    pub fn ext(&self, w: usize) -> Vec<Bit> {
        if self.signed() { sext(&self.bits, w) } else { pad(&self.bits, w) }
    }
    /// the constant, when it is one
    pub fn as_const(&self) -> Option<i128> {
        if self.lo == self.hi { return Some(self.lo); }
        let mut v: i128 = 0;
        for (i, b) in self.bits.iter().enumerate() {
            match b { Bit::C0 => {} Bit::C1 => v |= 1 << i, Bit::W(_) => return None }
        }
        if self.signed() && self.bits.last() == Some(&Bit::C1) { v -= 1 << self.bits.len(); }
        Some(v)
    }
}

/// a value of range [lo, hi] computed in `w` bits (the low bits of a wider result)
fn ranged(bits: Vec<Bit>, lo: i128, hi: i128) -> SVal {
    let w = int_bits(lo, hi) as usize;
    let mut b = bits;
    b.truncate(w);
    let fill = Bit::C0;
    while b.len() < w { b.push(fill); }
    SVal { bits: b, lo, hi, boolean: false }
}

fn common(a: &SVal, b: &SVal, extra: usize) -> usize {
    let lo = a.lo.min(b.lo);
    let hi = a.hi.max(b.hi);
    int_bits(lo, hi) as usize + extra
}

pub fn add(g: &mut Builder, a: &SVal, b: &SVal) -> SVal {
    let (lo, hi) = (a.lo + b.lo, a.hi + b.hi);
    let w = int_bits(lo, hi) as usize;
    let s = g.add(&a.ext(w), &b.ext(w));
    ranged(s, lo, hi)
}

pub fn sub(g: &mut Builder, a: &SVal, b: &SVal) -> SVal {
    let (lo, hi) = (a.lo - b.hi, a.hi - b.lo);
    let w = int_bits(lo, hi) as usize;
    let s = g.sub(&a.ext(w), &b.ext(w));
    ranged(s, lo, hi)
}

pub fn neg(g: &mut Builder, a: &SVal) -> SVal { sub(g, &SVal::konst(0), a) }

pub fn mul(g: &mut Builder, a: &SVal, b: &SVal) -> SVal {
    let p = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
    let (lo, hi) = (*p.iter().min().unwrap(), *p.iter().max().unwrap());
    let w = int_bits(lo, hi) as usize;
    let m = g.mul(&a.ext(w), &b.ext(w));
    ranged(m, lo, hi)
}

/// quotient and remainder, truncating toward zero (like Rust and the simulator)
pub fn divrem(g: &mut Builder, a: &SVal, b: &SVal) -> R<(SVal, SVal)> {
    if b.lo <= 0 && b.hi >= 0 { return Err("division: the divisor's range includes 0 (E0102)".into()); }
    let amax = a.lo.abs().max(a.hi.abs());
    let bmin = b.lo.abs().min(b.hi.abs()).max(1);
    let qmax = amax / bmin;
    let (qlo, qhi) = if a.signed() || b.signed() { (-qmax, qmax) } else { (a.lo / b.hi.max(1), qmax) };
    let rmax = b.lo.abs().max(b.hi.abs()) - 1;
    let (rlo, rhi) = if a.signed() { (-rmax, rmax) } else { (0, rmax.min(a.hi)) };
    let w = common(a, b, 1);
    let (q, r) = if a.signed() || b.signed() { g.sdivmod(&a.ext(w), &b.ext(w)) } else { g.udivmod(&a.ext(w), &b.ext(w)) };
    Ok((ranged(q, qlo, qhi), ranged(r, rlo, rhi)))
}

pub fn shl_const(a: &SVal, k: u32) -> SVal {
    let (lo, hi) = (a.lo << k, a.hi << k);
    let w = int_bits(lo, hi) as usize;
    let mut b = vec![Bit::C0; k as usize];
    b.extend(a.ext(w.saturating_sub(k as usize)));
    ranged(b, lo, hi)
}

pub fn shr_const(a: &SVal, k: u32) -> SVal {
    let (lo, hi) = (a.lo >> k, a.hi >> k);
    let w = a.bits.len();
    let fill = if a.signed() { *a.bits.last().unwrap() } else { Bit::C0 };
    let mut b: Vec<Bit> = a.bits.iter().skip(k as usize).copied().collect();
    while b.len() < w { b.push(fill); }
    ranged(sext_if(b, a.signed(), int_bits(lo, hi) as usize), lo, hi)
}

fn sext_if(b: Vec<Bit>, s: bool, w: usize) -> Vec<Bit> { if s { sext(&b, w) } else { pad(&b, w) } }

/// a shift by a variable amount: the result's range assumes the largest shift the amount allows
pub fn shift(g: &mut Builder, a: &SVal, amt: &SVal, left: bool) -> R<SVal> {
    if amt.lo < 0 { return Err("a shift amount must not be negative".into()); }
    if left {
        let k = amt.hi.min(120) as u32;
        let (lo, hi) = (a.lo.min(a.lo << k), a.hi.max(a.hi << k));
        let w = int_bits(lo, hi) as usize;
        let s = g.shift(&a.ext(w), &amt.bits, true, Bit::C0);
        Ok(ranged(s, lo, hi))
    } else {
        let w = a.bits.len();
        let fill = if a.signed() { *a.bits.last().unwrap() } else { Bit::C0 };
        let s = g.shift(&a.bits, &amt.bits, false, fill);
        Ok(ranged(sext_if(s, a.signed(), w), a.lo.min(0), a.hi.max(0)))
    }
}

/// &, |, ^ bit by bit: the result has the operands' common width
pub fn bitop(g: &mut Builder, a: &SVal, b: &SVal, op: char) -> SVal {
    if a.boolean && b.boolean {
        let x = match op { '&' => g.and(a.bits[0], b.bits[0]), '|' => g.or(a.bits[0], b.bits[0]), _ => g.xor(a.bits[0], b.bits[0]) };
        return SVal::bit(x);
    }
    let w = common(a, b, 0);
    let (x, y) = (a.ext(w), b.ext(w));
    let bits: Vec<Bit> = (0..w).map(|i| match op { '&' => g.and(x[i], y[i]), '|' => g.or(x[i], y[i]), _ => g.xor(x[i], y[i]) }).collect();
    let signed = a.signed() || b.signed();
    let (lo, hi) = if signed { (-(1i128 << (w - 1)), (1i128 << (w - 1)) - 1) } else { (0, (1i128 << w) - 1) };
    SVal { bits, lo, hi, boolean: false }
}

pub fn not_bits(g: &mut Builder, a: &SVal) -> SVal {
    if a.boolean { return SVal::bit(g.not(a.bits[0])); }
    let bits = g.notv(&a.bits);
    // ~x = -x - 1
    SVal { bits, lo: -a.hi - 1, hi: -a.lo - 1, boolean: false }.normalized()
}

impl SVal {
    /// the bits re-fitted to the range (after an operation that changed the range, not the value)
    pub fn normalized(self) -> SVal {
        let w = int_bits(self.lo, self.hi) as usize;
        let signed_now = self.lo < 0;
        let bits = if self.bits.len() >= w { self.bits[..w].to_vec() } else if signed_now { sext(&self.bits, w) } else { pad(&self.bits, w) };
        SVal { bits, ..self }
    }
}

/// a < b, a <= b, a == b, … as one bit
pub fn compare(g: &mut Builder, a: &SVal, b: &SVal, op: &str) -> SVal {
    let w = common(a, b, 1);
    let (x, y) = (a.ext(w), b.ext(w));
    let bit = match op {
        "==" => g.eq(&x, &y),
        "!=" => { let e = g.eq(&x, &y); g.not(e) }
        "<" => g.slt(&x, &y),
        ">" => g.slt(&y, &x),
        "<=" => { let l = g.slt(&y, &x); g.not(l) }
        _ => { let l = g.slt(&x, &y); g.not(l) } // >=
    };
    SVal::bit(bit)
}

/// `c ? a : b` for values: the result's range covers both
pub fn select(g: &mut Builder, c: Bit, a: &SVal, b: &SVal) -> SVal {
    if a.boolean && b.boolean { return SVal::bit(g.mux(c, b.bits[0], a.bits[0])); }
    let (lo, hi) = (a.lo.min(b.lo), a.hi.max(b.hi));
    let w = int_bits(lo, hi) as usize;
    let bits = g.muxv(c, &b.ext(w), &a.ext(w));
    SVal { bits, lo, hi, boolean: false }
}

/// the value as type `t`, keeping its low bits (a landing site: `let x: T = e`, an argument,
/// `as T`, `.wrap::<T>()`; the simulator checks the range, the hardware does not)
pub fn wrap_to(v: &SVal, t: Ty) -> SVal {
    if t == Ty::Bool {
        let b = if v.boolean { v.bits[0] } else { v.bits[0] };
        return SVal::bit(b);
    }
    let w = t.width();
    SVal::of_ty(v.ext(w.max(v.bits.len()))[..w].to_vec(), t)
}

/// the value clamped into type `t` (`.sat::<T>()`, `ovf = sat`)
pub fn sat_to(g: &mut Builder, v: &SVal, t: Ty) -> SVal {
    let (lo, hi) = t.range();
    if v.lo >= lo && v.hi <= hi { return wrap_to(v, t); }
    let mut x = v.clone();
    if v.hi > hi {
        let k = SVal::konst(hi);
        let over = compare(g, &x, &k, ">");
        x = select(g, over.bits[0], &k, &x);
    }
    if v.lo < lo {
        let k = SVal::konst(lo);
        let under = compare(g, &x, &k, "<");
        x = select(g, under.bits[0], &k, &x);
    }
    x.lo = x.lo.max(lo);
    x.hi = x.hi.min(hi);
    wrap_to(&x, t)
}

/// the bool a value stands for (non-zero)
pub fn truth(g: &mut Builder, v: &SVal) -> Bit {
    if v.boolean { v.bits[0] } else { g.reduce_or(&v.bits) }
}
