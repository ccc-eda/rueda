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

/// a value with the signedness of its range: the text says it, so that a cast extends it right
pub fn mk(text: String, rng: Rng) -> TV {
    let text = if rng.signed() && !text.starts_with("$signed(") { format!("$signed({})", text) } else { text };
    TV { text, rng }
}

impl TV {
    pub fn konst(n: i128) -> TV { let r = Rng::konst(n); mk(lit(n, r.width().max(1), false), r) }
    pub fn boolean(text: String) -> TV { TV { text, rng: Rng::bool() } }
    /// the bits the value needs
    pub fn width(&self) -> usize { if self.rng.boolean { 1 } else { self.rng.width().max(1) } }
    /// the value as it is written into a register of `w` bits (the high bits are dropped)
    pub fn at(&self, w: usize) -> String { if self.width() == w { format!("({})", self.text) } else { format!("{}'({})", w, self.text) } }
    /// the value when it is a constant
    pub fn as_const(&self) -> Option<i128> { if self.rng.lo == self.rng.hi { Some(self.rng.lo) } else { None } }
}

/// a sized decimal literal (`8'd255`, `-8'sd128`)
pub fn lit(v: i128, w: usize, signed: bool) -> String {
    let w = w.max(1);
    if v < 0 { format!("-{}'sd{}", w, -v) } else if signed { format!("{}'sd{}", w, v) } else { format!("{}'d{}", w, v) }
}

/// the value extended (or truncated) to `w` bits: a cast, which extends as the text says (the
/// text is signed when the value is, so the fill is the value's sign bit, like the gates)
fn ext(t: &TV, w: usize) -> String {
    let ow = t.width();
    if ow == w { return format!("({})", t.text); }
    format!("{}'({})", w, t.text)
}

/// the value scaled to `f` fraction bits (the raw integer shifted left)
fn upscale(t: &TV, f: u32) -> TV {
    if t.rng.frac >= f { return t.clone(); }
    let k = f - t.rng.frac;
    mk(format!("(({}) << {})", t.text, k), t.rng.upscale(f))
}

/// `a + b`
pub fn add(a: &TV, b: &TV) -> TV {
    let f = a.rng.frac.max(b.rng.frac);
    let (a, b) = (upscale(a, f), upscale(b, f));
    let r = rng_add(a.rng, b.rng);
    let w = r.width().max(1);
    mk(format!("({} + {})", ext(&a, w), ext(&b, w)), r)
}

/// `a - b`
pub fn sub(a: &TV, b: &TV) -> TV {
    let f = a.rng.frac.max(b.rng.frac);
    let (a, b) = (upscale(a, f), upscale(b, f));
    let r = rng_sub(a.rng, b.rng);
    let w = r.width().max(1);
    mk(format!("({} - {})", ext(&a, w), ext(&b, w)), r)
}

/// `-a`
pub fn neg(a: &TV) -> TV {
    let r = rng_neg(a.rng);
    let w = r.width().max(1);
    mk(format!("(-{})", ext(a, w)), r)
}

/// `a * b`
pub fn mul(a: &TV, b: &TV) -> TV {
    let r = rng_mul(a.rng, b.rng);
    let w = r.width().max(1);
    mk(format!("({} * {})", ext(a, w), ext(b, w)), r)
}

/// `a / b` and `a % b` (truncating toward zero, like the simulator)
pub fn divrem(a: &TV, b: &TV) -> R<(TV, TV)> {
    let (q, r) = rng_divrem(a.rng, b.rng)?;
    // the division happens at the width of the operands (one bit more, as the gates do)
    let signed = a.rng.signed() || b.rng.signed();
    let w = a.width().max(b.width()) + 1;
    let (x, y) = (ext(a, w), ext(b, w));
    let (x, y) = if signed { (format!("$signed({})", x), format!("$signed({})", y)) } else { (x, y) };
    Ok((mk(format!("({} / {})", x, y), q), mk(format!("({} % {})", x, y), r)))
}

/// `a << k`
pub fn shl(a: &TV, k: u32) -> TV {
    let r = rng_shl(a.rng, k);
    let w = r.width().max(1);
    mk(format!("({} << {})", ext(a, w), k), r)
}

/// `a >> k` (floor: an arithmetic shift of a signed value)
pub fn shr(a: &TV, k: u32) -> TV {
    let r = rng_shr(a.rng, k);
    let w = r.width().max(1);
    let x = ext(a, w);
    let text = if a.rng.signed() { format!("(({}) >>> {})", x, k) } else { format!("({} >> {})", x, k) };
    mk(text, r)
}

/// a shift by a variable amount
pub fn shift(a: &TV, amt: &TV, left: bool) -> R<TV> {
    let r = rng_shift(a.rng, amt.rng, left)?;
    let w = r.width().max(1);
    let x = ext(a, w);
    let text = if left {
        format!("({} << {})", x, amt.text)
    } else if a.rng.signed() {
        format!("(({}) >>> {})", x, amt.text)
    } else {
        format!("({} >> {})", x, amt.text)
    };
    Ok(mk(text, r))
}

/// `a & b`, `a | b`, `a ^ b`
pub fn bitop(a: &TV, b: &TV, op: char) -> TV {
    let r = rng_bitop(a.rng, b.rng);
    let w = r.width().max(1);
    mk(format!("({} {} {})", ext(a, w), op, ext(b, w)), r)
}

/// `~a`
pub fn not(a: &TV) -> TV {
    let r = rng_not(a.rng);
    let w = r.width().max(1);
    mk(format!("(~{})", ext(a, w)), r)
}

/// `a == b`, `a < b`, … as one bit
pub fn compare(a: &TV, b: &TV, op: &str) -> TV {
    let f = a.rng.frac.max(b.rng.frac);
    let (a, b) = (upscale(a, f), upscale(b, f));
    let signed = a.rng.signed() || b.rng.signed();
    let w = a.width().max(b.width()) + if signed { 1 } else { 0 };
    let (x, y) = (ext(&a, w), ext(&b, w));
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
    mk(format!("(({}) ? {} : {})", c.text, ext(a, w), ext(b, w)), r)
}

/// the value converted to type `t`: the fraction bits rounded as `round` says, then the range
/// (`ovf`: sat clamps, wrap keeps the low bits)
pub fn convert(v: &TV, t: &Ty, round: Round, ovf: Ovf) -> TV {
    let tf = t.frac();
    let x = if v.rng.frac > tf { round_shift(v, v.rng.frac - tf, round) } else { upscale(v, tf) };
    let (lo, hi) = t.range();
    let w = t.width().max(1);
    let text = if ovf == Ovf::Sat && (x.rng.lo < lo || x.rng.hi > hi) {
        // the comparisons, not `min`/`max`: those are not in every tool (iverilog has no `min`)
        let signed = lo < 0;
        let hi_l = lit(hi, w, signed);
        let lo_l = lit(lo, w, signed);
        format!("(({}) > {}) ? {} : ((({}) < {}) ? {} : ({}))", x.text, hi_l, hi_l, x.text, lo_l, lo_l, x.text)
    } else {
        // the cast keeps the low bits (the wrap) and extends as the value says
        format!("{}'({})", w, x.text)
    };
    // the value is of the type now: signed or not, as the type says
    let text = if t.range().0 < 0 { format!("$signed({})", text) } else { format!("$unsigned({})", text) };
    TV { text, rng: rng_convert(x.rng, t) }
}

