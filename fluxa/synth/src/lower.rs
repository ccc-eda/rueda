//! A task's body as a control-flow graph whose blocks are the states of its FSM.
//!
//! First every binding gets a unique name (`x` → `x.3`), so variables can become registers
//! without scopes. Then the body is cut into blocks: a block is straight-line code (pure
//! statements: no channel operation inside, executed in one clock cycle by the datapath)
//! followed by a terminator: a jump, a branch, or one channel operation (`recv`, `send`,
//! `close`) that waits for its handshake. A `recv` inside an expression is lifted out into a
//! temporary first, in evaluation order (`acc += a.recv() * b.recv()` → two receive states).

use crate::ty::R;
use fluxa::ast::*;
use std::collections::HashMap;

pub type B = usize;

#[derive(Clone, Debug)]
pub enum Term {
    Goto(B),
    Branch(Expr, B, B),
    /// receive a token of `ch` into `dst`; a closed channel goes to `closed` (`for v in rx`)
    Recv { dst: String, ch: String, closed: Option<B>, next: B },
    Send { ch: String, val: Expr, next: B },
    Close { ch: String, next: B },
    /// the end of the task: close `ch` unless the program closed it (end of stream)
    CloseOwned { ch: String, next: B },
    Halt,
}

pub struct Bb {
    pub stmts: Vec<Stmt>,
    pub term: Term,
}

pub struct Cfg {
    pub blocks: Vec<Bb>,
    /// the unique names of the task's parameters, in order
    pub params: Vec<String>,
    /// variables made by lowering (the receive temporaries, loop counters) with the type to give them
    pub temps: Vec<(String, Temp)>,
}

#[derive(Clone, Debug)]
pub enum Temp {
    /// the element type of channel (unique name)
    ChanElem(String),
    /// the type of an expression (loop bounds), found when the datapath is built
    Like(Expr),
}

// ---------------------------------------------------------------- renaming

struct Renamer {
    scopes: Vec<HashMap<String, String>>,
    n: usize,
}

impl Renamer {
    fn bind(&mut self, name: &str) -> String {
        self.n += 1;
        let u = format!("{}.{}", name, self.n);
        self.scopes.last_mut().unwrap().insert(name.to_string(), u.clone());
        u
    }
    fn lookup(&self, name: &str) -> Option<&String> { self.scopes.iter().rev().find_map(|s| s.get(name)) }

    fn pat(&mut self, p: &Pattern) -> Pattern {
        match p {
            Pattern::Wild => Pattern::Wild,
            Pattern::Ident(n) => Pattern::Ident(self.bind(n)),
            Pattern::Tuple(ps) => Pattern::Tuple(ps.iter().map(|p| self.pat(p)).collect()),
        }
    }

    fn block(&mut self, b: &Block) -> Block {
        self.scopes.push(HashMap::new());
        let stmts = b.stmts.iter().map(|s| self.stmt(s)).collect();
        let tail = b.tail.as_ref().map(|e| Box::new(self.expr(e)));
        self.scopes.pop();
        Block { stmts, tail }
    }

    fn stmt(&mut self, s: &Stmt) -> Stmt {
        let kind = match &s.kind {
            StmtKind::Let { pat, ty, placement, init } => {
                let init = init.as_ref().map(|e| self.expr(e));
                let ty = ty.as_ref().map(|t| self.ty(t));
                StmtKind::Let { pat: self.pat(pat), ty, placement: placement.clone(), init }
            }
            StmtKind::Carry { name, ty, init } => {
                let init = self.expr(init);
                let ty = self.ty(ty);
                StmtKind::Carry { name: self.bind(name), ty, init }
            }
            StmtKind::Assign { target, op, value } => StmtKind::Assign { target: self.expr(target), op: *op, value: self.expr(value) },
            StmtKind::For { attr, pat, iter, body } => {
                let iter = self.expr(iter);
                self.scopes.push(HashMap::new());
                let pat = self.pat(pat);
                let body = self.block(body);
                self.scopes.pop();
                StmtKind::For { attr: attr.clone(), pat, iter, body }
            }
            StmtKind::Spawn(e) => StmtKind::Spawn(self.expr(e)),
            StmtKind::Parallel(ss) => StmtKind::Parallel(ss.iter().map(|s| self.stmt(s)).collect()),
            StmtKind::Assert { cond, text, msg } => StmtKind::Assert { cond: self.expr(cond), text: text.clone(), msg: msg.as_ref().map(|m| self.expr(m)) },
            StmtKind::Return(e) => StmtKind::Return(e.as_ref().map(|e| self.expr(e))),
            StmtKind::Expr(e) => StmtKind::Expr(self.expr(e)),
        };
        Stmt { kind, line: s.line }
    }

