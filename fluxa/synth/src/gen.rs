//! Gates for a lowered task: one FSM state per block. In a state the block's statements are
//! evaluated symbolically into a datapath (variables are registers read at the start of the
//! cycle), then its terminator decides: a jump or branch moves on at the clock edge; a channel
//! operation waits for its handshake and only then commits the block's register updates.
//!
//! Ports: `clk`, `rst` (synchronous, active high), then per parameter of the task:
//! an input channel `c` is `c_data`, `c_close` (the token ends the stream), `c_valid` in and
//! `c_ready` out; an output channel the same with the directions swapped; any other parameter
//! is an input (or a constant given with `--param`).

use crate::lower::{Cfg, Temp, Term};
use crate::ty::*;
use eda::netlist::{self, bits_for, Bit, Builder, Dir, Port};
use fluxa::ast::*;
use fluxa::interp::Interp;
use fluxa::runtime::{block_on, Shared};
use fluxa::value::{Ovf, Round, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// what a task parameter is in hardware
#[derive(Clone)]
pub enum ParamHw {
    InChan(Ty),
    OutChan(Ty),
    /// a parameter that is not a channel: a constant (a value) or an input port (None)
    Scalar(Ty, Option<Value>),
}

/// a constant parameter's value in hardware (an array of constants: a ROM, folded into gates)
fn const_hv(g: &mut Builder, v: &Value, t: &Ty) -> Option<HV> {
    Some(match (v, t) {
        (Value::Array(xs), Ty::Array(e, n)) if xs.len() == *n => HV::A(xs.iter().map(|x| const_hv(g, x, e)).collect::<Option<_>>()?),
        (Value::Tuple(xs), Ty::Tuple(ts)) if xs.len() == ts.len() => HV::A(xs.iter().zip(ts).map(|(x, t)| const_hv(g, x, t)).collect::<Option<_>>()?),
        (Value::Int(k), t) if t.is_scalar() => HV::S(fit(g, &SVal::konst(*k), t)),
        (Value::Bool(b), t) if t.is_scalar() => HV::S(fit(g, &SVal::konst(*b as i128), t)),
        _ => return None,
    })
}

pub struct Gen<'p> {
    pub g: Builder,
    prog: &'p Arc<fluxa::ast::Program>,
    it: Interp,
    /// the variables' types (the width of their registers)
    pub vars: HashMap<String, Ty>,
    /// untyped variables: their type grows with what is assigned (until the type pass settles)
    untyped: HashSet<String>,
    pub env: HashMap<String, HV>,
    grew: bool,
    rename: usize,
    depth: usize,
}

fn err<T>(line: u32, prog: &fluxa::ast::Program, m: impl Into<String>) -> R<T> { Err(format!("{}: {}", prog.loc(line), m.into())) }

