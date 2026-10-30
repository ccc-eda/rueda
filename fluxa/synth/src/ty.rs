//! Hardware values. A scalar (`SVal`) is bits plus the exact range of the integer they hold
//! (spec §3.2) and, for fixed point, its fraction bits: the value is `raw / 2^frac`, the range
//! is the raw integer's. Widths follow the ranges, so arithmetic never overflows: `u8 + u8` is
//! `int<0..=510>`, 9 bits; `i16 * fixed<1,15>` is a raw product with 15 fraction bits.
//! Arrays and tuples are trees of scalars (`HV`).

use eda::netlist::{const_bits, pad, sext, Bit, Builder};
use fluxa::value::{int_bits, Ovf, Round, RtType};

pub type R<T> = Result<T, String>;

/// a type that has hardware
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Int { lo: i128, hi: i128 },
    Bool,
    /// signed fixed point: `i` integer and `f` fraction bits; conversions into it round and
    /// handle overflow as the type says
    Fixed { i: u32, f: u32, round: Round, ovf: Ovf },
    Array(Box<Ty>, usize),
    Tuple(Vec<Ty>),
}

impl Ty {
    /// bits in hardware (all the elements of an array)
    pub fn width(&self) -> usize {
        match self {
            Ty::Int { lo, hi } => int_bits(*lo, *hi) as usize,
            Ty::Bool => 1,
            Ty::Fixed { i, f, .. } => (i + f).max(1) as usize,
            Ty::Array(e, n) => e.width() * n,
            Ty::Tuple(ts) => ts.iter().map(|t| t.width()).sum(),
        }
    }
    /// a scalar's raw range
    pub fn range(&self) -> (i128, i128) {
        match self {
            Ty::Int { lo, hi } => (*lo, *hi),
            Ty::Bool => (0, 1),
            Ty::Fixed { i, f, .. } => { let w = i + f; if w == 0 { (0, 0) } else { (-(1i128 << (w - 1)), (1i128 << (w - 1)) - 1) } }
            _ => (0, 0),
        }
    }
    pub fn frac(&self) -> u32 { if let Ty::Fixed { f, .. } = self { *f } else { 0 } }
    pub fn is_scalar(&self) -> bool { !matches!(self, Ty::Array(..) | Ty::Tuple(..)) }
    pub fn of_rt(t: &RtType) -> R<Ty> {
        match t {
            RtType::Int { lo, hi } => Ok(Ty::Int { lo: *lo, hi: *hi }),
            RtType::Bool => Ok(Ty::Bool),
            RtType::Fixed { i, f, round, ovf } => Ok(Ty::Fixed { i: *i, f: *f, round: *round, ovf: *ovf }),
            RtType::Array(e, n) => Ok(Ty::Array(Box::new(Ty::of_rt(e)?), *n)),
            RtType::Tuple(ts) => Ok(Ty::Tuple(ts.iter().map(Ty::of_rt).collect::<R<_>>()?)),
            RtType::Chan(..) => Err("a channel is not a value".into()),
            RtType::Mem(e, n) => Ok(Ty::Array(Box::new(Ty::of_rt(e)?), *n)),
            RtType::Any => Err("this value needs a type for synthesis".into()),
        }
    }
}

/// a scalar in hardware
#[derive(Clone, Debug)]
pub struct SVal {
    pub bits: Vec<Bit>,
    pub lo: i128,
    pub hi: i128,
    pub boolean: bool,
    /// fraction bits (fixed point); 0 for integers
    pub frac: u32,
}

/// a value in hardware: a scalar, or an array / tuple of values
#[derive(Clone, Debug)]
pub enum HV {
    S(SVal),
    A(Vec<HV>),
}

