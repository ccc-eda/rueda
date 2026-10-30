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
use fluxa::value::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// what a task parameter is in hardware
#[derive(Clone)]
pub enum ParamHw {
    InChan(Ty),
    OutChan(Ty),
    Scalar(Ty, Option<i128>),
}

pub struct Gen<'p> {
    pub g: Builder,
    prog: &'p Arc<fluxa::ast::Program>,
    it: Interp,
    /// the variables' types (the width of their registers)
    pub vars: HashMap<String, Ty>,
    /// untyped variables: their type grows with what is assigned (until the type pass settles)
    untyped: std::collections::HashSet<String>,
    pub env: HashMap<String, SVal>,
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

    /// a type expression's hardware type (constants and generics are known at compile time)
    pub fn ty(&mut self, t: &TypeExpr, line: u32) -> R<Ty> {
        let rt = block_on(self.it.eval_type(t)).map_err(|e| format!("{}: type: {:?}", self.prog.loc(line), e))?;
        Ty::of_rt(&rt).map_err(|m| format!("{}: {}", self.prog.loc(line), m))
    }

    /// a compile-time constant (`comptime`, constants); None when it reads anything else
    fn konst(&mut self, e: &Expr) -> Option<Value> {
        let mut uses_local = false;
        crate::walk_paths(e, &mut |p| if p.contains('.') || p.contains('~') { uses_local = true });
        if uses_local { return None; }
        block_on(self.it.eval(e)).ok()
    }

    fn value(&self, v: &Value, line: u32) -> R<SVal> {
        match v {
            Value::Int(n) => Ok(SVal::konst(*n)),
            Value::Bool(b) => Ok(SVal::bool_const(*b)),
            other => err(line, self.prog, format!("the constant {} is not synthesized yet", fluxa::value::kind(other))),
        }
    }

    // ------------------------------------------------------------ variables

    /// declare `name` of type `t` (None: the value's own type) with value `v`
    fn declare(&mut self, name: &str, t: Option<Ty>, v: SVal) {
        let t = match t {
            Some(t) => { self.untyped.remove(name); *self.vars.entry(name.into()).or_insert(t) }
            None => {
                if !self.vars.contains_key(name) { self.untyped.insert(name.into()); }
                let cur = self.vars.get(name).copied();
                let t = widen(cur, v.ty());
                if cur != Some(t) { self.grew = true; self.vars.insert(name.into(), t); }
                t
            }
        };
        self.env.insert(name.into(), wrap_to(&v, t));
    }

    fn assign(&mut self, name: &str, v: SVal, line: u32) -> R<()> {
        if !self.vars.contains_key(name) && self.untyped.contains(name) { self.declare(name, None, v); return Ok(()); }
        let t = match self.vars.get(name) { Some(t) => *t, None => return err(line, self.prog, format!("assignment to `{}`, which is not a variable here", base(name))) };
        if self.untyped.contains(name) {
            let w = widen(Some(t), v.ty());
            if w != t { self.grew = true; self.vars.insert(name.into(), w); }
            let t = self.vars[name];
            self.env.insert(name.into(), wrap_to(&v, t));
        } else {
            self.env.insert(name.into(), wrap_to(&v, t));
        }
        Ok(())
    }

    // ------------------------------------------------------------ statements

    pub fn stmts(&mut self, ss: &[Stmt]) -> R<()> {
        for s in ss { self.stmt(s)?; }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        let line = s.line;
        match &s.kind {
            StmtKind::Let { pat, ty, init, .. } => {
                let t = match ty { Some(t) => Some(self.ty(t, line)?), None => None };
                let v = match init {
                    Some(e) => self.eval(e)?,
                    None => match t { Some(t) => SVal::of_ty(vec![Bit::C0; t.width()], t), None => return err(line, self.prog, "`let` needs an initializer or a type") },
                };
                match pat {
                    Pattern::Ident(n) => self.declare(n, t, v),
                    Pattern::Wild => {}
                    Pattern::Tuple(_) => return err(line, self.prog, "tuple patterns are not synthesized yet"),
                }
                Ok(())
            }
            StmtKind::Carry { name, ty, init } => {
                let t = self.ty(ty, line)?;
                let v = self.eval(init)?;
                self.declare(name, Some(t), v);
                Ok(())
            }
            StmtKind::Assign { target, op, value } => {
                let name = match &target.kind {
                    ExprKind::Path(p) if p.len() == 1 => p[0].clone(),
                    _ => return err(line, self.prog, "only whole variables can be assigned in synthesized code so far"),
                };
                let rhs = self.eval(value)?;
                let v = match op {
                    None => rhs,
                    Some(op) => {
                        let cur = self.eval(target)?;
                        self.binop(*op, &cur, &rhs, Some(&name), line)?
                    }
                };
                self.assign(&name, v, line)
            }
            StmtKind::Expr(e) => { self.eval(e)?; Ok(()) }
            StmtKind::Assert { .. } => Ok(()),
            StmtKind::For { pat, iter, body, .. } => {
                // a loop with constant bounds and no channel operation: unrolled
                let (lo, hi, incl) = match &iter.kind { ExprKind::Range(a, b, i) => (a, b, *i), _ => return err(line, self.prog, "this loop is not synthesized yet") };
                let lv = self.eval(lo)?.as_const();
                let hv = self.eval(hi)?.as_const();
                let (Some(l), Some(h)) = (lv, hv) else { return err(line, self.prog, "a loop without channel operations needs constant bounds to be unrolled") };
                let h = if incl { h + 1 } else { h };
                if h - l > 4096 { return err(line, self.prog, format!("{} iterations are too many to unroll", h - l)); }
                for k in l..h {
                    if let Pattern::Ident(n) = pat { self.declare(n, Some(Ty::Int { lo: l, hi: (h - 1).max(l) }), SVal::konst(k)); }
                    self.stmts(&body.stmts)?;
                    if let Some(t) = &body.tail { self.eval(t)?; }
                }
                Ok(())
            }
            StmtKind::Return(_) => err(line, self.prog, "`return` here is not synthesized yet"),
            StmtKind::Spawn(_) | StmtKind::Parallel(_) => err(line, self.prog, "spawn is not synthesized here"),
        }
    }

    // ------------------------------------------------------------ expressions

    pub fn eval(&mut self, e: &Expr) -> R<SVal> {
        let line = e.line;
        match &e.kind {
            ExprKind::Int(n) => Ok(SVal::konst(*n)),
            ExprKind::Bool(b) => Ok(SVal::bool_const(*b)),
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
                let v = self.eval(x)?;
                Ok(match c {
                    '-' => neg(&mut self.g, &v),
                    '!' => { let t = truth(&mut self.g, &v); SVal::bit(self.g.not(t)) }
                    _ => not_bits(&mut self.g, &v),
                })
            }
            ExprKind::Binary(op, l, r) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    let a = self.eval(l)?;
                    let b = self.eval(r)?;
                    let (x, y) = (truth(&mut self.g, &a), truth(&mut self.g, &b));
                    return Ok(SVal::bit(if *op == BinOp::And { self.g.and(x, y) } else { self.g.or(x, y) }));
                }
                let a = self.eval(l)?;
                let b = self.eval(r)?;
                let left = match &l.kind { ExprKind::Path(p) if p.len() == 1 => Some(p[0].clone()), _ => None };
                self.binop(*op, &a, &b, left.as_deref(), line)
            }
            ExprKind::Cast(x, t) => {
                let v = self.eval(x)?;
                let t = self.ty(t, line)?;
                Ok(wrap_to(&v, t))
            }
            ExprKind::Call(c, a) => self.call(c, a, line),
            ExprKind::Method(r, n, t, a) => self.method(r, n, t, a, line),
            ExprKind::Index(x, i) => {
                let v = self.eval(x)?;
                let k = self.eval(i)?;
                match k.as_const() {
                    Some(k) if k >= 0 => Ok(SVal::of_ty(vec![v.ext(k as usize + 1)[k as usize]], Ty::Int { lo: 0, hi: 1 })),
                    _ => {
                        let s = shift(&mut self.g, &v, &k, false)?;
                        Ok(SVal::of_ty(vec![s.bits[0]], Ty::Int { lo: 0, hi: 1 }))
                    }
                }
            }
            ExprKind::Slice(x, h, l) => {
                let v = self.eval(x)?;
                let (Some(h), Some(l)) = (self.eval(h)?.as_const(), self.eval(l)?.as_const()) else { return err(line, self.prog, "a bit slice needs constant bounds") };
                if l < 0 || h < l { return err(line, self.prog, format!("invalid bit slice [{}:{}]", h, l)); }
                let w = (h - l + 1) as usize;
                let bits = v.ext(h as usize + 1)[l as usize..=h as usize].to_vec();
                Ok(SVal::of_ty(bits, Ty::Int { lo: 0, hi: (1i128 << w) - 1 }))
            }
            ExprKind::Block(b) => {
                self.stmts(&b.stmts)?;
                match &b.tail { Some(t) => self.eval(t), None => Ok(SVal::konst(0)) }
            }
            ExprKind::Comptime(_) => match self.konst(e) {
                Some(v) => self.value(&v, line),
                None => err(line, self.prog, "comptime block that reads run-time values"),
            },
            ExprKind::If(c, t, el) => {
                let cv = self.eval(c)?;
                let cb = truth(&mut self.g, &cv);
                let before = self.env.clone();
                let tv = { self.stmts(&t.stmts)?; match &t.tail { Some(x) => self.eval(x)?, None => SVal::konst(0) } };
                let after_t = std::mem::replace(&mut self.env, before);
                let fv = match el { Some(x) => self.eval(x)?, None => SVal::konst(0) };
                let after_f = std::mem::take(&mut self.env);
                self.env = self.merge(cb, after_t, after_f);
                Ok(select(&mut self.g, cb, &tv, &fv))
            }
            ExprKind::Match(s, arms) => self.matching(s, arms, line),
            ExprKind::Str(_) => Ok(SVal::konst(0)),
            _ => err(line, self.prog, "this expression is not synthesized yet"),
        }
    }

    /// the variables after `c ? then : else`
    fn merge(&mut self, c: Bit, t: HashMap<String, SVal>, f: HashMap<String, SVal>) -> HashMap<String, SVal> {
        let mut out = HashMap::new();
        for (k, tv) in &t {
            match f.get(k) {
                Some(fv) if fv.bits == tv.bits => { out.insert(k.clone(), tv.clone()); }
                Some(fv) => {
                    let ty = self.vars.get(k).copied().unwrap_or(tv.ty());
                    let v = select(&mut self.g, c, tv, fv);
                    out.insert(k.clone(), wrap_to(&v, ty));
                }
                None => { out.insert(k.clone(), tv.clone()); }
            }
        }
        for (k, fv) in f { out.entry(k).or_insert(fv); }
        out
    }

    fn matching(&mut self, s: &Expr, arms: &[Arm], line: u32) -> R<SVal> {
        let sv = self.eval(s)?;
        // arms from the last to the first: each one a condition in front of the rest
        let mut conds = vec![];
        let mut vals = vec![];
        let mut envs = vec![];
        let start = self.env.clone();
        for a in arms {
            self.env = start.clone();
            let c = match &a.pat {
                MatchPat::Wild => Bit::C1,
                MatchPat::Bind(n) => { self.declare(n, None, sv.clone()); Bit::C1 }
                MatchPat::Lit(e) => { let v = self.eval(e)?; compare(&mut self.g, &sv, &v, "==").bits[0] }
                MatchPat::Range(lo, hi, incl) => {
                    let (l, h) = (self.eval(lo)?, self.eval(hi)?);
                    let a1 = compare(&mut self.g, &sv, &l, ">=").bits[0];
                    let a2 = compare(&mut self.g, &sv, &h, if *incl { "<=" } else { "<" }).bits[0];
                    self.g.and(a1, a2)
                }
                MatchPat::Tuple(_) => return err(line, self.prog, "tuple patterns are not synthesized yet"),
            };
            let c = match &a.guard {
                Some(g) => { let gv = self.eval(g)?; let gb = truth(&mut self.g, &gv); self.g.and(c, gb) }
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
            v = select(&mut self.g, c, &tv, &v);
            env = self.merge(c, te, env);
        }
        self.env = env;
        Ok(v)
    }

    fn binop(&mut self, op: BinOp, a: &SVal, b: &SVal, left: Option<&str>, line: u32) -> R<SVal> {
        let g = &mut self.g;
        Ok(match op {
            BinOp::Add => add(g, a, b),
            BinOp::Sub => sub(g, a, b),
            BinOp::Mul => mul(g, a, b),
            BinOp::Div => divrem(g, a, b).map_err(|m| format!("{}: {}", self.prog.loc(line), m))?.0,
            BinOp::Rem => divrem(g, a, b).map_err(|m| format!("{}: {}", self.prog.loc(line), m))?.1,
            BinOp::AddW | BinOp::SubW | BinOp::MulW => {
                let t = left.and_then(|n| self.vars.get(n).copied()).ok_or_else(|| format!("{}: wrapping operators need a typed variable on the left", self.prog.loc(line)))?;
                let g = &mut self.g;
                let v = match op { BinOp::AddW => add(g, a, b), BinOp::SubW => sub(g, a, b), _ => mul(g, a, b) };
                wrap_to(&v, t)
            }
            BinOp::Shl | BinOp::Shr => match b.as_const() {
                Some(k) if (0..=120).contains(&k) => if op == BinOp::Shl { shl_const(a, k as u32) } else { shr_const(a, k as u32) },
                _ => shift(g, a, b, op == BinOp::Shl).map_err(|m| format!("{}: {}", self.prog.loc(line), m))?,
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

    /// `T` of `x.to::<T>()` and friends
    fn targ_ty(&mut self, t: &[TArg], line: u32) -> R<Ty> {
        match t.first() { Some(TArg::Ty(t)) => self.ty(t, line), _ => err(line, self.prog, "expected a type argument, e.g. `.sat::<u8>()`") }
    }

    fn method(&mut self, r: &Expr, n: &str, t: &[TArg], a: &[Arg], line: u32) -> R<SVal> {
        let v = self.eval(r)?;
        match n {
            "wrap" => { let t = self.targ_ty(t, line)?; Ok(wrap_to(&v, t)) }
            "sat" => { let t = self.targ_ty(t, line)?; Ok(sat_to(&mut self.g, &v, t)) }
            "to" | "quantize" => {
                let t = self.targ_ty(t, line)?;
                let ovf = a.iter().find(|x| x.name.as_deref() == Some("ovf")).and_then(|x| match &x.value.kind { ExprKind::Path(p) => p.last().cloned(), _ => None });
                Ok(if ovf.as_deref() == Some("sat") { sat_to(&mut self.g, &v, t) } else { wrap_to(&v, t) })
            }
            "abs" => {
                let ng = neg(&mut self.g, &v);
                let s = compare(&mut self.g, &v, &SVal::konst(0), "<").bits[0];
                let r = select(&mut self.g, s, &ng, &v);
                Ok(SVal { lo: 0.max(v.lo.min(0).abs().min(v.lo.max(0))), hi: v.lo.abs().max(v.hi.abs()), ..r }.normalized())
            }
            "xor_reduce" => Ok(SVal::of_ty(vec![self.g.reduce_xor(&v.bits)], Ty::Int { lo: 0, hi: 1 })),
            "popcount" => {
                let mut acc = SVal::konst(0);
                for b in v.bits.clone() { let x = SVal::of_ty(vec![b], Ty::Int { lo: 0, hi: 1 }); acc = add(&mut self.g, &acc, &x); }
                Ok(acc)
            }
            other => err(line, self.prog, format!("method `{}` is not synthesized", other)),
        }
    }

    fn call(&mut self, c: &Expr, a: &[Arg], line: u32) -> R<SVal> {
        let name = match &c.kind { ExprKind::Path(p) | ExprKind::PathT(p, _) => p.last().unwrap().clone(), _ => return err(line, self.prog, "callee must be a function name") };
        match name.as_str() {
            "print" => return Ok(SVal::konst(0)),
            "min" | "max" if a.len() == 2 => {
                let (x, y) = (self.eval(&a[0].value)?, self.eval(&a[1].value)?);
                let lt = compare(&mut self.g, &x, &y, "<").bits[0];
                return Ok(if name == "min" { select(&mut self.g, lt, &x, &y) } else { select(&mut self.g, lt, &y, &x) });
            }
            "abs" if a.len() == 1 => {
                let r = Expr { kind: ExprKind::Method(Box::new(a[0].value.clone()), "abs".into(), vec![], vec![]), line };
                return self.eval(&r);
            }
            _ => {}
        }
        let decl = match self.prog.fns.get(&name).cloned() {
            Some(d) => d,
            None => return err(line, self.prog, format!("`{}` is not synthesized", name)),
        };
        if decl.kind == FnKind::Task { return err(line, self.prog, format!("`{}` is a task: spawn it", name)); }
        if self.depth > 32 { return err(line, self.prog, "recursion is not synthesized"); }
        // the arguments, then the body inlined with fresh names
        let mut vals = vec![];
        for x in a { vals.push((x.name.clone(), self.eval(&x.value)?)); }
        self.rename += 1;
        let (params, body) = crate::lower::rename_fn(&decl, self.rename);
        let mut positional = vals.iter().filter(|(n, _)| n.is_none()).map(|(_, v)| v.clone());
        for (p, u) in decl.params.iter().zip(&params) {
            let v = match vals.iter().find(|(n, _)| n.as_deref() == Some(p.name.as_str())) { Some((_, v)) => v.clone(), None => match positional.next() { Some(v) => v, None => return err(line, self.prog, format!("missing argument `{}`", p.name)) } };
            let t = self.ty(&p.ty, line)?;
            self.declare(u, Some(t), v);
        }
        self.depth += 1;
        let mut stmts = body.stmts.clone();
        let mut tail = body.tail.clone();
        // `return e` as the last statement is the tail
        if tail.is_none() {
            if let Some(Stmt { kind: StmtKind::Return(Some(e)), .. }) = stmts.last() { tail = Some(Box::new(e.clone())); stmts.pop(); }
        }
        let r = self.stmts(&stmts).and_then(|_| match &tail { Some(t) => self.eval(t), None => Ok(SVal::konst(0)) });
        self.depth -= 1;
        let v = r?;
        match &decl.ret { Some(rt) => { let t = self.ty(rt, line)?; Ok(wrap_to(&v, t)) } None => Ok(v) }
    }
}

fn widen(cur: Option<Ty>, t: Ty) -> Ty {
    match (cur, t) {
        (None, t) => t,
        (Some(Ty::Bool), Ty::Bool) => Ty::Bool,
        (Some(a), b) => { let ((l1, h1), (l2, h2)) = (a.range(), b.range()); Ty::Int { lo: l1.min(l2), hi: h1.max(h2) } }
    }
}

/// the source name of a unique name (`x.3` → `x`)
pub fn base(n: &str) -> &str { n.split(['.', '~']).next().unwrap_or(n) }

// ---------------------------------------------------------------- the circuit

struct Chan { ty: Ty, out: bool }

/// the task `decl` lowered to `cfg` as a netlist; `params` fixes scalar parameters to constants
pub fn build(prog: &Arc<fluxa::ast::Program>, decl: &FnDecl, cfg: &Cfg, params: &HashMap<String, i128>) -> R<crate::Synth> {
    let mut warnings = vec![];
    // the parameters
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
            t => ParamHw::Scalar(gen.ty(t, decl.line)?, params.get(&p.name).copied()),
        };
        hw.push((p.name.clone(), u.clone(), h));
    }
    let chans: HashMap<String, Chan> = hw.iter().filter_map(|(_, u, h)| match h {
        ParamHw::InChan(t) => Some((u.clone(), Chan { ty: *t, out: false })),
        ParamHw::OutChan(t) => Some((u.clone(), Chan { ty: *t, out: true })),
        _ => None,
    }).collect();

    // the type pass: the widths of the variables, untyped ones widened until nothing grows
    let mut vars: HashMap<String, Ty> = HashMap::new();
    let mut untyped = Default::default();
    for round in 0.. {
        if round == 12 {
            let grow: Vec<&str> = gen.untyped.iter().map(|n| base(n)).collect();
            return Err(format!("{}: variable(s) {} keep growing with every assignment: give them a type (or use `carry`)", decl.name, grow.join(", ")));
        }
        let mut t = Gen::new(prog);
        t.vars = vars.clone();
        t.untyped = untyped;
        temps(&mut t, cfg, &chans, &hw)?;
        for b in &cfg.blocks {
            t.env = scratch_env(&mut t, &hw);
            t.stmts(&b.stmts)?;
            term_values(&mut t, &b.term, &chans)?;
        }
        vars = t.vars.clone();
        untyped = t.untyped.clone();
        if !t.grew { break; }
        gen.untyped = t.untyped.clone();
    }

    // the circuit
    let mut g = Gen::new(prog);
    g.vars = vars.clone();
    g.untyped = untyped;
    let clk = g.g.wire();
    let rst = g.g.wire();
    let mut ports = vec![Port { name: "clk".into(), dir: Dir::In, bits: vec![clk] }, Port { name: "rst".into(), dir: Dir::In, bits: vec![rst] }];
    // inputs
    struct ChanPorts { data: Vec<Bit>, close: Bit, valid: Bit, ready: Bit }
    let mut cp: HashMap<String, ChanPorts> = HashMap::new();
    let mut scalars: HashMap<String, SVal> = HashMap::new();
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
            ParamHw::Scalar(t, Some(k)) => { scalars.insert(u.clone(), wrap_to(&SVal::konst(*k), *t)); }
            ParamHw::Scalar(t, None) => {
                let bits = g.g.wires(t.width() as u32);
                ports.push(Port { name: n.clone(), dir: Dir::In, bits: bits.clone() });
                scalars.insert(u.clone(), SVal::of_ty(bits, *t));
            }
        }
    }
    // registers: the variables, the state, which channels are closed
    let mut names: Vec<String> = vars.keys().cloned().collect();
    names.sort();
    let q: HashMap<String, SVal> = names.iter().map(|n| { let t = vars[n]; (n.clone(), SVal::of_ty(g.g.wires(t.width() as u32), t)) }).collect();
    let nst = cfg.blocks.len();
    let sw = bits_for(nst).max(1);
    let state = g.g.wires(sw as u32);
    let mut closed: HashMap<String, Bit> = HashMap::new();
    for u in chans.keys() { closed.insert(u.clone(), g.g.wire()); }

    let mut var_d: HashMap<String, Vec<(Bit, Vec<Bit>)>> = HashMap::new();
    let mut state_d: Vec<(Bit, Vec<Bit>)> = vec![];
    let mut closed_set: HashMap<String, Vec<Bit>> = HashMap::new();
    // what each output channel offers: (state active, data, close flag)
    let mut offers: HashMap<String, Vec<(Bit, Vec<Bit>, Bit)>> = HashMap::new();
    let mut readies: HashMap<String, Vec<Bit>> = HashMap::new();
    let konst = |g: &mut Builder, k: usize| -> Vec<Bit> { let _ = g; (0..sw).map(|i| Bit::of((k >> i) & 1 == 1)).collect() };

    temps(&mut g, cfg, &chans, &hw)?;
    for (s, b) in cfg.blocks.iter().enumerate() {
        let is_s = g.g.decode(&state, s);
        g.env = q.clone();
        for (u, v) in &scalars { g.env.insert(u.clone(), v.clone()); }
        g.stmts(&b.stmts)?;
        let (fire, next): (Bit, Vec<Bit>) = match &b.term {
            Term::Goto(t) => (Bit::C1, konst(&mut g.g, *t)),
            Term::Halt => (Bit::C0, konst(&mut g.g, s)),
            Term::Branch(c, t, f) => {
                let cv = g.eval(c)?;
                let cb = truth(&mut g.g, &cv);
                let (kt, kf) = (konst(&mut g.g, *t), konst(&mut g.g, *f));
                (Bit::C1, g.g.muxv(cb, &kf, &kt))
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
                let t = vars[dst];
                g.env.insert(dst.clone(), wrap_to(&SVal::of_ty(c.data.clone(), chans[ch].ty), t));
                let kn = konst(&mut g.g, *next);
                let nx = match on_close { Some(x) => { let kx = konst(&mut g.g, *x); g.g.muxv(is_close, &kn, &kx) } None => kn };
                (valid, nx)
            }
            Term::Send { ch, val, next } => {
                let v = g.eval(val)?;
                let v = wrap_to(&v, chans[ch].ty);
                offers.entry(ch.clone()).or_default().push((is_s, v.bits.clone(), Bit::C0));
                (cp[ch].ready, konst(&mut g.g, *next))
            }
            Term::Close { ch, next } => {
                offers.entry(ch.clone()).or_default().push((is_s, vec![], Bit::C1));
                let fired = g.g.and(is_s, cp[ch].ready);
                closed_set.entry(ch.clone()).or_default().push(fired);
                (cp[ch].ready, konst(&mut g.g, *next))
            }
            Term::CloseOwned { ch, next } => {
                let cl = closed[ch];
                let nc = g.g.not(cl);
                let act = g.g.and(is_s, nc);
                offers.entry(ch.clone()).or_default().push((act, vec![], Bit::C1));
                let fired = g.g.and(act, cp[ch].ready);
                closed_set.entry(ch.clone()).or_default().push(fired);
                (g.g.or(cp[ch].ready, cl), konst(&mut g.g, *next))
            }
        };
        let go = g.g.and(is_s, fire);
        state_d.push((go, next));
        for n in &names {
            let v = &g.env[n];
            if v.bits != q[n].bits { var_d.entry(n.clone()).or_default().push((go, v.bits.clone())); }
        }
    }

    // flip-flops
    let ff = |g: &mut Builder, d: Bit, qb: Bit| g.ff("$_DFF_P_".into(), vec![("C", clk), ("D", d), ("Q", qb)]);
    let ffr = |g: &mut Builder, d: Bit, qb: Bit| g.ff("$_SDFF_PP0_".into(), vec![("C", clk), ("D", d), ("R", rst), ("Q", qb)]);
    let mut nets = vec![];
    for n in &names {
        let qb = q[n].bits.clone();
        let mut d = qb.clone();
        for (c, v) in var_d.get(n).into_iter().flatten() { d = g.g.muxv(*c, &d, v); }
        for (i, b) in qb.iter().enumerate() { if d[i] != *b { ff(&mut g.g, d[i], *b); } }
        // a register never written keeps its power-up 0
        for (i, b) in qb.iter().enumerate() { if d[i] == *b { ff(&mut g.g, Bit::C0, *b); } }
        nets.push((n.clone(), qb));
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
                let w = t.width();
                let mut data = vec![Bit::C0; w];
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
            ParamHw::InChan(t) => iface.ins.push((n.clone(), *t)),
            ParamHw::OutChan(t) => iface.outs.push((n.clone(), *t)),
            ParamHw::Scalar(t, None) => iface.scalars.push((n.clone(), *t)),
            ParamHw::Scalar(_, Some(_)) => {}
        }
    }
    Ok(crate::Synth { netlist, iface, warnings })
}