impl<'p> Gen<'p> {
    pub fn new(prog: &'p Arc<fluxa::ast::Program>) -> Gen<'p> {
        let sh = Shared::new(prog.clone(), false, false);
        Gen { g: Builder::new(), prog, it: Interp::new(sh, 0, "synth".into()), vars: HashMap::new(), untyped: Default::default(), env: HashMap::new(), grew: false, rename: 1_000_000, depth: 0 }
    }

    fn here(&self, line: u32) -> String { self.prog.loc(line) }

    /// a type expression's hardware type (constants and generics are known at compile time)
    pub fn ty(&mut self, t: &TypeExpr, line: u32) -> R<Ty> {
        let rt = block_on(self.it.eval_type(t)).map_err(|e| format!("{}: type: {:?}", self.here(line), e))?;
        Ty::of_rt(&rt).map_err(|m| format!("{}: {}", self.here(line), m))
    }

    /// a compile-time constant (`comptime`, constants); None when it reads anything else
    fn konst(&mut self, e: &Expr) -> Option<Value> {
        let mut uses_local = false;
        crate::walk_paths(e, &mut |p| if p.contains('.') || p.contains('~') { uses_local = true });
        if uses_local { return None; }
        block_on(self.it.eval(e)).ok()
    }

    fn value(&self, v: &Value, line: u32) -> R<HV> {
        Ok(match v {
            Value::Int(n) => HV::S(SVal::konst(*n)),
            Value::Bool(b) => HV::S(SVal::bool_const(*b)),
            Value::Fixed(r, f) => HV::S(SVal::fixed_const(*r, *f)),
            Value::Array(xs) | Value::Tuple(xs) => HV::A(xs.iter().map(|x| self.value(x, line)).collect::<R<_>>()?),
            Value::Range(lo, hi) => HV::A((*lo..*hi).map(|k| HV::S(SVal::konst(k))).collect()),
            Value::Str(_) | Value::Unit => HV::S(SVal::konst(0)),
            // a float that is a binary fraction (0.375 = 3/8) is an exact fixed-point constant
            Value::Float(x) => {
                let f = (0..=60).find(|f| { let y = x * 2f64.powi(*f); y.fract() == 0.0 && y.abs() < 2f64.powi(100) });
                match f {
                    Some(f) => HV::S(SVal::fixed_const((x * 2f64.powi(f)) as i128, f as u32)),
                    None => return err(line, self.prog, format!("the float {} is not a binary fraction: give it as a fixed-point constant", x)),
                }
            }
            other => return err(line, self.prog, format!("the constant {} is not synthesized", fluxa::value::kind(other))),
        })
    }

    // ------------------------------------------------------------ variables

    /// declare `name` of type `t` (None: the value's own type, growing) with value `v`
    fn declare(&mut self, name: &str, t: Option<Ty>, v: HV) -> R<()> {
        let t = match t {
            Some(t) => { self.untyped.remove(name); self.vars.entry(name.into()).or_insert(t).clone() }
            None => {
                if !self.vars.contains_key(name) { self.untyped.insert(name.into()); }
                let cur = self.vars.get(name).cloned();
                let t = widen(cur.clone(), v.ty());
                if cur.as_ref() != Some(&t) { self.grew = true; self.vars.insert(name.into(), t.clone()); }
                t
            }
        };
        let v = fit_hv(&mut self.g, &v, &t)?;
        self.env.insert(name.into(), v);
        Ok(())
    }

    /// `name` (whole, or the element / field at `path`) gets `v`
    fn store(&mut self, name: &str, path: &[PathStep], v: HV, line: u32) -> R<()> {
        if !self.vars.contains_key(name) && self.untyped.contains(name) && path.is_empty() { return self.declare(name, None, v); }
        let t = match self.vars.get(name) { Some(t) => t.clone(), None => return err(line, self.prog, format!("assignment to `{}`, which is not a variable here", base(name))) };
        let cur = match self.env.get(name) { Some(c) => c.clone(), None => HV::konst_of(&t) };
        let new = self.put(&cur, &t, path, v, line)?;
        if self.untyped.contains(name) {
            let w = widen(Some(t.clone()), new.ty());
            if w != t { self.grew = true; self.vars.insert(name.into(), w); }
            let w = self.vars[name].clone();
            let new = fit_hv(&mut self.g, &new, &w)?;
            self.env.insert(name.into(), new);
        } else {
            let new = fit_hv(&mut self.g, &new, &t).map_err(|m| format!("{}: {}", self.here(line), m))?;
            self.env.insert(name.into(), new);
        }
        Ok(())
    }

    /// `cur` (of type `t`) with the part at `path` replaced by `v`
    fn put(&mut self, cur: &HV, t: &Ty, path: &[PathStep], v: HV, line: u32) -> R<HV> {
        let Some((step, rest)) = path.split_first() else { return Ok(v) };
        let (xs, et): (&Vec<HV>, Vec<Ty>) = match (cur, t) {
            (HV::A(xs), Ty::Array(e, n)) => (xs, vec![(**e).clone(); *n]),
            (HV::A(xs), Ty::Tuple(ts)) => (xs, ts.clone()),
            _ => return err(line, self.prog, "indexing something that is not an array"),
        };
        let mut out = xs.clone();
        match step {
            PathStep::Const(k) => {
                let k = *k;
                if k >= out.len() { return err(line, self.prog, format!("index {} out of bounds (length {})", k, out.len())); }
                out[k] = self.put(&xs[k], &et[k], rest, v, line)?;
            }
            PathStep::Var(i) => {
                // every element: the new value where the index matches
                for k in 0..out.len() {
                    if i.hi < k as i128 || i.lo > k as i128 { continue; }
                    let hit = compare(&mut self.g, i, &SVal::konst(k as i128), "==").bits[0];
                    let nv = self.put(&xs[k], &et[k], rest, v.clone(), line)?;
                    let nv = fit_hv(&mut self.g, &nv, &et[k])?;
                    let old = fit_hv(&mut self.g, &xs[k], &et[k])?;
                    out[k] = select_hv(&mut self.g, hit, &nv, &old).map_err(|m| format!("{}: {}", self.here(line), m))?;
                }
            }
        }
        Ok(HV::A(out))
    }

    // ------------------------------------------------------------ statements

    pub fn stmts(&mut self, ss: &[Stmt]) -> R<()> {
        for s in ss { self.stmt(s)?; }
        Ok(())
    }

    fn bind(&mut self, p: &Pattern, v: HV, t: Option<Ty>, line: u32) -> R<()> {
        match p {
            Pattern::Ident(n) => self.declare(n, t, v),
            Pattern::Wild => Ok(()),
            Pattern::Tuple(ps) => match v {
                HV::A(xs) if xs.len() == ps.len() => { for (p, x) in ps.iter().zip(xs) { self.bind(p, x, None, line)?; } Ok(()) }
                _ => err(line, self.prog, format!("cannot destructure this value into a {}-tuple", ps.len())),
            },
        }
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        let line = s.line;
        match &s.kind {
            StmtKind::Let { pat, ty, init, .. } => {
                let t = match ty { Some(t) => Some(self.ty(t, line)?), None => None };
                let v = match init {
                    Some(e) => self.eval(e)?,
                    None => match &t { Some(t) => HV::konst_of(t), None => return err(line, self.prog, "`let` needs an initializer or a type") },
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
                        HV::S(self.binop(*op, &a, &b, wrap_ty, line)?)
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
    fn iterate(&mut self, iter: &Expr, line: u32) -> R<Vec<HV>> {
        if let ExprKind::Range(lo, hi, incl) = &iter.kind {
            let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else {
                return err(line, self.prog, "a loop without channel operations needs constant bounds to be unrolled");
            };
            let h = if *incl { h + 1 } else { h };
            return Ok((l..h).map(|k| HV::S(SVal::konst(k))).collect());
        }
        match self.eval(iter)? {
            HV::A(xs) => Ok(xs),
            HV::S(_) => err(line, self.prog, "a loop over something that is neither a range nor an array"),
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
    fn target(&mut self, e: &Expr) -> R<(String, Vec<PathStep>)> {
        match &e.kind {
            ExprKind::Path(p) if p.len() == 1 => Ok((p[0].clone(), vec![])),
            ExprKind::Index(b, i) => {
                let (n, mut path) = self.target(b)?;
                let iv = self.scalar(i)?;
                path.push(match iv.as_const() { Some(k) if k >= 0 => PathStep::Const(k as usize), _ => PathStep::Var(iv) });
                Ok((n, path))
            }
            ExprKind::Field(b, f) => {
                let (n, mut path) = self.target(b)?;
                let k: usize = f.parse().map_err(|_| format!("{}: unknown field `{}`", self.here(e.line), f))?;
                path.push(PathStep::Const(k));
                Ok((n, path))
            }
            _ => err(e.line, self.prog, "invalid assignment target"),
        }
    }

    /// the type of the part of variable `name` at `path`
    fn part_ty(&self, name: &str, path: &[PathStep], line: u32) -> R<Ty> {
        let mut t = self.vars.get(name).cloned().ok_or_else(|| format!("{}: wrapping operators need a typed variable on the left", self.here(line)))?;
        for s in path {
            t = match (t, s) {
                (Ty::Array(e, _), _) => *e,
                (Ty::Tuple(ts), PathStep::Const(k)) => ts[*k].clone(),
                _ => return err(line, self.prog, "bad index"),
            };
        }
        Ok(t)
    }

    // ------------------------------------------------------------ expressions

    pub fn scalar(&mut self, e: &Expr) -> R<SVal> {
        let v = self.eval(e)?;
        v.s().cloned().map_err(|m| format!("{}: {}", self.here(e.line), m))
    }

    pub fn eval(&mut self, e: &Expr) -> R<HV> {
        let line = e.line;
        match &e.kind {
            ExprKind::Int(n) => Ok(HV::S(SVal::konst(*n))),
            ExprKind::Bool(b) => Ok(HV::S(SVal::bool_const(*b))),
            ExprKind::Float(_) => match self.konst(e) { Some(v) => self.value(&v, line), None => err(line, self.prog, "floating point is only for compile-time constants") },
            ExprKind::Path(p) => {
                let n = p.last().unwrap();
                if p.len() == 1 { if let Some(v) = self.env.get(n) { return Ok(v.clone()); } }
                let c = self.prog.consts.read().unwrap().get(n).cloned();
                match c {
                    Some(v) => self.value(&v, line),
                    None => err(line, self.prog, format!("`{}` has no value here (not a variable or constant)", base(n))),
                }
            }
            ExprKind::Unary(c, x) => {
                let v = self.scalar(x)?;
                Ok(HV::S(match c {
                    '-' => neg(&mut self.g, &v),
                    '!' => { let t = truth(&mut self.g, &v); SVal::bit(self.g.not(t)) }
                    _ => not_bits(&mut self.g, &v),
                }))
            }
            ExprKind::Binary(op, l, r) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    let (a, b) = (self.scalar(l)?, self.scalar(r)?);
                    let (x, y) = (truth(&mut self.g, &a), truth(&mut self.g, &b));
                    return Ok(HV::S(SVal::bit(if *op == BinOp::And { self.g.and(x, y) } else { self.g.or(x, y) })));
                }
                let (a, b) = (self.scalar(l)?, self.scalar(r)?);
                let wrap_ty = if matches!(op, BinOp::AddW | BinOp::SubW | BinOp::MulW) {
                    let (n, path) = self.target(l).map_err(|_| format!("{}: wrapping operators need a typed variable on the left", self.here(line)))?;
                    Some(self.part_ty(&n, &path, line)?)
                } else { None };
                Ok(HV::S(self.binop(*op, &a, &b, wrap_ty, line)?))
            }
            ExprKind::Cast(x, t) => {
                let v = self.eval(x)?;
                let t = self.ty(t, line)?;
                self.convert_hv(&v, &t, Round::Trunc, Ovf::Wrap, line)
            }
            ExprKind::Call(c, a) => self.call(c, a, line),
            ExprKind::Method(r, n, t, a) => self.method(r, n, t, a, line),
            ExprKind::Field(x, f) => {
                let v = self.eval(x)?;
                let k: usize = f.parse().map_err(|_| format!("{}: unknown field `{}`", self.here(line), f))?;
                match v { HV::A(xs) if k < xs.len() => Ok(xs[k].clone()), _ => err(line, self.prog, format!("no field `{}`", f)) }
            }
            ExprKind::Index(x, i) => {
                let v = self.eval(x)?;
                // a slice x[a..b] of an array
                if let ExprKind::Range(lo, hi, incl) = &i.kind {
                    let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else { return err(line, self.prog, "a slice needs constant bounds") };
                    let h = if *incl { h + 1 } else { h };
                    return match v {
                        HV::A(xs) if l >= 0 && h >= l && (h as usize) <= xs.len() => Ok(HV::A(xs[l as usize..h as usize].to_vec())),
                        _ => err(line, self.prog, format!("slice {}..{} out of bounds", l, h)),
                    };
                }
                let k = self.scalar(i)?;
                match v {
                    HV::A(xs) => self.index(&xs, &k, line),
                    HV::S(s) => match k.as_const() {
                        Some(k) if k >= 0 => Ok(HV::S(SVal::of_ty(vec![s.ext(k as usize + 1)[k as usize]], &Ty::Int { lo: 0, hi: 1 }))),
                        _ => { let sh = shift(&mut self.g, &s, &k, false)?; Ok(HV::S(SVal::of_ty(vec![sh.bits[0]], &Ty::Int { lo: 0, hi: 1 }))) }
                    },
                }
            }
            ExprKind::Slice(x, h, l) => {
                let v = self.scalar(x)?;
                let (Some(h), Some(l)) = (self.scalar(h)?.as_const(), self.scalar(l)?.as_const()) else { return err(line, self.prog, "a bit slice needs constant bounds") };
                if l < 0 || h < l { return err(line, self.prog, format!("invalid bit slice [{}:{}]", h, l)); }
                let w = (h - l + 1) as usize;
                let bits = v.ext(h as usize + 1)[l as usize..=h as usize].to_vec();
                Ok(HV::S(SVal::of_ty(bits, &Ty::Int { lo: 0, hi: (1i128 << w) - 1 })))
            }
            ExprKind::Range(lo, hi, incl) => {
                let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else { return err(line, self.prog, "a range used as a value needs constant bounds") };
                let h = if *incl { h + 1 } else { h };
                Ok(HV::A((l..h).map(|k| HV::S(SVal::konst(k))).collect()))
            }
            ExprKind::Array(xs) | ExprKind::Tuple(xs) => Ok(HV::A(xs.iter().map(|x| self.eval(x)).collect::<R<_>>()?)),
            ExprKind::Repeat(x, n) => {
                let v = self.eval(x)?;
                let Some(n) = self.scalar(n)?.as_const() else { return err(line, self.prog, "[x; n] needs a constant n") };
                Ok(HV::A(vec![v; n.max(0) as usize]))
            }
            ExprKind::Block(b) => {
                self.stmts(&b.stmts)?;
                match &b.tail { Some(t) => self.eval(t), None => Ok(HV::S(SVal::konst(0))) }
            }
            ExprKind::Comptime(_) => match self.konst(e) {
                Some(v) => self.value(&v, line),
                None => err(line, self.prog, "comptime block that reads run-time values"),
            },
            ExprKind::If(c, t, el) => {
                let cv = self.scalar(c)?;
                let cb = truth(&mut self.g, &cv);
                let before = self.env.clone();
                let tv = { self.stmts(&t.stmts)?; match &t.tail { Some(x) => self.eval(x)?, None => HV::S(SVal::konst(0)) } };
                let after_t = std::mem::replace(&mut self.env, before);
                let fv = match el { Some(x) => self.eval(x)?, None => HV::S(SVal::konst(0)) };
                let after_f = std::mem::take(&mut self.env);
                self.env = self.merge(cb, after_t, after_f)?;
                Ok(select_hv(&mut self.g, cb, &tv, &fv).unwrap_or(tv))
            }
            ExprKind::Match(s, arms) => self.matching(s, arms, line),
            ExprKind::Str(_) => Ok(HV::S(SVal::konst(0))),
            _ => err(line, self.prog, "this expression is not synthesized yet"),
        }
    }

    /// element `k` of `xs` (a run-time index: a multiplexer over the elements)
    fn index(&mut self, xs: &[HV], k: &SVal, line: u32) -> R<HV> {
        if let Some(c) = k.as_const() {
            return xs.get(c as usize).cloned().ok_or_else(|| format!("{}: index {} out of bounds (length {})", self.here(line), c, xs.len()));
        }
        if xs.is_empty() { return err(line, self.prog, "indexing an empty array"); }
        let t = xs.iter().skip(1).fold(xs[0].ty(), |a, b| widen(Some(a), b.ty()));
        let mut out = fit_hv(&mut self.g, &xs[0], &t)?;
        for (j, x) in xs.iter().enumerate().skip(1) {
            if k.hi < j as i128 || k.lo > j as i128 { continue; }
            let hit = compare(&mut self.g, k, &SVal::konst(j as i128), "==").bits[0];
            let xv = fit_hv(&mut self.g, x, &t)?;
            out = select_hv(&mut self.g, hit, &xv, &out)?;
        }
        Ok(out)
    }

    /// the variables after `c ? then : else`
    fn merge(&mut self, c: Bit, t: HashMap<String, HV>, f: HashMap<String, HV>) -> R<HashMap<String, HV>> {
        let mut out = HashMap::new();
        for (k, tv) in &t {
            match f.get(k) {
                Some(fv) if fv.flat() == tv.flat() => { out.insert(k.clone(), tv.clone()); }
                Some(fv) => {
                    let ty = self.vars.get(k).cloned().unwrap_or(tv.ty());
                    let (a, b) = (fit_hv(&mut self.g, tv, &ty)?, fit_hv(&mut self.g, fv, &ty)?);
                    out.insert(k.clone(), select_hv(&mut self.g, c, &a, &b)?);
                }
                None => { out.insert(k.clone(), tv.clone()); }
            }
        }
        for (k, fv) in f { out.entry(k).or_insert(fv); }
        Ok(out)
    }

    fn matching(&mut self, s: &Expr, arms: &[Arm], line: u32) -> R<HV> {
        let sv = self.scalar(s)?;
        let mut conds = vec![];
        let mut vals = vec![];
        let mut envs = vec![];
        let start = self.env.clone();
        for a in arms {
            self.env = start.clone();
            let c = match &a.pat {
                MatchPat::Wild => Bit::C1,
                MatchPat::Bind(n) => {
                    let c = self.prog.consts.read().unwrap().get(n).cloned();
                    match c {
                        Some(v) => { let k = self.value(&v, line)?; compare(&mut self.g, &sv, k.s()?, "==").bits[0] }
                        None => { self.declare(n, None, HV::S(sv.clone()))?; Bit::C1 }
                    }
                }
                MatchPat::Lit(e) => { let v = self.scalar(e)?; compare(&mut self.g, &sv, &v, "==").bits[0] }
                MatchPat::Range(lo, hi, incl) => {
                    let (l, h) = (self.scalar(lo)?, self.scalar(hi)?);
                    let a1 = compare(&mut self.g, &sv, &l, ">=").bits[0];
                    let a2 = compare(&mut self.g, &sv, &h, if *incl { "<=" } else { "<" }).bits[0];
                    self.g.and(a1, a2)
                }
                MatchPat::Tuple(_) => return err(line, self.prog, "tuple patterns are not synthesized yet"),
            };
            let c = match &a.guard {
                Some(g) => { let gv = self.scalar(g)?; let gb = truth(&mut self.g, &gv); self.g.and(c, gb) }
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
            v = select_hv(&mut self.g, c, &tv, &v)?;
            env = self.merge(c, te, env)?;
        }
        self.env = env;
        Ok(v)
    }

    fn binop(&mut self, op: BinOp, a: &SVal, b: &SVal, wrap_ty: Option<Ty>, line: u32) -> R<SVal> {
        let at = self.here(line);
        let g = &mut self.g;
        Ok(match op {
            BinOp::Add => add(g, a, b),
            BinOp::Sub => sub(g, a, b),
            BinOp::Mul => mul(g, a, b),
            BinOp::Div => divrem(g, a, b).map_err(|m| format!("{}: {}", at, m))?.0,
            BinOp::Rem => divrem(g, a, b).map_err(|m| format!("{}: {}", at, m))?.1,
            BinOp::AddW | BinOp::SubW | BinOp::MulW => {
                let t = wrap_ty.ok_or_else(|| format!("{}: wrapping operators need a typed variable on the left", at))?;
                let v = match op { BinOp::AddW => add(g, a, b), BinOp::SubW => sub(g, a, b), _ => mul(g, a, b) };
                convert(g, &v, &t, Round::Trunc, Ovf::Wrap)
            }
            BinOp::Shl | BinOp::Shr => match b.as_const() {
                Some(k) if (0..=120).contains(&k) => if op == BinOp::Shl { shl_const(a, k as u32) } else { shr_const(a, k as u32) },
                _ => shift(g, a, b, op == BinOp::Shl).map_err(|m| format!("{}: {}", at, m))?,
            },
            BinOp::BitAnd => bitop(g, a, b, '&'),
            BinOp::BitOr => bitop(g, a, b, '|'),
            BinOp::BitXor => bitop(g, a, b, '^'),
            BinOp::Eq => compare(g, a, b, "=="),
            BinOp::Ne => compare(g, a, b, "!="),
            BinOp::Lt => compare(g, a, b, "<"),
            BinOp::Le => compare(g, a, b, "<="),
            BinOp::Gt => compare(g, a, b, ">"),
            BinOp::Ge => compare(g, a, b, ">="),
            BinOp::And | BinOp::Or => unreachable!(),
        })
    }

    /// `v` converted to `t` leaf by leaf
    fn convert_hv(&mut self, v: &HV, t: &Ty, r: Round, o: Ovf, line: u32) -> R<HV> {
        match (v, t) {
            (HV::S(s), t) if t.is_scalar() => Ok(HV::S(convert(&mut self.g, s, t, r, o))),
            (HV::A(xs), Ty::Array(e, _)) => Ok(HV::A(xs.iter().map(|x| self.convert_hv(x, e, r, o, line)).collect::<R<_>>()?)),
            (HV::A(xs), t) if t.is_scalar() => Ok(HV::A(xs.iter().map(|x| self.convert_hv(x, t, r, o, line)).collect::<R<_>>()?)),
            _ => err(line, self.prog, "cannot convert this value to that type"),
        }
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

    fn method(&mut self, r: &Expr, n: &str, t: &[TArg], a: &[Arg], line: u32) -> R<HV> {
        let v = self.eval(r)?;
        match (n, &v) {
            ("to" | "quantize", _) => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, None); self.convert_hv(&v, &ty, rr, o, line) }
            ("sat", _) => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, Some(Ovf::Sat)); self.convert_hv(&v, &ty, rr, o, line) }
            ("wrap", _) => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, Some(Ovf::Wrap)); self.convert_hv(&v, &ty, rr, o, line) }
            ("len", HV::A(xs)) => Ok(HV::S(SVal::konst(xs.len() as i128))),
            ("rev", HV::A(xs)) => Ok(HV::A(xs.iter().rev().cloned().collect())),
            ("unrolled" | "iter" | "collect", HV::A(_)) => Ok(v),
            ("enumerate", HV::A(xs)) => Ok(HV::A(xs.iter().enumerate().map(|(i, x)| HV::A(vec![HV::S(SVal::konst(i as i128)), x.clone()])).collect())),
            ("map", HV::A(xs)) => { let xs = xs.clone(); let f = a.first().ok_or("map() takes a lambda")?.value.clone(); Ok(HV::A(self.lambda(&f, xs, line)?)) }
            ("sum", HV::A(xs)) => {
                let mut it = xs.iter();
                let mut acc = it.next().ok_or_else(|| format!("{}: sum of an empty array", self.here(line)))?.s()?.clone();
                for x in it { acc = add(&mut self.g, &acc, x.s()?); }
                Ok(HV::S(acc))
            }
            ("tree_sum", HV::A(xs)) => {
                let mut items: Vec<SVal> = xs.iter().map(|x| x.s().cloned()).collect::<R<_>>()?;
                if items.is_empty() { return err(line, self.prog, "sum of an empty array"); }
                while items.len() > 1 {
                    let mut next = vec![];
                    for ch in items.chunks(2) { next.push(if ch.len() == 2 { add(&mut self.g, &ch[0], &ch[1]) } else { ch[0].clone() }); }
                    items = next;
                }
                Ok(HV::S(items.remove(0)))
            }
            ("abs", HV::S(s)) => {
                let s = s.clone();
                let ng = neg(&mut self.g, &s);
                let lt = compare(&mut self.g, &s, &SVal { frac: s.frac, ..SVal::konst(0) }, "<").bits[0];
                let x = select(&mut self.g, lt, &ng, &s);
                let hi = s.lo.abs().max(s.hi.abs());
                let lo = if s.lo >= 0 { s.lo } else if s.hi <= 0 { -s.hi } else { 0 };
                Ok(HV::S(SVal { lo, hi, ..x }.normalized()))
            }
            ("xor_reduce", HV::S(s)) => Ok(HV::S(SVal::of_ty(vec![self.g.reduce_xor(&s.bits)], &Ty::Int { lo: 0, hi: 1 }))),
            ("popcount", HV::S(s)) => {
                let mut acc = SVal::konst(0);
                for b in s.bits.clone() { let x = SVal::of_ty(vec![b], &Ty::Int { lo: 0, hi: 1 }); acc = add(&mut self.g, &acc, &x); }
                Ok(HV::S(acc))
            }
            (other, _) => err(line, self.prog, format!("method `{}` is not synthesized here", other)),
        }
    }

    /// a lambda `|x| ..` (or `|i, x| ..` over pairs) applied to each item
    fn lambda(&mut self, lam: &Expr, items: Vec<HV>, line: u32) -> R<Vec<HV>> {
        let (ps, body) = match &lam.kind { ExprKind::Lambda(p, b) => (p.clone(), b.clone()), _ => return err(line, self.prog, "expected a lambda such as `|x| x * 2`") };
        let mut out = vec![];
        for it in items {
            if ps.len() == 1 { self.declare(&ps[0], None, it)?; }
            else if let HV::A(xs) = it { for (p, x) in ps.iter().zip(xs) { self.declare(p, None, x)?; } }
            out.push(self.eval(&body)?);
        }
        Ok(out)
    }

    fn call(&mut self, c: &Expr, a: &[Arg], line: u32) -> R<HV> {
        let name = match &c.kind { ExprKind::Path(p) | ExprKind::PathT(p, _) => p.last().unwrap().clone(), _ => return err(line, self.prog, "callee must be a function name") };
        match name.as_str() {
            "print" => return Ok(HV::S(SVal::konst(0))),
            "min" | "max" if a.len() == 2 => {
                let (x, y) = (self.scalar(&a[0].value)?, self.scalar(&a[1].value)?);
                let lt = compare(&mut self.g, &x, &y, "<").bits[0];
                return Ok(HV::S(if name == "min" { select(&mut self.g, lt, &x, &y) } else { select(&mut self.g, lt, &y, &x) }));
            }
            "abs" if a.len() == 1 => {
                let r = Expr { kind: ExprKind::Method(Box::new(a[0].value.clone()), "abs".into(), vec![], vec![]), line };
                return self.eval(&r);
            }
            "concat" => {
                let mut out = vec![];
                for x in a { match self.eval(&x.value)? { HV::A(xs) => out.extend(xs), _ => return err(line, self.prog, "concat expects arrays") } }
                return Ok(HV::A(out));
            }
            "len" if a.len() == 1 => return match self.eval(&a[0].value)? { HV::A(xs) => Ok(HV::S(SVal::konst(xs.len() as i128))), _ => err(line, self.prog, "len() expects an array") },
            "zip" if a.len() == 2 => {
                let (x, y) = (self.eval(&a[0].value)?, self.eval(&a[1].value)?);
                return match (x, y) { (HV::A(xs), HV::A(ys)) => Ok(HV::A(xs.into_iter().zip(ys).map(|(p, q)| HV::A(vec![p, q])).collect())), _ => err(line, self.prog, "zip() expects two arrays") };
            }
            "array" if a.len() == 2 => {
                let Some(n) = self.scalar(&a[0].value)?.as_const() else { return err(line, self.prog, "array(n, ..) needs a constant n") };
                let items = (0..n).map(|k| HV::S(SVal::konst(k))).collect();
                return Ok(HV::A(self.lambda(&a[1].value, items, line)?));
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
            let t = self.ty(&p.ty, line)?;
            self.declare(u, Some(t), v)?;
        }
        self.depth += 1;
        let mut stmts = body.stmts.clone();
        let mut tail = body.tail.clone();
        if tail.is_none() {
            if let Some(Stmt { kind: StmtKind::Return(Some(e)), .. }) = stmts.last() { tail = Some(Box::new(e.clone())); stmts.pop(); }
        }
        let r = self.stmts(&stmts).and_then(|_| match &tail { Some(t) => self.eval(t), None => Ok(HV::S(SVal::konst(0))) });
        self.depth -= 1;
        let v = r?;
        match &decl.ret { Some(rt) => { let t = self.ty(rt, line)?; fit_hv(&mut self.g, &v, &t) } None => Ok(v) }
    }
}

/// a step into an array or tuple: a constant index, or one known only at run time
#[derive(Clone)]
pub enum PathStep {
    Const(usize),
    Var(SVal),
}

/// the source name of a unique name (`x.3` → `x`)
pub fn base(n: &str) -> &str { n.split(['.', '~']).next().unwrap_or(n) }

// ---------------------------------------------------------------- the circuit

struct Chan { ty: Ty, out: bool }

/// a memory read port: address, word read
type MemRead = (Vec<Bit>, Vec<Bit>);
/// a memory write: when, address, word
type MemWrite = (Bit, Vec<Bit>, Vec<Bit>);

/// the task `decl` lowered to `cfg` as a netlist; `params` fixes parameters to constants
pub fn build(prog: &Arc<fluxa::ast::Program>, decl: &FnDecl, cfg: &Cfg, params: &crate::Params) -> R<crate::Synth> {
    let mut warnings = vec![];
    let mut gen = Gen::new(prog);
    let mut hw = vec![];
    for (p, u) in decl.params.iter().zip(&cfg.params) {
        let h = match &p.ty {
            TypeExpr::Chan(t, _) => {
                let ty = gen.ty(t, decl.line)?;
                match crate::dir_of(decl, &p.name) {
                    Some(true) => ParamHw::OutChan(ty),
                    Some(false) => ParamHw::InChan(ty),
                    None => { warnings.push(format!("channel `{}` is never used", p.name)); ParamHw::InChan(ty) }
                }
            }
            t => {
                let ty = gen.ty(t, decl.line)?;
                // a constant (an array: a ROM), else an input port of all its bits
                let k = params.get(&p.name).cloned();
                if let Some(v) = &k {
                    if const_hv(&mut Builder::new(), v, &ty).is_none() { return Err(format!("{}: parameter `{}`: the value ({}) does not have its type", prog.loc(decl.line), p.name, fluxa::value::kind(v))); }
                }
                ParamHw::Scalar(ty, k)
            }
        };
        hw.push((p.name.clone(), u.clone(), h));
    }
    let chans: HashMap<String, Chan> = hw.iter().filter_map(|(_, u, h)| match h {
        ParamHw::InChan(t) => Some((u.clone(), Chan { ty: t.clone(), out: false })),
        ParamHw::OutChan(t) => Some((u.clone(), Chan { ty: t.clone(), out: true })),
        _ => None,
    }).collect();

    // the memories: word type and size
    let mut mems: HashMap<String, (Ty, usize)> = HashMap::new();
    for (m, t, n) in &cfg.mems {
        match gen.ty(&TypeExpr::Mem(Box::new(t.clone()), Box::new(n.clone())), decl.line)? {
            Ty::Array(e, n) if e.is_scalar() => { mems.insert(m.clone(), (*e, n)); }
            _ => return Err(format!("{}: memory `{}`: words that are arrays or tuples are not synthesized", prog.loc(decl.line), base(m))),
        }
    }
    warnings.extend(cfg.warnings.iter().cloned());

    // the type pass: the widths of the variables, untyped ones widened until nothing grows
    let mut vars: HashMap<String, Ty> = HashMap::new();
    let mut untyped: HashSet<String> = Default::default();
    for round in 0.. {
        let mut t = Gen::new(prog);
        t.vars = vars.clone();
        t.untyped = untyped.clone();
        temps(&mut t, cfg, &chans, &mems, &hw)?;
        for b in &cfg.blocks {
            t.env = scratch_env(&mut t, &hw);
            if let Term::Pipe { head, cond, recvs, .. } = &b.term {
                for (dst, ch) in head.iter().chain(recvs) { term_values(&mut t, &Term::Recv { dst: dst.clone(), ch: ch.clone(), closed: None, next: 0 }, &chans, &mems)?; }
                if let Some(c) = cond { t.eval(c)?; }
            }
            t.stmts(&b.stmts)?;
            term_values(&mut t, &b.term, &chans, &mems)?;
        }
        if !t.grew { vars = t.vars; untyped = t.untyped; break; }
        if round == 12 {
            let grow: Vec<&str> = t.untyped.iter().map(|n| base(n)).collect();
            return Err(format!("{}: variable(s) {} keep growing with every assignment: give them a type (or use `carry`)", decl.name, grow.join(", ")));
        }
        vars = t.vars;
        untyped = t.untyped;
    }

    // the circuit
    let mut g = Gen::new(prog);
    g.vars = vars.clone();
    g.untyped = untyped;
    let clk = g.g.wire();
    let rst = g.g.wire();
    let mut ports = vec![Port { name: "clk".into(), dir: Dir::In, bits: vec![clk] }, Port { name: "rst".into(), dir: Dir::In, bits: vec![rst] }];
    struct ChanPorts { data: Vec<Bit>, close: Bit, valid: Bit, ready: Bit }
    let mut cp: HashMap<String, ChanPorts> = HashMap::new();
    let mut scalars: HashMap<String, HV> = HashMap::new();
    for (n, u, h) in &hw {
        match h {
            ParamHw::InChan(t) => {
                let (data, close, valid) = (g.g.wires(t.width() as u32), g.g.wire(), g.g.wire());
                ports.push(Port { name: format!("{}_data", n), dir: Dir::In, bits: data.clone() });
                ports.push(Port { name: format!("{}_close", n), dir: Dir::In, bits: vec![close] });
                ports.push(Port { name: format!("{}_valid", n), dir: Dir::In, bits: vec![valid] });
                cp.insert(u.clone(), ChanPorts { data, close, valid, ready: Bit::C0 });
            }
            ParamHw::OutChan(_) => {
                let ready = g.g.wire();
                ports.push(Port { name: format!("{}_ready", n), dir: Dir::In, bits: vec![ready] });
                cp.insert(u.clone(), ChanPorts { data: vec![], close: Bit::C0, valid: Bit::C0, ready });
            }
            ParamHw::Scalar(t, Some(k)) => { let v = const_hv(&mut g.g, k, t).unwrap(); scalars.insert(u.clone(), v); }
            ParamHw::Scalar(t, None) => {
                let bits = g.g.wires(t.width() as u32);
                ports.push(Port { name: n.clone(), dir: Dir::In, bits: bits.clone() });
                scalars.insert(u.clone(), HV::of_ty(&bits, t));
            }
        }
    }
    // registers: the variables, the state, which channels are closed
    let mut names: Vec<String> = vars.keys().cloned().collect();
    names.sort();
    let q: HashMap<String, HV> = names.iter().map(|n| { let t = &vars[n]; let w = g.g.wires(t.width() as u32); (n.clone(), HV::of_ty(&w, t)) }).collect();
    let nst = cfg.blocks.len();
    let sw = bits_for(nst).max(1);
    let state = g.g.wires(sw as u32);
    let mut closed: HashMap<String, Bit> = HashMap::new();
    for u in chans.keys() { closed.insert(u.clone(), g.g.wire()); }

    // a memory's words (flip-flops; a block RAM where the target has one, see eda::bram) and its
    // ports: reads (address, word read), writes (when, address, word)
    let mut store: HashMap<String, Vec<Vec<Bit>>> = HashMap::new();
    for (m, (t, n)) in &mems { store.insert(m.clone(), (0..*n).map(|_| g.g.wires(t.width() as u32)).collect()); }
    let mut mem_reads: HashMap<String, Vec<MemRead>> = HashMap::new();
    let mut mem_writes: HashMap<String, Vec<MemWrite>> = HashMap::new();
    let mut var_d: HashMap<String, Vec<(Bit, Vec<Bit>)>> = HashMap::new();
    let mut state_d: Vec<(Bit, Vec<Bit>)> = vec![];
    let mut closed_set: HashMap<String, Vec<Bit>> = HashMap::new();
    // what each output channel offers: (state active, data, close flag)
    let mut offers: HashMap<String, Vec<(Bit, Vec<Bit>, Bit)>> = HashMap::new();
    let mut readies: HashMap<String, Vec<Bit>> = HashMap::new();
    let konst = |k: usize| -> Vec<Bit> { (0..sw).map(|i| Bit::of((k >> i) & 1 == 1)).collect() };

    temps(&mut g, cfg, &chans, &mems, &hw)?;
    for (s, b) in cfg.blocks.iter().enumerate() {
        let is_s = g.g.decode(&state, s);
        g.env = q.clone();
        for (u, v) in &scalars { g.env.insert(u.clone(), v.clone()); }
        // a pipelined loop: whether an iteration can start (inputs here) and whether the loop ends,
        // looked at before the body runs
        let mut pipe = (Bit::C0, Bit::C0);
        if let Term::Pipe { head, cond, recvs, exit, .. } = &b.term {
            let mut more = Bit::C1;
            let mut ends = Bit::C0;
            let mut ins = Bit::C1;
            for (k, (dst, ch)) in head.iter().chain(recvs).enumerate() {
                let c = &cp[ch];
                let cl = closed[ch];
                let valid = g.g.or(c.valid, cl);
                if k == 0 && head.is_some() {
                    let is_close = { let x = g.g.and(c.valid, c.close); g.g.or(x, cl) };
                    ends = g.g.and(valid, is_close);
                    let nc = g.g.not(is_close);
                    more = g.g.and(valid, nc);
                } else {
                    ins = g.g.and(ins, valid);
                }
                let v = HV::of_ty(&c.data, &chans[ch].ty);
                let v = fit_hv(&mut g.g, &v, &vars[dst])?;
                g.env.insert(dst.clone(), v);
            }
            if let Some(c) = cond {
                let cv = g.scalar(c)?;
                more = truth(&mut g.g, &cv);
                ends = g.g.not(more);
            }
            let leave = g.g.and(is_s, ends);
            state_d.push((leave, konst(*exit)));
            let m = g.g.and(is_s, more);
            pipe = (g.g.and(m, ins), leave);
        }
        g.stmts(&b.stmts)?;
        let (fire, next): (Bit, Vec<Bit>) = match &b.term {
            Term::Goto(t) => (Bit::C1, konst(*t)),
            Term::Halt => (Bit::C0, konst(s)),
            Term::Branch(c, t, f) => {
                let cv = g.scalar(c)?;
                let cb = truth(&mut g.g, &cv);
                (Bit::C1, g.g.muxv(cb, &konst(*f), &konst(*t)))
            }
            Term::Recv { dst, ch, closed: on_close, next } => {
                let c = &cp[ch];
                let cl = closed[ch];
                let valid = g.g.or(c.valid, cl);
                let is_close = { let x = g.g.and(c.valid, c.close); g.g.or(x, cl) };
                let nc = g.g.not(cl);
                let rdy = g.g.and(is_s, nc);
                readies.entry(ch.clone()).or_default().push(rdy);
                let take_close = { let x = g.g.and(is_s, c.valid); let y = g.g.and(x, c.close); g.g.and(y, nc) };
                closed_set.entry(ch.clone()).or_default().push(take_close);
                let t = vars[dst].clone();
                let v = HV::of_ty(&c.data, &chans[ch].ty);
                let v = fit_hv(&mut g.g, &v, &t)?;
                g.env.insert(dst.clone(), v);
                let kn = konst(*next);
                let nx = match on_close { Some(x) => g.g.muxv(is_close, &kn, &konst(*x)), None => kn };
                (valid, nx)
            }
            Term::Send { ch, val, next } => {
                let v = g.eval(val)?;
                let v = fit_hv(&mut g.g, &v, &chans[ch].ty)?;
                offers.entry(ch.clone()).or_default().push((is_s, v.flat(), Bit::C0));
                (cp[ch].ready, konst(*next))
            }
            Term::Close { ch, next } => {
                offers.entry(ch.clone()).or_default().push((is_s, vec![], Bit::C1));
                let fired = g.g.and(is_s, cp[ch].ready);
                closed_set.entry(ch.clone()).or_default().push(fired);
                (cp[ch].ready, konst(*next))
            }
            Term::MemRead { dst, mem, addr, next } => {
                let a = g.scalar(addr)?;
                let a = mem_addr(&a, mems[mem].1);
                let rd = g.g.select(&store[mem], &a);
                g.env.insert(dst.clone(), HV::S(SVal::of_ty(rd.clone(), &mems[mem].0)));
                mem_reads.entry(mem.clone()).or_default().push((a, rd));
                (Bit::C1, konst(*next))
            }
            Term::MemWrite { mem, addr, val, next } => {
                let a = g.scalar(addr)?;
                let a = mem_addr(&a, mems[mem].1);
                let v = g.scalar(val)?;
                let v = fit(&mut g.g, &v, &mems[mem].0);
                mem_writes.entry(mem.clone()).or_default().push((is_s, a, v.bits));
                (Bit::C1, konst(*next))
            }
            Term::Pipe { head, recvs, sends, .. } => {
                let (start, leave) = pipe;
                let readys: Vec<Bit> = sends.iter().map(|(ch, _)| cp[ch].ready).collect();
                let all = g.g.reduce_and(&readys);
                let fire = g.g.and(start, all);
                for (k, (ch, v)) in sends.iter().enumerate() {
                    let others: Vec<Bit> = readys.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, r)| *r).collect();
                    let o = g.g.reduce_and(&others);
                    let act = g.g.and(start, o);
                    let val = g.env.get(v).cloned().ok_or_else(|| format!("the value sent on `{}` is not known", base(ch)))?;
                    let val = fit_hv(&mut g.g, &val, &chans[ch].ty)?;
                    offers.entry(ch.clone()).or_default().push((act, val.flat(), Bit::C0));
                }
                for (k, (_, ch)) in head.iter().chain(recvs).enumerate() {
                    let c = &cp[ch];
                    let nc = g.g.not(closed[ch]);
                    // the head takes the end of the stream as the loop leaves
                    let take = if k == 0 && head.is_some() { g.g.or(fire, leave) } else { fire };
                    let rdy = g.g.and(take, nc);
                    readies.entry(ch.clone()).or_default().push(rdy);
                    let x = g.g.and(rdy, c.valid);
                    let tc = g.g.and(x, c.close);
                    closed_set.entry(ch.clone()).or_default().push(tc);
                }
                (fire, konst(s))
            }
            Term::CloseOwned { ch, next } => {
                let cl = closed[ch];
                let nc = g.g.not(cl);
                let act = g.g.and(is_s, nc);
                offers.entry(ch.clone()).or_default().push((act, vec![], Bit::C1));
                let fired = g.g.and(act, cp[ch].ready);
                closed_set.entry(ch.clone()).or_default().push(fired);
                (g.g.or(cp[ch].ready, cl), konst(*next))
            }
        };
        let go = g.g.and(is_s, fire);
        state_d.push((go, next));
        for n in &names {
            let Some(v) = g.env.get(n).cloned() else { continue };
            let v = fit_hv(&mut g.g, &v, &vars[n])?;
            let vb = v.flat();
            if vb != q[n].flat() { var_d.entry(n.clone()).or_default().push((go, vb)); }
        }
    }

