//! A synthesized task as a stream processor: its netlist in eda's gate simulator, driven through
//! its valid / ready ports one clock at a time. Tests and the co-simulation (the Fluxa tests run
//! with the task replaced by its gates) use it.

use crate::ty::Ty;
use eda::netlist::{Bit, Netlist};
use eda::netsim::NetSim;
use fluxa::runtime::{HwProvider, HwTask};
use fluxa::value::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// a token on a channel: a value, or the end of the stream
pub use fluxa::runtime::HwTok as Tok;

/// a synthesized task's ports, by parameter
#[derive(Clone, Debug, Default)]
pub struct Iface {
    pub ins: Vec<(String, Ty)>,
    pub outs: Vec<(String, Ty)>,
    /// scalar parameters that are input ports (not fixed with `--param`)
    pub scalars: Vec<(String, Ty)>,
    /// memories whose initial words are a parameter given at run time: (parameter, memory net, type)
    pub mem_inits: Vec<(String, String, Ty)>,
}

pub struct HwSim<'n> {
    sim: NetSim<'n>,
    io: &'n Iface,
    /// the flip-flops' outputs: a cycle that changes none of them, with no handshake, is idle
    state: Vec<Bit>,
    /// whether the last cycle changed the state
    pub changed: bool,
    /// clock cycles run since the reset
    pub cycles: usize,
    /// memories to load (`Iface::mem_inits`) that the netlist does not have
    pub missing: Vec<String>,
}

/// a scalar as `t`'s bits (two's complement)
fn int_bits(v: i128, t: &Ty) -> Vec<bool> { (0..t.width()).map(|i| if i < 128 { (v >> i) & 1 == 1 } else { v < 0 }).collect() }

fn bits_int(b: &[bool], t: &Ty) -> i128 {
    let mut v: i128 = 0;
    for (i, x) in b.iter().enumerate().take(128) { if *x { v |= 1i128 << i; } }
    let w = b.len();
    if t.range().0 < 0 && w > 0 && w < 128 && b[w - 1] { v - (1i128 << w) } else { v }
}

/// a token's bits on a channel of type `t` (a tuple's first element in the low bits)
fn tok_bits(tok: &Tok, t: &Ty) -> Vec<bool> {
    match (tok, t) {
        (Tok::Tuple(xs), Ty::Tuple(ts)) => xs.iter().zip(ts).flat_map(|(x, t)| int_bits(*x, t)).collect(),
        (Tok::Data(v), t) => int_bits(*v, t),
        _ => vec![false; t.width()],
    }
}

fn bits_tok(b: &[bool], t: &Ty) -> Tok {
    match t {
        Ty::Tuple(ts) => {
            let mut at = 0;
            Tok::Tuple(ts.iter().map(|t| { let w = t.width(); let v = bits_int(&b[at..at + w], t); at += w; v }).collect())
        }
        t => Tok::Data(bits_int(b, t)),
    }
}

/// an array of scalars as memory words (each word `t`'s bits, low first)
fn value_words(v: &Value, t: &Ty) -> Option<Vec<u128>> {
    let (Value::Array(xs), Ty::Array(e, n)) = (v, t) else { return None };
    if xs.len() != *n { return None; }
    xs.iter().map(|x| value_bits(x, e).map(|b| b.iter().enumerate().take(128).fold(0u128, |a, (i, y)| a | ((*y as u128) << i)))).collect()
}

/// a parameter's value as `t`'s bits (scalars, arrays, tuples)
pub fn value_bits(v: &Value, t: &Ty) -> Option<Vec<bool>> {
    match (v, t) {
        (Value::Int(k), t) => Some(int_bits(*k, t)),
        (Value::Bool(b), t) => Some(int_bits(*b as i128, t)),
        (Value::Fixed(r, _), t) => Some(int_bits(*r, t)),
        (Value::Array(xs), Ty::Array(e, n)) if xs.len() == *n => xs.iter().map(|x| value_bits(x, e)).collect::<Option<Vec<_>>>().map(|v| v.concat()),
        (Value::Tuple(xs), Ty::Tuple(ts)) if xs.len() == ts.len() => xs.iter().zip(ts).map(|(x, t)| value_bits(x, t)).collect::<Option<Vec<_>>>().map(|v| v.concat()),
        _ => None,
    }
}