    fn ty(&mut self, t: &TypeExpr) -> TypeExpr {
        match t {
            TypeExpr::Name(p, a) => TypeExpr::Name(p.clone(), a.iter().map(|a| self.targ(a)).collect()),
            TypeExpr::Fixed(i, f, o) => TypeExpr::Fixed(Box::new(self.expr(i)), Box::new(self.expr(f)), o.clone()),
            TypeExpr::Range(a, b) => TypeExpr::Range(Box::new(self.expr(a)), Box::new(self.expr(b))),
            TypeExpr::Bits(n) => TypeExpr::Bits(Box::new(self.expr(n))),
            TypeExpr::Tuple(ts) => TypeExpr::Tuple(ts.iter().map(|t| self.ty(t)).collect()),
            TypeExpr::Array(t, n) => TypeExpr::Array(Box::new(self.ty(t)), Box::new(self.expr(n))),
            TypeExpr::Chan(t, d) => TypeExpr::Chan(Box::new(self.ty(t)), d.as_ref().map(|d| Box::new(self.expr(d)))),
            TypeExpr::Mem(t, n) => TypeExpr::Mem(Box::new(self.ty(t)), Box::new(self.expr(n))),
        }
    }
    fn targ(&mut self, a: &TArg) -> TArg { match a { TArg::Ty(t) => TArg::Ty(self.ty(t)), TArg::Ex(e) => TArg::Ex(self.expr(e)) } }
    fn args(&mut self, a: &[Arg]) -> Vec<Arg> { a.iter().map(|x| Arg { name: x.name.clone(), value: self.expr(&x.value) }).collect() }

    fn mpat(&mut self, p: &MatchPat) -> MatchPat {
        match p {
            MatchPat::Bind(n) => MatchPat::Bind(self.bind(n)),
            MatchPat::Tuple(ps) => MatchPat::Tuple(ps.iter().map(|p| self.mpat(p)).collect()),
            MatchPat::Lit(e) => MatchPat::Lit(self.expr(e)),
            MatchPat::Range(a, b, i) => MatchPat::Range(self.expr(a), self.expr(b), *i),
            MatchPat::Wild => MatchPat::Wild,
        }
    }

    fn expr(&mut self, e: &Expr) -> Expr {
        let b = |x: &Expr, r: &mut Self| Box::new(r.expr(x));
        let kind = match &e.kind {
            ExprKind::Path(p) if p.len() == 1 => match self.lookup(&p[0]) { Some(u) => ExprKind::Path(vec![u.clone()]), None => e.kind.clone() },
            ExprKind::PathT(p, a) => ExprKind::PathT(p.clone(), a.iter().map(|a| self.targ(a)).collect()),
            ExprKind::Unary(c, x) => ExprKind::Unary(*c, b(x, self)),
            ExprKind::Binary(op, l, r) => ExprKind::Binary(*op, b(l, self), b(r, self)),
            ExprKind::Cast(x, t) => ExprKind::Cast(b(x, self), self.ty(t)),
            ExprKind::Call(c, a) => ExprKind::Call(b(c, self), self.args(a)),
            ExprKind::Method(r, n, t, a) => ExprKind::Method(b(r, self), n.clone(), t.iter().map(|t| self.targ(t)).collect(), self.args(a)),
            ExprKind::Field(x, n) => ExprKind::Field(b(x, self), n.clone()),
            ExprKind::Index(x, i) => ExprKind::Index(b(x, self), b(i, self)),
            ExprKind::Slice(x, h, l) => ExprKind::Slice(b(x, self), b(h, self), b(l, self)),
            ExprKind::Range(l, h, i) => ExprKind::Range(b(l, self), b(h, self), *i),
            ExprKind::Array(xs) => ExprKind::Array(xs.iter().map(|x| self.expr(x)).collect()),
            ExprKind::Repeat(x, n) => ExprKind::Repeat(b(x, self), b(n, self)),
            ExprKind::Tuple(xs) => ExprKind::Tuple(xs.iter().map(|x| self.expr(x)).collect()),
            ExprKind::Lambda(ps, body) => {
                self.scopes.push(HashMap::new());
                let ps = ps.iter().map(|p| self.bind(p)).collect();
                let body = b(body, self);
                self.scopes.pop();
                ExprKind::Lambda(ps, body)
            }
            ExprKind::Block(bl) => ExprKind::Block(self.block(bl)),
            ExprKind::Comptime(bl) => ExprKind::Comptime(self.block(bl)),
            ExprKind::If(c, t, el) => ExprKind::If(b(c, self), self.block(t), el.as_ref().map(|x| b(x, self))),
            ExprKind::Match(s, arms) => {
                let s = b(s, self);
                let arms = arms.iter().map(|a| {
                    self.scopes.push(HashMap::new());
                    let pat = self.mpat(&a.pat);
                    let guard = a.guard.as_ref().map(|g| self.expr(g));
                    let body = self.expr(&a.body);
                    self.scopes.pop();
                    Arm { pat, guard, body }
                }).collect();
                ExprKind::Match(s, arms)
            }
            k => k.clone(),
        };
        Expr { kind, line: e.line }
    }
}