    // flip-flops (a register never written keeps its power-up 0)
    let ff = |g: &mut Builder, d: Bit, qb: Bit| g.ff("$_DFF_P_".into(), vec![("C", clk), ("D", d), ("Q", qb)]);
    let ffr = |g: &mut Builder, d: Bit, qb: Bit| g.ff("$_SDFF_PP0_".into(), vec![("C", clk), ("D", d), ("R", rst), ("Q", qb)]);
    let mut nets = vec![];
    for n in &names {
        let qb = q[n].flat();
        let mut d = qb.clone();
        for (c, v) in var_d.get(n).into_iter().flatten() { d = g.g.muxv(*c, &d, v); }
        for (i, b) in qb.iter().enumerate() { ff(&mut g.g, if d[i] == *b { Bit::C0 } else { d[i] }, *b); }
        nets.push((n.clone(), qb));
    }
    let mut mnames: Vec<&String> = store.keys().collect();
    mnames.sort();
    for m in mnames {
        let words = &store[m];
        let ws = mem_writes.get(m).cloned().unwrap_or_default();
        let (aw, dw) = (bits_for(words.len()), words[0].len());
        let (mut we, mut wa, mut wd) = (Bit::C0, vec![Bit::C0; aw], vec![Bit::C0; dw]);
        for (c, a, d) in &ws {
            we = g.g.or(we, *c);
            wa = g.g.muxv(*c, &wa, a);
            wd = g.g.muxv(*c, &wd, d);
        }
        for (k, w) in words.iter().enumerate() {
            let at = g.g.decode(&wa, k);
            let hit = g.g.and(we, at);
            let d = g.g.muxv(hit, w, &wd);
            for (i, b) in w.iter().enumerate() { ff(&mut g.g, d[i], *b); }
        }
        nets.push((m.clone(), words.concat()));
        nets.push((format!("$mem:{}:clk", m), vec![clk]));
        nets.push((format!("$mem:{}:we", m), vec![we]));
        nets.push((format!("$mem:{}:wa", m), wa));
        nets.push((format!("$mem:{}:wd", m), wd));
        for (i, (ra, rd)) in mem_reads.get(m).into_iter().flatten().enumerate() {
            nets.push((format!("$mem:{}:ra{}", m, i), ra.clone()));
            nets.push((format!("$mem:{}:rd{}", m, i), rd.clone()));
        }
    }
    let mut sd = state.clone();
    for (c, v) in &state_d { sd = g.g.muxv(*c, &sd, v); }
    for (i, b) in state.iter().enumerate() { ffr(&mut g.g, sd[i], *b); }
    nets.push(("state".into(), state.clone()));
    for (u, qb) in &closed {
        let sets = closed_set.get(u).cloned().unwrap_or_default();
        let any = g.g.reduce_or(&sets);
        let d = g.g.or(*qb, any);
        ffr(&mut g.g, d, *qb);
    }