impl<'n> HwSim<'n> {
    /// the circuit after a reset cycle, with the scalar inputs set
    pub fn new(nl: &'n Netlist, io: &'n Iface, scalars: &[i128]) -> HwSim<'n> {
        let bits: Vec<Vec<bool>> = io.scalars.iter().zip(scalars).map(|((_, t), v)| int_bits(*v, t)).collect();
        HwSim::with_inputs(nl, io, &bits, &[])
    }

    /// the same, the parameter inputs (arrays too) given as bits, and the words of the memories
    /// whose initial contents are given at run time (`Iface::mem_inits`; missing: all 0)
    pub fn with_inputs(nl: &'n Netlist, io: &'n Iface, inputs: &[Vec<bool>], mems: &[Vec<u128>]) -> HwSim<'n> {
        let mut sim = NetSim::new(nl);
        for ((n, _), v) in io.scalars.iter().zip(inputs) { sim.poke_bits(n, v); }
        let mut missing = vec![];
        for ((_, m, _), w) in io.mem_inits.iter().zip(mems) { if sim.load_mem(m, w) == 0 { missing.push(m.clone()); } }
        sim.set("rst", 1);
        sim.step();
        sim.set("rst", 0);
        let state = nl.cells.iter().filter(|c| c.is_ff()).map(|c| c.out()).collect();
        HwSim { sim, io, state, changed: true, cycles: 0, missing }
    }

    /// one clock: `offer` is the token each input channel offers (None: nothing),
    /// `room` whether each output channel can take a token. Returns which input tokens were
    /// taken and what each output channel sent.
    pub fn cycle(&mut self, offer: &[Option<Tok>], room: &[bool]) -> (Vec<bool>, Vec<Option<Tok>>) {
        for ((n, t), o) in self.io.ins.iter().zip(offer) {
            let (valid, close, data) = match o { Some(Tok::Close) => (1, 1, vec![]), Some(d) => (1, 0, tok_bits(d, t)), None => (0, 0, vec![]) };
            self.sim.poke(&format!("{}_valid", n), valid);
            self.sim.poke(&format!("{}_close", n), close);
            self.sim.poke_bits(&format!("{}_data", n), &data);
        }
        for ((n, _), r) in self.io.outs.iter().zip(room) { self.sim.poke(&format!("{}_ready", n), *r as u128); }
        self.sim.settle();
        let taken: Vec<bool> = self.io.ins.iter().zip(offer).map(|((n, _), o)| o.is_some() && self.sim.get(&format!("{}_ready", n)) == 1).collect();
        let sent: Vec<Option<Tok>> = self.io.outs.iter().zip(room).map(|((n, t), r)| {
            if !*r || self.sim.get(&format!("{}_valid", n)) == 0 { return None; }
            Some(if self.sim.get(&format!("{}_close", n)) == 1 { Tok::Close } else { bits_tok(&self.sim.get_bits(&format!("{}_data", n)), t) })
        }).collect();
        let before: Vec<bool> = self.state.iter().map(|b| self.sim.bit(*b)).collect();
        self.sim.step();
        self.sim.settle();
        self.cycles += 1;
        self.changed = self.state.iter().zip(&before).any(|(b, v)| self.sim.bit(*b) != *v);
        (taken, sent)
    }

    /// feed every input stream (ending in Close) and collect the outputs, the outputs always
    /// ready, until every output is closed or `max` cycles pass
    pub fn run_streams(&mut self, inputs: &[Vec<Tok>], max: usize) -> Vec<Vec<Tok>> {
        let mut pos = vec![0usize; inputs.len()];
        let mut outs: Vec<Vec<Tok>> = vec![vec![]; self.io.outs.len()];
        for _ in 0..max {
            let offer: Vec<Option<Tok>> = inputs.iter().zip(&pos).map(|(s, p)| s.get(*p).cloned()).collect();
            let room = vec![true; self.io.outs.len()];
            let (taken, sent) = self.cycle(&offer, &room);
            for (p, t) in pos.iter_mut().zip(taken) { if t { *p += 1; } }
            for (o, s) in outs.iter_mut().zip(sent) { if let Some(t) = s { o.push(t); } }
            if outs.iter().all(|o| o.last() == Some(&Tok::Close)) { break; }
        }
        outs
    }
}

/// a task's netlist and ports, kept for the whole run
type Built = (&'static Netlist, &'static Iface);

/// tasks replaced by their synthesized netlists in the Fluxa simulator (`reda gatesim KIND x.fx`):
/// a spawn of one of them runs its netlist instead of the interpreter. Each task is synthesized
/// (scalar parameters as input ports) and mapped to `kind` (opt, ice40, ecp5, asic, sky130) at
/// its first spawn.
pub struct Provider {
    prog: Arc<fluxa::ast::Program>,
    kind: String,
    /// only this task (else every task)
    only: Option<String>,
    built: std::sync::Mutex<HashMap<String, Result<Built, String>>>,
}

