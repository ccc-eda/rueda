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
use std::collections::{HashMap, HashSet};

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
    /// a `pipeline(ii = 1)` loop as one state: an iteration each clock whose inputs are all valid
    /// and whose outputs are all ready. The block's statements are the body; before them `head`
    /// (`for v in rx`: the token into `dst`, the end of the stream leaves) or `cond` (a counted
    /// loop goes on while it holds) is looked at, and `recvs` take their tokens; `sends` are the
    /// values the body sent (variables), sent when the iteration fires.
    /// read word `addr` of memory `mem` into `dst` (a clock, like a block RAM's registered read)
    MemRead { dst: String, mem: String, addr: Expr, next: B },
    MemWrite { mem: String, addr: Expr, val: Expr, next: B },
    Pipe { head: Option<(String, String)>, cond: Option<Expr>, recvs: Vec<(String, String)>, sends: Vec<(String, String)>, exit: B },
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
    /// the memories (`mems_of`)
    pub mems: Vec<MemDecl>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Temp {
    /// the element type of channel (unique name)
    ChanElem(String),
    /// the type of an expression (loop bounds), found when the datapath is built
    Like(Expr),
    /// the word type of a memory
    MemElem(String),
}

// ---------------------------------------------------------------- renaming

struct Renamer {
    scopes: Vec<HashMap<String, String>>,
    n: usize,
    /// the memory pass instead of renaming (see `mem_rewrite`)
    mem: Option<MemRw>,
    /// the program's constants: a pattern naming one compares with it (it binds nothing)
    consts: HashSet<String>,
}

struct MemRw {
    names: HashSet<String>,
    /// a memory used other than as `m[i]`
    bad: bool,
}

impl Renamer {
    fn bind(&mut self, name: &str) -> String {
        if self.mem.is_some() { return name.to_string(); }
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
        let keep: Vec<&Stmt> = b.stmts.iter().filter(|s| !self.mem_decl(s)).collect();
        let stmts = keep.into_iter().map(|s| self.stmt(s)).collect();
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
            StmtKind::Assign { target, op, value } => match self.mem_index(target) {
                // m[i] op= v → m.$write(i, m.$read(i) op v)
                Some((m, i)) => {
                    let (i, v) = (self.expr(i), self.expr(value));
                    let line = s.line;
                    let v = match op {
                        Some(o) => Expr { kind: ExprKind::Binary(*o, Box::new(mem_op(&m, "$read", vec![i.clone()], line)), Box::new(v)), line },
                        None => v,
                    };
                    StmtKind::Expr(mem_op(&m, "$write", vec![i, v], line))
                }
                None => StmtKind::Assign { target: self.expr(target), op: *op, value: self.expr(value) },
            },
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
            MatchPat::Bind(n) if self.lookup(n).is_none() && self.consts.contains(n) => MatchPat::Lit(path(n, 0)),
            MatchPat::Bind(n) => MatchPat::Bind(self.bind(n)),
            MatchPat::Tuple(ps) => MatchPat::Tuple(ps.iter().map(|p| self.mpat(p)).collect()),
            MatchPat::Lit(e) => MatchPat::Lit(self.expr(e)),
            MatchPat::Range(a, b, i) => MatchPat::Range(self.expr(a), self.expr(b), *i),
            MatchPat::Wild => MatchPat::Wild,
        }
    }

    fn expr(&mut self, e: &Expr) -> Expr {
        let b = |x: &Expr, r: &mut Self| Box::new(r.expr(x));
        if let Some((m, i)) = self.mem_index(e) { let i = self.expr(i); return mem_op(&m, "$read", vec![i], e.line); }
        let kind = match &e.kind {
            ExprKind::Path(p) if p.len() == 1 && self.mem.as_ref().is_some_and(|m| m.names.contains(&p[0])) => {
                self.mem.as_mut().unwrap().bad = true;
                e.kind.clone()
            }
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

impl Renamer {
    /// `m[i]` of a memory: (m, i)
    fn mem_index<'e>(&self, e: &'e Expr) -> Option<(String, &'e Expr)> {
        let mr = self.mem.as_ref()?;
        match &e.kind {
            ExprKind::Index(x, i) => match &x.kind { ExprKind::Path(p) if p.len() == 1 && mr.names.contains(&p[0]) => Some((p[0].clone(), i)), _ => None },
            _ => None,
        }
    }
    /// a memory's declaration, left out by the memory pass
    fn mem_decl(&self, s: &Stmt) -> bool {
        let Some(mr) = &self.mem else { return false };
        matches!(&s.kind, StmtKind::Let { pat: Pattern::Ident(n), .. } if mr.names.contains(n))
    }
}

fn mem_op(m: &str, op: &str, args: Vec<Expr>, line: u32) -> Expr {
    let args = args.into_iter().map(|value| Arg { name: None, value }).collect();
    Expr { kind: ExprKind::Method(Box::new(path(m, line)), op.into(), vec![], args), line }
}

/// a memory: name, word type, size, the parameter its initial words come from (else all 0)
pub type MemDecl = (String, TypeExpr, Expr, Option<String>);

/// the memories of a body: `let m: mem<T, N>` starting all 0 (`[0; N]` or nothing) or with an
/// array parameter's words (`= data`)
fn mems_of(b: &Block, params: &[String], out: &mut Vec<MemDecl>) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { pat: Pattern::Ident(n), ty: Some(TypeExpr::Mem(t, k)), init, .. } => {
                let zero = |e: &Expr| matches!(e.kind, ExprKind::Int(0));
                let from = match init {
                    None => Some(None),
                    Some(e) => match &e.kind {
                        ExprKind::Repeat(x, _) if zero(x) => Some(None),
                        // its initial words are an array parameter's
                        ExprKind::Path(p) if p.len() == 1 && params.contains(&p[0]) => Some(Some(p[0].clone())),
                        _ => None,
                    },
                };
                if let Some(from) = from { out.push((n.clone(), (**t).clone(), (**k).clone(), from)); }
            }
            StmtKind::For { body, .. } => mems_of(body, params, out),
            _ => {}
        }
    }
}