    // outputs
    for (n, u, h) in &hw {
        match h {
            ParamHw::InChan(_) => {
                let r = readies.get(u).cloned().unwrap_or_default();
                let rb = g.g.reduce_or(&r);
                ports.push(Port { name: format!("{}_ready", n), dir: Dir::Out, bits: vec![rb] });
            }
            ParamHw::OutChan(t) => {
                let os = offers.get(u).cloned().unwrap_or_default();
                let mut data = vec![Bit::C0; t.width()];
                let mut close = Bit::C0;
                let mut valid = Bit::C0;
                for (act, d, c) in &os {
                    if !d.is_empty() { data = g.g.muxv(*act, &data, d); }
                    close = g.g.mux(*act, close, *c);
                    valid = g.g.or(valid, *act);
                }
                ports.push(Port { name: format!("{}_data", n), dir: Dir::Out, bits: data });
                ports.push(Port { name: format!("{}_close", n), dir: Dir::Out, bits: vec![close] });
                ports.push(Port { name: format!("{}_valid", n), dir: Dir::Out, bits: vec![valid] });
            }
            _ => {}
        }
    }
    let netlist = netlist::optimize(g.g, &HashMap::new(), ports, nets, &decl.name)?;
    let mut iface = crate::hw::Iface::default();
    for (n, _, h) in &hw {
        match h {
            ParamHw::InChan(t) => iface.ins.push((n.clone(), t.clone())),
            ParamHw::OutChan(t) => iface.outs.push((n.clone(), t.clone())),
            ParamHw::Scalar(t, None) => iface.scalars.push((n.clone(), t.clone())),
            ParamHw::Scalar(_, Some(_)) => {}
        }
    }
    Ok(crate::Synth { netlist, iface, warnings, insts: vec![(String::new(), decl.name.clone())] })
}