impl Provider {
    /// `top` as `kind` gates (synthesized now, so errors show at once)
    pub fn new(prog: &Arc<fluxa::ast::Program>, top: &str, kind: &str) -> Result<Provider, String> {
        let p = Provider { prog: prog.clone(), kind: kind.into(), only: Some(top.into()), built: Default::default() };
        // a generic task is built at its spawn, for its generics' values
        if prog.fns.get(top).is_none_or(|d| d.generics.is_empty()) { p.netlist(top)?; }
        Ok(p)
    }

    /// every task as `kind` gates
    pub fn all(prog: &Arc<fluxa::ast::Program>, kind: &str) -> Provider {
        Provider { prog: prog.clone(), kind: kind.into(), only: None, built: Default::default() }
    }

    fn netlist(&self, task: &str) -> Result<(&'static Netlist, &'static Iface), String> { self.netlist_with(task, task, &Default::default()) }

    /// `task` with scalar parameters `params` (a network's are constants), cached as `key`
    fn netlist_with(&self, key: &str, task: &str, params: &crate::Params) -> Result<(&'static Netlist, &'static Iface), String> {
        let mut b = self.built.lock().unwrap();
        if let Some(r) = b.get(key) { return r.clone(); }
        let r = crate::synthesize(&self.prog, task, params).and_then(|s| {
            let nl = eda::netsim::mapped(&s.netlist, &self.kind)?;
            // one netlist for the whole run: every spawn of the task simulates it
            Ok((&*Box::leak(Box::new(nl)), &*Box::leak(Box::new(s.iface))))
        });
        b.insert(key.to_string(), r.clone());
        r
    }

    /// the tasks synthesized so far: (name, cells) or why not
    pub fn report(&self) -> Vec<(String, Result<usize, String>)> {
        let mut v: Vec<(String, Result<usize, String>)> = self.built.lock().unwrap().iter().map(|(k, r)| (k.clone(), r.as_ref().map(|(n, _)| n.cells.len()).map_err(|e| e.clone()))).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
}

struct Running(HwSim<'static>);

impl HwTask for Running {
    fn cycle(&mut self, offer: &[Option<Tok>], room: &[bool]) -> (Vec<bool>, Vec<Option<Tok>>, bool) {
        let (t, s) = self.0.cycle(offer, room);
        (t, s, self.0.changed)
    }
}

impl HwProvider for Provider {
    fn start(&self, task: &str, args: &[(String, Value)]) -> Option<Result<Box<dyn HwTask>, String>> {
        if self.only.as_deref().is_some_and(|o| o != task) { return None; }
        let decl = self.prog.fns.get(task)?;
        // a network has its scalar arguments built in, a generic task its const generics: one
        // netlist per set of them
        let net = crate::graph::is_graph(decl);
        let fixed = |n: &str| net || decl.generics.iter().any(|g| g.name == n);
        let ps: crate::Params = args.iter().filter(|(n, _)| fixed(n)).filter(|(_, v)| matches!(v, Value::Int(_) | Value::Bool(_))).cloned().collect();
        let built = if ps.is_empty() { self.netlist(task) } else {
            let mut key: Vec<String> = ps.iter().map(|(k, v)| format!("{}={}", k, match v { Value::Bool(b) => *b as i128, Value::Int(k) => *k, _ => 0 })).collect();
            key.sort();
            self.netlist_with(&format!("{}({})", task, key.join(",")), task, &ps)
        };
        let (nl, io) = match built {
            Ok(x) => x,
            // replacing one task: its error; every task: the ones that do not synthesize stay in the interpreter
            Err(e) => return if self.only.is_some() { Some(Err(e)) } else { None },
        };
        let mut inputs = vec![];
        for (n, t) in &io.scalars {
            match args.iter().find(|(a, _)| a == n).and_then(|(_, v)| value_bits(v, t)) {
                Some(b) => inputs.push(b),
                None => return Some(Err(format!("hardware `{}`: parameter `{}` needs a value of type {:?}", task, n, t))),
            }
        }
        let mut mems = vec![];
        for (n, m, t) in &io.mem_inits {
            match args.iter().find(|(a, _)| a == n).and_then(|(_, v)| value_words(v, t)) {
                Some(w) => mems.push(w),
                None => return Some(Err(format!("hardware `{}`: memory `{}` needs parameter `{}` of type {:?}", task, m, n, t))),
            }
        }
        let sim = HwSim::with_inputs(nl, io, &inputs, &mems);
        // the memory must be found in the netlist of this kind (flip-flops or block RAMs)
        if let Some(m) = sim.missing.first() { return Some(Err(format!("hardware `{}`: memory `{}` is not in the {} netlist", task, m, self.kind))); }
        Some(Ok(Box::new(Running(sim))))
    }
}
