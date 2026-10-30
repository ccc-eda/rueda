//! 樹走訪直譯器：每個 task 一個 `Interp`，各跑在自己的執行緒上。

use crate::ast::*;
use crate::runtime::*;
use crate::value::*;
use std::collections::HashMap;
use std::sync::Arc;

/// 可當作符號（而非變數）使用的裸識別字，例如 `nearest`、`hamming`
const SYMBOLS: &[&str] = &[
    "trunc", "nearest", "even", "sat", "wrap", "check", "hamming", "hann", "rect",
];

pub struct Var {
    pub val: Value,
    pub ty: Option<RtType>,
    /// `@reg`：迭代內讀到舊值，寫入在迭代結束提交
    pub reg: bool,
    pub pending: Option<Value>,
}

#[derive(Clone, Copy)]
enum Step {
    Idx(usize),
    Fld(usize),
}

pub struct Interp {
    pub sh: Arc<Shared>,
    pub task: usize,
    pub task_name: String,
    scopes: Vec<HashMap<String, Var>>,
    loop_depth: usize,
    pub line: u32,
}

fn index_value(b: &Value, i: &Value, line: u32) -> R<Value> {
    match (b, i) {
        (Value::Array(xs), Value::Int(n)) | (Value::Tuple(xs), Value::Int(n)) => {
            if *n < 0 || *n as usize >= xs.len() {
                msg(line, format!("index {} out of bounds (length {})", n, xs.len()))
            } else {
                Ok(xs[*n as usize].clone())
            }
        }
        (Value::Array(xs), Value::Range(lo, hi)) => {
            if *lo < 0 || *hi < *lo || *hi as usize > xs.len() {
                msg(line, format!("slice {}..{} out of bounds (length {})", lo, hi, xs.len()))
            } else {
                Ok(Value::Array(xs[*lo as usize..*hi as usize].to_vec()))
            }
        }
        (Value::Int(x), Value::Int(n)) => {
            if !(0..120).contains(n) {
                return msg(line, format!("bit index {} out of range", n));
            }
            Ok(Value::Int((x >> n) & 1))
        }
        (b, i) => msg(line, format!("cannot index {} with {}", kind(b), kind(i))),
    }
}

fn set_path(base: &mut Value, steps: &[Step], v: Value, line: u32) -> R<()> {
    match steps.split_first() {
        None => {
            *base = v;
            Ok(())
        }
        Some((Step::Idx(i), rest)) | Some((Step::Fld(i), rest)) => match base {
            Value::Array(xs) | Value::Tuple(xs) => {
                if *i < xs.len() {
                    set_path(&mut xs[*i], rest, v, line)
                } else {
                    msg(line, format!("index {} out of bounds (length {})", i, xs.len()))
                }
            }
            other => msg(line, format!("cannot index into {}", kind(other))),
        },
    }
}

fn sub_type(t: &RtType, st: &Step) -> RtType {
    match (t, st) {
        (RtType::Array(e, _), Step::Idx(_)) | (RtType::Mem(e, _), Step::Idx(_)) => (**e).clone(),
        (RtType::Tuple(ts), Step::Fld(i)) | (RtType::Tuple(ts), Step::Idx(i)) => {
            ts.get(*i).cloned().unwrap_or(RtType::Any)
        }
        _ => RtType::Any,
    }
}

fn parse_round(s: &str) -> Option<Round> {
    match s {
        "trunc" => Some(Round::Trunc),
        "nearest" => Some(Round::Nearest),
        "even" => Some(Round::Even),
        _ => None,
    }
}
fn parse_ovf(s: &str) -> Option<Ovf> {
    match s {
        "wrap" => Some(Ovf::Wrap),
        "sat" => Some(Ovf::Sat),
        "check" => Some(Ovf::Check),
        _ => None,
    }
}

fn tree_sum(mut items: Vec<Value>) -> Result<Value, String> {
    if items.is_empty() {
        return Err("sum of empty array".to_string());
    }
    while items.len() > 1 {
        let mut next = Vec::with_capacity(items.len() / 2 + 1);
        for ch in items.chunks(2) {
            if ch.len() == 2 {
                next.push(binop(BinOp::Add, &ch[0], &ch[1])?);
            } else {
                next.push(ch[0].clone());
            }
        }
        items = next;
    }
    Ok(items.remove(0))
}

impl Interp {
    pub fn new(sh: Arc<Shared>, task: usize, name: String) -> Interp {
        Interp { sh, task, task_name: name, scopes: vec![HashMap::new()], loop_depth: 0, line: 0 }
    }

    fn err<T>(&self, m: impl Into<String>) -> R<T> {
        msg(self.line, m)
    }
    fn me(&self, m: String) -> RtErr {
        RtErr::Msg(self.line, m)
    }