/// a memory address: the bits that pick one of `n` words (a block RAM has no more; an
/// address past the end is an error in the simulator)
fn mem_addr(a: &SVal, n: usize) -> Vec<Bit> { (0..bits_for(n)).map(|i| a.bits.get(i).copied().unwrap_or(Bit::C0)).collect() }

/// the lowering's temporaries get their types: a channel's element type, a loop counter the
/// range from its lower bound to its upper bound (it reaches the bound when the loop ends)
fn temps(g: &mut Gen, cfg: &Cfg, chans: &HashMap<String, Chan>, mems: &HashMap<String, (Ty, usize)>, hw: &[(String, String, ParamHw)]) -> R<()> {
    for (n, t) in &cfg.temps {
        if g.vars.contains_key(n) && !g.untyped.contains(n) { continue; }
        match t {
            Temp::ChanElem(ch) => { let ty = chans.get(ch).map(|c| c.ty.clone()).ok_or_else(|| format!("`{}` is not a channel parameter of the task", base(ch)))?; g.vars.insert(n.clone(), ty); }
            Temp::MemElem(m) => { g.vars.insert(n.clone(), mems[m].0.clone()); }
            Temp::Like(e) => {
                let fresh = scratch_env(g, hw);
                let saved = std::mem::replace(&mut g.env, fresh);
                let r = match &e.kind {
                    ExprKind::Range(lo, hi, incl) => g.scalar(lo).and_then(|l| g.scalar(hi).map(|h| (l, h, *incl))).map(|(l, h, incl)| Ty::Int { lo: l.lo.min(h.lo), hi: (h.hi + incl as i128).max(l.hi) }),
                    _ => g.eval(e).map(|v| v.ty()),
                };
                g.env = saved;
                match r {
                    Ok(t) => { g.vars.insert(n.clone(), t); g.untyped.remove(n); }
                    // the bounds read a variable not typed yet: the next round
                    Err(_) => { g.untyped.insert(n.clone()); }
                }
            }
        }
    }
    Ok(())
}