/// memories become one-clock accesses: `m[i]` → `m.$read(i)` (the word comes out of the memory
/// a clock later, as from a block RAM), `m[i] = v` → `m.$write(i, v)`. None when a memory is
/// used some other way (it then stays an array of registers).
fn mem_rewrite(body: &Block, mems: &[MemDecl]) -> Option<Block> {
    let names = mems.iter().map(|m| m.0.clone()).collect();
    let mut rw = Renamer { scopes: vec![HashMap::new()], n: 0, mem: Some(MemRw { names, bad: false }), consts: HashSet::new() };
    let b = rw.block(body);
    (!rw.mem.unwrap().bad).then_some(b)
}

// ---------------------------------------------------------------- channel operations

/// the channel a `ch.recv()` / `ch.send(..)` / `ch.close()` names (a parameter or variable)
fn chan_name(e: &Expr) -> Option<String> { match &e.kind { ExprKind::Path(p) if p.len() == 1 => Some(p[0].clone()), _ => None } }

fn is_chan_op(e: &Expr) -> bool { matches!(&e.kind, ExprKind::Method(_, n, _, _) if n == "recv" || n == "send" || n == "close" || n == "$read" || n == "$write") }

/// does the expression contain a channel operation (it then needs states, not just gates)?
pub fn has_chan(e: &Expr) -> bool {
    if is_chan_op(e) || has_return(e) || expr_serial(e) { return true; }
    let mut found = false;
    visit(e, &mut |x| if is_chan_op(x) { found = true });
    found
}

/// does a block contain a `serial for`? Its iterations take a clock each (states, one per
/// iteration), where another loop without channel operations is unrolled into gates
fn block_serial(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::For { attr: LoopAttr::Serial, .. } => true,
        StmtKind::For { body, .. } => block_serial(body),
        StmtKind::Let { init: Some(e), .. } | StmtKind::Carry { init: e, .. } | StmtKind::Expr(e) | StmtKind::Assign { value: e, .. } => expr_serial(e),
        _ => false,
    }) || b.tail.as_ref().is_some_and(|t| expr_serial(t))
}

fn expr_serial(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Block(b) => block_serial(b),
        ExprKind::If(_, t, el) => block_serial(t) || el.as_ref().is_some_and(|x| expr_serial(x)),
        ExprKind::Match(_, arms) => arms.iter().any(|a| expr_serial(&a.body)),
        _ => false,
    }
}