// ---------------------------------------------------------------- channel operations

/// the channel a `ch.recv()` / `ch.send(..)` / `ch.close()` names (a parameter or variable)
fn chan_name(e: &Expr) -> Option<String> { match &e.kind { ExprKind::Path(p) if p.len() == 1 => Some(p[0].clone()), _ => None } }

fn is_chan_op(e: &Expr) -> bool { matches!(&e.kind, ExprKind::Method(_, n, _, _) if n == "recv" || n == "send" || n == "close") }

/// does the expression contain a channel operation (it then needs states, not just gates)?
pub fn has_chan(e: &Expr) -> bool {
    if is_chan_op(e) { return true; }
    let mut found = false;
    visit(e, &mut |x| if is_chan_op(x) { found = true });
    found
}

fn stmt_has_chan(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Let { init, .. } => init.as_ref().is_some_and(has_chan),
        StmtKind::Carry { init, .. } => has_chan(init),
        StmtKind::Assign { target, value, .. } => has_chan(target) || has_chan(value),
        StmtKind::For { iter, body, .. } => has_chan(iter) || block_has_chan(body) || matches!(&iter.kind, ExprKind::Path(_)),
        StmtKind::Spawn(_) | StmtKind::Parallel(_) | StmtKind::Return(_) => true,
        StmtKind::Assert { .. } => false,
        StmtKind::Expr(e) => has_chan(e),
    }
}

fn block_has_chan(b: &Block) -> bool { b.stmts.iter().any(stmt_has_chan) || b.tail.as_ref().is_some_and(|e| has_chan(e)) }

/// every sub-expression, in evaluation order (children before the node)
fn visit(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    let mut v = |x: &Expr| { visit(x, f); };
    match &e.kind {
        ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Field(x, _) => v(x),
        ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) | ExprKind::Range(l, r, _) | ExprKind::Repeat(l, r) => { v(l); v(r); }
        ExprKind::Slice(x, h, l) => { v(x); v(h); v(l); }
        ExprKind::Call(c, a) => { v(c); for x in a { v(&x.value); } }
        ExprKind::Method(r, _, _, a) => { v(r); for x in a { v(&x.value); } }
        ExprKind::Array(xs) | ExprKind::Tuple(xs) => for x in xs { v(x); },
        ExprKind::Lambda(_, b) => v(b),
        ExprKind::Block(b) | ExprKind::Comptime(b) => { for s in &b.stmts { visit_stmt(s, f); } if let Some(t) = &b.tail { visit(t, f); } }
        ExprKind::If(c, t, el) => { visit(c, f); for s in &t.stmts { visit_stmt(s, f); } if let Some(x) = &t.tail { visit(x, f); } if let Some(x) = el { visit(x, f); } }
        ExprKind::Match(s, arms) => { visit(s, f); for a in arms { if let Some(g) = &a.guard { visit(g, f); } visit(&a.body, f); } }
        _ => {}
    }
    f(e);
}

fn visit_stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Let { init: Some(e), .. } | StmtKind::Carry { init: e, .. } | StmtKind::Expr(e) => visit(e, f),
        StmtKind::Assign { target, value, .. } => { visit(value, f); visit(target, f); }
        StmtKind::For { iter, body, .. } => { visit(iter, f); for s in &body.stmts { visit_stmt(s, f); } }
        _ => {}
    }
}

// ---------------------------------------------------------------- the graph