    // ------------------------------------------------------------ 環境
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
    fn declare(&mut self, name: &str, var: Var) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), var);
    }
    fn plain(&mut self, name: &str, v: Value) {
        self.declare(name, Var { val: v, ty: None, reg: false, pending: None });
    }
    fn lookup(&self, name: &str) -> Option<&Var> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }
    fn lookup_mut(&mut self, name: &str) -> Option<&mut Var> {
        self.scopes.iter_mut().rev().find_map(|s| s.get_mut(name))
    }
    fn commit_regs(&mut self) {
        for sc in self.scopes.iter_mut() {
            for v in sc.values_mut() {
                if let Some(p) = v.pending.take() {
                    v.val = p;
                }
            }
        }
    }

    // ------------------------------------------------------------ 型別
    fn eval_int(&mut self, e: &Expr) -> R<i128> {
        match self.eval(e)? {
            Value::Int(n) => Ok(n),
            other => self.err(format!("expected a compile-time integer, found {}", kind(&other))),
        }
    }
    fn eval_usize(&mut self, e: &Expr) -> R<usize> {
        let n = self.eval_int(e)?;
        if !(0..=100_000_000).contains(&n) {
            return self.err(format!("size {} out of range", n));
        }
        Ok(n as usize)
    }

    pub fn eval_type(&mut self, t: &TypeExpr) -> R<RtType> {
        Ok(match t {
            TypeExpr::Name(path, _) => {
                let n = path.last().unwrap().as_str();
                if n == "bit" {
                    RtType::Int { lo: 0, hi: 1 }
                } else if n == "bool" {
                    RtType::Bool
                } else if let Some(t) = parse_int_type(n) {
                    t
                } else {
                    RtType::Any
                }
            }
            TypeExpr::Fixed(i, f, opts) => {
                let i = self.eval_int(i)?;
                let f = self.eval_int(f)?;
                if i < 0 || f < 0 || i + f > MAX_WIDTH as i128 {
                    return self.err(format!("fixed<{},{}> exceeds the supported width of {} bits", i, f, MAX_WIDTH));
                }
                let mut round = Round::Trunc;
                let mut ovf = Ovf::Check;
                for (k, v) in opts {
                    match k.as_str() {
                        "round" => round = parse_round(v).ok_or_else(|| self.me(format!("unknown rounding mode `{}`", v)))?,
                        "ovf" => ovf = parse_ovf(v).ok_or_else(|| self.me(format!("unknown overflow mode `{}`", v)))?,
                        _ => return self.err(format!("unknown fixed option `{}`", k)),
                    }
                }
                RtType::Fixed { i: i as u32, f: f as u32, round, ovf }
            }
            TypeExpr::Range(lo, hi) => {
                let (lo, hi) = (self.eval_int(lo)?, self.eval_int(hi)?);
                if lo > hi {
                    return self.err(format!("empty range type int<{}..={}>", lo, hi));
                }
                RtType::Int { lo, hi }
            }
            TypeExpr::Bits(n) => {
                let n = self.eval_int(n)?;
                if n < 1 || n > MAX_WIDTH as i128 {
                    return self.err(format!("bits<{}> out of range", n));
                }
                unsigned_type(n as u32)
            }
            TypeExpr::Tuple(ts) => {
                let mut v = Vec::new();
                for t in ts {
                    v.push(self.eval_type(t)?);
                }
                RtType::Tuple(v)
            }
            TypeExpr::Array(t, n) => {
                let t = self.eval_type(t)?;
                RtType::Array(Box::new(t), self.eval_usize(n)?)
            }
            TypeExpr::Chan(t, d) => {
                let t = self.eval_type(t)?;
                let d = match d {
                    Some(d) => self.eval_usize(d)?,
                    None => 2,
                };
                RtType::Chan(Box::new(t), d)
            }
            TypeExpr::Mem(t, n) => {
                let t = self.eval_type(t)?;
                RtType::Mem(Box::new(t), self.eval_usize(n)?)
            }
        })
    }

    // ------------------------------------------------------------ 區塊與陳述式
    pub fn exec_block(&mut self, b: &Block) -> R<Value> {
        self.push_scope();
        let r = self.exec_body(b);
        self.pop_scope();
        r
    }

    fn exec_body(&mut self, b: &Block) -> R<Value> {
        for s in &b.stmts {
            self.exec_stmt(s)?;
            if self.loop_depth == 0 {
                self.commit_regs();
            }
        }
        match &b.tail {
            Some(e) => self.eval(e),
            None => Ok(Value::Unit),
        }
    }

    fn bind_pattern(&mut self, p: &Pattern, v: Value) -> R<()> {
        match p {
            Pattern::Wild => Ok(()),
            Pattern::Ident(n) => {
                self.plain(n, v);
                Ok(())
            }
            Pattern::Tuple(ps) => match v {
                Value::Tuple(xs) | Value::Array(xs) if xs.len() == ps.len() => {
                    for (p, x) in ps.iter().zip(xs) {
                        self.bind_pattern(p, x)?;
                    }
                    Ok(())
                }
                other => self.err(format!("cannot destructure {} into a {}-tuple", kind(&other), ps.len())),
            },
        }
    }

    fn exec_stmt(&mut self, s: &Stmt) -> R<()> {
        self.line = s.line;
        if self.sh.stopped() {
            return Err(RtErr::Shutdown);
        }
        match &s.kind {
            StmtKind::Let { pat, ty, placement, init } => {
                let rt = match ty {
                    Some(t) => Some(self.eval_type(t)?),
                    None => None,
                };
                let mut val = match init {
                    Some(e) => self.eval(e)?,
                    None => match &rt {
                        Some(t) => zero_value(t),
                        None => return self.err("`let` needs an initializer or a type"),
                    },
                };
                self.line = s.line;
                let label = match pat {
                    Pattern::Ident(n) => n.clone(),
                    _ => "pattern".to_string(),
                };
                if let Some(t) = &rt {
                    val = coerce(val, t).map_err(|m| self.me(format!("`let {}`: {}", label, m)))?;
                }
                // 通道命名（用於統計與死結報告）
                if let (Pattern::Tuple(ps), Value::Tuple(xs)) = (pat, &val) {
                    if ps.len() == 2 && xs.len() == 2 {
                        if let (Pattern::Ident(p), Pattern::Ident(q), Value::Tx(a), Value::Rx(b)) =
                            (&ps[0], &ps[1], &xs[0], &xs[1])
                        {
                            if a == b {
                                self.sh.rename_chan(*a, format!("{}:{}->{}", self.task_name, p, q));
                            }
                        }
                    }
                }
                match pat {
                    Pattern::Ident(n) => {
                        let reg = placement.as_deref() == Some("reg");
                        self.declare(n, Var { val, ty: rt, reg, pending: None });
                    }
                    other => self.bind_pattern(other, val)?,
                }
                Ok(())
            }
            StmtKind::Carry { name, ty, init } => {
                let t = self.eval_type(ty)?;
                let v = self.eval(init)?;
                self.line = s.line;
                let v = coerce(v, &t).map_err(|m| self.me(format!("`carry {}` initializer: {}", name, m)))?;
                self.declare(name, Var { val: v, ty: Some(t), reg: false, pending: None });
                Ok(())
            }
            StmtKind::Assign { target, op, value } => {
                let rhs = self.eval(value)?;
                self.line = s.line;
                let nv = match op {
                    None => rhs,
                    Some(op) => {
                        let cur = self.eval(target)?;
                        self.line = s.line;
                        binop(*op, &cur, &rhs).map_err(|m| self.me(m))?
                    }
                };
                self.store(target, nv)
            }
            StmtKind::For { attr, pat, iter, body } => {
                let is_unroll = matches!(attr, LoopAttr::Unroll(_));
                let itv = self.eval(iter)?;
                self.line = s.line;
                match itv {
                    Value::Range(lo, hi) => {
                        let mut i = lo;
                        while i < hi {
                            self.iter_body(pat, Value::Int(i), body, is_unroll)?;
                            i += 1;
                        }
                    }
                    Value::Array(xs) => {
                        for x in xs {
                            self.iter_body(pat, x, body, is_unroll)?;
                        }
                    }
                    Value::Rx(ch) => {
                        while let Some(v) = self.sh.recv(self.task, ch, s.line)? {
                            self.iter_body(pat, v, body, is_unroll)?;
                        }
                    }
                    other => return self.err(format!("cannot iterate over {}", kind(&other))),
                }
                Ok(())
            }
            StmtKind::Spawn(e) => {
                self.do_spawn(e)?;
                Ok(())
            }
            StmtKind::Parallel(stmts) => {
                let mut ids = Vec::new();
                for st in stmts {
                    match &st.kind {
                        StmtKind::Expr(e) | StmtKind::Spawn(e) => {
                            self.line = st.line;
                            ids.push(self.do_spawn(e)?)
                        }
                        _ => return self.err("`parallel` blocks may only contain task calls"),
                    }
                }
                self.line = s.line;
                self.sh.join_children(self.task, ids)
            }
            StmtKind::Assert { cond, text, msg } => {
                let (ok, detail) = match &cond.kind {
                    ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), l, r) => {
                        let lv = self.eval(l)?;
                        let rv = self.eval(r)?;
                        self.line = s.line;
                        let res = binop(*op, &lv, &rv).map_err(|m| self.me(m))?;
                        let ok = truthy(&res).map_err(|m| self.me(m))?;
                        (ok, format!("\n      left:  {}\n      right: {}", lv, rv))
                    }
                    _ => {
                        let v = self.eval(cond)?;
                        self.line = s.line;
                        (truthy(&v).map_err(|m| self.me(m))?, String::new())
                    }
                };
                if !ok {
                    let extra = match msg {
                        Some(m) => format!(": {}", self.eval(m)?),
                        None => String::new(),
                    };
                    self.line = s.line;
                    return self.err(format!("assertion failed: {}{}{}", text, extra, detail));
                }
                Ok(())
            }
            StmtKind::Return(e) => {
                let v = match e {
                    Some(e) => self.eval(e)?,
                    None => Value::Unit,
                };
                Err(RtErr::Ret(v))
            }
            StmtKind::Expr(e) => {
                self.eval(e)?;
                Ok(())
            }
        }
    }

    fn iter_body(&mut self, pat: &Pattern, v: Value, body: &Block, is_unroll: bool) -> R<()> {
        if self.sh.stopped() {
            return Err(RtErr::Shutdown);
        }
        self.push_scope();
        let r = (|| {
            self.bind_pattern(pat, v)?;
            if !is_unroll {
                self.loop_depth += 1;
            }
            let r = self.exec_body(body);
            if !is_unroll {
                self.loop_depth -= 1;
                if r.is_ok() {
                    self.commit_regs();
                }
            }
            r.map(|_| ())
        })();
        self.pop_scope();
        r
    }

    /// 賦值：展開成「根變數 + 路徑」，先對子型別做範圍檢查，再寫入
    fn store(&mut self, target: &Expr, v: Value) -> R<()> {
        let line = self.line;
        let mut steps_rev: Vec<Step> = Vec::new();
        let mut cur = target;
        let name = loop {
            match &cur.kind {
                ExprKind::Index(b, i) => {
                    let iv = self.eval(i)?;
                    match iv {
                        Value::Int(n) if n >= 0 => steps_rev.push(Step::Idx(n as usize)),
                        other => return self.err(format!("invalid index {} in assignment", other)),
                    }
                    cur = b;
                }
                ExprKind::Field(b, n) => {
                    match n.parse::<usize>() {
                        Ok(k) => steps_rev.push(Step::Fld(k)),
                        Err(_) => return self.err(format!("unknown field `{}`", n)),
                    }
                    cur = b;
                }
                ExprKind::Path(segs) if segs.len() == 1 => break segs[0].clone(),
                _ => return self.err("invalid assignment target"),
            }
        };
        steps_rev.reverse();
        let steps = steps_rev;
        let var = match self.lookup_mut(&name) {
            Some(v) => v,
            None => {
                return self.err(format!("assignment to undefined or constant name `{}`", name));
            }
        };
        let v = match &var.ty {
            Some(t) => {
                let mut sub = t.clone();
                for st in &steps {
                    sub = sub_type(&sub, st);
                }
                coerce(v, &sub).map_err(|m| RtErr::Msg(line, format!("assignment to `{}`: {}", name, m)))?
            }
            None => v,
        };
        let base: &mut Value = if var.reg {
            if var.pending.is_none() {
                var.pending = Some(var.val.clone());
            }
            var.pending.as_mut().unwrap()
        } else {
            &mut var.val
        };
        set_path(base, &steps, v, line)
    }

    // ------------------------------------------------------------ 運算式
    pub fn eval(&mut self, e: &Expr) -> R<Value> {
        self.line = e.line;
        match &e.kind {
            ExprKind::Int(n) => Ok(Value::Int(*n)),
            ExprKind::Float(x) => Ok(Value::Float(*x)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Path(segs) => {
                let name = segs.last().unwrap();
                if segs.len() == 1 {
                    if let Some(v) = self.lookup(name) {
                        return Ok(v.val.clone());
                    }
                }
                if let Some(v) = self.sh.program.consts.read().unwrap().get(name) {
                    return Ok(v.clone());
                }
                if SYMBOLS.contains(&name.as_str()) {
                    return Ok(Value::Str(name.clone()));
                }
                self.err(format!("undefined name `{}`", name))
            }
            ExprKind::PathT(..) => self.err("a generic path can only be called"),
            ExprKind::Unary(c, x) => {
                let v = self.eval(x)?;
                self.line = e.line;
                match (c, v) {
                    ('-', Value::Int(n)) => Ok(Value::Int(-n)),
                    ('-', Value::Float(f)) => Ok(Value::Float(-f)),
                    ('-', Value::Fixed(r, f)) => Ok(Value::Fixed(-r, f)),
                    ('!', Value::Bool(b)) => Ok(Value::Bool(!b)),
                    ('!', Value::Int(n)) if n == 0 || n == 1 => Ok(Value::Int(1 - n)),
                    ('~', Value::Int(n)) => Ok(Value::Int(!n)),
                    (c, v) => self.err(format!("operator `{}` cannot be applied to {}", c, kind(&v))),
                }
            }
            ExprKind::Binary(op, l, r) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    let lv = self.eval(l)?;
                    let lb = truthy(&lv).map_err(|m| self.me(m))?;
                    if (*op == BinOp::And && !lb) || (*op == BinOp::Or && lb) {
                        return Ok(Value::Bool(lb));
                    }
                    let rv = self.eval(r)?;
                    self.line = e.line;
                    return Ok(Value::Bool(truthy(&rv).map_err(|m| self.me(m))?));
                }
                let lv = self.eval(l)?;
                let rv = self.eval(r)?;
                self.line = e.line;
                let mut out = binop(*op, &lv, &rv).map_err(|m| self.me(m))?;
                if matches!(op, BinOp::AddW | BinOp::SubW | BinOp::MulW) {
                    let ty = match &l.kind {
                        ExprKind::Path(s) if s.len() == 1 => self.lookup(&s[0]).and_then(|v| v.ty.clone()),
                        _ => None,
                    };
                    match ty {
                        Some(t) => {
                            out = convert(&out, &t, Round::Trunc, Ovf::Wrap).map_err(|m| self.me(m))?;
                        }
                        None => {
                            return self.err(
                                "wrapping operators need a typed variable on the left; use `.wrap::<T>()` otherwise",
                            )
                        }
                    }
                }
                Ok(out)
            }
            ExprKind::Cast(x, t) => {
                let v = self.eval(x)?;
                let t = self.eval_type(t)?;
                self.line = e.line;
                convert(&v, &t, Round::Trunc, Ovf::Check).map_err(|m| self.me(format!("cast: {}", m)))
            }
            ExprKind::Call(callee, args) => self.eval_call(callee, args, e.line),
            ExprKind::Method(recv, name, targs, args) => self.eval_method(recv, name, targs, args, e.line),
            ExprKind::Field(b, n) => {
                let bv = self.eval(b)?;
                self.line = e.line;
                match (&bv, n.parse::<usize>()) {
                    (Value::Tuple(xs), Ok(i)) if i < xs.len() => Ok(xs[i].clone()),
                    _ => self.err(format!("no field `{}` on {}", n, kind(&bv))),
                }
            }
            ExprKind::Index(b, i) => {
                let iv = self.eval(i)?;
                if let ExprKind::Path(s) = &b.kind {
                    if s.len() == 1 {
                        // 快速路徑：直接在變數／常數上索引，不複製整個陣列
                        if let Some(var) = self.lookup(&s[0]) {
                            return index_value(&var.val, &iv, e.line);
                        }
                        if let Some(v) = self.sh.program.consts.read().unwrap().get(&s[0]) {
                            return index_value(v, &iv, e.line);
                        }
                    }
                }
                let bv = self.eval(b)?;
                index_value(&bv, &iv, e.line)
            }
            ExprKind::Slice(b, hi, lo) => {
                let bv = self.eval(b)?;
                let h = self.eval_int(hi)?;
                let l = self.eval_int(lo)?;
                self.line = e.line;
                match bv {
                    Value::Int(x) if l >= 0 && h >= l && h < 120 => {
                        let w = (h - l + 1) as u32;
                        Ok(Value::Int((x >> l) & (pow2(w) - 1)))
                    }
                    other => self.err(format!("invalid bit slice [{}:{}] of {}", h, l, kind(&other))),
                }
            }
            ExprKind::Range(lo, hi, incl) => {
                let l = self.eval_int(lo)?;
                let h = self.eval_int(hi)?;
                Ok(Value::Range(l, if *incl { h + 1 } else { h }))
            }
            ExprKind::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval(it)?);
                }
                Ok(Value::Array(out))
            }
            ExprKind::Repeat(x, n) => {
                let v = self.eval(x)?;
                let n = self.eval_usize(n)?;
                Ok(Value::Array(vec![v; n]))
            }
            ExprKind::Tuple(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval(it)?);
                }
                Ok(Value::Tuple(out))
            }
            ExprKind::Lambda(..) => self.err("a lambda can only be passed directly to `.map(..)` or `array(..)`"),
            ExprKind::Block(b) | ExprKind::Comptime(b) => self.exec_block(b),
            ExprKind::If(c, then, els) => {
                let cv = self.eval(c)?;
                self.line = e.line;
                if truthy(&cv).map_err(|m| self.me(m))? {
                    self.exec_block(then)
                } else if let Some(el) = els {
                    self.eval(el)
                } else {
                    Ok(Value::Unit)
                }
            }
            ExprKind::Match(s, arms) => {
                let v = self.eval(s)?;
                for arm in arms {
                    self.push_scope();
                    let matched = self.match_pat(&arm.pat, &v);
                    let matched = match matched {
                        Ok(m) => m,
                        Err(e) => {
                            self.pop_scope();
                            return Err(e);
                        }
                    };
                    if matched {
                        let ok = match &arm.guard {
                            Some(g) => match self.eval(g).and_then(|x| truthy(&x).map_err(|m| self.me(m))) {
                                Ok(b) => b,
                                Err(e) => {
                                    self.pop_scope();
                                    return Err(e);
                                }
                            },
                            None => true,
                        };
                        if ok {
                            let r = self.eval(&arm.body);
                            self.pop_scope();
                            return r;
                        }
                    }
                    self.pop_scope();
                }
                self.line = e.line;
                self.err("no match arm matched")
            }
        }
    }

    fn match_pat(&mut self, p: &MatchPat, v: &Value) -> R<bool> {
        match p {
            MatchPat::Wild => Ok(true),
            MatchPat::Bind(n) => {
                if let Some(c) = self.sh.program.consts.read().unwrap().get(n).cloned() {
                    return Ok(values_equal(&c, v));
                }
                self.plain(n, v.clone());
                Ok(true)
            }
            MatchPat::Lit(e) => {
                let x = self.eval(e)?;
                Ok(values_equal(&x, v))
            }
            MatchPat::Range(lo, hi, incl) => {
                let (l, h) = (self.eval_int(lo)?, self.eval_int(hi)?);
                match v {
                    Value::Int(n) => Ok(*n >= l && if *incl { *n <= h } else { *n < h }),
                    _ => Ok(false),
                }
            }
            MatchPat::Tuple(ps) => match v {
                Value::Tuple(xs) if xs.len() == ps.len() => {
                    for (p, x) in ps.iter().zip(xs) {
                        if !self.match_pat(p, x)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                _ => Ok(false),
            },
        }
    }

    // ------------------------------------------------------------ 呼叫
    fn eval_args(&mut self, args: &[Arg]) -> R<Vec<(Option<String>, Value)>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push((a.name.clone(), self.eval(&a.value)?));
        }
        Ok(out)
    }

    fn eval_generics(&mut self, decl: &FnDecl, targs: &[TArg]) -> R<Vec<Value>> {
        let mut out = Vec::new();
        for (k, g) in decl.generics.iter().enumerate() {
            match targs.get(k) {
                Some(TArg::Ex(e)) if g.is_const => out.push(self.eval(e)?),
                Some(_) => out.push(Value::Unit),
                None if g.is_const => {
                    return self.err(format!("missing const generic argument `{}` for `{}`", g.name, decl.name))
                }
                None => out.push(Value::Unit),
            }
        }
        Ok(out)
    }

    fn callee_name<'a>(&self, callee: &'a Expr) -> R<(String, &'a [TArg])> {
        match &callee.kind {
            ExprKind::Path(s) => Ok((s.last().unwrap().clone(), &[])),
            ExprKind::PathT(s, t) => Ok((s.last().unwrap().clone(), t.as_slice())),
            _ => self.err("callee must be a function or task name"),
        }
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Arg], line: u32) -> R<Value> {
        let (name, targs) = self.callee_name(callee)?;
        if let Some(decl) = self.sh.program.fns.get(&name).cloned() {
            let gvals = self.eval_generics(&decl, targs)?;
            let argv = self.eval_args(args)?;
            self.line = line;
            return self.run_decl(&decl, gvals, argv);
        }
        self.eval_builtin(&name, targs, args, line)
    }

    pub fn run_decl(&mut self, decl: &Arc<FnDecl>, gvals: Vec<Value>, args: Vec<(Option<String>, Value)>) -> R<Value> {
        let line = self.line;
        let saved_scopes = std::mem::replace(&mut self.scopes, vec![HashMap::new()]);
        let saved_depth = std::mem::replace(&mut self.loop_depth, 0);
        let r = self.run_decl_inner(decl, gvals, &args);
        self.scopes = saved_scopes;
        self.loop_depth = saved_depth;
        self.line = line;
        if decl.kind == FnKind::Task && !matches!(r, Err(RtErr::Msg(..)) | Err(RtErr::Shutdown) | Err(RtErr::Deadlock)) {
            for (_, v) in &args {
                self.sh.close_owned_tx(self.task, v);
            }
        }
        r
    }

    fn run_decl_inner(&mut self, decl: &Arc<FnDecl>, gvals: Vec<Value>, args: &[(Option<String>, Value)]) -> R<Value> {
        for (g, v) in decl.generics.iter().zip(gvals) {
            if g.is_const {
                self.plain(&g.name, v);
            }
        }
        let mut positional = args.iter().filter(|(n, _)| n.is_none()).map(|(_, v)| v);
        for p in &decl.params {
            let named = args.iter().find(|(n, _)| n.as_deref() == Some(p.name.as_str())).map(|(_, v)| v);
            let v = match named {
                Some(v) => v.clone(),
                None => match positional.next() {
                    Some(v) => v.clone(),
                    None => return self.err(format!("missing argument `{}` in call to `{}`", p.name, decl.name)),
                },
            };
            let t = self.eval_type(&p.ty)?;
            let v = coerce(v, &t)
                .map_err(|m| self.me(format!("argument `{}` of `{}`: {}", p.name, decl.name, m)))?;
            self.declare(&p.name, Var { val: v, ty: Some(t), reg: false, pending: None });
        }
        if positional.next().is_some() {
            return self.err(format!("too many arguments in call to `{}`", decl.name));
        }
        let val = match self.exec_block(&decl.body) {
            Ok(v) => v,
            Err(RtErr::Ret(v)) => v,
            Err(e) => return Err(e),
        };
        match &decl.ret {
            Some(rt) => {
                let t = self.eval_type(rt)?;
                coerce(val, &t).map_err(|m| self.me(format!("return value of `{}`: {}", decl.name, m)))
            }
            None => Ok(val),
        }
    }

    fn do_spawn(&mut self, call: &Expr) -> R<usize> {
        let line = call.line;
        let (callee, args) = match &call.kind {
            ExprKind::Call(c, a) => (c, a),
            _ => return self.err("`spawn` expects a task call such as `spawn Worker(rx, tx);`"),
        };
        let (name, targs) = self.callee_name(callee)?;
        let decl = match self.sh.program.fns.get(&name).cloned() {
            Some(d) => d,
            None => return self.err(format!("`{}` is not a task defined in this program", name)),
        };
        let gvals = self.eval_generics(&decl, targs)?;
        let argv = self.eval_args(args)?;
        self.line = line;
        let id = self.sh.alloc_task(&decl.name);
        for (_, v) in &argv {
            if let Err(e) = self.sh.transfer(v, self.task, id, line) {
                self.sh.task_done(id);
                return Err(e);
            }
        }
        let sh2 = self.sh.clone();
        let tname = decl.name.clone();
        self.sh.launch(id, move |id| {
            let mut it = Interp::new(sh2, id, tname);
            it.line = line;
            it.run_decl(&decl, gvals, argv).map(|_| ())
        });
        Ok(id)
    }

    // ------------------------------------------------------------ 內建函式
    fn apply_lambda(&mut self, lam: &Expr, items: Vec<Value>) -> R<Vec<Value>> {
        let (params, body) = match &lam.kind {
            ExprKind::Lambda(p, b) => (p, b),
            _ => return self.err("expected a lambda such as `|x| x * 2`"),
        };
        let mut out = Vec::with_capacity(items.len());
        for it in items {
            self.push_scope();
            if params.len() == 1 {
                self.plain(&params[0], it);
            } else if let Value::Tuple(xs) = it {
                if xs.len() != params.len() {
                    self.pop_scope();
                    return self.err("lambda parameter count does not match tuple size");
                }
                for (p, x) in params.iter().zip(xs) {
                    self.plain(p, x);
                }
            } else if params.is_empty() {
                // 無參數
            } else {
                self.pop_scope();
                return self.err("lambda parameter count does not match its argument");
            }
            let r = self.eval(body);
            self.pop_scope();
            out.push(r?);
        }
        Ok(out)
    }

    fn eval_builtin(&mut self, name: &str, targs: &[TArg], args: &[Arg], line: u32) -> R<Value> {
        match name {
            "chan" => {
                let elem = match targs.first() {
                    Some(TArg::Ty(t)) => self.eval_type(t)?,
                    _ => RtType::Any,
                };
                let vals = self.eval_args(args)?;
                let mut depth = 2usize;
                for (n, v) in &vals {
                    if n.as_deref().is_none_or(|n| n == "depth") {
                        match v {
                            Value::Int(d) if *d >= 1 && *d <= 1_000_000 => depth = *d as usize,
                            other => return self.err(format!("invalid channel depth {}", other)),
                        }
                    }
                }
                self.line = line;
                let id = self.sh.new_chan(self.task, String::new(), elem, depth);
                self.sh.rename_chan(id, format!("{}.ch{}", self.task_name, id));
                Ok(Value::Tuple(vec![Value::Tx(id), Value::Rx(id)]))
            }
            "print" => {
                let vals = self.eval_args(args)?;
                let s: Vec<String> = vals.iter().map(|(_, v)| v.to_string()).collect();
                println!("{}", s.join(" "));
                Ok(Value::Unit)
            }
            "concat" => {
                let vals = self.eval_args(args)?;
                let mut out = Vec::new();
                for (_, v) in vals {
                    match v {
                        Value::Array(xs) => out.extend(xs),
                        other => return self.err(format!("concat expects arrays, found {}", kind(&other))),
                    }
                }
                Ok(Value::Array(out))
            }
            "len" => {
                let vals = self.eval_args(args)?;
                match vals.first().map(|(_, v)| v) {
                    Some(Value::Array(xs)) => Ok(Value::Int(xs.len() as i128)),
                    Some(Value::Range(a, b)) => Ok(Value::Int((b - a).max(0))),
                    _ => self.err("len() expects an array or range"),
                }
            }
            "abs" | "min" | "max" => {
                let vals = self.eval_args(args)?;
                let vs: Vec<Value> = vals.into_iter().map(|(_, v)| v).collect();
                match (name, vs.as_slice()) {
                    ("abs", [Value::Int(n)]) => Ok(Value::Int(n.abs())),
                    ("abs", [Value::Fixed(r, f)]) => Ok(Value::Fixed(r.abs(), *f)),
                    ("min", [a, b]) | ("max", [a, b]) => {
                        let lt = binop(BinOp::Lt, a, b).map_err(|m| self.me(m))?;
                        let a_lt = matches!(lt, Value::Bool(true));
                        Ok(if (name == "min") == a_lt { a.clone() } else { b.clone() })
                    }
                    _ => self.err(format!("bad arguments to {}()", name)),
                }
            }
            "zip" => {
                let vals = self.eval_args(args)?;
                match vals.as_slice() {
                    [(_, Value::Array(a)), (_, Value::Array(b))] => Ok(Value::Array(
                        a.iter().zip(b).map(|(x, y)| Value::Tuple(vec![x.clone(), y.clone()])).collect(),
                    )),
                    _ => self.err("zip() expects two arrays"),
                }
            }
            "array" => {
                if args.len() != 2 {
                    return self.err("array(n, |i| ..) expects two arguments");
                }
                let n = self.eval_usize(&args[0].value)?;
                let items: Vec<Value> = (0..n as i128).map(Value::Int).collect();
                Ok(Value::Array(self.apply_lambda(&args[1].value, items)?))
            }
            "design_lowpass" => {
                let vals = self.eval_args(args)?;
                let mut taps = 0i128;
                let mut cutoff = 0.25f64;
                let mut window = "hamming".to_string();
                for (n, v) in &vals {
                    match (n.as_deref(), v) {
                        (Some("taps"), Value::Int(t)) => taps = *t,
                        (Some("cutoff"), v) => cutoff = as_f64(v).ok_or_else(|| self.me("cutoff must be a number".to_string()))?,
                        (Some("window"), Value::Str(s)) => window = s.clone(),
                        _ => return self.err("design_lowpass(taps = N, cutoff = fc, window = hamming|hann|rect)"),
                    }
                }
                if taps < 1 || !(0.0 < cutoff && cutoff < 0.5) {
                    return self.err("design_lowpass: need taps >= 1 and 0 < cutoff < 0.5 (fraction of the sample rate)");
                }
                let m = taps as usize;
                let center = (m as f64 - 1.0) / 2.0;
                let mut h: Vec<f64> = (0..m)
                    .map(|n| {
                        let x = n as f64 - center;
                        let sinc = if x == 0.0 { 1.0 } else { (std::f64::consts::PI * 2.0 * cutoff * x).sin() / (std::f64::consts::PI * 2.0 * cutoff * x) };
                        let w = if m == 1 {
                            1.0
                        } else {
                            let a = 2.0 * std::f64::consts::PI * n as f64 / (m as f64 - 1.0);
                            match window.as_str() {
                                "hamming" => 0.54 - 0.46 * a.cos(),
                                "hann" => 0.5 - 0.5 * a.cos(),
                                _ => 1.0,
                            }
                        };
                        2.0 * cutoff * sinc * w
                    })
                    .collect();
                let sum: f64 = h.iter().sum();
                h.iter_mut().for_each(|x| *x /= sum);
                Ok(Value::Array(h.into_iter().map(Value::Float).collect()))
            }
            "broadcast" => {
                let vals = self.eval_args(args)?;
                let (src, n) = match vals.as_slice() {
                    [(_, Value::Rx(c)), (_, Value::Int(n))] if *n >= 1 && *n <= 4096 => (*c, *n as usize),
                    _ => return self.err("broadcast(rx, n) expects an rx endpoint and a count"),
                };
                let (elem, depth) = self.sh.chan_info(src);
                let id = self.sh.alloc_task("broadcast");
                if let Err(e) = self.sh.transfer(&Value::Rx(src), self.task, id, line) {
                    self.sh.task_done(id);
                    return Err(e);
                }
                let mut outs = Vec::new();
                for k in 0..n {
                    let c = self.sh.new_chan_owned(id, self.task, format!("broadcast#{}.out{}", id, k), elem.clone(), depth);
                    outs.push(c);
                }
                let sh2 = self.sh.clone();
                let outs2 = outs.clone();
                self.sh.launch(id, move |id| {
                    while let Some(v) = sh2.recv(id, src, 0)? {
                        for &o in &outs2 {
                            sh2.send(id, o, v.clone(), 0)?;
                        }
                    }
                    for &o in &outs2 {
                        sh2.close(id, o, 0)?;
                    }
                    Ok(())
                });
                Ok(Value::Array(outs.into_iter().map(Value::Rx).collect()))
            }
            other => self.err(format!("unknown function `{}`", other)),
        }
    }

    // ------------------------------------------------------------ 方法
    fn targ_type(&mut self, targs: &[TArg]) -> R<RtType> {
        match targs.first() {
            Some(TArg::Ty(t)) => self.eval_type(t),
            _ => self.err("expected a type argument, e.g. `.to::<i16>(..)`"),
        }
    }

    fn conv_opts(&mut self, t: &RtType, args: &[Arg], ovf_default: Option<Ovf>) -> R<(Round, Ovf)> {
        let (mut round, mut ovf) = match t {
            RtType::Fixed { round, ovf, .. } => (*round, *ovf),
            _ => (Round::Trunc, Ovf::Check),
        };
        if let Some(o) = ovf_default {
            ovf = o;
        }
        for a in args {
            let v = self.eval(&a.value)?;
            let s = match &v {
                Value::Str(s) => s.clone(),
                other => return self.err(format!("expected a mode name, found {}", kind(other))),
            };
            match a.name.as_deref() {
                Some("round") => round = parse_round(&s).ok_or_else(|| self.me(format!("unknown rounding mode `{}`", s)))?,
                Some("ovf") => ovf = parse_ovf(&s).ok_or_else(|| self.me(format!("unknown overflow mode `{}`", s)))?,
                _ => return self.err("expected `round = ..` or `ovf = ..`"),
            }
        }
        Ok((round, ovf))
    }

    fn eval_method(&mut self, recv: &Expr, name: &str, targs: &[TArg], args: &[Arg], line: u32) -> R<Value> {
        let rv = self.eval(recv)?;
        self.line = line;
        match (&rv, name) {
            (Value::Tx(ch), "send") => {
                if args.len() != 1 {
                    return self.err("send() takes exactly one argument");
                }
                let v = self.eval(&args[0].value)?;
                self.sh.send(self.task, *ch, v, line)?;
                return Ok(Value::Unit);
            }
            (Value::Tx(ch), "close") => {
                self.sh.close(self.task, *ch, line)?;
                return Ok(Value::Unit);
            }
            (Value::Rx(ch), "recv") => {
                return match self.sh.recv(self.task, *ch, line)? {
                    Some(v) => Ok(v),
                    None => self.err("recv() on a closed, empty channel"),
                };
            }
            (Value::Rx(_), "try_recv") | (Value::Rx(_), "empty") | (Value::Rx(_), "len") => {
                return self.err(format!(
                    "`{}` is non-deterministic and is only allowed in `stage` code (task-level programs must use blocking recv)",
                    name
                ));
            }
            _ => {}
        }
        let rv = match rv {
            Value::Range(lo, hi) => {
                if hi - lo > 5_000_000 {
                    return self.err("range too large to materialize");
                }
                Value::Array((lo..hi).map(Value::Int).collect())
            }
            other => other,
        };
        match (rv, name) {
            (Value::Array(xs), "len") => Ok(Value::Int(xs.len() as i128)),
            (Value::Array(xs), "rev") => Ok(Value::Array(xs.into_iter().rev().collect())),
            (Value::Array(xs), "unrolled") | (Value::Array(xs), "iter") | (Value::Array(xs), "collect") => Ok(Value::Array(xs)),
            (Value::Array(xs), "enumerate") => Ok(Value::Array(
                xs.into_iter().enumerate().map(|(i, x)| Value::Tuple(vec![Value::Int(i as i128), x])).collect(),
            )),
            (Value::Array(xs), "map") => {
                if args.len() != 1 {
                    return self.err("map() takes one lambda");
                }
                Ok(Value::Array(self.apply_lambda(&args[0].value, xs)?))
            }
            (Value::Array(xs), "tree_sum") => tree_sum(xs).map_err(|m| self.me(m)),
            (Value::Array(xs), "sum") => {
                let mut it = xs.into_iter();
                let mut acc = match it.next() {
                    Some(x) => x,
                    None => return self.err("sum of empty array"),
                };
                for x in it {
                    acc = binop(BinOp::Add, &acc, &x).map_err(|m| self.me(m))?;
                }
                Ok(acc)
            }
            (v @ Value::Array(_), "quantize") => {
                let t = self.targ_type(targs)?;
                let (r, o) = self.conv_opts(&t, args, None)?;
                let et = RtType::Array(Box::new(t), match &v { Value::Array(x) => x.len(), _ => 0 });
                convert(&v, &et, r, o).map_err(|m| self.me(format!("quantize: {}", m)))
            }
            (v @ (Value::Int(_) | Value::Fixed(..) | Value::Float(_) | Value::Bool(_)), "to") => {
                let t = self.targ_type(targs)?;
                let (r, o) = self.conv_opts(&t, args, None)?;
                convert(&v, &t, r, o).map_err(|m| self.me(format!("to(): {}", m)))
            }
            (v @ (Value::Int(_) | Value::Fixed(..) | Value::Float(_)), "quantize") => {
                let t = self.targ_type(targs)?;
                let (r, o) = self.conv_opts(&t, args, None)?;
                convert(&v, &t, r, o).map_err(|m| self.me(format!("quantize: {}", m)))
            }
            (v @ (Value::Int(_) | Value::Fixed(..)), "sat") => {
                let t = self.targ_type(targs)?;
                let (r, o) = self.conv_opts(&t, args, Some(Ovf::Sat))?;
                convert(&v, &t, r, o).map_err(|m| self.me(format!("sat(): {}", m)))
            }
            (v @ (Value::Int(_) | Value::Fixed(..)), "wrap") => {
                let t = self.targ_type(targs)?;
                let (r, o) = self.conv_opts(&t, args, Some(Ovf::Wrap))?;
                convert(&v, &t, r, o).map_err(|m| self.me(format!("wrap(): {}", m)))
            }
            (Value::Int(n), "abs") => Ok(Value::Int(n.abs())),
            (Value::Fixed(r, f), "abs") => Ok(Value::Fixed(r.abs(), f)),
            (Value::Int(n), "xor_reduce") if n >= 0 => Ok(Value::Int((n.count_ones() & 1) as i128)),
            (Value::Int(n), "popcount") if n >= 0 => Ok(Value::Int(n.count_ones() as i128)),
            (v, m) => self.err(format!("no method `{}` on {}", m, kind(&v))),
        }
    }
}
