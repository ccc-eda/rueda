//! A readable RTL view of a task: the same lowering (`lower.rs`) as the gate netlist, printed as
//! SystemVerilog. The datapath is word-level: a value is an *expression*, not a bit vector, so
//! the RTL reads like the program (`acc + a * b`) instead of a gate soup.
//!
//! The ranges (the widths) come from `ty.rs`, which the gate backend uses too: the two backends
//! cannot disagree about a width, only about how the bits are computed.

use crate::ty::{rng_add, rng_bitop, rng_convert, rng_divrem, rng_mul, rng_neg, rng_not, rng_select, rng_shift, rng_shl, rng_shr, rng_sub, Rng, Ty, R};
use fluxa::value::{Ovf, Round};

/// a value in the RTL: an expression and the range it holds
#[derive(Clone, Debug)]
pub struct TV {
    /// the expression (SystemVerilog)
    pub text: String,
    /// the range: the value's bounds, its fraction bits
    pub rng: Rng,
}

impl TV {
    pub fn konst(n: i128) -> TV { TV { text: lit(n, 1, false), rng: Rng::konst(n) } }
    pub fn boolean(text: String) -> TV { TV { text, rng: Rng::bool() } }
    /// the bits the value needs
    pub fn width(&self) -> usize { if self.rng.boolean { 1 } else { self.rng.width().max(1) } }
    /// the value as it is written into a register of `w` bits (the high bits are dropped)
    pub fn at(&self, w: usize) -> String { if self.width() == w { format!("({})", self.text) } else { format!("{}'({})", w, self.text) } }
}

/// a sized decimal literal (`8'd255`, `-8'sd128`)
pub fn lit(v: i128, w: usize, signed: bool) -> String {
    let w = w.max(1);
    if v < 0 { format!("-{}'sd{}", w, -v) } else if signed { format!("{}'sd{}", w, v) } else { format!("{}'d{}", w, v) }
}

/// the value extended (or truncated) to `w` bits, signed by its own range
fn ext(t: &TV, w: usize) -> String {
    let ow = t.width();
    if ow == w { return format!("({})", t.text); }
    if ow > w { return format!("{}'({})", w, t.text); }
    let fill = if t.rng.signed() { "1'b1" } else { "1'b0" };
    format!("{{{}{{{}}}, ({})}}", w - ow, fill, t.text)
}

/// the value scaled to `f` fraction bits (the raw integer shifted left)
fn upscale(t: &TV, f: u32) -> TV {
    if t.rng.frac >= f { return t.clone(); }
    let k = f - t.rng.frac;
    TV { text: format!("(({}) << {})", t.text, k), rng: t.rng.upscale(f) }
}

/// `a + b`
pub fn add(a: &TV, b: &TV) -> TV {
    let f = a.rng.frac.max(b.rng.frac);
    let (a, b) = (upscale(a, f), upscale(b, f));
    let r = rng_add(a.rng, b.rng);
    let w = r.width().max(1);
    TV { text: format!("({} + {})", ext(&a, w), ext(&b, w)), rng: r }
}

/// `a - b`
pub fn sub(a: &TV, b: &TV) -> TV {
    let f = a.rng.frac.max(b.rng.frac);
    let (a, b) = (upscale(a, f), upscale(b, f));
    let r = rng_sub(a.rng, b.rng);
    let w = r.width().max(1);
    TV { text: format!("({} - {})", ext(&a, w), ext(&b, w)), rng: r }
}

/// `-a`
pub fn neg(a: &TV) -> TV {
    let r = rng_neg(a.rng);
    let w = r.width().max(1);
    TV { text: format!("(-{})", ext(a, w)), rng: r }
}

/// `a * b`
pub fn mul(a: &TV, b: &TV) -> TV {
    let r = rng_mul(a.rng, b.rng);
    let w = r.width().max(1);
    TV { text: format!("({} * {})", ext(a, w), ext(b, w)), rng: r }
}

/// `a / b` and `a % b` (truncating toward zero, like the simulator)
pub fn divrem(a: &TV, b: &TV) -> R<(TV, TV)> {
    let (q, r) = rng_divrem(a.rng, b.rng)?;
    // the division happens at the width of the operands (one bit more, as the gates do)
    let w = (a.rng.lo.min(b.rng.lo).abs().max(a.rng.hi.abs().max(b.rng.hi.abs())) as usize).max(a.width()).max(b.width()) + 1;
    let (x, y) = (ext(a, w), ext(b, w));
    let signed = a.rng.signed() || b.rng.signed();
    let (x, y) = if signed { (format!("$signed({})", x), format!("$signed({})", y)) } else { (x, y) };
    Ok((TV { text: format!("({} / {})", x, y), rng: q }, TV { text: format!("({} % {})", x, y), rng: r }))
}