impl SVal {
    pub fn signed(&self) -> bool { self.lo < 0 }
    /// the type it has (a fixed-point value keeps the narrowest fixed type holding its range)
    pub fn ty(&self) -> Ty {
        if self.boolean { return Ty::Bool; }
        if self.frac > 0 {
            let w = int_bits(self.lo.min(-1), self.hi) as u32;
            return Ty::Fixed { i: w.saturating_sub(self.frac), f: self.frac, round: Round::Trunc, ovf: Ovf::Wrap };
        }
        Ty::Int { lo: self.lo, hi: self.hi }
    }
    pub fn konst(n: i128) -> SVal {
        let w = int_bits(n, n);
        SVal { bits: const_bits(n as u128, w), lo: n, hi: n, boolean: false, frac: 0 }
    }
    pub fn fixed_const(raw: i128, frac: u32) -> SVal { SVal { frac, ..SVal::konst(raw) } }
    pub fn bool_const(b: bool) -> SVal { SVal { bits: vec![Bit::of(b)], lo: 0, hi: 1, boolean: true, frac: 0 } }
    pub fn bit(b: Bit) -> SVal { SVal { bits: vec![b], lo: 0, hi: 1, boolean: true, frac: 0 } }
    /// `bits` holding a value of scalar type `t`
    pub fn of_ty(bits: Vec<Bit>, t: &Ty) -> SVal {
        let (lo, hi) = t.range();
        SVal { bits, lo, hi, boolean: *t == Ty::Bool, frac: t.frac() }
    }
    /// the bits extended to `w` (sign- or zero-extended)
    pub fn ext(&self, w: usize) -> Vec<Bit> {
        if self.signed() { sext(&self.bits, w) } else { pad(&self.bits, w) }
    }
    /// the raw constant, when it is one
    pub fn as_const(&self) -> Option<i128> {
        if self.lo == self.hi { return Some(self.lo); }
        let mut v: i128 = 0;
        for (i, b) in self.bits.iter().enumerate() {
            match b { Bit::C0 => {} Bit::C1 => v |= 1 << i, Bit::W(_) => return None }
        }
        if self.signed() && self.bits.last() == Some(&Bit::C1) { v -= 1 << self.bits.len(); }
        Some(v)
    }
    /// the bits re-fitted to the range (after an operation that changed the range, not the value)
    pub fn normalized(self) -> SVal {
        let w = int_bits(self.lo, self.hi) as usize;
        let bits = if self.bits.len() >= w { self.bits[..w].to_vec() } else if self.lo < 0 { sext(&self.bits, w) } else { pad(&self.bits, w) };
        SVal { bits, ..self }
    }
}

/// a value of range [lo, hi] from the low bits of a (wide enough) result
fn ranged(bits: Vec<Bit>, lo: i128, hi: i128, frac: u32) -> SVal {
    let w = int_bits(lo, hi) as usize;
    let mut b = bits;
    b.truncate(w);
    while b.len() < w { b.push(Bit::C0); }
    SVal { bits: b, lo, hi, boolean: false, frac }
}

/// the value with `f` fraction bits (f >= its own): the raw integer shifted left
fn upscale(a: &SVal, f: u32) -> SVal { if a.frac >= f { a.clone() } else { SVal { frac: f, ..shl_const(a, f - a.frac) } } }

/// both values with the same fraction bits
fn align(a: &SVal, b: &SVal) -> (SVal, SVal) { let f = a.frac.max(b.frac); (upscale(a, f), upscale(b, f)) }

pub fn add(g: &mut Builder, a: &SVal, b: &SVal) -> SVal {
    let (a, b) = align(a, b);
    let (lo, hi) = (a.lo + b.lo, a.hi + b.hi);
    let w = int_bits(lo, hi) as usize;
    let s = g.add(&a.ext(w), &b.ext(w));
    ranged(s, lo, hi, a.frac)
}

pub fn sub(g: &mut Builder, a: &SVal, b: &SVal) -> SVal {
    let (a, b) = align(a, b);
    let (lo, hi) = (a.lo - b.hi, a.hi - b.lo);
    let w = int_bits(lo, hi) as usize;
    let s = g.sub(&a.ext(w), &b.ext(w));
    ranged(s, lo, hi, a.frac)
}

pub fn neg(g: &mut Builder, a: &SVal) -> SVal { sub(g, &SVal { frac: a.frac, ..SVal::konst(0) }, a) }

pub fn mul(g: &mut Builder, a: &SVal, b: &SVal) -> SVal {
    let p = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
    let (lo, hi) = (*p.iter().min().unwrap(), *p.iter().max().unwrap());
    let w = int_bits(lo, hi) as usize;
    let m = g.mul(&a.ext(w), &b.ext(w));
    ranged(m, lo, hi, a.frac + b.frac)
}

