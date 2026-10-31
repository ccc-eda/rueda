//! Clean-ups of the emitter's code: operations whose results nothing reads are removed (a
//! state loads every variable it may use, a branch not taken may have computed values for
//! nothing), and `if`s left empty go too.

use crate::ir::*;
use std::collections::HashSet;

/// the registers an operation reads
fn uses(o: &Op, out: &mut Vec<Reg>) {
    let addr = |a: &Addr, out: &mut Vec<Reg>| if let Some(r) = a.off { out.push(r) };
    match o {
        Op::Const(..) | Op::Trap(_) => {}
        Op::Mov(_, s) | Op::Ext(_, s, _) | Op::Low(_, s) => out.push(*s),
        Op::Load(_, a) => addr(a, out),
        Op::Store(a, s) => { addr(a, out); out.push(*s); }
        Op::Bin(_, _, a, b) => { out.push(*a); out.push(*b); }
        Op::Sel(_, c, a, b) => { out.push(*c); out.push(*a); out.push(*b); }
        Op::If(c, ..) => out.push(*c),
        Op::Copy(d, s, _) => { addr(d, out); addr(s, out); }
    }
}

/// the register an operation sets (None: it has effects instead)
fn def(o: &Op) -> Option<Reg> {
    match o {
        Op::Const(d, _) | Op::Mov(d, _) | Op::Load(d, _) | Op::Bin(_, d, _, _) | Op::Ext(d, _, _) | Op::Low(d, _) | Op::Sel(d, _, _, _) => Some(*d),
        _ => None,
    }
}

/// remove dead operations from `ops`, given the registers live after them; returns those live before
fn dce(ops: &mut Vec<Op>, live_out: &HashSet<Reg>) -> HashSet<Reg> {
    let mut live = live_out.clone();
    let mut keep = vec![true; ops.len()];
    for k in (0..ops.len()).rev() {
        if let Op::If(c, t, f) = &mut ops[k] {
            let lt = dce(t, &live);
            let lf = dce(f, &live);
            if t.is_empty() && f.is_empty() { keep[k] = false; continue; }
            let c = *c;
            live = lt;
            live.extend(lf);
            live.insert(c);
            continue;
        }
        if let Some(d) = def(&ops[k]) {
            if !live.contains(&d) { keep[k] = false; continue; }
            live.remove(&d);
        }
        let mut u = vec![];
        uses(&ops[k], &mut u);
        live.extend(u);
    }
    let mut k = 0;
    ops.retain(|_| { k += 1; keep[k - 1] });
    live
}

pub fn optimize(code: &mut Code) {
    for s in &mut code.states {
        let mut live = HashSet::new();
        if let Term::Branch(c, ..) = s.term { live.insert(c); }
        dce(&mut s.ops, &live);
    }
}