/// the raw integer shifted right by `s` bits, rounded as the simulator does
fn round_shift(v: &TV, s: u32, r: Round) -> TV {
    if s == 0 { return v.clone(); }
    match r {
        Round::Trunc => shr(v, s),
        Round::Nearest => {
            // (v + half) >> s
            let half = mk(lit(1i128 << (s - 1), v.width(), false), Rng::konst(1i128 << (s - 1)));
            let t = add(&TV { text: v.text.clone(), rng: v.rng.with_frac(0) }, &half);
            shr(&t, s)
        }
        Round::Even => {
            // the quotient, the remainder (the low s bits, unsigned) and the tie
            let q = shr(&TV { text: v.text.clone(), rng: v.rng.with_frac(0) }, s);
            let w = (s as usize).max(1);
            let rem = mk(format!("(({}) & {})", v.text, lit((1i128 << s) - 1, w, false)), Rng::int(0, (1i128 << s) - 1));
            let half = mk(lit(1i128 << (s - 1), w, false), Rng::konst(1i128 << (s - 1)));
            let gt = compare(&rem, &half, ">");
            let eq = compare(&rem, &half, "==");
            let odd = TV { text: format!("({}[0])", q.text), rng: Rng::bool() };
            let up = TV { text: format!("({} | ({} & {}))", gt.text, eq.text, odd.text), rng: Rng::bool() };
            let inc = TV { text: up.text, rng: Rng::int(0, 1) };
            let t = add(&q, &inc);
            mk(t.text, t.rng.with_frac(v.rng.frac))
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

// ------------------------------------------------------------ the RTL of a task

use crate::lower::{Cfg, Term};
use fluxa::ast::*;
use fluxa::interp::Interp;
use fluxa::runtime::{block_on, Shared};
use fluxa::value::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// a value in the RTL: a scalar or an array
#[derive(Clone, Debug)]
pub enum RV {
    S(TV),
    A(Vec<RV>),
}

impl RV {
    pub fn s(&self) -> R<&TV> { match self { RV::S(t) => Ok(t), RV::A(_) => Err("this value is an array".into()) } }
    /// the value as one expression (an array: its elements, the last one first)
    pub fn text(&self) -> String {
        match self {
            RV::S(t) => t.text.clone(),
            RV::A(xs) => {
                let parts: Vec<String> = xs.iter().rev().map(|x| x.text()).collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }
    /// the type of the value
    pub fn ty(&self) -> Ty {
        match self {
            RV::S(t) => t.rng.ty(),
            RV::A(xs) => Ty::Array(Box::new(xs[0].ty()), xs.len()),
        }
    }
}

/// a name as a SystemVerilog identifier
pub fn ident(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.is_empty() || out.chars().next().unwrap().is_ascii_digit() { out.insert(0, '_'); }
    out
}

/// the range of a type
pub fn rng_of(t: &Ty) -> Rng {
    let (lo, hi) = t.range();
    Rng { lo, hi, frac: t.frac(), boolean: matches!(t, Ty::Bool) }
}

/// a fixed-point constant
fn fixed_tv(raw: i128, frac: u32) -> TV {
    let w = (fluxa::value::int_bits(raw, raw) as usize + frac as usize).max(1);
    mk(lit(raw, w, raw < 0), Rng { lo: raw, hi: raw, frac, boolean: false })
}

/// a step into an array or tuple: a constant index, or one known only at run time
#[derive(Clone, Debug)]
pub enum RStep {
    Const(usize),
    Var(TV),
}

/// the RTL of a task: the values as expressions
pub struct Rtl<'p> {
    prog: &'p Arc<Program>,
    it: Interp,
    /// the variables' types (the widths of their registers)
    vars: HashMap<String, Ty>,
    /// the values
    env: HashMap<String, RV>,
    rename: usize,
    depth: usize,
}

fn err<T>(line: u32, prog: &Program, m: impl Into<String>) -> R<T> { Err(format!("{}: {}", prog.loc(line), m.into())) }

impl<'p> Rtl<'p> {
    pub fn new(prog: &'p Arc<Program>, vars: HashMap<String, Ty>) -> Rtl<'p> {
        let sh = Shared::new(prog.clone(), false, false);
        Rtl { prog, it: Interp::new(sh, 0, "synth".into()), vars, env: HashMap::new(), rename: 1_000_000, depth: 0 }
    }

    fn here(&self, line: u32) -> String { self.prog.loc(line) }

    /// the type of a type expression
    fn ty(&mut self, t: &TypeExpr, line: u32) -> R<Ty> {
        let rt = block_on(self.it.eval_type(t)).map_err(|e| format!("{}: type: {:?}", self.here(line), e))?;
        Ty::of_rt(&rt).map_err(|m| format!("{}: {}", self.here(line), m))
    }

    /// the type of a type expression, or None when the simulator leaves it open
    fn ty_opt(&mut self, t: &TypeExpr, line: u32) -> R<Option<Ty>> {
        let rt = block_on(self.it.eval_type(t)).map_err(|e| format!("{}: type: {:?}", self.here(line), e))?;
        if matches!(rt, fluxa::value::RtType::Any) { return Ok(None); }
        Ty::of_rt(&rt).map(Some).map_err(|m| format!("{}: {}", self.here(line), m))
    }

    /// a compile-time constant (`comptime`, constants); None when it reads anything else
    fn konst(&mut self, e: &Expr) -> Option<Value> {
        let mut uses_local = false;
        crate::walk_paths(e, &mut |p| if p.contains('.') || p.contains('~') { uses_local = true; });
        if uses_local { return None; }
        block_on(self.it.eval(e)).ok()
    }

    /// a constant as a value
    fn value(&self, v: &Value, line: u32) -> R<RV> {
        Ok(match v {
            Value::Int(n) => RV::S(TV::konst(*n)),
            Value::Bool(b) => RV::S(TV { text: lit(*b as i128, 1, false), rng: Rng::bool() }),
            Value::Fixed(r, f) => RV::S(fixed_tv(*r, *f)),
            Value::Array(xs) | Value::Tuple(xs) => RV::A(xs.iter().map(|x| self.value(x, line)).collect::<R<_>>()?),
            Value::Range(lo, hi) => RV::A((*lo..*hi).map(|k| RV::S(TV::konst(k))).collect()),
            Value::Str(_) | Value::Unit => RV::S(TV::konst(0)),
            // a float that is a binary fraction (0.375 = 3/8) is an exact fixed-point constant
            Value::Float(x) => {
                let f = (0..=60).find(|f| { let y = x * 2f64.powi(*f); y.fract() == 0.0 && y.abs() < 2f64.powi(100) });
                match f {
                    Some(f) => RV::S(fixed_tv((x * 2f64.powi(f)) as i128, f as u32)),
                    None => return err(line, self.prog, format!("the float {} is not a binary fraction: give it as a fixed-point constant", x)),
                }
            }
            other => return err(line, self.prog, format!("the constant {} is not synthesized", fluxa::value::kind(other))),
        })
    }

    // ------------------------------------------------------------ variables

    /// declare `name` of type `t` (None: the type the type pass settled) with value `v`
    fn declare(&mut self, name: &str, t: Option<Ty>, v: RV) -> R<()> {
        let t = match t {
            Some(t) => t,
            None => self.vars.get(name).cloned().unwrap_or_else(|| v.ty()),
        };
        let v = self.fit_rv(&v, &t)?;
        self.env.insert(name.into(), v);
        Ok(())
    }

    /// `name` (whole, or the element / field at `path`) gets `v`
    fn store(&mut self, name: &str, path: &[RStep], v: RV, line: u32) -> R<()> {
        let t = match self.vars.get(name) { Some(t) => t.clone(), None => return err(line, self.prog, format!("assignment to `{}`, which is not a variable here", crate::gen::base(name))) };
        let cur = self.env.get(name).cloned().unwrap_or(RV::S(TV::konst(0)));
        let new = self.put(&cur, &t, path, v, line)?;
        let new = self.fit_rv(&new, &t)?;
        self.env.insert(name.into(), new);
        Ok(())
    }

    /// `cur` (of type `t`) with the part at `path` replaced by `v`
    fn put(&mut self, cur: &RV, t: &Ty, path: &[RStep], v: RV, line: u32) -> R<RV> {
        let Some((step, rest)) = path.split_first() else { return Ok(v); };
        let (xs, et): (&Vec<RV>, Vec<Ty>) = match (cur, t) {
            (RV::A(xs), Ty::Array(e, n)) => (xs, vec![(**e).clone(); *n]),
            (RV::A(xs), Ty::Tuple(ts)) => (xs, ts.clone()),
            _ => return err(line, self.prog, "indexing something that is not an array"),
        };
        let mut out = xs.clone();
        match step {
            RStep::Const(k) => {
                let k = *k;
                if k >= out.len() { return err(line, self.prog, format!("index {} out of bounds (length {})", k, out.len())); }
                out[k] = self.put(&xs[k], &et[k], rest, v, line)?;
            }
            RStep::Var(i) => {
                // every element: the new value where the index matches
                for k in 0..out.len() {
                    if i.rng.hi < k as i128 || i.rng.lo > k as i128 { continue; }
                    let hit = compare(i, &TV::konst(k as i128), "==");
                    let nv = self.put(&xs[k], &et[k], rest, v.clone(), line)?;
                    let nv = self.fit_rv(&nv, &et[k])?;
                    let old = self.fit_rv(&xs[k], &et[k])?;
                    out[k] = self.select_rv(&hit, &nv, &old)?;
                }
            }
        }
        Ok(RV::A(out))
    }

    /// `c ? a : b`, leaf by leaf
    fn select_rv(&self, c: &TV, a: &RV, b: &RV) -> R<RV> {
        Ok(match (a, b) {
            (RV::S(x), RV::S(y)) => RV::S(select(c, x, y)),
            (RV::A(xs), RV::A(ys)) if xs.len() == ys.len() => {
                let mut out = vec![];
                for (x, y) in xs.iter().zip(ys) { out.push(self.select_rv(c, x, y)?); }
                RV::A(out)
            }
            _ => a.clone(),
        })
    }

    /// the value converted to type `t`, leaf by leaf
    fn fit_rv(&self, v: &RV, t: &Ty) -> R<RV> {
        Ok(match (v, t) {
            (RV::S(s), t) if t.is_scalar() => RV::S(fit(s, t)),
            (RV::A(xs), Ty::Array(e, _)) => RV::A(xs.iter().map(|x| self.fit_rv(x, e)).collect::<R<_>>()?),
            (RV::A(xs), t) if t.is_scalar() => RV::A(xs.iter().map(|x| self.fit_rv(x, t)).collect::<R<_>>()?),
            _ => return Err("cannot convert this value to that type".into()),
        })
    }

    // ------------------------------------------------------------ statements

    pub fn stmts(&mut self, ss: &[Stmt]) -> R<()> {
        for s in ss { self.stmt(s)?; }
        Ok(())
    }

    fn bind(&mut self, p: &Pattern, v: RV, t: Option<Ty>, line: u32) -> R<()> {
        match p {
            Pattern::Ident(n) => self.declare(n, t, v),
            Pattern::Wild => Ok(()),
            Pattern::Tuple(ps) => match v {
                RV::A(xs) if xs.len() == ps.len() => { for (p, x) in ps.iter().zip(xs) { self.bind(p, x, None, line)?; } Ok(()) }
                _ => err(line, self.prog, format!("cannot destructure this value into a {}-tuple", ps.len())),
            },
        }
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        let line = s.line;
        match &s.kind {
            StmtKind::Let { pat, ty, init, .. } => {
                let t = match ty { Some(t) => self.ty_opt(t, line)?, None => None };
                let v = match init {
                    Some(e) => self.eval(e)?,
                    None => match &t { Some(t) => self.const_of(t), None => return err(line, self.prog, "`let` needs an initializer or a type") },
                };
                self.bind(pat, v, t, line)
            }
            StmtKind::Carry { name, ty, init } => {
                let t = self.ty(ty, line)?;
                let v = self.eval(init)?;
                self.declare(name, Some(t), v)
            }
            StmtKind::Assign { target, op, value } => {
                let (name, path) = self.target(target)?;
                let rhs = self.eval(value)?;
                let v = match op {
                    None => rhs,
                    Some(op) => {
                        let cur = self.eval(target)?;
                        let (a, b) = (cur.s()?.clone(), rhs.s()?.clone());
                        let wrap_ty = if matches!(op, BinOp::AddW | BinOp::SubW | BinOp::MulW) { Some(self.part_ty(&name, &path, line)?) } else { None };
                        RV::S(self.binop(*op, &a, &b, wrap_ty, line)?)
                    }
                };
                self.store(&name, &path, v, line)
            }
            StmtKind::Expr(e) => { self.eval(e)?; Ok(()) }
            StmtKind::Assert { .. } => Ok(()),
            StmtKind::For { attr, pat, iter, body } => {
                // a loop without channel operations: unrolled (the iterated values known at compile time)
                let items = self.iterate(iter, line)?;
                if items.len() > 4096 { return err(line, self.prog, format!("{} iterations are too many to unroll", items.len())); }
                let commit = !matches!(attr, LoopAttr::Unroll(_));
                for it in items {
                    self.bind(pat, it, None, line)?;
                    self.stmts(&body.stmts)?;
                    if let Some(t) = &body.tail { self.eval(t)?; }
                    if commit { self.commit_regs(); }
                }
                Ok(())
            }
            StmtKind::Return(_) => err(line, self.prog, "`return` here is not synthesized yet"),
            StmtKind::Spawn(_) | StmtKind::Parallel(_) => err(line, self.prog, "spawn is not synthesized here"),
        }
    }

    /// the values a pure loop goes through
    fn iterate(&mut self, iter: &Expr, line: u32) -> R<Vec<RV>> {
        if let ExprKind::Range(lo, hi, incl) = &iter.kind {
            let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else {
                return err(line, self.prog, "a loop without channel operations needs constant bounds to be unrolled");
            };
            let h = if *incl { h + 1 } else { h };
            return Ok((l..h).map(|k| RV::S(TV::konst(k))).collect());
        }
        match self.eval(iter)? {
            RV::A(xs) => Ok(xs),
            RV::S(_) => err(line, self.prog, "a loop over something that is neither a range nor an array"),
        }
    }

    /// `@reg` variables written in a loop body: their pending values (lowering writes them to
    /// `NAME~p`) become current at the end of the iteration
    pub fn commit_regs(&mut self) {
        let pend: Vec<String> = self.env.keys().filter(|k| k.ends_with("~p")).cloned().collect();
        for p in pend {
            let v = self.env[&p].clone();
            self.env.insert(p.trim_end_matches("~p").to_string(), v);
        }
    }

    /// an assignment target: the variable and the path inside it
    fn target(&mut self, e: &Expr) -> R<(String, Vec<RStep>)> {
        match &e.kind {
            ExprKind::Path(p) if p.len() == 1 => Ok((p[0].clone(), vec![])),
            ExprKind::Index(b, i) => {
                let (n, mut path) = self.target(b)?;
                let iv = self.scalar(i)?;
                path.push(match iv.as_const() { Some(k) if k >= 0 => RStep::Const(k as usize), _ => RStep::Var(iv) });
                Ok((n, path))
            }
            ExprKind::Field(b, f) => {
                let (n, mut path) = self.target(b)?;
                let k: usize = f.parse().map_err(|_| format!("{}: unknown field `{}`", self.here(e.line), f))?;
                path.push(RStep::Const(k));
                Ok((n, path))
            }
            _ => err(e.line, self.prog, "invalid assignment target"),
        }
    }

    /// the type of the part of variable `name` at `path`
    fn part_ty(&self, name: &str, path: &[RStep], line: u32) -> R<Ty> {
        let need = || format!("{}: wrapping operators need a typed variable on the left; use `.wrap::<T>()` otherwise", self.here(line));
        let mut t = self.vars.get(name).cloned().ok_or_else(need)?;
        for s in path {
            t = match (t, s) {
                (Ty::Array(e, _), _) => *e,
                (Ty::Tuple(ts), RStep::Const(k)) => ts[*k].clone(),
                _ => return err(line, self.prog, "bad index"),
            };
        }
        Ok(t)
    }

    /// a constant of type `t` (the value a register without an initializer has)
    fn const_of(&self, t: &Ty) -> RV {
        let (lo, hi) = t.range();
        let v = if lo == hi { lo } else { 0 };
        RV::S(mk(lit(v, t.width().max(1), lo < 0), rng_of(t)))
    }

    // ------------------------------------------------------------ expressions

    pub fn scalar(&mut self, e: &Expr) -> R<TV> {
        let v = self.eval(e)?;
        v.s().cloned().map_err(|m| format!("{}: {}", self.here(e.line), m))
    }

    pub fn eval(&mut self, e: &Expr) -> R<RV> {
        let line = e.line;
        match &e.kind {
            ExprKind::Int(n) => Ok(RV::S(TV::konst(*n))),
            ExprKind::Bool(b) => Ok(RV::S(TV { text: lit(*b as i128, 1, false), rng: Rng::bool() })),
            ExprKind::Float(_) => match self.konst(e) { Some(v) => self.value(&v, line), None => err(line, self.prog, "floating point is only for compile-time constants") },
            ExprKind::Path(p) => {
                let n = p.last().unwrap();
                if p.len() == 1 { if let Some(v) = self.env.get(n) { return Ok(v.clone()); } }
                let c = self.prog.consts.read().unwrap().get(n).cloned();
                match c {
                    Some(v) => self.value(&v, line),
                    None => err(line, self.prog, format!("`{}` has no value here (not a variable or constant)", crate::gen::base(n))),
                }
            }
            ExprKind::Unary(c, x) => {
                let v = self.scalar(x)?;
                Ok(RV::S(match c {
                    '-' => neg(&v),
                    '!' => not(&truth(&v)),
                    _ => not(&v),
                }))
            }
            ExprKind::Binary(op, l, r) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    let a = truth(&self.scalar(l)?);
                    let b = truth(&self.scalar(r)?);
                    let text = if *op == BinOp::And { format!("(({}) && ({}))", a.text, b.text) } else { format!("(({}) || ({}))", a.text, b.text) };
                    return Ok(RV::S(TV { text, rng: Rng::bool() }));
                }
                let (a, b) = (self.scalar(l)?, self.scalar(r)?);
                let wrap_ty = if matches!(op, BinOp::AddW | BinOp::SubW | BinOp::MulW) {
                    let (n, path) = self.target(l).map_err(|_| format!("{}: wrapping operators need a typed variable on the left", self.here(line)))?;
                    Some(self.part_ty(&n, &path, line)?)
                } else { None };
                Ok(RV::S(self.binop(*op, &a, &b, wrap_ty, line)?))
            }
            ExprKind::Cast(x, t) => {
                let v = self.eval(x)?;
                let t = self.ty(t, line)?;
                self.convert_rv(&v, &t, Round::Trunc, Ovf::Wrap)
            }
            ExprKind::Call(c, a) => self.call(c, a, line),
            ExprKind::Method(r, n, t, a) => self.method(r, n, t, a, line),
            ExprKind::Field(x, f) => {
                let v = self.eval(x)?;
                let k: usize = f.parse().map_err(|_| format!("{}: unknown field `{}`", self.here(line), f))?;
                match v { RV::A(xs) if k < xs.len() => Ok(xs[k].clone()), _ => err(line, self.prog, format!("no field `{}`", f)) }
            }
            ExprKind::Index(x, i) => {
                let v = self.eval(x)?;
                // a slice x[a..b] of an array
                if let ExprKind::Range(lo, hi, incl) = &i.kind {
                    let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else { return err(line, self.prog, "a slice needs constant bounds") };
                    let h = if *incl { h + 1 } else { h };
                    return match v {
                        RV::A(xs) if l >= 0 && h >= l && (h as usize) <= xs.len() => Ok(RV::A(xs[l as usize..h as usize].to_vec())),
                        _ => err(line, self.prog, format!("slice {}..{} out of bounds", l, h)),
                    };
                }
                let k = self.scalar(i)?;
                match v {
                    RV::A(xs) => self.index(&xs, &k, line),
                    RV::S(s) => match k.as_const() {
                        Some(k) if k >= 0 => Ok(RV::S(TV { text: format!("(({}) >> {})", s.text, k), rng: Rng::bool() })),
                        _ => err(line, self.prog, "a bit of a value that is not an array needs a constant index"),
                    },
                }
            }
            ExprKind::Slice(x, h, l) => {
                let v = self.scalar(x)?;
                let (Some(h), Some(l)) = (self.scalar(h)?.as_const(), self.scalar(l)?.as_const()) else { return err(line, self.prog, "a bit slice needs constant bounds") };
                if l < 0 || h < l { return err(line, self.prog, format!("invalid bit slice [{}:{}]", h, l)); }
                let w = (h - l + 1) as usize;
                let text = format!("(({}) >> {})", v.text, l);
                Ok(RV::S(TV { text, rng: Rng::int(0, (1i128 << w) - 1) }))
            }
            ExprKind::Range(lo, hi, incl) => {
                let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else { return err(line, self.prog, "a range used as a value needs constant bounds") };
                let h = if *incl { h + 1 } else { h };
                Ok(RV::A((l..h).map(|k| RV::S(TV::konst(k))).collect()))
            }
            ExprKind::Array(xs) | ExprKind::Tuple(xs) => Ok(RV::A(xs.iter().map(|x| self.eval(x)).collect::<R<_>>()?)),
            ExprKind::Repeat(x, n) => {
                let v = self.eval(x)?;
                let Some(n) = self.scalar(n)?.as_const() else { return err(line, self.prog, "[x; n] needs a constant n") };
                Ok(RV::A(vec![v; n.max(0) as usize]))
            }
            ExprKind::Block(b) => {
                self.stmts(&b.stmts)?;
                match &b.tail { Some(t) => self.eval(t), None => Ok(RV::S(TV::konst(0))) }
            }
            ExprKind::Comptime(_) => match self.konst(e) {
                Some(v) => self.value(&v, line),
                None => err(line, self.prog, "comptime block that reads run-time values"),
            },
            ExprKind::If(c, t, el) => {
                let cb = truth(&self.scalar(c)?);
                let before = self.env.clone();
                let tv = { self.stmts(&t.stmts)?; match &t.tail { Some(x) => self.eval(x)?, None => RV::S(TV::konst(0)) } };
                let after_t = std::mem::replace(&mut self.env, before);
                let fv = match el { Some(x) => self.eval(x)?, None => RV::S(TV::konst(0)) };
                let after_f = std::mem::take(&mut self.env);
                self.env = self.merge(&cb, after_t, after_f)?;
                Ok(self.select_rv(&cb, &tv, &fv)?)
            }
            ExprKind::Match(s, arms) => self.matching(s, arms, line),
            ExprKind::Str(_) => Ok(RV::S(TV::konst(0))),
            _ => err(line, self.prog, "this expression is not synthesized yet"),
        }
    }

    /// element `k` of `xs` (a run-time index: a multiplexer over the elements)
    fn index(&mut self, xs: &[RV], k: &TV, line: u32) -> R<RV> {
        if let Some(c) = k.rng.lo.checked_sub(k.rng.hi).and_then(|_| k.as_const()) {
            return xs.get(c as usize).cloned().ok_or_else(|| format!("{}: index {} out of bounds (length {})", self.here(line), c, xs.len()));
        }
        if xs.is_empty() { return err(line, self.prog, "indexing an empty array"); }
        let mut out = xs[0].clone();
        for (j, x) in xs.iter().enumerate().skip(1) {
            if k.rng.hi < j as i128 || k.rng.lo > j as i128 { continue; }
            let hit = compare(k, &TV::konst(j as i128), "==");
            out = self.select_rv(&hit, x, &out)?;
        }
        Ok(out)
    }

    /// the variables after `c ? then : else`
    fn merge(&self, c: &TV, t: HashMap<String, RV>, f: HashMap<String, RV>) -> R<HashMap<String, RV>> {
        let mut out = HashMap::new();
        for (k, tv) in &t {
            match f.get(k) {
                Some(fv) if fv.text() == tv.text() => { out.insert(k.clone(), tv.clone()); }
                Some(fv) => {
                    let ty = self.vars.get(k).cloned().unwrap_or_else(|| tv.ty());
                    let (a, b) = (self.fit_rv(tv, &ty)?, self.fit_rv(fv, &ty)?);
                    out.insert(k.clone(), self.select_rv(c, &a, &b)?);
                }
                None => { out.insert(k.clone(), tv.clone()); }
            }
        }
        for (k, fv) in f { out.entry(k).or_insert(fv); }
        Ok(out)
    }

    fn matching(&mut self, s: &Expr, arms: &[Arm], line: u32) -> R<RV> {
        let sv = self.scalar(s)?;
        let mut conds = vec![];
        let mut vals = vec![];
        let mut envs = vec![];
        let start = self.env.clone();
        for a in arms {
            self.env = start.clone();
            let c = match &a.pat {
                MatchPat::Wild => TV { text: "1'b1".into(), rng: Rng::bool() },
                MatchPat::Bind(n) => {
                    let c = self.prog.consts.read().unwrap().get(n).cloned();
                    match c {
                        Some(v) => { let k = self.value(&v, line)?; compare(&sv, k.s()?, "==") }
                        None => { self.declare(n, None, RV::S(sv.clone()))?; TV { text: "1'b1".into(), rng: Rng::bool() } }
                    }
                }
                MatchPat::Lit(e) => { let v = self.scalar(e)?; compare(&sv, &v, "==") }
                MatchPat::Range(lo, hi, incl) => {
                    let (l, h) = (self.scalar(lo)?, self.scalar(hi)?);
                    let a1 = compare(&sv, &l, ">=");
                    let a2 = compare(&sv, &h, if *incl { "<=" } else { "<" });
                    TV { text: format!("(({}) && ({}))", a1.text, a2.text), rng: Rng::bool() }
                }
                MatchPat::Tuple(_) => return err(line, self.prog, "tuple patterns are not synthesized yet"),
            };
            let c = match &a.guard {
                Some(g) => { let gb = truth(&self.scalar(g)?); TV { text: format!("(({}) && ({}))", c.text, gb.text), rng: Rng::bool() } }
                None => c,
            };
            let v = self.eval(&a.body)?;
            conds.push(c);
            vals.push(v);
            envs.push(std::mem::take(&mut self.env));
        }
        if conds.is_empty() { return err(line, self.prog, "a match without arms"); }
        let mut v = vals.pop().unwrap();
        let mut env = envs.pop().unwrap();
        conds.pop();
        while let Some(c) = conds.pop() {
            let tv = vals.pop().unwrap();
            let te = envs.pop().unwrap();
            v = self.select_rv(&c, &tv, &v)?;
            env = self.merge(&c, te, env)?;
        }
        self.env = env;
        Ok(v)
    }

    fn binop(&mut self, op: BinOp, a: &TV, b: &TV, wrap_ty: Option<Ty>, line: u32) -> R<TV> {
        let at = self.here(line);
        Ok(match op {
            BinOp::Add => add(a, b),
            BinOp::Sub => sub(a, b),
            BinOp::Mul => mul(a, b),
            BinOp::Div => divrem(a, b).map_err(|m| format!("{}: {}", at, m))?.0,
            BinOp::Rem => divrem(a, b).map_err(|m| format!("{}: {}", at, m))?.1,
            BinOp::AddW | BinOp::SubW | BinOp::MulW => {
                let t = wrap_ty.ok_or_else(|| format!("{}: wrapping operators need a typed variable on the left", at))?;
                let v = match op { BinOp::AddW => add(a, b), BinOp::SubW => sub(a, b), _ => mul(a, b) };
                convert(&v, &t, Round::Trunc, Ovf::Wrap)
            }
            BinOp::Shl | BinOp::Shr => match b.as_const() {
                Some(k) if (0..=120).contains(&k) => if op == BinOp::Shl { shl(a, k as u32) } else { shr(a, k as u32) },
                _ => shift(a, b, op == BinOp::Shl).map_err(|m| format!("{}: {}", at, m))?,
            },
            BinOp::BitAnd => bitop(a, b, '&'),
            BinOp::BitOr => bitop(a, b, '|'),
            BinOp::BitXor => bitop(a, b, '^'),
            BinOp::Eq => compare(a, b, "=="),
            BinOp::Ne => compare(a, b, "!="),
            BinOp::Lt => compare(a, b, "<"),
            BinOp::Le => compare(a, b, "<="),
            BinOp::Gt => compare(a, b, ">"),
            BinOp::Ge => compare(a, b, ">="),
            BinOp::And | BinOp::Or => unreachable!(),
        })
    }

    /// `v` converted to `t` leaf by leaf
    fn convert_rv(&self, v: &RV, t: &Ty, r: Round, o: Ovf) -> R<RV> {
        Ok(match (v, t) {
            (RV::S(s), t) if t.is_scalar() => RV::S(convert(s, t, r, o)),
            (RV::A(xs), Ty::Array(e, _)) => RV::A(xs.iter().map(|x| self.convert_rv(x, e, r, o)).collect::<R<_>>()?),
            (RV::A(xs), t) if t.is_scalar() => RV::A(xs.iter().map(|x| self.convert_rv(x, t, r, o)).collect::<R<_>>()?),
            _ => return Err("cannot convert this value to that type".into()),
        })
    }

    /// `T` of `x.to::<T>()` and friends
    fn targ_ty(&mut self, t: &[TArg], line: u32) -> R<Ty> {
        match t.first() { Some(TArg::Ty(t)) => self.ty(t, line), _ => err(line, self.prog, "expected a type argument, e.g. `.sat::<u8>()`") }
    }

    /// the rounding and overflow modes of `.to::<T>(round = r, ovf = o)` (the type's own by default)
    fn modes(&self, t: &Ty, a: &[Arg], ovf_default: Option<Ovf>) -> (Round, Ovf) {
        let (mut r, mut o) = match t { Ty::Fixed { round, ovf, .. } => (*round, *ovf), _ => (Round::Trunc, Ovf::Check) };
        if let Some(d) = ovf_default { o = d; }
        for x in a {
            let name = match &x.value.kind { ExprKind::Path(p) => p.last().cloned().unwrap_or_default(), _ => continue };
            match (x.name.as_deref(), name.as_str()) {
                (Some("round"), "trunc") => r = Round::Trunc,
                (Some("round"), "nearest") => r = Round::Nearest,
                (Some("round"), "even") => r = Round::Even,
                (Some("ovf"), "sat") => o = Ovf::Sat,
                (Some("ovf"), "wrap") => o = Ovf::Wrap,
                (Some("ovf"), "check") => o = Ovf::Check,
                _ => {}
            }
        }
        (r, o)
    }

    fn method(&mut self, r: &Expr, n: &str, t: &[TArg], a: &[Arg], line: u32) -> R<RV> {
        let v = self.eval(r)?;
        match (n, &v) {
            ("to" | "quantize", _) => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, None); self.convert_rv(&v, &ty, rr, o) }
            ("sat", _) => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, Some(Ovf::Sat)); self.convert_rv(&v, &ty, rr, o) }
            ("wrap", _) => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, Some(Ovf::Wrap)); self.convert_rv(&v, &ty, rr, o) }
            ("len", RV::A(xs)) => Ok(RV::S(TV::konst(xs.len() as i128))),
            ("rev", RV::A(xs)) => Ok(RV::A(xs.iter().rev().cloned().collect())),
            ("unrolled" | "iter" | "collect", RV::A(_)) => Ok(v),
            ("enumerate", RV::A(xs)) => Ok(RV::A(xs.iter().enumerate().map(|(i, x)| RV::A(vec![RV::S(TV::konst(i as i128)), x.clone()])).collect())),
            ("map", RV::A(xs)) => { let xs = xs.clone(); let f = a.first().ok_or("map() takes a lambda")?.value.clone(); Ok(RV::A(self.lambda(&f, xs, line)?)) }
            ("sum", RV::A(xs)) => {
                let mut it = xs.iter();
                let mut acc = it.next().ok_or_else(|| format!("{}: sum of an empty array", self.here(line)))?.s()?.clone();
                for x in it { acc = add(&acc, x.s()?); }
                Ok(RV::S(acc))
            }
            ("tree_sum", RV::A(xs)) => {
                let mut items: Vec<TV> = xs.iter().map(|x| x.s().cloned()).collect::<R<_>>()?;
                if items.is_empty() { return err(line, self.prog, "sum of an empty array"); }
                while items.len() > 1 {
                    let mut next = vec![];
                    for ch in items.chunks(2) { next.push(if ch.len() == 2 { add(&ch[0], &ch[1]) } else { ch[0].clone() }); }
                    items = next;
                }
                Ok(RV::S(items.remove(0)))
            }
            ("abs", RV::S(s)) => {
                let s = s.clone();
                let ng = neg(&s);
                let lt = compare(&s, &TV::konst(0), "<");
                let x = select(&lt, &ng, &s);
                let hi = s.rng.hi.abs().max(s.rng.lo.abs());
                let lo = if s.rng.lo >= 0 { s.rng.lo } else if s.rng.hi <= 0 { -s.rng.hi } else { 0 };
                Ok(RV::S(TV { text: x.text, rng: Rng::int(lo, hi).with_frac(s.rng.frac) }))
            }
            ("xor_reduce", RV::S(s)) => Ok(RV::S(TV { text: format!("({}[0])", s.text), rng: Rng::bool() })),
            ("popcount", RV::S(s)) => {
                let mut acc = TV::konst(0);
                for i in 0..s.width() {
                    let x = TV { text: format!("(({})[{}])", s.text, i), rng: Rng::bool() };
                    acc = add(&acc, &x);
                }
                Ok(RV::S(acc))
            }
            (other, _) => err(line, self.prog, format!("method `{}` is not synthesized here", other)),
        }
    }

    /// a lambda `|x| ..` (or `|i, x| ..` over pairs) applied to each item
    fn lambda(&mut self, lam: &Expr, items: Vec<RV>, line: u32) -> R<Vec<RV>> {
        let (ps, body) = match &lam.kind { ExprKind::Lambda(p, b) => (p.clone(), b.clone()), _ => return err(line, self.prog, "expected a lambda such as `|x| x * 2`") };
        let mut out = vec![];
        for it in items {
            if ps.len() == 1 { self.declare(&ps[0], None, it)?; }
            else if let RV::A(xs) = it { for (p, x) in ps.iter().zip(xs) { self.declare(p, None, x)?; } }
            out.push(self.eval(&body)?);
        }
        Ok(out)
    }

    fn call(&mut self, c: &Expr, a: &[Arg], line: u32) -> R<RV> {
        let name = match &c.kind { ExprKind::Path(p) | ExprKind::PathT(p, _) => p.last().unwrap().clone(), _ => return err(line, self.prog, "callee must be a function name") };
        match name.as_str() {
            "print" => return Ok(RV::S(TV::konst(0))),
            "min" | "max" if a.len() == 2 => {
                let (x, y) = (self.scalar(&a[0].value)?, self.scalar(&a[1].value)?);
                let lt = compare(&x, &y, "<");
                return Ok(RV::S(if name == "min" { select(&lt, &x, &y) } else { select(&lt, &y, &x) }));
            }
            "abs" if a.len() == 1 => {
                let r = Expr { kind: ExprKind::Method(Box::new(a[0].value.clone()), "abs".into(), vec![], vec![]), line };
                return self.eval(&r);
            }
            "concat" => {
                let mut out = vec![];
                for x in a { match self.eval(&x.value)? { RV::A(xs) => out.extend(xs), _ => return err(line, self.prog, "concat expects arrays") } }
                return Ok(RV::A(out));
            }
            "len" if a.len() == 1 => return match self.eval(&a[0].value)? { RV::A(xs) => Ok(RV::S(TV::konst(xs.len() as i128))), _ => err(line, self.prog, "len() expects an array") },
            "zip" if a.len() == 2 => {
                let (x, y) = (self.eval(&a[0].value)?, self.eval(&a[1].value)?);
                return match (x, y) { (RV::A(xs), RV::A(ys)) => Ok(RV::A(xs.into_iter().zip(ys).map(|(p, q)| RV::A(vec![p, q])).collect())), _ => err(line, self.prog, "zip() expects two arrays") };
            }
            "array" if a.len() == 2 => {
                let Some(n) = self.scalar(&a[0].value)?.as_const() else { return err(line, self.prog, "array(n, ..) needs a constant n") };
                let items = (0..n).map(|k| RV::S(TV::konst(k))).collect();
                return Ok(RV::A(self.lambda(&a[1].value, items, line)?));
            }
            _ => {}
        }
        let decl = match self.prog.fns.get(&name).cloned() {
            Some(d) => d,
            None => return match self.konst(&Expr { kind: ExprKind::Call(Box::new(c.clone()), a.to_vec()), line }) {
                Some(v) => self.value(&v, line),
                None => err(line, self.prog, format!("`{}` is not synthesized", name)),
            },
        };
        if decl.kind == FnKind::Task { return err(line, self.prog, format!("`{}` is a task: spawn it", name)); }
        if self.depth > 32 { return err(line, self.prog, "recursion is not synthesized"); }
        // the arguments, then the body inlined with fresh names
        let mut vals = vec![];
        for x in a { vals.push((x.name.clone(), self.eval(&x.value)?)); }
        self.rename += 1;
        let consts: HashSet<String> = self.prog.consts.read().unwrap().keys().cloned().collect();
        let (params, body) = crate::lower::rename_fn(&decl, self.rename, &consts);
        let mut positional = vals.iter().filter(|(n, _)| n.is_none()).map(|(_, v)| v.clone());
        for (p, u) in decl.params.iter().zip(&params) {
            let v = match vals.iter().find(|(n, _)| n.as_deref() == Some(p.name.as_str())) { Some((_, v)) => v.clone(), None => match positional.next() { Some(v) => v, None => return err(line, self.prog, format!("missing argument `{}`", p.name)) } };
            match self.ty_opt(&p.ty, line)? {
                Some(t) => {
                    self.declare(u, Some(t.clone()), v.clone())?;
                    let k = self.fit_rv(&v, &t)?;
                    self.env.insert(u.clone(), k);
                }
                None => self.declare(u, None, v)?,
            }
        }
        self.depth += 1;
        let mut stmts = body.stmts.clone();
        let mut tail = body.tail.clone();
        if tail.is_none() {
            if let Some(Stmt { kind: StmtKind::Return(Some(e)), .. }) = stmts.last() { tail = Some(Box::new(e.clone())); stmts.pop(); }
        }
        let r = self.stmts(&stmts).and_then(|_| match &tail { Some(t) => self.eval(t), None => Ok(RV::S(TV::konst(0))) });
        self.depth -= 1;
        let v = r?;
        match &decl.ret {
            Some(rt) => match self.ty_opt(rt, line)? { Some(t) => self.fit_rv(&v, &t), None => Ok(v) },
            None => Ok(v),
        }
    }
}