/// does an `if` / `match` / block contain a `return` (a jump to the task's end: states too)?
fn has_return(e: &Expr) -> bool {
    let blk = |b: &Block| b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Return(_) => true,
        StmtKind::Expr(x) => has_return(x),
        StmtKind::For { body, .. } => body.stmts.iter().any(|s| matches!(&s.kind, StmtKind::Return(_)) || matches!(&s.kind, StmtKind::Expr(x) if has_return(x))) || body.tail.as_ref().is_some_and(|t| has_return(t)),
        _ => false,
    }) || b.tail.as_ref().is_some_and(|t| has_return(t));
    match &e.kind {
        ExprKind::Block(b) => blk(b),
        ExprKind::If(_, t, el) => blk(t) || el.as_ref().is_some_and(|x| has_return(x)),
        ExprKind::Match(_, arms) => arms.iter().any(|a| has_return(&a.body)),
        _ => false,
    }
}

/// does a statement need states (a channel operation, a loop over a channel, spawn, return)?
fn stmt_has_chan(s: &Stmt, chans: &HashSet<String>) -> bool {
    match &s.kind {
        StmtKind::Let { init, .. } => init.as_ref().is_some_and(has_chan),
        StmtKind::Carry { init, .. } => has_chan(init),
        StmtKind::Assign { target, value, .. } => has_chan(target) || has_chan(value),
        StmtKind::For { attr, iter, body, .. } => matches!(attr, LoopAttr::Serial) || has_chan(iter) || block_has_chan(body, chans) || is_chan_path(iter, chans),
        StmtKind::Spawn(_) | StmtKind::Parallel(_) | StmtKind::Return(_) => true,
        StmtKind::Assert { .. } => false,
        StmtKind::Expr(e) => has_chan(e),
    }
}

fn block_has_chan(b: &Block, chans: &HashSet<String>) -> bool { b.stmts.iter().any(|s| stmt_has_chan(s, chans)) || b.tail.as_ref().is_some_and(|e| has_chan(e)) }

fn is_chan_path(e: &Expr, chans: &HashSet<String>) -> bool { matches!(&e.kind, ExprKind::Path(p) if p.len() == 1 && chans.contains(&p[0])) }

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
        ExprKind::If(c, t, el) => {
            visit(c, f);
            for s in &t.stmts { visit_stmt(s, f); }
            if let Some(x) = &t.tail { visit(x, f); }
            if let Some(x) = el { visit(x, f); }
        }
        ExprKind::Match(s, arms) => { visit(s, f); for a in arms { if let Some(g) = &a.guard { visit(g, f); } visit(&a.body, f); } }
        _ => {}
    }
    f(e);
}

fn visit_stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Let { init: Some(e), .. } | StmtKind::Carry { init: e, .. } | StmtKind::Expr(e) => visit(e, f),
        StmtKind::Assign { target, value, .. } => { visit(value, f); visit(target, f); }
        StmtKind::For { iter, body, .. } => { visit(iter, f); for s in &body.stmts { visit_stmt(s, f); } if let Some(t) = &body.tail { visit(t, f); } }
        _ => {}
    }
}

// ---------------------------------------------------------------- the graph

struct Lower {
    blocks: Vec<Bb>,
    /// the channel parameters (unique names)
    chans: HashSet<String>,
    /// `@reg` variables: written to `NAME~p` in loops, current again at each iteration's end
    regs: Vec<String>,
    temps: Vec<(String, Temp)>,
    n: usize,
    /// where `return` goes
    end: B,
    /// lowering a pipelined loop's body: its channel operations, in order
    pipe: Option<Vec<PipeOp>>,
}