struct Lower {
    blocks: Vec<Bb>,
    temps: Vec<(String, Temp)>,
    n: usize,
    /// where `return` goes
    end: B,
}

impl Lower {
    fn new_block(&mut self) -> B {
        self.blocks.push(Bb { stmts: vec![], term: Term::Halt });
        self.blocks.len() - 1
    }
    fn temp(&mut self, base: &str, t: Temp) -> String {
        self.n += 1;
        let name = format!("{}~{}", base, self.n);
        self.temps.push((name.clone(), t));
        name
    }

    /// replace each `ch.recv()` in `e` (evaluation order) by a temporary, receiving it in its own state
    fn lift(&mut self, e: &Expr, cur: &mut B) -> R<Expr> {
        if !has_chan(e) { return Ok(e.clone()); }
        let line = e.line;
        match &e.kind {
            ExprKind::Method(r, n, _, a) if n == "recv" && a.is_empty() => {
                let ch = chan_name(r).ok_or_else(|| format!("line {}: recv on something that is not a channel name", line))?;
                let dst = self.temp("recv", Temp::ChanElem(ch.clone()));
                let next = self.new_block();
                self.blocks[*cur].term = Term::Recv { dst: dst.clone(), ch, closed: None, next };
                *cur = next;
                Ok(Expr { kind: ExprKind::Path(vec![dst]), line })
            }
            ExprKind::Method(_, n, _, _) if n == "send" || n == "close" => Err(format!("line {}: `{}` inside an expression is not synthesized; make it a statement", line, n)),
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), l, r) if has_chan(r) => {
                let _ = (op, l);
                Err(format!("line {}: a recv on the right of && / || is not synthesized (it is not always evaluated)", line))
            }
            ExprKind::Unary(c, x) => Ok(Expr { kind: ExprKind::Unary(*c, Box::new(self.lift(x, cur)?)), line }),
            ExprKind::Binary(op, l, r) => {
                let l = self.lift(l, cur)?;
                let r = self.lift(r, cur)?;
                Ok(Expr { kind: ExprKind::Binary(*op, Box::new(l), Box::new(r)), line })
            }
            ExprKind::Cast(x, t) => Ok(Expr { kind: ExprKind::Cast(Box::new(self.lift(x, cur)?), t.clone()), line }),
            ExprKind::Call(c, a) => {
                let mut args = vec![];
                for x in a { args.push(Arg { name: x.name.clone(), value: self.lift(&x.value, cur)? }); }
                Ok(Expr { kind: ExprKind::Call(c.clone(), args), line })
            }
            ExprKind::Method(r, n, t, a) => {
                let r = self.lift(r, cur)?;
                let mut args = vec![];
                for x in a { args.push(Arg { name: x.name.clone(), value: self.lift(&x.value, cur)? }); }
                Ok(Expr { kind: ExprKind::Method(Box::new(r), n.clone(), t.clone(), args), line })
            }
            ExprKind::Index(x, i) => {
                let x = self.lift(x, cur)?;
                let i = self.lift(i, cur)?;
                Ok(Expr { kind: ExprKind::Index(Box::new(x), Box::new(i)), line })
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => {
                let mut v = vec![];
                for x in xs { v.push(self.lift(x, cur)?); }
                Ok(Expr { kind: if matches!(e.kind, ExprKind::Tuple(_)) { ExprKind::Tuple(v) } else { ExprKind::Array(v) }, line })
            }
            _ => Err(format!("line {}: a channel operation in this kind of expression is not synthesized; give it its own statement", line)),
        }
    }

    /// lower statements starting in block `cur`; the block where control continues
    fn stmts(&mut self, stmts: &[Stmt], tail: Option<&Expr>, mut cur: B) -> R<B> {
        for s in stmts {
            cur = self.stmt(s, cur)?;
        }
        if let Some(t) = tail {
            if has_chan(t) { return self.chan_stmt(t, cur); }
            self.blocks[cur].stmts.push(Stmt { kind: StmtKind::Expr(t.clone()), line: t.line });
        }
        Ok(cur)
    }

    fn chan_stmt(&mut self, e: &Expr, mut cur: B) -> R<B> {
        match &e.kind {
            ExprKind::Method(r, n, _, a) if n == "send" && a.len() == 1 => {
                let ch = chan_name(r).ok_or_else(|| format!("line {}: send on something that is not a channel name", e.line))?;
                let val = self.lift(&a[0].value, &mut cur)?;
                let next = self.new_block();
                self.blocks[cur].term = Term::Send { ch, val, next };
                Ok(next)
            }
            ExprKind::Method(r, n, _, a) if n == "close" && a.is_empty() => {
                let ch = chan_name(r).ok_or_else(|| format!("line {}: close on something that is not a channel name", e.line))?;
                let next = self.new_block();
                self.blocks[cur].term = Term::Close { ch, next };
                Ok(next)
            }
            ExprKind::Method(_, n, _, a) if n == "recv" && a.is_empty() => {
                // a value thrown away: still a receive
                let _ = self.lift(e, &mut cur)?;
                Ok(cur)
            }
            ExprKind::Block(b) => self.stmts(&b.stmts, b.tail.as_deref(), cur),
            ExprKind::If(c, t, el) => {
                let c = self.lift(c, &mut cur)?;
                let (bt, bf, join) = (self.new_block(), self.new_block(), self.new_block());
                self.blocks[cur].term = Term::Branch(c, bt, bf);
                let et = self.stmts(&t.stmts, t.tail.as_deref(), bt)?;
                self.blocks[et].term = Term::Goto(join);
                let ef = match el {
                    Some(x) => self.chan_or_pure(x, bf)?,
                    None => bf,
                };
                self.blocks[ef].term = Term::Goto(join);
                Ok(join)
            }
            _ => {
                let v = self.lift(e, &mut cur)?;
                self.blocks[cur].stmts.push(Stmt { kind: StmtKind::Expr(v), line: e.line });
                Ok(cur)
            }
        }
    }

    fn chan_or_pure(&mut self, e: &Expr, cur: B) -> R<B> {
        if has_chan(e) { self.chan_stmt(e, cur) } else {
            self.blocks[cur].stmts.push(Stmt { kind: StmtKind::Expr(e.clone()), line: e.line });
            Ok(cur)
        }
    }

    fn stmt(&mut self, s: &Stmt, mut cur: B) -> R<B> {
        if !stmt_has_chan(s) {
            self.blocks[cur].stmts.push(s.clone());
            return Ok(cur);
        }
        let line = s.line;
        match &s.kind {
            StmtKind::Let { pat, ty, placement, init } => {
                let init = match init { Some(e) => Some(self.lift(e, &mut cur)?), None => None };
                self.blocks[cur].stmts.push(Stmt { kind: StmtKind::Let { pat: pat.clone(), ty: ty.clone(), placement: placement.clone(), init }, line });
                Ok(cur)
            }
            StmtKind::Carry { name, ty, init } => {
                let init = self.lift(init, &mut cur)?;
                self.blocks[cur].stmts.push(Stmt { kind: StmtKind::Carry { name: name.clone(), ty: ty.clone(), init }, line });
                Ok(cur)
            }
            StmtKind::Assign { target, op, value } => {
                let value = self.lift(value, &mut cur)?;
                let target = self.lift(target, &mut cur)?;
                self.blocks[cur].stmts.push(Stmt { kind: StmtKind::Assign { target, op: *op, value }, line });
                Ok(cur)
            }
            StmtKind::Expr(e) => self.chan_stmt(e, cur),
            StmtKind::Return(e) => {
                if let Some(e) = e { if !matches!(e.kind, ExprKind::Tuple(ref x) if x.is_empty()) { return Err(format!("line {}: a task's return value is not synthesized", line)); } }
                self.blocks[cur].term = Term::Goto(self.end);
                Ok(self.new_block())
            }
            StmtKind::Spawn(_) | StmtKind::Parallel(_) => Err(format!("line {}: spawn inside the synthesized task is not supported yet", line)),
            StmtKind::For { pat, iter, body, .. } => {
                let var = match pat {
                    Pattern::Ident(n) => Some(n.clone()),
                    Pattern::Wild => None,
                    Pattern::Tuple(_) => return Err(format!("line {}: a tuple pattern in a synthesized loop is not supported yet", line)),
                };
                match &iter.kind {
                    // for v in rx: receive until the channel is closed
                    ExprKind::Path(p) if p.len() == 1 => {
                        let ch = p[0].clone();
                        let dst = self.temp("recv", Temp::ChanElem(ch.clone()));
                        let (head, first, exit) = (self.new_block(), self.new_block(), self.new_block());
                        self.blocks[cur].term = Term::Goto(head);
                        self.blocks[head].term = Term::Recv { dst: dst.clone(), ch, closed: Some(exit), next: first };
                        if let Some(v) = var { self.blocks[first].stmts.push(let_stmt(&v, path(&dst, line), line)); }
                        let end = self.stmts(&body.stmts, body.tail.as_deref(), first)?;
                        self.blocks[end].term = Term::Goto(head);
                        Ok(exit)
                    }
                    // for i in a..b (a body with channel operations, or a bound known only at run time)
                    ExprKind::Range(lo, hi, incl) => {
                        let lo = self.lift(lo, &mut cur)?;
                        let hi = self.lift(hi, &mut cur)?;
                        let i = self.temp("i", Temp::Like(Expr { kind: ExprKind::Range(Box::new(lo.clone()), Box::new(hi.clone()), *incl), line }));
                        let end = self.temp("end", Temp::Like(hi.clone()));
                        self.blocks[cur].stmts.push(assign(&i, lo, line));
                        self.blocks[cur].stmts.push(assign(&end, hi, line));
                        let (head, first, exit) = (self.new_block(), self.new_block(), self.new_block());
                        self.blocks[cur].term = Term::Goto(head);
                        let op = if *incl { BinOp::Le } else { BinOp::Lt };
                        let cond = Expr { kind: ExprKind::Binary(op, Box::new(path(&i, line)), Box::new(path(&end, line))), line };
                        self.blocks[head].term = Term::Branch(cond, first, exit);
                        if let Some(v) = var { self.blocks[first].stmts.push(let_stmt(&v, path(&i, line), line)); }
                        let last = self.stmts(&body.stmts, body.tail.as_deref(), first)?;
                        let inc = Expr { kind: ExprKind::Binary(BinOp::Add, Box::new(path(&i, line)), Box::new(Expr { kind: ExprKind::Int(1), line })), line };
                        self.blocks[last].stmts.push(assign(&i, inc, line));
                        self.blocks[last].term = Term::Goto(head);
                        Ok(exit)
                    }
                    _ => Err(format!("line {}: this loop is not synthesized yet (a channel or a range)", line)),
                }
            }
            StmtKind::Assert { .. } => Ok(cur),
        }
    }
}