/// the pipeline loop's handshake: computed before the body's statements, as the gates do
pub struct Pipe {
    /// the loop goes on
    pub more: String,
    /// the loop leaves
    pub ends: String,
    /// the other recvs' tokens must all be there
    pub ins: Vec<String>,
    /// the outputs: the iteration fires when they all accept
    pub readys: Vec<String>,
}

/// the channels of a task: the unique name → (the port prefix, sends?, the element type)
pub type Chans = HashMap<String, (String, bool, Ty)>;
/// the memories of a task: the unique name → (the register name, the word type, the words)
pub type Mems = HashMap<String, (String, Ty, usize)>;

/// the RTL of a task: a module with one state per block of the lowered task
pub fn task_module(prog: &Arc<Program>, decl: &FnDecl, cfg: &Cfg, params: &crate::Params) -> R<String> {
    let hw = crate::gen::hw_params(prog, decl, cfg, params)?;
    let (vars, _untyped) = crate::gen::types(prog, decl, cfg, &hw.params, &hw.chans, &hw.mems)?;

    // the ports
    let mut ports: Vec<String> = vec!["  input  logic clk".into(), "  input  logic rst".into()];
    let mut chans: Chans = HashMap::new();
    let mut scalars: Vec<(String, String, Ty)> = vec![];
    let mut consts: Vec<(String, RV)> = vec![];
    for (name, u, h) in &hw.params {
        let p = ident(name);
        match h {
            crate::gen::ParamHw::InChan(t) => {
                ports.push(format!("  input  logic [{}:0] {}_data", t.width().max(1) - 1, p));
                ports.push(format!("  input  logic {}_close", p));
                ports.push(format!("  input  logic {}_valid", p));
                ports.push(format!("  output logic {}_ready", p));
                chans.insert(u.clone(), (p, false, t.clone()));
            }
            crate::gen::ParamHw::OutChan(t) => {
                ports.push(format!("  input  logic {}_ready", p));
                ports.push(format!("  output logic [{}:0] {}_data", t.width().max(1) - 1, p));
                ports.push(format!("  output logic {}_close", p));
                ports.push(format!("  output logic {}_valid", p));
                chans.insert(u.clone(), (p, true, t.clone()));
            }
            crate::gen::ParamHw::Scalar(t, None) => {
                ports.push(format!("  input  logic [{}:0] {}", t.width().max(1) - 1, p));
                scalars.push((u.clone(), p, t.clone()));
            }
            crate::gen::ParamHw::Scalar(t, Some(v)) => {
                let r0 = Rtl::new(prog, vars.clone());
                let rv = r0.value(v, decl.line)?;
                let rv = r0.fit_rv(&rv, t)?;
                consts.push((u.clone(), rv));
            }
            crate::gen::ParamHw::MemInit(_) => {}
        }
    }

    // the memories
    let mut mems: Mems = HashMap::new();
    let mut mem_init: Vec<(String, Vec<String>)> = vec![];
    for (m, (wt, n)) in &hw.mems {
        mems.insert(m.clone(), (ident(m), wt.clone(), *n));
    }
    for (m, _t, _n, from) in &cfg.mems {
        let Some(p) = from else { continue };
        let Some(h) = hw.params.iter().find(|x| &x.1 == p) else { continue };
        let crate::gen::ParamHw::Scalar(_, Some(v)) = &h.2 else { continue };
        let Some((mr, wt, _)) = mems.get(m) else { continue };
        let r0 = Rtl::new(prog, vars.clone());
        let rv = r0.value(v, decl.line)?;
        let RV::A(xs) = rv else { continue };
        let words: Vec<String> = xs.iter().map(|x| x.text()).collect();
        let _ = (mr, wt);
        mem_init.push((m.clone(), words));
    }

    // the registers: the variables
    let mut names: Vec<&String> = vars.keys().collect();
    names.sort();
    let regs: Vec<(String, String, Ty)> = names.iter().map(|n| ((*n).clone(), ident(n), vars[*n].clone())).collect();

    // the states
    let nst = cfg.blocks.len();
    let sw = eda::netlist::bits_for(nst).max(1);
    let mut states: Vec<Vec<String>> = vec![];
    // the source line of every block (the comment in the RTL)
    let mut lines_of: Vec<u32> = vec![];
    let mut last_line = 0u32;

    for b in &cfg.blocks {
        let mut r = Rtl::new(prog, vars.clone());
        // the environment: the registers and the parameters
        for (n, rn, t) in &regs { r.env.insert(n.clone(), reg_value(rn, t)); }
        for (u, pn, t) in &scalars { r.env.insert(u.clone(), reg_value(pn, t)); }
        for (u, v) in &consts { r.env.insert(u.clone(), v.clone()); }
        let mut lines: Vec<String> = vec![];

        // a pipeline loop: the head's token and the recvs' tokens are the body's inputs, and the
        // loop's condition is looked at before the body runs (as the gates do)
        let mut pipe = None;
        if let Term::Pipe { head, cond, recvs, sends, .. } = &b.term {
            for (dst, ch) in head.iter().chain(recvs) {
                let Some((p, _, t)) = chans.get(ch) else { continue };
                let dt = vars.get(dst).cloned().unwrap_or(t.clone());
                let v = reg_value(&format!("{}_data", p), t);
                let v = r.fit_rv(&v, &dt)?;
                r.env.insert(dst.clone(), v);
            }
            // the loop goes on while the head's stream has tokens (or the condition holds), and
            // leaves when the stream ends (or the condition fails)
            let mut more = "1'b1".to_string();
            let mut ends = "1'b0".to_string();
            if let Some((_, hch)) = head {
                let (p, _, _) = &chans[hch];
                more = format!("{}_valid && !{}_closed", p, p);
                ends = format!("{}_valid && ({}_close || {}_closed)", p, p, p);
            }
            if let Some(c) = cond {
                let cv = r.scalar(c)?;
                let cb = truth(&cv);
                more = cb.text.clone();
                ends = format!("!({})", cb.text);
            }
            // the other recvs' tokens must all be there
            let mut ins: Vec<String> = vec![];
            for (_, ch) in recvs {
                let Some((p, _, _)) = chans.get(ch) else { continue };
                ins.push(format!("{}_valid", p));
            }
            // the outputs: the iteration fires when they all accept
            let mut readys: Vec<String> = vec![];
            for (ch, _) in sends {
                let Some((p, _, _)) = chans.get(ch) else { continue };
                readys.push(format!("{}_ready", p));
            }
            pipe = Some(Pipe { more, ends, ins, readys });
        }

        // the block's statements
        r.stmts(&b.stmts)?;

        // the terminator: the handshake and the next state
        let go = term(b, &mut r, &mut lines, &Ctx { chans: &chans, mems: &mems, sw, pipe: pipe.as_ref() })?;

        // the registers the state writes
        for (n, rn, t) in &regs {
            let Some(v) = r.env.get(n) else { continue };
            let text = v.text();
            if text == *rn { continue; }
            lines.push(format!("        if ({}) {}_next = {}'({});", go, rn, t.width().max(1), text));
        }
        // the block's place in the program: the line of the statement it comes from (a block
        // with no statement of its own continues the previous one)
        let line = b.stmts.first().map(|s| s.line).unwrap_or(last_line);
        if line > 0 { last_line = line; }
        lines_of.push(line);
        states.push(lines);
    }

    // ------------------------------------------------------------ the module

    let mut out = vec![];
    out.push(format!("// {}: the RTL of the task `{}` (generated by the Fluxa synthesizer)", decl.name, decl.name));
    out.push(format!("module {} (", ident(&decl.name)));
    out.push(ports.join(",\n"));
    out.push(");".into());
    out.push("".into());
    // the state
    out.push("  // the state: one per block of the task".to_string());
    out.push(format!("  logic [{}:0] state;", sw - 1));
    out.push(format!("  logic [{}:0] state_next;", sw - 1));
    // the registers
    if !regs.is_empty() {
        out.push("  // the variables".into());
        for (_, rn, t) in &regs { out.push(format!("  logic [{}:0] {};", t.width().max(1) - 1, rn)); }
        for (_, rn, t) in &regs { out.push(format!("  logic [{}:0] {}_next;", t.width().max(1) - 1, rn)); }
    }
    // the closed flags
    if !chans.is_empty() {
        out.push("  // the channels that have ended".into());
        for (p, _, _) in chans.values() {
            out.push(format!("  logic {}_closed;", p));
            out.push(format!("  logic {}_closed_set;", p));
        }
    }
    // the memories
    for (m, (mr, wt, n)) in &mems {
        out.push(format!("  // the memory `{}`", m));
        out.push(format!("  logic [{}:0] {} [0:{}];", wt.width().max(1) - 1, mr, n - 1));
        out.push(format!("  logic [{}:0] {}_rd;", wt.width().max(1) - 1, mr));
        out.push(format!("  logic [{}:0] {}_rd_next;", wt.width().max(1) - 1, mr));
        out.push(format!("  logic {}_we;", mr));
        out.push(format!("  logic [{}:0] {}_wa;", eda::netlist::bits_for(*n).max(1) - 1, mr));
        out.push(format!("  logic [{}:0] {}_wd;", wt.width().max(1) - 1, mr));
    }
    out.push("".into());

    // the combinational part
    out.push("  always_comb begin".into());
    out.push("    // the defaults".into());
    out.push("    state_next = state;".into());
    for (_, rn, _) in &regs { out.push(format!("    {}_next = {};", rn, rn)); }
    for (p, _, _) in chans.values() {
        out.push(format!("    {}_closed_set = 1'b0;", p));
    }
    for (p, out_ch, _) in chans.values() {
        if !out_ch { out.push(format!("    {}_ready = 1'b0;", p)); }
        else {
            out.push(format!("    {}_data = '0;", p));
            out.push(format!("    {}_close = 1'b0;", p));
            out.push(format!("    {}_valid = 1'b0;", p));
        }
    }
    for (mr, _, _) in mems.values() {
        out.push(format!("    {}_rd_next = {}_rd;", mr, mr));
        out.push(format!("    {}_we = 1'b0;", mr));
        out.push(format!("    {}_wa = '0;", mr));
        out.push(format!("    {}_wd = '0;", mr));
    }
    out.push("    case (state)".into());
    for (s, lines) in states.iter().enumerate() {
        // the state of the block, and where it is in the program
        if lines_of[s] > 0 { out.push(format!("      // the block at line {}", lines_of[s])); }
        out.push(format!("      {}'d{}: begin", sw, s));
        for l in lines { out.push(l.clone()); }
        out.push("      end".into());
    }    out.push("    endcase".into());
    out.push("  end".into());
    out.push("".into());

    // the flip-flops
    out.push("  always_ff @(posedge clk) begin".into());
    out.push("    if (rst) begin".into());
    out.push(format!("      state <= {}'d0;", sw));
    for (_, rn, _) in &regs { out.push(format!("      {} <= '0;", rn)); }
    for (p, _, _) in chans.values() { out.push(format!("      {}_closed <= 1'b0;", p)); }
    out.push("    end else begin".into());
    out.push("      state <= state_next;".into());
    for (_, rn, _) in &regs { out.push(format!("      {} <= {}_next;", rn, rn)); }
    for (p, _, _) in chans.values() { out.push(format!("      {}_closed <= {}_closed | {}_closed_set;", p, p, p)); }
    out.push("    end".into());
    out.push("  end".into());
    for (mr, _, _) in mems.values() {
        out.push("  always_ff @(posedge clk) begin".into());
        out.push(format!("    if ({}_we) {}[{}_wa] <= {}_wd;", mr, mr, mr, mr));
        out.push(format!("    {}_rd <= {}_rd_next;", mr, mr));
        out.push("  end".into());
    }
    // the memories that start with a constant's words
    for (m, words) in &mem_init {
        let Some((mr, _, _)) = mems.get(m) else { continue };
        out.push("  initial begin".into());
        for (k, w) in words.iter().enumerate() { out.push(format!("    {}[{}] = {};", mr, k, w)); }
        out.push("  end".into());
    }
    out.push("endmodule".into());
    Ok(out.join("\n") + "\n")
}