/// quotient and remainder of integers, truncating toward zero (like the simulator)
pub fn divrem(g: &mut Builder, a: &SVal, b: &SVal) -> R<(SVal, SVal)> {
    if a.frac > 0 || b.frac > 0 { return Err("division of fixed-point values is not synthesized".into()); }
    if b.lo <= 0 && b.hi >= 0 { return Err("division: the divisor's range includes 0 (E0102)".into()); }
    let amax = a.lo.abs().max(a.hi.abs());
    let bmin = b.lo.abs().min(b.hi.abs()).max(1);
    let qmax = amax / bmin;
    let (qlo, qhi) = if a.signed() || b.signed() { (-qmax, qmax) } else { (a.lo / b.hi.max(1), qmax) };
    let rmax = b.lo.abs().max(b.hi.abs()) - 1;
    let (rlo, rhi) = if a.signed() { (-rmax, rmax) } else { (0, rmax.min(a.hi)) };
    let w = int_bits(a.lo.min(b.lo), a.hi.max(b.hi)) as usize + 1;
    let (q, r) = if a.signed() || b.signed() { g.sdivmod(&a.ext(w), &b.ext(w)) } else { g.udivmod(&a.ext(w), &b.ext(w)) };
    Ok((ranged(q, qlo, qhi, 0), ranged(r, rlo, rhi, 0)))
}

pub fn shl_const(a: &SVal, k: u32) -> SVal {
    let (lo, hi) = (a.lo << k, a.hi << k);
    let w = int_bits(lo, hi) as usize;
    let mut b = vec![Bit::C0; k as usize];
    b.extend(a.ext(w.saturating_sub(k as usize)));
    ranged(b, lo, hi, a.frac)
}

/// arithmetic shift right (floor)
pub fn shr_const(a: &SVal, k: u32) -> SVal {
    let (lo, hi) = (a.lo >> k, a.hi >> k);
    let fill = if a.signed() { *a.bits.last().unwrap() } else { Bit::C0 };
    let mut b: Vec<Bit> = a.bits.iter().skip(k as usize).copied().collect();
    while b.len() < a.bits.len().max(1) { b.push(fill); }
    let w = int_bits(lo, hi) as usize;
    let b = if a.signed() { sext(&b, w) } else { pad(&b, w) };
    ranged(b, lo, hi, a.frac)
}

/// a shift by a variable amount: the range assumes the largest shift the amount allows
pub fn shift(g: &mut Builder, a: &SVal, amt: &SVal, left: bool) -> R<SVal> {
    if amt.lo < 0 { return Err("a shift amount must not be negative".into()); }
    if left {
        let k = amt.hi.min(120) as u32;
        let (lo, hi) = (a.lo.min(a.lo << k), a.hi.max(a.hi << k));
        let w = int_bits(lo, hi) as usize;
        let s = g.shift(&a.ext(w), &amt.bits, true, Bit::C0);
        Ok(ranged(s, lo, hi, a.frac))
    } else {
        let fill = if a.signed() { *a.bits.last().unwrap() } else { Bit::C0 };
        let s = g.shift(&a.bits, &amt.bits, false, fill);
        let (lo, hi) = (a.lo.min(0), a.hi.max(0));
        let w = int_bits(lo, hi) as usize;
        let s = if a.signed() { sext(&s, w) } else { pad(&s, w) };
        Ok(ranged(s, lo, hi, a.frac))
    }
}

/// &, |, ^ bit by bit: the result has the operands' common width
pub fn bitop(g: &mut Builder, a: &SVal, b: &SVal, op: char) -> SVal {
    if a.boolean && b.boolean {
        let x = match op { '&' => g.and(a.bits[0], b.bits[0]), '|' => g.or(a.bits[0], b.bits[0]), _ => g.xor(a.bits[0], b.bits[0]) };
        return SVal::bit(x);
    }
    let w = int_bits(a.lo.min(b.lo), a.hi.max(b.hi)) as usize;
    let (x, y) = (a.ext(w), b.ext(w));
    let bits: Vec<Bit> = (0..w).map(|i| match op { '&' => g.and(x[i], y[i]), '|' => g.or(x[i], y[i]), _ => g.xor(x[i], y[i]) }).collect();
    let (lo, hi) = if a.signed() || b.signed() { (-(1i128 << (w - 1)), (1i128 << (w - 1)) - 1) } else { (0, (1i128 << w) - 1) };
    SVal { bits, lo, hi, boolean: false, frac: 0 }
}

pub fn not_bits(g: &mut Builder, a: &SVal) -> SVal {
    if a.boolean { return SVal::bit(g.not(a.bits[0])); }
    let bits = g.notv(&a.bits);
    // ~x = -x - 1
    SVal { bits, lo: -a.hi - 1, hi: -a.lo - 1, boolean: false, frac: 0 }.normalized()
}

