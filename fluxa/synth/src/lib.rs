//! Fluxa synthesis: a `task` (with the tasks it spawns, later) as eda's gate netlist, so the
//! FPGA and ASIC flows of `eda` take Fluxa programs as they take Veryl ones.
//!
//! A task becomes an FSM with a datapath (`lower`: states, `gen`: gates); its channel parameters
//! become valid / ready stream ports, its other parameters inputs (or constants). The plan:
//! fluxa/docs/synthesis-plan.md.

pub mod gen;
pub mod graph;
pub mod hw;
pub mod lower;
pub mod ty;

#[cfg(not(target_arch = "wasm32"))]
pub mod cli;

use eda::netlist::Netlist;
use fluxa::ast::{Block, Expr, ExprKind, FnDecl, FnKind, Program, Stmt, StmtKind};
use std::collections::HashMap;
use std::sync::Arc;

/// a synthesized task
pub struct Synth {
    pub netlist: Netlist,
    /// its stream and scalar ports, by parameter
    pub iface: hw::Iface,
    pub warnings: Vec<String>,
    /// the task and, for a network, its instances: (path, task); the task itself is ("", name)
    pub insts: Vec<(String, String)>,
}

/// synthesize task `top` of `prog`; `params` fixes scalar parameters (else they are inputs)
pub fn synthesize(prog: &Arc<Program>, top: &str, params: &HashMap<String, i128>) -> Result<Synth, String> {
    let decl = prog.fns.get(top).ok_or_else(|| format!("no task `{}`", top))?.clone();
    if decl.kind != FnKind::Task { return Err(format!("`{}` is not a task", top)); }
    if !decl.generics.is_empty() { return Err(format!("`{}`: generic tasks are not synthesized yet", top)); }
    // a task that builds a network: its instances connected by FIFOs
    if graph::is_graph(&decl) { return graph::build(prog, &decl, params); }
    let consts: std::collections::HashSet<String> = prog.consts.read().unwrap().keys().cloned().collect();
    let cfg = lower::lower(&decl, &consts)?;
    gen::build(prog, &decl, &cfg, params)
}

/// the task synthesized by default: the last one declared
pub fn default_top(prog: &Program) -> Option<String> {
    prog.fns.values().filter(|d| d.kind == FnKind::Task).max_by_key(|d| d.line).map(|d| d.name.clone())
}

/// the tasks of a program, in declaration order
pub fn tasks(prog: &Program) -> Vec<String> {
    let mut t: Vec<&Arc<FnDecl>> = prog.fns.values().filter(|d| d.kind == FnKind::Task).collect();
    t.sort_by_key(|d| d.line);
    t.into_iter().map(|d| d.name.clone()).collect()
}

/// is channel parameter `name` of `decl` an output (Some(true)) or an input (Some(false))?
/// The `in` / `out` mark, else how the body uses it (send / close: output; recv, `for v in c`: input)
pub fn dir_of(decl: &FnDecl, name: &str) -> Option<bool> {
    let p = decl.params.iter().find(|p| p.name == name)?;
    if p.out.is_some() { return p.out; }
    let mut dir = None;
    let mut look = |e: &Expr| {
        match &e.kind {
            ExprKind::Method(r, m, _, _) if matches!(&r.kind, ExprKind::Path(x) if x.len() == 1 && x[0] == name) => {
                if m == "send" || m == "close" { dir = Some(true) } else if m == "recv" && dir.is_none() { dir = Some(false) }
            }
            _ => {}
        }
    };
    walk_block(&decl.body, &mut look);
    if dir.is_none() {
        // `for v in name`
        fn fors(b: &Block, name: &str) -> bool {
            b.stmts.iter().any(|s| match &s.kind {
                StmtKind::For { iter, body, .. } => matches!(&iter.kind, ExprKind::Path(x) if x.len() == 1 && x[0] == name) || fors(body, name),
                _ => false,
            })
        }
        if fors(&decl.body, name) { dir = Some(false); }
    }
    dir
}

/// every expression of a block (statements and nested blocks)
pub fn walk_block(b: &Block, f: &mut dyn FnMut(&Expr)) {
    for s in &b.stmts { walk_stmt(s, f); }
    if let Some(t) = &b.tail { walk(t, f); }
}

fn walk_stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Let { init: Some(e), .. } | StmtKind::Carry { init: e, .. } | StmtKind::Expr(e) | StmtKind::Spawn(e) => walk(e, f),
        StmtKind::Assign { target, value, .. } => { walk(target, f); walk(value, f); }
        StmtKind::For { iter, body, .. } => { walk(iter, f); walk_block(body, f); }
        StmtKind::Parallel(ss) => for s in ss { walk_stmt(s, f) },
        StmtKind::Assert { cond, .. } => walk(cond, f),
        StmtKind::Return(Some(e)) => walk(e, f),
        _ => {}
    }
}

/// an expression and all its sub-expressions
pub fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Field(x, _) => walk(x, f),
        ExprKind::Binary(_, l, r) | ExprKind::Index(l, r) | ExprKind::Range(l, r, _) | ExprKind::Repeat(l, r) => { walk(l, f); walk(r, f); }
        ExprKind::Slice(x, h, l) => { walk(x, f); walk(h, f); walk(l, f); }
        ExprKind::Call(c, a) => { walk(c, f); for x in a { walk(&x.value, f); } }
        ExprKind::Method(r, _, _, a) => { walk(r, f); for x in a { walk(&x.value, f); } }
        ExprKind::Array(xs) | ExprKind::Tuple(xs) => for x in xs { walk(x, f) },
        ExprKind::Lambda(_, b) => walk(b, f),
        ExprKind::Block(b) | ExprKind::Comptime(b) => walk_block(b, f),
        ExprKind::If(c, t, el) => { walk(c, f); walk_block(t, f); if let Some(x) = el { walk(x, f); } }
        ExprKind::Match(s, arms) => { walk(s, f); for a in arms { if let Some(g) = &a.guard { walk(g, f); } walk(&a.body, f); } }
        _ => {}
    }
}

/// every variable or constant name an expression reads
pub fn walk_paths(e: &Expr, f: &mut dyn FnMut(&str)) {
    walk(e, &mut |x| if let ExprKind::Path(p) = &x.kind { if p.len() == 1 { f(&p[0]) } });
}