/// what the terminator of a block needs to know about the task
struct Ctx<'a> {
    /// the task's channels
    chans: &'a Chans,
    /// the task's memories
    mems: &'a Mems,
    /// the width of the state register
    sw: usize,
    /// the pipeline loop's handshake (a `Pipe` block)
    pipe: Option<&'a Pipe>,
}

/// a register's value: a scalar is the register itself, an array (or a tuple) its elements (the
/// register holds them one after the other, the first element in the low bits)
fn reg_value(rn: &str, t: &Ty) -> RV {
    match t {
        Ty::Array(e, n) => {
            let w = e.width().max(1);
            RV::A((0..*n).map(|i| reg_value(&format!("{}'(({}) >> {})", w, rn, i * w), e)).collect())
        }
        Ty::Tuple(ts) => {
            let mut at = 0;
            let mut out = vec![];
            for e in ts {
                let w = e.width().max(1);
                out.push(reg_value(&format!("{}'(({}) >> {})", w, rn, at), e));
                at += w;
            }
            RV::A(out)
        }
        _ => RV::S(mk(rn.to_string(), rng_of(t))),
    }
}

/// the terminator of a block: the handshake it does and the condition under which the state
/// moves on (the gates' `go`)
fn term(b: &crate::lower::Bb, r: &mut Rtl, lines: &mut Vec<String>, c: &Ctx) -> R<String> {
    let sw = c.sw;
    let pipe = c.pipe;
    let chans = c.chans;
    let mems = c.mems;
    match &b.term {
        Term::Goto(t) => {
            lines.push(format!("        state_next = {}'d{};", sw, t));
            Ok("1'b1".into())
        }
        Term::Halt => Ok("1'b0".into()),
        Term::Branch(c, t, f) => {
            let cv = r.scalar(c)?;
            let cb = truth(&cv);
            lines.push(format!("        state_next = ({}) ? {}'d{} : {}'d{};", cb.text, sw, t, sw, f));
            Ok("1'b1".into())
        }
        Term::Recv { dst, ch, closed, next } => {
            let Some((p, out_ch, t)) = chans.get(ch) else { return Err(format!("`{}` is not a channel of the task", crate::gen::base(ch))) };
            if *out_ch { return Err(format!("`{}` is an output channel: it cannot be received from", crate::gen::base(ch))); }
            let valid = format!("{}_valid", p);
            let close = format!("{}_close", p);
            let cl = format!("{}_closed", p);
            lines.push(format!("        {}_ready = !{};", p, cl));
            let fire = format!("({} || {})", valid, cl);
            let is_close = format!("(({} && {}) || {})", valid, close, cl);
            let nx = match closed {
                Some(x) => format!("({}) ? {}'d{} : {}'d{}", is_close, sw, x, sw, next),
                None => format!("{}'d{}", sw, next),
            };
            lines.push(format!("        if ({}) state_next = {};", fire, nx));
            lines.push(format!("        {}_closed_set = {} && {} && !{};", p, valid, close, cl));
            // the token's value: the channel's data
            let dt = r.vars.get(dst).cloned().unwrap_or(t.clone());
            let v = reg_value(&format!("{}_data", p), t);
            let v = r.fit_rv(&v, &dt)?;
            r.env.insert(dst.clone(), v);
            Ok(fire)
        }
        Term::Send { ch, val, next } => {
            let Some((p, out_ch, t)) = chans.get(ch) else { return Err(format!("`{}` is not a channel of the task", crate::gen::base(ch))) };
            if !*out_ch { return Err(format!("`{}` is an input channel: it cannot be sent on", crate::gen::base(ch))); }
            let v = r.eval(val)?;
            let v = r.fit_rv(&v, t)?;
            lines.push(format!("        {}_data = {};", p, v.text()));
            lines.push(format!("        {}_close = 1'b0;", p));
            lines.push(format!("        {}_valid = 1'b1;", p));
            lines.push(format!("        if ({}_ready) state_next = {}'d{};", p, sw, next));
            Ok(format!("{}_ready", p))
        }
        Term::Close { ch, next } => {
            let Some((p, _, _)) = chans.get(ch) else { return Err(format!("`{}` is not a channel of the task", crate::gen::base(ch))) };
            lines.push(format!("        {}_close = 1'b1;", p));
            lines.push(format!("        {}_valid = 1'b1;", p));
            lines.push(format!("        {}_closed_set = {}_ready;", p, p));
            lines.push(format!("        if ({}_ready) state_next = {}'d{};", p, sw, next));
            Ok(format!("{}_ready", p))
        }
        Term::CloseOwned { ch, next } => {
            let Some((p, _, _)) = chans.get(ch) else { return Err(format!("`{}` is not a channel of the task", crate::gen::base(ch))) };
            let cl = format!("{}_closed", p);
            lines.push(format!("        {}_close = 1'b1;", p));
            lines.push(format!("        {}_valid = !{};", p, cl));
            lines.push(format!("        {}_closed_set = !{} && {}_ready;", p, cl, p));
            let fire = format!("({}_ready || {})", p, cl);
            lines.push(format!("        if ({}) state_next = {}'d{};", fire, sw, next));
            Ok(fire)
        }
        Term::MemRead { dst, mem, addr, next } => {
            let Some((mr, wt, n)) = mems.get(mem) else { return Err(format!("`{}` is not a memory of the task", crate::gen::base(mem))) };
            let a = r.scalar(addr)?;
            let aw = eda::netlist::bits_for(*n).max(1);
            lines.push(format!("        {}_rd_next = {}[{}'({})];", mr, mr, aw, a.text));
            lines.push(format!("        state_next = {}'d{};", sw, next));
            r.env.insert(dst.clone(), RV::S(mk(format!("{}_rd", mr), rng_of(wt))));
            Ok("1'b1".into())
        }
        Term::MemWrite { mem, addr, val, next } => {
            let Some((mr, wt, n)) = mems.get(mem) else { return Err(format!("`{}` is not a memory of the task", crate::gen::base(mem))) };
            let a = r.scalar(addr)?;
            let aw = eda::netlist::bits_for(*n).max(1);
            let v = r.scalar(val)?;
            let v = fit(&v, wt);
            lines.push(format!("        {}_we = 1'b1;", mr));
            lines.push(format!("        {}_wa = {}'({});", mr, aw, a.text));
            lines.push(format!("        {}_wd = {};", mr, v.text));
            lines.push(format!("        state_next = {}'d{};", sw, next));
            Ok("1'b1".into())
        }
        Term::Pipe { head, recvs, sends, exit, .. } => {
            let Some(pipe) = pipe else { return Err("the pipeline loop was not prepared".into()) };
            let start = if pipe.ins.is_empty() { pipe.more.clone() } else { format!("({}) && ({})", pipe.more, pipe.ins.join(" && ")) };
            let readys = &pipe.readys;
            let fire = if readys.is_empty() { start.clone() } else { format!("({}) && ({})", start, readys.join(" && ")) };
            let leave = pipe.ends.clone();
            // the recvs: the head's ready also holds when the loop leaves
            for (k, (_, ch)) in head.iter().chain(recvs).enumerate() {
                let Some((p, _, _)) = chans.get(ch) else { continue };
                let cl = format!("{}_closed", p);
                let take = if k == 0 && head.is_some() { format!("({}) || ({})", fire, leave) } else { fire.clone() };
                lines.push(format!("        {}_ready = ({}) && !{};", p, take, cl));
                lines.push(format!("        {}_closed_set = {}_ready && {}_valid && {}_close;", p, p, p, p));
            }
            // the outputs: each one waits for the others
            for (k, (ch, v)) in sends.iter().enumerate() {
                let Some((p, _, t)) = chans.get(ch) else { continue };
                let others: Vec<String> = readys.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, r)| r.clone()).collect();
                let act = if others.is_empty() { start.clone() } else { format!("({}) && ({})", start, others.join(" && ")) };
                let Some(val) = r.env.get(v).cloned() else { return Err(format!("the value sent on `{}` is not known", crate::gen::base(ch))) };
                let val = r.fit_rv(&val, t)?;
                lines.push(format!("        {}_valid = {};", p, act));
                lines.push(format!("        {}_data = {};", p, val.text()));
                lines.push(format!("        {}_close = 1'b0;", p));
            }
            lines.push(format!("        if ({}) state_next = {}'d{};", leave, sw, exit));
            Ok(fire)
        }
    }
}