/// a < b, a == b, … as one bit (fixed point aligned first)
pub fn compare(g: &mut Builder, a: &SVal, b: &SVal, op: &str) -> SVal {
    let (a, b) = align(a, b);
    let w = int_bits(a.lo.min(b.lo), a.hi.max(b.hi)) as usize + 1;
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

/// `c ? a : b`: the result's range covers both
pub fn select(g: &mut Builder, c: Bit, a: &SVal, b: &SVal) -> SVal {
    if a.boolean && b.boolean { return SVal::bit(g.mux(c, b.bits[0], a.bits[0])); }
    let (a, b) = align(a, b);
    let (lo, hi) = (a.lo.min(b.lo), a.hi.max(b.hi));
    let w = int_bits(lo, hi) as usize;
    let bits = g.muxv(c, &b.ext(w), &a.ext(w));
    SVal { bits, lo, hi, boolean: false, frac: a.frac }
}

/// the raw integer shifted right by `s` bits, rounded as the simulator's `round_shift` does
fn round_shift(g: &mut Builder, a: &SVal, s: u32, r: Round) -> SVal {
    if s == 0 { return a.clone(); }
    match r {
        Round::Trunc => shr_const(a, s),
        Round::Nearest => {
            let half = SVal::konst(1i128 << (s - 1));
            let t = add(g, &SVal { frac: 0, ..a.clone() }, &half);
            SVal { frac: a.frac, ..shr_const(&t, s) }
        }
        Round::Even => {
            let q = shr_const(&SVal { frac: 0, ..a.clone() }, s);
            // the remainder: the low s bits, unsigned
            let low: Vec<Bit> = a.ext(s as usize)[..s as usize].to_vec();
            let rem = SVal { bits: low, lo: 0, hi: (1i128 << s) - 1, boolean: false, frac: 0 };
            let half = SVal::konst(1i128 << (s - 1));
            let gt = compare(g, &rem, &half, ">").bits[0];
            let eq = compare(g, &rem, &half, "==").bits[0];
            let odd = q.bits[0];
            let up = { let e = g.and(eq, odd); g.or(gt, e) };
            let inc = SVal { bits: vec![up], lo: 0, hi: 1, boolean: false, frac: 0 };
            SVal { frac: a.frac, ..add(g, &q, &inc) }
        }
    }
}

/// convert a scalar to scalar type `t` (spec T-FCONV): the fraction bits to `t`'s (rounding
/// as `round` says when there are fewer), then the range (`ovf`: sat clamps, wrap and check keep
/// the low bits: check is an assertion in simulation and no logic in hardware)
pub fn convert(g: &mut Builder, v: &SVal, t: &Ty, round: Round, ovf: Ovf) -> SVal {
    if *t == Ty::Bool { let b = truth(g, v); return SVal::bit(b); }
    let tf = t.frac();
    let x = if v.frac > tf { let r = round_shift(g, v, v.frac - tf, round); SVal { frac: tf, ..r } } else { upscale(v, tf) };
    let (lo, hi) = t.range();
    let x = if ovf == Ovf::Sat && (x.lo < lo || x.hi > hi) {
        let mut y = x.clone();
        if x.hi > hi { let k = SVal { frac: tf, ..SVal::konst(hi) }; let over = compare(g, &y, &k, ">").bits[0]; y = select(g, over, &k, &y); }
        if x.lo < lo { let k = SVal { frac: tf, ..SVal::konst(lo) }; let under = compare(g, &y, &k, "<").bits[0]; y = select(g, under, &k, &y); }
        y
    } else { x };
    let w = t.width();
    SVal::of_ty(x.ext(w.max(x.bits.len()))[..w].to_vec(), t)
}

/// the value as type `t` at a landing site (`let x: T = e`, an argument, a register): the
/// type's own rounding (or trunc) and wrap
pub fn fit(g: &mut Builder, v: &SVal, t: &Ty) -> SVal {
    let r = if let Ty::Fixed { round, .. } = t { *round } else { Round::Trunc };
    convert(g, v, t, r, Ovf::Wrap)
}

/// the bool a value stands for (non-zero)
pub fn truth(g: &mut Builder, v: &SVal) -> Bit {
    if v.boolean { v.bits[0] } else { g.reduce_or(&v.bits) }
}

// ---------------------------------------------------------------- trees

impl HV {
    pub fn s(&self) -> R<&SVal> { match self { HV::S(s) => Ok(s), HV::A(_) => Err("an array or tuple where a number is needed".into()) } }
    pub fn ty(&self) -> Ty {
        match self {
            HV::S(s) => s.ty(),
            HV::A(xs) => {
                if xs.is_empty() { return Ty::Array(Box::new(Ty::Int { lo: 0, hi: 0 }), 0); }
                let ts: Vec<Ty> = xs.iter().map(|x| x.ty()).collect();
                // an array when the elements fit one type, else a tuple
                if ts.iter().all(|t| t.is_scalar() == ts[0].is_scalar() && same_shape(t, &ts[0])) {
                    let e = ts.iter().skip(1).fold(ts[0].clone(), |a, b| widen(Some(a), b.clone()));
                    Ty::Array(Box::new(e), xs.len())
                } else { Ty::Tuple(ts) }
            }
        }
    }
    /// every bit, leaves in order
    pub fn flat(&self) -> Vec<Bit> {
        match self { HV::S(s) => s.bits.clone(), HV::A(xs) => xs.iter().flat_map(|x| x.flat()).collect() }
    }
    /// bits laid out as type `t` (as `flat` makes them)
    pub fn of_ty(bits: &[Bit], t: &Ty) -> HV {
        match t {
            Ty::Array(e, n) => { let w = e.width(); HV::A((0..*n).map(|k| HV::of_ty(&bits[k * w..(k + 1) * w], e)).collect()) }
            Ty::Tuple(ts) => { let mut at = 0; HV::A(ts.iter().map(|t| { let w = t.width(); let v = HV::of_ty(&bits[at..at + w], t); at += w; v }).collect()) }
            t => HV::S(SVal::of_ty(bits.to_vec(), t)),
        }
    }
    pub fn konst_of(t: &Ty) -> HV { HV::of_ty(&vec![Bit::C0; t.width()], t) }
}

fn same_shape(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Array(x, n), Ty::Array(y, m)) => n == m && same_shape(x, y),
        (Ty::Tuple(x), Ty::Tuple(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same_shape(a, b)),
        (a, b) => a.is_scalar() && b.is_scalar(),
    }
}