/// `a << k`
pub fn shl(a: &TV, k: u32) -> TV {
    let r = rng_shl(a.rng, k);
    let w = r.width().max(1);
    TV { text: format!("({} << {})", ext(a, w), k), rng: r }
}

/// `a >> k` (floor: an arithmetic shift of a signed value)
pub fn shr(a: &TV, k: u32) -> TV {
    let r = rng_shr(a.rng, k);
    let w = r.width().max(1);
    let x = ext(a, w);
    let text = if a.rng.signed() { format!("($signed({}) >>> {})", x, k) } else { format!("({} >> {})", x, k) };
    TV { text, rng: r }
}

/// a shift by a variable amount
pub fn shift(a: &TV, amt: &TV, left: bool) -> R<TV> {
    let r = rng_shift(a.rng, amt.rng, left)?;
    let w = r.width().max(1);
    let x = ext(a, w);
    let text = if left {
        format!("({} << {})", x, amt.text)
    } else if a.rng.signed() {
        format!("($signed({}) >>> {})", x, amt.text)
    } else {
        format!("({} >> {})", x, amt.text)
    };
    Ok(TV { text, rng: r })
}

/// `a & b`, `a | b`, `a ^ b`
pub fn bitop(a: &TV, b: &TV, op: char) -> TV {
    let r = rng_bitop(a.rng, b.rng);
    let w = r.width().max(1);
    TV { text: format!("({} {} {})", ext(a, w), op, ext(b, w)), rng: r }
}

/// `~a`
pub fn not(a: &TV) -> TV {
    let r = rng_not(a.rng);
    let w = r.width().max(1);
    TV { text: format!("(~{})", ext(a, w)), rng: r }
}

/// `a == b`, `a < b`, … as one bit
pub fn compare(a: &TV, b: &TV, op: &str) -> TV {
    let f = a.rng.frac.max(b.rng.frac);
    let (a, b) = (upscale(a, f), upscale(b, f));
    let w = (a.rng.lo.min(b.rng.lo).abs().max(a.rng.hi.abs().max(b.rng.hi.abs())) as usize).max(a.width()).max(b.width()) + 1;
    let (x, y) = (ext(&a, w), ext(&b, w));
    let signed = a.rng.signed() || b.rng.signed();
    let (x, y) = if signed { (format!("$signed({})", x), format!("$signed({})", y)) } else { (x, y) };
    let text = match op {
        "==" => format!("({} == {})", x, y),
        "!=" => format!("({} != {})", x, y),
        "<" => format!("({} < {})", x, y),
        ">" => format!("({} > {})", x, y),
        "<=" => format!("({} <= {})", x, y),
        _ => format!("({} >= {})", x, y),
    };
    TV { text, rng: Rng::bool() }
}

/// `c ? a : b`
pub fn select(c: &TV, a: &TV, b: &TV) -> TV {
    let r = rng_select(a.rng, b.rng);
    let w = r.width().max(1);
    TV { text: format!("(({}) ? {} : {})", c.text, ext(a, w), ext(b, w)), rng: r }
}

/// the value converted to type `t`: the fraction bits rounded as `round` says, then the range
/// (`ovf`: sat clamps, wrap keeps the low bits)
pub fn convert(v: &TV, t: &Ty, round: Round, ovf: Ovf) -> TV {
    let tf = t.frac();
    let x = if v.rng.frac > tf { round_shift(v, v.rng.frac - tf, round) } else { upscale(v, tf) };
    let (lo, hi) = t.range();
    let text = if ovf == Ovf::Sat && (x.rng.lo < lo || x.rng.hi > hi) {
        // the comparisons, not `min`/`max`: those are not in every tool (iverilog has no `min`)
        let w = t.width().max(1);
        let signed = lo < 0;
        let hi_l = lit(hi, w, signed);
        let lo_l = lit(lo, w, signed);
        format!("(({}) > {}) ? {} : ((({}) < {}) ? {} : ({}))", x.text, hi_l, hi_l, x.text, lo_l, lo_l, x.text)
    } else {
        x.text
    };
    TV { text, rng: rng_convert(x.rng, t) }
}