/// the FIFO between two instances: a queue of `DEPTH` tokens of `W` data bits (and the close
/// flag). The same as the gates' FIFO (`graph.rs`): the push side takes a token when there is
/// room, the pop side gives one when there is one.
pub fn fifo_module() -> String {
    let mut out: Vec<String> = vec![
        "// the FIFO between two instances: a queue of DEPTH tokens of W data bits".into(),
        "module FluxaFifo #(parameter int W = 8, parameter int DEPTH = 2) (".into(),
    ];
    out.push("  input  logic clk,".into());
    out.push("  input  logic rst,".into());
    out.push("  // the push side: a token goes in".into());
    out.push("  input  logic [W-1:0] push_data,".into());
    out.push("  input  logic push_close,".into());
    out.push("  input  logic push_valid,".into());
    out.push("  output logic push_ready,".into());
    out.push("  // the pop side: a token comes out".into());
    out.push("  output logic [W-1:0] pop_data,".into());
    out.push("  output logic pop_close,".into());
    out.push("  output logic pop_valid,".into());
    out.push("  input  logic pop_ready".into());
    out.push(");".into());
    out.push("  localparam int IW = (DEPTH <= 1) ? 1 : $clog2(DEPTH);".into());
    out.push("  localparam int CW = (DEPTH <= 1) ? 1 : $clog2(DEPTH + 1);".into());
    out.push("  logic [W:0] entries [0:DEPTH-1];".into());
    out.push("  logic [IW-1:0] wr, rd;".into());
    out.push("  logic [CW-1:0] cnt;".into());
    out.push("  logic push, pop;".into());
    out.push("".into());
    out.push("  always_comb begin".into());
    out.push("    push_ready = (cnt != DEPTH);".into());
    out.push("    pop_valid = (cnt != 0);".into());
    out.push("    push = push_valid && push_ready;".into());
    out.push("    pop = pop_ready && pop_valid;".into());
    out.push("  end".into());
    out.push("".into());
    out.push("  always_ff @(posedge clk) begin".into());
    out.push("    if (rst) begin".into());
    out.push("      wr <= '0;".into());
    out.push("      rd <= '0;".into());
    out.push("      cnt <= '0;".into());
    out.push("    end else begin".into());
    out.push("      if (push) begin".into());
    out.push("        entries[wr] <= {push_data, push_close};".into());
    out.push("        wr <= (wr == DEPTH - 1) ? '0 : wr + 1'b1;".into());
    out.push("      end".into());
    out.push("      if (pop) rd <= (rd == DEPTH - 1) ? '0 : rd + 1'b1;".into());
    out.push("      if (push && !pop) cnt <= cnt + 1'b1;".into());
    out.push("      else if (pop && !push) cnt <= cnt - 1'b1;".into());
    out.push("    end".into());
    out.push("  end".into());
    out.push("".into());
    out.push("  assign pop_data = entries[rd][W:1];".into());
    out.push("  assign pop_close = entries[rd][0];".into());
    out.push("endmodule".into());
    out.join("\n") + "\n"
}