fn path(n: &str, line: u32) -> Expr { Expr { kind: ExprKind::Path(vec![n.to_string()]), line } }
fn let_stmt(n: &str, e: Expr, line: u32) -> Stmt { Stmt { kind: StmtKind::Let { pat: Pattern::Ident(n.to_string()), ty: None, placement: None, init: Some(e) }, line } }
fn assign(n: &str, e: Expr, line: u32) -> Stmt { Stmt { kind: StmtKind::Assign { target: path(n, line), op: None, value: e }, line } }

/// the task `decl` as states; `outs` are its output channel parameters (closed at the end)
pub fn lower(decl: &FnDecl) -> R<Cfg> {
    let mut rn = Renamer { scopes: vec![HashMap::new()], n: 0 };
    let params: Vec<String> = decl.params.iter().map(|p| rn.bind(&p.name)).collect();
    let body = rn.block(&decl.body);
    let mut l = Lower { blocks: vec![], temps: vec![], n: 0, end: 0 };
    let entry = l.new_block();
    let end = l.new_block();
    l.end = end;
    let last = l.stmts(&body.stmts, body.tail.as_deref(), entry)?;
    l.blocks[last].term = Term::Goto(end);
    // end of the task: every output channel is closed (end of stream), then the task stops
    let mut cur = end;
    for (p, u) in decl.params.iter().zip(&params) {
        if matches!(&p.ty, TypeExpr::Chan(..)) && param_dir_out(decl, &p.name) {
            let next = l.new_block();
            l.blocks[cur].term = Term::CloseOwned { ch: u.clone(), next };
            cur = next;
        }
    }
    l.blocks[cur].term = Term::Halt;
    Ok(Cfg { blocks: l.blocks, params, temps: l.temps })
}

/// is parameter `name` an output channel (`out chan<T>`, or sent on)?
fn param_dir_out(decl: &FnDecl, name: &str) -> bool { crate::dir_of(decl, name) == Some(true) }

/// a function's parameters and body with fresh names (to inline it), numbered from `start`
pub fn rename_fn(decl: &FnDecl, start: usize) -> (Vec<String>, Block) {
    let mut rn = Renamer { scopes: vec![HashMap::new()], n: start * 1000 };
    let params = decl.params.iter().map(|p| rn.bind(&p.name)).collect();
    let body = rn.block(&decl.body);
    (params, body)
}