/// a type holding values of both (the type of an untyped variable grows with what it is given)
pub fn widen(cur: Option<Ty>, t: Ty) -> Ty {
    match (cur, t) {
        (None, t) => t,
        (Some(Ty::Bool), Ty::Bool) => Ty::Bool,
        (Some(Ty::Array(a, n)), Ty::Array(b, m)) if n == m => Ty::Array(Box::new(widen(Some(*a), *b)), n),
        (Some(Ty::Tuple(a)), Ty::Tuple(b)) if a.len() == b.len() => Ty::Tuple(a.into_iter().zip(b).map(|(x, y)| widen(Some(x), y)).collect()),
        (Some(a), b) if a.is_scalar() && b.is_scalar() => {
            let f = a.frac().max(b.frac());
            let ((l1, h1), (l2, h2)) = (a.range(), b.range());
            let (l1, h1) = (l1 << (f - a.frac()), h1 << (f - a.frac()));
            let (l2, h2) = (l2 << (f - b.frac()), h2 << (f - b.frac()));
            let (lo, hi) = (l1.min(l2), h1.max(h2));
            if f == 0 { Ty::Int { lo, hi } } else {
                let w = int_bits(lo.min(-1), hi) as u32;
                Ty::Fixed { i: w.saturating_sub(f), f, round: Round::Trunc, ovf: Ovf::Wrap }
            }
        }
        (Some(a), _) => a,
    }
}

/// the value as type `t`, leaf by leaf
pub fn fit_hv(g: &mut Builder, v: &HV, t: &Ty) -> R<HV> {
    match (v, t) {
        (HV::S(s), t) if t.is_scalar() => Ok(HV::S(fit(g, s, t))),
        (HV::A(xs), Ty::Array(e, n)) if xs.len() == *n => Ok(HV::A(xs.iter().map(|x| fit_hv(g, x, e)).collect::<R<_>>()?)),
        (HV::A(xs), Ty::Tuple(ts)) if xs.len() == ts.len() => Ok(HV::A(xs.iter().zip(ts).map(|(x, t)| fit_hv(g, x, t)).collect::<R<_>>()?)),
        (v, t) => Err(format!("a value of type {:?} does not fit {:?}", v.ty(), t)),
    }
}

/// `c ? a : b` for trees of the same shape
pub fn select_hv(g: &mut Builder, c: Bit, a: &HV, b: &HV) -> R<HV> {
    match (a, b) {
        (HV::S(x), HV::S(y)) => Ok(HV::S(select(g, c, x, y))),
        (HV::A(xs), HV::A(ys)) if xs.len() == ys.len() => Ok(HV::A(xs.iter().zip(ys).map(|(x, y)| select_hv(g, c, x, y)).collect::<R<_>>()?)),
        _ => Err("the two sides have different shapes".into()),
    }
}