enum PipeOp { Recv(String, String), Send(String, String) }

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
                if let Some(ops) = &mut self.pipe {
                    ops.push(PipeOp::Recv(dst.clone(), ch));
                } else {
                    let next = self.new_block();
                    self.blocks[*cur].term = Term::Recv { dst: dst.clone(), ch, closed: None, next };
                    *cur = next;
                }
                Ok(Expr { kind: ExprKind::Path(vec![dst]), line })
            }
            ExprKind::Method(r, n, _, a) if n == "$read" => {
                if self.pipe.is_some() { return Err("a memory read (a clock each)".into()); }
                let mem = chan_name(r).unwrap();
                let addr = self.lift(&a[0].value, cur)?;
                let dst = self.temp("rd", Temp::MemElem(mem.clone()));
                let next = self.new_block();
                self.blocks[*cur].term = Term::MemRead { dst: dst.clone(), mem, addr, next };
                *cur = next;
                Ok(Expr { kind: ExprKind::Path(vec![dst]), line })
            }
            ExprKind::Method(_, n, _, _) if n == "send" || n == "close" || n == "$write" => Err(format!("line {}: `{}` inside an expression is not synthesized; make it a statement", line, n)),
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
            // a value made by statements with channel operations: their states, then the value
            ExprKind::Block(b) => {
                if self.pipe.is_some() { return Err("a channel operation in a block".into()); }
                *cur = self.stmts(&b.stmts, None, *cur)?;
                match &b.tail { Some(t) => self.lift(t, cur), None => Ok(Expr { kind: ExprKind::Tuple(vec![]), line }) }
            }
            ExprKind::If(c, t, el) => {
                let c = self.lift(c, cur)?;
                let pure = !block_has_chan(t, &self.chans) && !el.as_ref().is_some_and(|x| has_chan(x));
                if pure { return Ok(Expr { kind: ExprKind::If(Box::new(c), t.clone(), el.clone()), line }); }
                if self.pipe.is_some() { return Err("a channel operation under a condition (each iteration must do the same channel operations)".into()); }
                let Some(el) = el else { return Err(format!("line {}: an `if` without `else` used as a value", line)) };
                // each branch puts its value into one variable
                self.n += 1;
                let v = format!("if~{}", self.n);
                let mut ends = vec![];
                self.if_into(c, t, el, &v, *cur, &mut ends, line)?;
                // after the branches (the type pass sees the value assigned before it is read)
                let join = self.new_block();
                for b in ends { self.blocks[b].term = Term::Goto(join); }
                *cur = join;
                Ok(path(&v, line))
            }
            ExprKind::Match(sc, arms) => {
                let e = self.match_as_if(sc, arms, cur, line)?;
                self.lift(&e, cur)
            }
            _ => Err(format!("line {}: a channel operation in this kind of expression is not synthesized; give it its own statement", line)),
        }
    }

    /// `if c { t } else { el }` from `cur`, each branch's value into `v`; an `else if` with
    /// channel operations puts its value into the same `v` (a chain is one register, not one
    /// per level). The blocks where the branches end go to `ends`.
    #[allow(clippy::too_many_arguments)]
    fn if_into(&mut self, c: Expr, t: &Block, el: &Expr, v: &str, cur: B, ends: &mut Vec<B>, line: u32) -> R<()> {
        let (bt, bf) = (self.new_block(), self.new_block());
        self.blocks[cur].term = Term::Branch(c, bt, bf);
        let mut b = bt;
        let x = self.lift(&Expr { kind: ExprKind::Block(t.clone()), line }, &mut b)?;
        self.blocks[b].stmts.push(let_stmt(v, x, line));
        ends.push(b);
        let mut b = bf;
        if let ExprKind::If(c2, t2, Some(el2)) = &el.kind {
            if has_chan(el) {
                let c2 = self.lift(c2, &mut b)?;
                return self.if_into(c2, t2, el2, v, b, ends, line);
            }
        }
        let x = self.lift(el, &mut b)?;
        self.blocks[b].stmts.push(let_stmt(v, x, line));
        ends.push(b);
        Ok(())
    }

    /// `match s { p => e, .. }` with channel operations in its arms as `if s == p { e } else ..`
    /// (the scrutinee evaluated once, into a variable)
    fn match_as_if(&mut self, sc: &Expr, arms: &[Arm], cur: &mut B, line: u32) -> R<Expr> {
        let sc = self.lift(sc, cur)?;
        self.n += 1;
        let s = format!("match~{}", self.n);
        self.blocks[*cur].stmts.push(let_stmt(&s, sc, line));
        let bin = |op, a: Expr, b: Expr| Expr { kind: ExprKind::Binary(op, Box::new(a), Box::new(b)), line };
        let mut out: Option<Expr> = None;
        for a in arms.iter().rev() {
            let mut stmts = vec![];
            let mut cond = match &a.pat {
                MatchPat::Wild => None,
                MatchPat::Bind(n) => { stmts.push(let_stmt(n, path(&s, line), line)); None }
                MatchPat::Lit(e) => Some(bin(BinOp::Eq, path(&s, line), e.clone())),
                MatchPat::Range(lo, hi, incl) => Some(bin(BinOp::And, bin(BinOp::Ge, path(&s, line), lo.clone()), bin(if *incl { BinOp::Le } else { BinOp::Lt }, path(&s, line), hi.clone()))),
                MatchPat::Tuple(_) => return Err(format!("line {}: a tuple pattern in a `match` with channel operations is not synthesized", line)),
            };
            if let Some(g) = &a.guard {
                if matches!(a.pat, MatchPat::Bind(_)) { return Err(format!("line {}: a guard on a binding in a `match` with channel operations is not synthesized", line)); }
                cond = Some(match cond { Some(c) => bin(BinOp::And, c, g.clone()), None => g.clone() });
            }
            let body = Expr { kind: ExprKind::Block(Block { stmts, tail: Some(Box::new(a.body.clone())) }), line };
            out = Some(match (cond, out) {
                // the last arm, or one that always matches: what is left
                (None, _) | (Some(_), None) => body,
                (Some(c), Some(rest)) => Expr { kind: ExprKind::If(Box::new(c), Block { stmts: vec![], tail: Some(Box::new(body)) }, Some(Box::new(rest))), line },
            });
        }
        out.ok_or_else(|| format!("line {}: an empty `match`", line))
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
                if self.pipe.is_some() {
                    // the value as it is here; sent when the iteration fires
                    self.n += 1;
                    let v = format!("send~{}", self.n);
                    self.blocks[cur].stmts.push(let_stmt(&v, val, e.line));
                    if let Some(ops) = &mut self.pipe { ops.push(PipeOp::Send(ch, v)); }
                    return Ok(cur);
                }
                let next = self.new_block();
                self.blocks[cur].term = Term::Send { ch, val, next };
                Ok(next)
            }
            ExprKind::Method(r, n, _, a) if n == "$write" => {
                if self.pipe.is_some() { return Err("a memory write (a clock each)".into()); }
                let mem = chan_name(r).unwrap();
                let addr = self.lift(&a[0].value, &mut cur)?;
                let val = self.lift(&a[1].value, &mut cur)?;
                let next = self.new_block();
                self.blocks[cur].term = Term::MemWrite { mem, addr, val, next };
                Ok(next)
            }
            ExprKind::Method(_, n, _, a) if n == "close" && a.is_empty() && self.pipe.is_some() => Err("a channel is closed in the loop".into()),
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
            ExprKind::Match(sc, arms) if self.pipe.is_none() => {
                let e = self.match_as_if(sc, arms, &mut cur, e.line)?;
                self.chan_stmt(&e, cur)
            }
            ExprKind::If(..) if self.pipe.is_some() => Err("a channel operation under a condition (each iteration must do the same channel operations)".into()),
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
        if !stmt_has_chan(s, &self.chans) {
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
            StmtKind::Return(_) if self.pipe.is_some() => Err("`return` in the loop".into()),
            StmtKind::Return(e) => {
                if let Some(e) = e { if !matches!(e.kind, ExprKind::Tuple(ref x) if x.is_empty()) { return Err(format!("line {}: a task's return value is not synthesized", line)); } }
                self.blocks[cur].term = Term::Goto(self.end);
                Ok(self.new_block())
            }
            StmtKind::Spawn(_) | StmtKind::Parallel(_) => Err(format!("line {}: spawn inside the synthesized task is not supported yet", line)),
            StmtKind::For { .. } if self.pipe.is_some() => Err("an inner loop with channel operations".into()),
            StmtKind::For { attr: LoopAttr::Pipeline(kv), pat, iter, body } => {
                let ii = kv.iter().find(|(k, _)| k == "ii").map(|(_, e)| match e.kind { ExprKind::Int(n) => n, _ => 1 }).unwrap_or(1);
                let mark = (self.blocks.len(), self.temps.len(), self.n);
                match self.pipelined(pat, iter, body, cur, line) {
                    Ok(b) => Ok(b),
                    // a longer initiation interval is allowed: the loop as ordinary states
                    Err(_) if ii > 1 => {
                        self.pipe = None;
                        self.blocks.truncate(mark.0);
                        self.temps.truncate(mark.1);
                        self.n = mark.2;
                        self.blocks[cur].term = Term::Halt;
                        let plain = Stmt { kind: StmtKind::For { attr: LoopAttr::None, pat: pat.clone(), iter: iter.clone(), body: body.clone() }, line };
                        self.stmt(&plain, cur)
                    }
                    Err(e) => Err(format!("line {}: error[E0412]: pipeline(ii = {}) cannot be met: {}", line, ii, e)),
                }
            }
            StmtKind::For { pat, iter, body, .. } => {
                match &iter.kind {
                    // for v in rx: receive until the channel is closed
                    ExprKind::Path(p) if p.len() == 1 && self.chans.contains(&p[0]) => {
                        let ch = p[0].clone();
                        let dst = self.temp("recv", Temp::ChanElem(ch.clone()));
                        let (head, first, exit) = (self.new_block(), self.new_block(), self.new_block());
                        self.blocks[cur].term = Term::Goto(head);
                        self.blocks[head].term = Term::Recv { dst: dst.clone(), ch, closed: Some(exit), next: first };
                        if let Some(b) = bind(pat, path(&dst, line), line) { self.blocks[first].stmts.push(b); }
                        let end = self.stmts(&body.stmts, body.tail.as_deref(), first)?;
                        self.commit(end, line);
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
                        if let Some(b) = bind(pat, path(&i, line), line) { self.blocks[first].stmts.push(b); }
                        let last = self.stmts(&body.stmts, body.tail.as_deref(), first)?;
                        self.commit(last, line);
                        let inc = Expr { kind: ExprKind::Binary(BinOp::Add, Box::new(path(&i, line)), Box::new(Expr { kind: ExprKind::Int(1), line })), line };
                        self.blocks[last].stmts.push(assign(&i, inc, line));
                        self.blocks[last].term = Term::Goto(head);
                        Ok(exit)
                    }
                    // for x in arr (channel operations in the body): an index over the array
                    _ => {
                        let len = Expr { kind: ExprKind::Call(Box::new(path("len", line)), vec![Arg { name: None, value: iter.clone() }]), line };
                        let k = self.temp("k", Temp::Like(Expr { kind: ExprKind::Range(Box::new(Expr { kind: ExprKind::Int(0), line }), Box::new(len.clone()), false), line }));
                        self.blocks[cur].stmts.push(assign(&k, Expr { kind: ExprKind::Int(0), line }, line));
                        let (head, first, exit) = (self.new_block(), self.new_block(), self.new_block());
                        self.blocks[cur].term = Term::Goto(head);
                        let cond = Expr { kind: ExprKind::Binary(BinOp::Lt, Box::new(path(&k, line)), Box::new(len)), line };
                        self.blocks[head].term = Term::Branch(cond, first, exit);
                        let item = Expr { kind: ExprKind::Index(Box::new(iter.clone()), Box::new(path(&k, line))), line };
                        if let Some(b) = bind(pat, item, line) { self.blocks[first].stmts.push(b); }
                        let last = self.stmts(&body.stmts, body.tail.as_deref(), first)?;
                        self.commit(last, line);
                        let inc = Expr { kind: ExprKind::Binary(BinOp::Add, Box::new(path(&k, line)), Box::new(Expr { kind: ExprKind::Int(1), line })), line };
                        self.blocks[last].stmts.push(assign(&k, inc, line));
                        self.blocks[last].term = Term::Goto(head);
                        Ok(exit)
                    }
                }
            }
            StmtKind::Assert { .. } => Ok(cur),
        }
    }
}

