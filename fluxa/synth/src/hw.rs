//! A synthesized task as a stream processor: its netlist in eda's gate simulator, driven through
//! its valid / ready ports one clock at a time. Tests and the co-simulation (the Fluxa tests run
//! with the task replaced by its gates) use it.

use crate::ty::Ty;
use eda::netlist::{Bit, Netlist};
use eda::netsim::NetSim;
use fluxa::runtime::{HwProvider, HwTask};
use fluxa::value::Value;
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
}

pub struct HwSim<'n> {
    sim: NetSim<'n>,
    io: &'n Iface,
    /// the flip-flops' outputs: a cycle that changes none of them, with no handshake, is idle
    state: Vec<Bit>,
    /// whether the last cycle changed the state
    pub changed: bool,
}

fn to_bits(v: i128, t: Ty) -> u128 {
    let w = t.width();
    let m = if w >= 128 { u128::MAX } else { (1u128 << w) - 1 };
    (v as u128) & m
}

fn from_bits(b: u128, t: Ty) -> i128 {
    let w = t.width() as u32;
    let (lo, _) = t.range();
    if lo < 0 && w < 128 && (b >> (w - 1)) & 1 == 1 { (b as i128) - (1i128 << w) } else { b as i128 }
}

impl<'n> HwSim<'n> {
    /// the circuit after a reset cycle, with the scalar inputs set
    pub fn new(nl: &'n Netlist, io: &'n Iface, scalars: &[i128]) -> HwSim<'n> {
        let mut sim = NetSim::new(nl);
        for ((n, t), v) in io.scalars.iter().zip(scalars) { sim.poke(n, to_bits(*v, *t)); }
        sim.set("rst", 1);
        sim.step();
        sim.set("rst", 0);
        let state = nl.cells.iter().filter(|c| c.is_ff()).map(|c| c.out()).collect();
        HwSim { sim, io, state, changed: true }
    }

    /// one clock: `offer` is the token each input channel offers (None: nothing),
    /// `room` whether each output channel can take a token. Returns which input tokens were
    /// taken and what each output channel sent.
    pub fn cycle(&mut self, offer: &[Option<Tok>], room: &[bool]) -> (Vec<bool>, Vec<Option<Tok>>) {
        for ((n, t), o) in self.io.ins.iter().zip(offer) {
            let (valid, close, data) = match o { Some(Tok::Data(v)) => (1, 0, to_bits(*v, *t)), Some(Tok::Close) => (1, 1, 0), None => (0, 0, 0) };
            self.sim.poke(&format!("{}_valid", n), valid);
            self.sim.poke(&format!("{}_close", n), close);
            self.sim.poke(&format!("{}_data", n), data);
        }
        for ((n, _), r) in self.io.outs.iter().zip(room) { self.sim.poke(&format!("{}_ready", n), *r as u128); }
        self.sim.settle();
        let taken: Vec<bool> = self.io.ins.iter().zip(offer).map(|((n, _), o)| o.is_some() && self.sim.get(&format!("{}_ready", n)) == 1).collect();
        let sent: Vec<Option<Tok>> = self.io.outs.iter().zip(room).map(|((n, t), r)| {
            if !*r || self.sim.get(&format!("{}_valid", n)) == 0 { return None; }
            Some(if self.sim.get(&format!("{}_close", n)) == 1 { Tok::Close } else { Tok::Data(from_bits(self.sim.get(&format!("{}_data", n)), *t)) })
        }).collect();
        let before: Vec<bool> = self.state.iter().map(|b| self.sim.bit(*b)).collect();
        self.sim.step();
        self.sim.settle();
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

/// a task replaced by its synthesized netlist in the Fluxa simulator (`reda gatesim KIND x.fx`):
/// every `spawn` of `top` runs the netlist instead of the interpreter
pub struct Provider {
    top: String,
    nl: &'static Netlist,
    io: &'static Iface,
    /// cells in the netlist (for the report)
    pub cells: usize,
}

impl Provider {
    /// synthesize `top` (scalar parameters as input ports) and map it to `kind`
    /// (opt, ice40, ecp5, asic, sky130)
    pub fn new(prog: &Arc<fluxa::ast::Program>, top: &str, kind: &str) -> Result<Provider, String> {
        let s = crate::synthesize(prog, top, &Default::default())?;
        let nl = eda::netsim::mapped(&s.netlist, kind)?;
        let cells = nl.cells.len();
        // one netlist for the whole run: every spawn of the task simulates it
        let nl: &'static Netlist = Box::leak(Box::new(nl));
        let io: &'static Iface = Box::leak(Box::new(s.iface));
        Ok(Provider { top: top.to_string(), nl, io, cells })
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
        if task != self.top { return None; }
        let mut scalars = vec![];
        for (n, _) in &self.io.scalars {
            match args.iter().find(|(a, _)| a == n).map(|(_, v)| v) {
                Some(Value::Int(k)) => scalars.push(*k),
                Some(Value::Bool(b)) => scalars.push(*b as i128),
                _ => return Some(Err(format!("hardware `{}`: parameter `{}` needs an integer", task, n))),
            }
        }
        Some(Ok(Box::new(Running(HwSim::new(self.nl, self.io, &scalars)))))
    }
}