/// the RTL of a task that builds a network: the top module, the instances' modules and the FIFO
pub fn network_module(prog: &Arc<Program>, decl: &Arc<FnDecl>, params: &crate::Params) -> R<String> {
    let net = crate::graph::elaborate(prog, decl, params)?;
    let mut out = vec![fifo_module()];
    // the instances' modules
    for inst in &net.insts {
        let text = match &inst.src {
            Some(src) => {
                let p2 = Arc::new(fluxa::load_many(&[("broadcast.fx".into(), src.clone())])?);
                let d = p2.fns.get("Broadcast").ok_or("the broadcast has no task")?.clone();
                let consts: std::collections::HashSet<String> = p2.consts.read().unwrap().keys().cloned().collect();
                let cfg = crate::lower::lower(&d, &consts)?;
                task_module(&p2, &d, &cfg, &crate::Params::default())?
            }
            None => {
                let d = prog.fns.get(&inst.name).ok_or_else(|| format!("no task `{}`", inst.name))?;
                let consts: std::collections::HashSet<String> = prog.consts.read().unwrap().keys().cloned().collect();
                let cfg = crate::lower::lower(d, &consts)?;
                task_module(prog, d, &cfg, &inst.params)?
            }
        };
        out.push(text);
    }
    out.push(top_module(decl, &net)?);
    Ok(out.join("\n"))
}

