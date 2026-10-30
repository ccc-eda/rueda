//! 4-state (0/1/x) bit vectors up to MAXW bits.

pub use bits::{mask, Bits, LIMBS, MAXW};
use std::cmp::Ordering;

/// Invariant: wherever an `x` bit is set, the matching `v` bit is 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Val {
    pub w: u32,
    pub v: Bits,
    pub x: Bits,
    pub s: bool,
}

impl Val {
    pub fn new(w: u32, v: u128) -> Self { Val::of(w, Bits::from_u128(v)) }
    pub fn of(w: u32, v: Bits) -> Self {
        let w = w.clamp(1, MAXW);
        Val { w, v: v & mask(w), x: Bits::ZERO, s: false }
    }
    pub fn allx(w: u32) -> Self {
        let w = w.clamp(1, MAXW);
        Val { w, v: Bits::ZERO, x: mask(w), s: false }
    }
    pub fn bit(b: bool) -> Self { Val::new(1, b as u128) }
    pub fn signed(mut self, s: bool) -> Self { self.s = s; self }

    /// Some(true) if any bit is a known 1, Some(false) if all bits are known 0, else None (x).
    pub fn truth(&self) -> Option<bool> {
        if !self.v.is_zero() { Some(true) } else if self.x.is_zero() { Some(false) } else { None }
    }

    pub fn resize(&self, w: u32) -> Val {
        let w = w.clamp(1, MAXW);
        if w <= self.w {
            return Val { w, v: self.v & mask(w), x: self.x & mask(w), s: self.s };
        }
        let ext = mask(w) & !mask(self.w);
        let (mut v, mut x) = (self.v, self.x);
        if self.s {
            if self.v.bit(self.w - 1) { v = v | ext } else if self.x.bit(self.w - 1) { x = x | ext }
        }
        Val { w, v, x, s: self.s }
    }

    /// sign bit set (signed values)
    pub fn negative(&self) -> bool { self.s && self.v.bit(self.w - 1) }

    /// two's complement order of two signed values of the same width
    pub fn scmp(&self, o: &Val) -> Ordering {
        match (self.negative(), o.negative()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => self.v.cmp(&o.v),
        }
    }

    /// magnitude of a signed value (for signed division)
    pub fn abs(&self) -> Bits { if self.negative() { self.v.wrapping_neg() & mask(self.w) } else { self.v } }

    pub fn to_i128(&self) -> i128 {
        if self.s && self.w <= 128 && self.v.bit(self.w - 1) {
            (self.v.lo() | !mask(self.w).lo()) as i128
        } else {
            self.v.lo() as i128
        }
    }

    pub fn fmt_bin(&self) -> String {
        (0..self.w).rev().map(|i| {
            if self.x.bit(i) { 'x' } else if self.v.bit(i) { '1' } else { '0' }
        }).collect()
    }

    pub fn fmt_hex(&self) -> String {
        let n = self.w.div_ceil(4);
        (0..n).rev().map(|i| {
            let sh = i * 4;
            let valid = mask((self.w - sh).min(4)).lo() as u32;
            let xm = ((self.x >> sh).lo() & 0xf) as u32;
            let vv = ((self.v >> sh).lo() & 0xf) as u32;
            if xm == valid { 'x' } else if xm != 0 { 'X' } else { std::char::from_digit(vv, 16).unwrap() }
        }).collect()
    }

    pub fn fmt_oct(&self) -> String {
        if !self.x.is_zero() { return "x".into(); }
        let n = self.w.div_ceil(3);
        (0..n).rev().map(|i| std::char::from_digit(((self.v >> (i * 3)).lo() & 7) as u32, 8).unwrap()).collect()
    }

    pub fn fmt_dec(&self) -> String {
        if !self.x.is_zero() {
            return if self.x == mask(self.w) { "x".into() } else { "X".into() };
        }
        if self.negative() { format!("-{}", self.abs().to_dec()) } else { self.v.to_dec() }
    }
}