impl Lower {
    /// a `pipeline` loop as one `Pipe` state, or why it cannot start an iteration every clock
    fn pipelined(&mut self, pat: &Pattern, iter: &Expr, body: &Block, mut cur: B, line: u32) -> R<B> {
        let (st, exit) = (self.new_block(), self.new_block());
        let (mut head, mut cond) = (None, None);
        let mut step = None;
        match &iter.kind {
            ExprKind::Path(p) if p.len() == 1 && self.chans.contains(&p[0]) => {
                let dst = self.temp("recv", Temp::ChanElem(p[0].clone()));
                if let Some(b) = bind(pat, path(&dst, line), line) { self.blocks[st].stmts.push(b); }
                head = Some((dst, p[0].clone()));
            }
            _ => {
                let (lo, hi, incl, item) = match &iter.kind {
                    ExprKind::Range(lo, hi, incl) => (self.lift(lo, &mut cur)?, self.lift(hi, &mut cur)?, *incl, None),
                    _ => {
                        let len = Expr { kind: ExprKind::Call(Box::new(path("len", line)), vec![Arg { name: None, value: iter.clone() }]), line };
                        (Expr { kind: ExprKind::Int(0), line }, len, false, Some(iter.clone()))
                    }
                };
                if self.pipe.is_some() { return Err("a channel operation in the loop bounds".into()); }
                let i = self.temp("i", Temp::Like(Expr { kind: ExprKind::Range(Box::new(lo.clone()), Box::new(hi.clone()), incl), line }));
                let end = self.temp("end", Temp::Like(hi.clone()));
                self.blocks[cur].stmts.push(assign(&i, lo, line));
                self.blocks[cur].stmts.push(assign(&end, hi, line));
                let op = if incl { BinOp::Le } else { BinOp::Lt };
                cond = Some(Expr { kind: ExprKind::Binary(op, Box::new(path(&i, line)), Box::new(path(&end, line))), line });
                let at = match item { Some(a) => Expr { kind: ExprKind::Index(Box::new(a), Box::new(path(&i, line))), line }, None => path(&i, line) };
                if let Some(b) = bind(pat, at, line) { self.blocks[st].stmts.push(b); }
                step = Some(assign(&i, Expr { kind: ExprKind::Binary(BinOp::Add, Box::new(path(&i, line)), Box::new(Expr { kind: ExprKind::Int(1), line })), line }, line));
            }
        }
        self.blocks[cur].term = Term::Goto(st);
        self.pipe = Some(vec![]);
        let r = self.stmts(&body.stmts, body.tail.as_deref(), st);
        let ops = self.pipe.take().unwrap();
        let last = r?;
        if last != st { return Err("the body has more than one state".into()); }
        self.commit(st, line);
        if let Some(s) = step { self.blocks[st].stmts.push(s); }
        // one token per channel and clock; a receive after a send waits for the send's effect
        let (mut recvs, mut sends) = (vec![], vec![]);
        let mut seen: HashSet<String> = head.iter().map(|(_, c)| c.clone()).collect();
        for op in ops {
            match op {
                PipeOp::Recv(d, c) => {
                    if !sends.is_empty() { return Err(format!("`{}` is received after a send in the same iteration", crate::gen::base(&c))); }
                    if !seen.insert(c.clone()) { return Err(format!("`{}` is received twice in an iteration (one token a clock): use ii = 2", crate::gen::base(&c))); }
                    recvs.push((d, c));
                }
                PipeOp::Send(c, v) => {
                    if sends.iter().any(|(x, _)| *x == c) { return Err(format!("`{}` is sent twice in an iteration (one token a clock): use ii = 2", crate::gen::base(&c))); }
                    sends.push((c, v));
                }
            }
        }
        self.blocks[st].term = Term::Pipe { head, cond, recvs, sends, exit };
        Ok(exit)
    }