/// values standing for the registers in the type pass (fresh wires of each variable's width)
fn scratch_env(g: &mut Gen, hw: &[(String, String, ParamHw)]) -> HashMap<String, HV> {
    let mut env = HashMap::new();
    let vars: Vec<(String, Ty)> = g.vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, t) in vars { let w = g.g.wires(t.width() as u32); env.insert(n, HV::of_ty(&w, &t)); }
    for (_, u, h) in hw {
        if let ParamHw::Scalar(t, k) = h {
            let v = match k { Some(k) => const_hv(&mut g.g, k, t).unwrap(), None => HV::of_ty(&g.g.wires(t.width() as u32), t) };
            env.insert(u.clone(), v);
        }
    }
    env
}

/// the type pass looks at the terminators' expressions too (a branch condition, a value sent)
fn term_values(g: &mut Gen, t: &Term, chans: &HashMap<String, Chan>, mems: &HashMap<String, (Ty, usize)>) -> R<()> {
    match t {
        Term::MemRead { dst, mem, addr, .. } => {
            g.eval(addr)?;
            let t = &mems[mem].0;
            let v = SVal::of_ty(g.g.wires(t.width() as u32), t);
            g.env.insert(dst.clone(), HV::S(v));
        }
        Term::MemWrite { addr, val, .. } => { g.eval(addr)?; g.eval(val)?; }
        Term::Branch(c, ..) => { g.eval(c)?; }
        Term::Send { val, ch, .. } => { g.eval(val)?; if !chans.get(ch).is_some_and(|c| c.out) { return Err(format!("send on `{}`, which is not an output channel of the task", base(ch))); } }
        Term::Pipe { sends, .. } => for (ch, _) in sends { if !chans.get(ch).is_some_and(|c| c.out) { return Err(format!("send on `{}`, which is not an output channel of the task", base(ch))); } },
        Term::Recv { dst, ch, .. } => {
            let c = chans.get(ch).ok_or_else(|| format!("recv on `{}`, which is not a channel parameter", base(ch)))?;
            if c.out { return Err(format!("recv on `{}`, an output channel", base(ch))); }
            let v = HV::of_ty(&g.g.wires(c.ty.width() as u32), &c.ty);
            g.env.insert(dst.clone(), v);
        }
        _ => {}
    }
    Ok(())
}