/// the lowering's temporaries get their types: a channel's element type, a loop counter the
/// range from its lower bound to its upper bound (it reaches the bound when the loop ends)
fn temps(g: &mut Gen, cfg: &Cfg, chans: &HashMap<String, Chan>, hw: &[(String, String, ParamHw)]) -> R<()> {
    for (n, t) in &cfg.temps {
        if g.vars.contains_key(n) && !g.untyped.contains(n) { continue; }
        match t {
            Temp::ChanElem(ch) => { let ty = chans.get(ch).map(|c| c.ty).ok_or_else(|| format!("`{}` is not a channel parameter of the task", base(ch)))?; g.vars.insert(n.clone(), ty); }
            Temp::Like(e) => {
                let fresh = scratch_env(g, hw);
                let saved = std::mem::replace(&mut g.env, fresh);
                let r = match &e.kind {
                    ExprKind::Range(lo, hi, incl) => g.eval(lo).and_then(|l| g.eval(hi).map(|h| (l, h, *incl))).map(|(l, h, incl)| Ty::Int { lo: l.lo.min(h.lo), hi: (h.hi + incl as i128).max(l.hi) }),
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
fn scratch_env(g: &mut Gen, hw: &[(String, String, ParamHw)]) -> HashMap<String, SVal> {
    let mut env = HashMap::new();
    let vars: Vec<(String, Ty)> = g.vars.iter().map(|(k, v)| (k.clone(), *v)).collect();
    for (n, t) in vars { let w = g.g.wires(t.width() as u32); env.insert(n, SVal::of_ty(w, t)); }
    for (_, u, h) in hw {
        if let ParamHw::Scalar(t, k) = h {
            let v = match k { Some(k) => wrap_to(&SVal::konst(*k), *t), None => SVal::of_ty(g.g.wires(t.width() as u32), *t) };
            env.insert(u.clone(), v);
        }
    }
    env
}

/// the type pass looks at the terminators' expressions too (a branch condition, a value sent)
fn term_values(g: &mut Gen, t: &Term, chans: &HashMap<String, Chan>) -> R<()> {
    match t {
        Term::Branch(c, ..) => { g.eval(c)?; }
        Term::Send { val, ch, .. } => { g.eval(val)?; if !chans.get(ch).is_some_and(|c| c.out) { return Err(format!("send on `{}`, which is not an output channel of the task", base(ch))); } }
        Term::Recv { dst, ch, .. } => {
            let c = chans.get(ch).ok_or_else(|| format!("recv on `{}`, which is not a channel parameter", base(ch)))?;
            if c.out { return Err(format!("recv on `{}`, an output channel", base(ch))); }
            let v = SVal::of_ty(g.g.wires(c.ty.width() as u32), c.ty);
            g.env.insert(dst.clone(), v);
        }
        _ => {}
    }
    Ok(())
}