    /// the end of a loop iteration: the `@reg` variables' pending values become current
    fn commit(&mut self, b: B, line: u32) {
        for r in self.regs.clone() {
            self.blocks[b].stmts.push(assign(&r, path(&format!("{}~p", r), line), line));
        }
    }
}

fn path(n: &str, line: u32) -> Expr { Expr { kind: ExprKind::Path(vec![n.to_string()]), line } }
/// a loop's pattern bound to the item (nothing for `_`)
fn bind(p: &Pattern, e: Expr, line: u32) -> Option<Stmt> {
    if matches!(p, Pattern::Wild) { return None; }
    Some(Stmt { kind: StmtKind::Let { pat: p.clone(), ty: None, placement: None, init: Some(e) }, line })
}
fn let_stmt(n: &str, e: Expr, line: u32) -> Stmt { Stmt { kind: StmtKind::Let { pat: Pattern::Ident(n.to_string()), ty: None, placement: None, init: Some(e) }, line } }
fn assign(n: &str, e: Expr, line: u32) -> Stmt { Stmt { kind: StmtKind::Assign { target: path(n, line), op: None, value: e }, line } }

/// the task `decl` as states; `outs` are its output channel parameters (closed at the end)
pub fn lower(decl: &FnDecl, consts: &HashSet<String>) -> R<Cfg> {
    let mut rn = Renamer { scopes: vec![HashMap::new()], n: 0, mem: None, consts: consts.clone() };
    let params: Vec<String> = decl.params.iter().map(|p| rn.bind(&p.name)).collect();
    let body = rn.block(&decl.body);
    let mut mems = vec![];
    mems_of(&body, &params, &mut mems);
    if !mems.is_empty() {
        let names: Vec<&str> = mems.iter().map(|m| base(&m.0)).collect();
        let why = match mem_rewrite(&body, &mems) {
            Some(b) => match lower_body(decl, &params, &b, mems.clone()) { Ok(c) => return Ok(c), Err(e) => e },
            None => "it is used other than as m[i]".to_string(),
        };
        let mut c = lower_body(decl, &params, &body, vec![])?;
        c.warnings.push(format!("memory {} stays registers (no block RAM): {}", names.join(", "), why));
        return Ok(c);
    }
    lower_body(decl, &params, &body, vec![])
}