/// the top module of a network: the instances, the FIFOs between them and the task's own ports
fn top_module(decl: &FnDecl, net: &crate::graph::Net) -> R<String> {
    let mut out = vec![];
    // the ports
    let mut ports = vec!["  input  logic clk".to_string(), "  input  logic rst".to_string()];
    for (name, t) in &net.iface.ins {
        ports.push(format!("  input  logic [{}:0] {}_data", t.width().max(1) - 1, name));
        ports.push(format!("  input  logic {}_close", name));
        ports.push(format!("  input  logic {}_valid", name));
        ports.push(format!("  output logic {}_ready", name));
    }
    for (name, t) in &net.iface.outs {
        ports.push(format!("  input  logic {}_ready", name));
        ports.push(format!("  output logic [{}:0] {}_data", t.width().max(1) - 1, name));
        ports.push(format!("  output logic {}_close", name));
        ports.push(format!("  output logic {}_valid", name));
    }
    // the array parameters the network passes on: the input ports
    for (n, t, _) in &net.passed {
        if !net.insts.iter().any(|i| i.passes.iter().any(|(_, j)| &net.passed[*j].0 == n)) { continue; }
        let w = Ty::of_rt(t).map(|t| t.width()).unwrap_or(1);
        ports.push(format!("  input  logic [{}:0] {}", w.max(1) - 1, n));
    }
    out.push(format!("// {}: the RTL of the network `{}` (generated by the Fluxa synthesizer)", decl.name, decl.name));
    out.push(format!("module {} (", ident(&decl.name)));
    out.push(ports.join(",\n"));
    out.push(");".into());
    out.push("".into());

    // the channels between two instances: a FIFO each
    let mut fifo_push: HashMap<usize, (String, usize, usize)> = HashMap::new();
    for (c, (_cname, _, depth)) in net.chans.iter().enumerate() {
        let (Some(crate::graph::End::Inst(..)), Some(crate::graph::End::Inst(..))) = (net.producer.get(&c), net.consumer.get(&c)) else { continue };
        let w = chan_width(net, c)?;
        out.push(format!("  // the channel {}: a FIFO of {} tokens of {} bits", c, depth, w));
        out.push(format!("  logic [{}:0] push{}_data;", w.max(1) - 1, c));
        out.push(format!("  logic push{}_close, push{}_valid, push{}_ready;", c, c, c));
        out.push(format!("  logic [{}:0] pop{}_data;", w.max(1) - 1, c));
        out.push(format!("  logic pop{}_close, pop{}_valid, pop{}_ready;", c, c, c));
        out.push(format!("  FluxaFifo #(.W({}), .DEPTH({})) fifo{} (", w.max(1), depth, c));
        out.push("    .clk(clk), .rst(rst),".into());
        out.push(format!("    .push_data(push{}_data), .push_close(push{}_close), .push_valid(push{}_valid), .push_ready(push{}_ready),", c, c, c, c));
        out.push(format!("    .pop_data(pop{}_data), .pop_close(pop{}_close), .pop_valid(pop{}_valid), .pop_ready(pop{}_ready)", c, c, c, c));
        out.push("  );".into());
        fifo_push.insert(c, (format!("fifo{}", c), w, *depth));
    }
    out.push("".into());

    // the instances
    for (k, inst) in net.insts.iter().enumerate() {
        let me = format!("{}{}", inst.name.trim_start_matches('$').to_lowercase(), k);
        let mut conns = vec![".clk(clk)".to_string(), ".rst(rst)".to_string()];
        for (n, c, sends) in &inst.ends {
            // the channel's other end
            let other = if *sends { net.consumer.get(c) } else { net.producer.get(c) };
            let Some(other) = other else { continue };
            let prefix = match other {
                crate::graph::End::Top(t) => t.clone(),
                crate::graph::End::Inst(..) => {
                    // the sender pushes into the FIFO, the receiver pops from it
                    let side = if *sends { "push" } else { "pop" };
                    format!("{}{}", side, c)
                }
            };
            conns.push(format!(".{}_data({}_data)", n, prefix));
            conns.push(format!(".{}_close({}_close)", n, prefix));
            conns.push(format!(".{}_valid({}_valid)", n, prefix));
            conns.push(format!(".{}_ready({}_ready)", n, prefix));
        }
        // the array parameters the network passes on
        for (q, j) in &inst.passes {
            conns.push(format!(".{}({})", q, net.passed[*j].0));
        }
        // the instance's module: the task's name (the broadcast has its own)
        let module = if inst.src.is_some() { "Broadcast".to_string() } else { inst.name.clone() };
        out.push(format!("  // the instance {} of the task `{}`", me, inst.name));
        out.push(format!("  {} {} (", module, me));
        out.push(format!("    {}", conns.join(",\n    ")));
        out.push("  );".into());
    }
    out.push("endmodule".into());
    Ok(out.join("\n") + "\n")
}

/// the width of a channel's tokens
fn chan_width(net: &crate::graph::Net, c: usize) -> R<usize> {
    let (_, t, _) = &net.chans[c];
    if let Ok(t) = Ty::of_rt(t) { return Ok(t.width()); }
    for e in [net.producer.get(&c), net.consumer.get(&c)].into_iter().flatten() {
        if let crate::graph::End::Inst(k, n) = e {
            if let Some((_, t, _)) = net.insts[*k].chans.iter().find(|x| &x.0 == n) { return Ok(t.width()); }
        }
    }
    Ty::of_rt(t).map(|t| t.width())
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