/// the raw integer shifted right by `s` bits, rounded as the simulator does
fn round_shift(v: &TV, s: u32, r: Round) -> TV {
    if s == 0 { return v.clone(); }
    match r {
        Round::Trunc => shr(v, s),
        Round::Nearest => {
            // (v + half) >> s
            let half = TV { text: lit(1i128 << (s - 1), v.width(), false), rng: Rng::konst(1i128 << (s - 1)) };
            let t = add(&TV { text: v.text.clone(), rng: v.rng.with_frac(0) }, &half);
            shr(&t, s)
        }
        Round::Even => {
            // the quotient, the remainder (the low s bits, unsigned) and the tie
            let q = shr(&TV { text: v.text.clone(), rng: v.rng.with_frac(0) }, s);
            let w = (s as usize).max(1);
            let rem = TV { text: format!("(({}) & {})", v.text, lit((1i128 << s) - 1, w, false)), rng: Rng::int(0, (1i128 << s) - 1) };
            let half = TV { text: lit(1i128 << (s - 1), w, false), rng: Rng::konst(1i128 << (s - 1)) };
            let gt = compare(&rem, &half, ">");
            let eq = compare(&rem, &half, "==");
            let odd = TV { text: format!("({}[0])", q.text), rng: Rng::bool() };
            let up = TV { text: format!("({} | ({} & {}))", gt.text, eq.text, odd.text), rng: Rng::bool() };
            let inc = TV { text: up.text, rng: Rng::int(0, 1) };
            let t = add(&q, &inc);
            TV { text: t.text, rng: t.rng.with_frac(v.rng.frac) }
        }
    }
}

/// the value as type `t` at a landing site (`let x: T = e`, a register): the type's own rounding
/// and wrap
pub fn fit(v: &TV, t: &Ty) -> TV {
    let r = if let Ty::Fixed { round, .. } = t { *round } else { Round::Trunc };
    convert(v, t, r, Ovf::Wrap)
}

/// the bool a value stands for (not zero)
pub fn truth(v: &TV) -> TV {
    if v.rng.boolean { return v.clone(); }
    TV { text: format!("(({}) != 0)", v.text), rng: Rng::bool() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::{SVal, Ty};

    fn v(n: i128, lo: i128, hi: i128) -> TV { TV { text: lit(n, 1, false), rng: Rng::int(lo, hi) } }

    /// the text backend and the gate backend must agree about every range
    #[test]
    fn the_ranges_agree_with_the_gates() {
        let cases = [(0i128, 255i128), (-128, 127), (-1, 1), (0, 65535), (-32768, 32767)];
        for (alo, ahi) in cases {
            for (blo, bhi) in cases {
                let a = SVal { bits: vec![], lo: alo, hi: ahi, boolean: false, frac: 0 };
                let b = SVal { bits: vec![], lo: blo, hi: bhi, boolean: false, frac: 0 };
                let ta = TV { text: "a".into(), rng: Rng::of(&a) };
                let tb = TV { text: "b".into(), rng: Rng::of(&b) };
                // add, sub, mul: the same bounds as the gate operations
                let (alo2, ahi2) = (a.lo + b.lo, a.hi + b.hi);
                assert_eq!(add(&ta, &tb).rng, Rng::int(alo2, ahi2), "add {} {}", alo, ahi);
                assert_eq!(sub(&ta, &tb).rng, Rng::int(a.lo - b.hi, a.hi - b.lo), "sub");
                let p = [a.lo * b.lo, a.lo * b.hi, a.hi * b.lo, a.hi * b.hi];
                assert_eq!(mul(&ta, &tb).rng, Rng::int(*p.iter().min().unwrap(), *p.iter().max().unwrap()), "mul");
                // the widths the RTL prints are the widths the gates use
                assert_eq!(add(&ta, &tb).width(), crate::ty::Ty::Int { lo: alo2, hi: ahi2 }.width());
            }
        }
    }

    /// a conversion to a type keeps the type's range (the gates do the same)
    #[test]
    fn a_conversion_takes_the_types_range() {
        let t = Ty::Int { lo: 0, hi: 255 };
        let x = v(300, 0, 65535);
        let c = convert(&x, &t, Round::Trunc, Ovf::Wrap);
        assert_eq!(c.rng, Rng::int(0, 255));
        assert_eq!(c.width(), 8);
        let s = convert(&x, &t, Round::Trunc, Ovf::Sat);
        assert_eq!(s.rng, Rng::int(0, 255));
        assert!(s.text.contains(">"), "the saturating conversion clamps: {}", s.text);
    }

    /// the fixed-point conversions round as the simulator does
    #[test]
    fn a_fixed_point_conversion_rounds() {
        // 3.375 = fixed<8,8> into fixed<2,2>: the range is [-2, 1.75], so it saturates to 1.75
        let x = TV { text: "x".into(), rng: Rng::fixed(864, 864, 8) };
        let t = Ty::Fixed { i: 2, f: 2, round: Round::Trunc, ovf: Ovf::Sat };
        let c = convert(&x, &t, Round::Trunc, Ovf::Sat);
        assert_eq!(c.rng, Rng::fixed(-8, 7, 2));
        assert!(c.text.contains(">"), "{}", c.text);
    }
}