fn base(n: &str) -> &str { crate::gen::base(n) }

fn lower_body(decl: &FnDecl, params: &[String], body: &Block, mems: Vec<MemDecl>) -> R<Cfg> {
    let params = params.to_vec();
    let chans: HashSet<String> = decl.params.iter().zip(&params).filter(|(p, _)| matches!(p.ty, TypeExpr::Chan(..))).map(|(_, u)| u.clone()).collect();
    let mut regs = vec![];
    let body = reg_rewrite(body, &mut regs, false);
    let mut l = Lower { blocks: vec![], chans, regs, temps: vec![], n: 0, end: 0, pipe: None };
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
    Ok(Cfg { blocks: l.blocks, params, temps: l.temps, mems, warnings: vec![] })
}

/// is parameter `name` an output channel (`out chan<T>`, or sent on)?
fn param_dir_out(decl: &FnDecl, name: &str) -> bool { crate::dir_of(decl, name) == Some(true) }

/// a function's parameters and body with fresh names (to inline it), numbered from `start`
pub fn rename_fn(decl: &FnDecl, start: usize, consts: &HashSet<String>) -> (Vec<String>, Block) {
    let mut rn = Renamer { scopes: vec![HashMap::new()], n: start * 1000, mem: None, consts: consts.clone() };
    let params = decl.params.iter().map(|p| rn.bind(&p.name)).collect();
    let body = rn.block(&decl.body);
    (params, body)
}

// ---------------------------------------------------------------- @reg

/// `let x @reg = e` gets a pending copy `x~p`; inside a loop a write to `x` (or a part of it)
/// goes to `x~p`, which the end of each iteration copies to `x` (reads in the iteration see the
/// old value, as the simulator does); outside loops a write goes to both
fn reg_rewrite(b: &Block, regs: &mut Vec<String>, in_loop: bool) -> Block {
    let mut stmts = vec![];
    for s in &b.stmts {
        let line = s.line;
        match &s.kind {
            StmtKind::Let { pat: Pattern::Ident(n), placement: Some(p), .. } if p == "reg" => {
                regs.push(n.clone());
                stmts.push(s.clone());
                stmts.push(let_stmt(&format!("{}~p", n), path(n, line), line));
            }
            StmtKind::Assign { target, op, value } if root(target).is_some_and(|r| regs.contains(&r)) => {
                let r = root(target).unwrap();
                if in_loop {
                    // x op= e reads the pending value
                    let pend = rename_root(target, &format!("{}~p", r));
                    let value = match op { Some(o) => Expr { kind: ExprKind::Binary(*o, Box::new(pend.clone()), Box::new(value.clone())), line }, None => value.clone() };
                    stmts.push(Stmt { kind: StmtKind::Assign { target: pend, op: None, value }, line });
                } else {
                    stmts.push(s.clone());
                    stmts.push(assign(&format!("{}~p", r), path(&r, line), line));
                }
            }
            StmtKind::For { attr, pat, iter, body } => {
                let body = reg_rewrite(body, regs, true);
                stmts.push(Stmt { kind: StmtKind::For { attr: attr.clone(), pat: pat.clone(), iter: iter.clone(), body }, line });
            }
            StmtKind::Expr(e) => stmts.push(Stmt { kind: StmtKind::Expr(reg_expr(e, regs, in_loop)), line }),
            _ => stmts.push(s.clone()),
        }
    }
    let tail = b.tail.as_ref().map(|t| Box::new(reg_expr(t, regs, in_loop)));
    Block { stmts, tail }
}

/// the blocks inside an expression (if / match / block) get the same rewriting
fn reg_expr(e: &Expr, regs: &mut Vec<String>, in_loop: bool) -> Expr {
    let kind = match &e.kind {
        ExprKind::Block(b) => ExprKind::Block(reg_rewrite(b, regs, in_loop)),
        ExprKind::If(c, t, el) => ExprKind::If(c.clone(), reg_rewrite(t, regs, in_loop), el.as_ref().map(|x| Box::new(reg_expr(x, regs, in_loop)))),
        _ => return e.clone(),
    };
    Expr { kind, line: e.line }
}

/// the variable an assignment target writes (`x`, `x[i]`, `x.0`)
fn root(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Path(p) if p.len() == 1 => Some(p[0].clone()),
        ExprKind::Index(b, _) | ExprKind::Field(b, _) => root(b),
        _ => None,
    }
}

fn rename_root(e: &Expr, to: &str) -> Expr {
    let kind = match &e.kind {
        ExprKind::Path(_) => ExprKind::Path(vec![to.to_string()]),
        ExprKind::Index(b, i) => ExprKind::Index(Box::new(rename_root(b, to)), i.clone()),
        ExprKind::Field(b, f) => ExprKind::Field(Box::new(rename_root(b, to)), f.clone()),
        k => k.clone(),
    };
    Expr { kind, line: e.line }
}
