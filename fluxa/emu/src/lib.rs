//! fluxa-emu: a Fluxa program compiled for fast simulation, in the spirit of veryl-emu. The
//! network a task builds is flattened (`elab`); each task (fluxa-synth's `lower`: blocks cut at
//! channel operations) becomes code (`emit`): states of straight-line operations on 64- or
//! 128-bit registers, its variables in a frame, channels ring buffers. The Fluxa simulator stays
//! the reference: `reda emu x.fx` runs a program's tests with its tasks compiled (`Provider`),
//! and every result must be the simulator's.

pub mod elab;
pub mod emit;
pub mod exec;
pub mod ir;
pub mod opt;
pub mod wasm;
pub mod wgen;

#[cfg(not(target_arch = "wasm32"))]
pub mod cli;

use elab::{Arg, Net};
use emit::{leaf_kind, leaves, ParamKind};
use fluxa::ast::{FnDecl, Program, TypeExpr};
use fluxa::interp::Interp;
use fluxa::runtime::{block_on, HwProvider, HwTask, HwTok, Shared};
use fluxa::value::{RtType, Value};
use fluxa_synth::ty::{Ty, R};
use ir::{Chan, Inst, Machine};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// a program with a generic task's const generics as constants
fn specialize(prog: &Arc<Program>, decl: &FnDecl, args: &[(String, Arg)]) -> R<Arc<Program>> {
    if decl.generics.is_empty() { return Ok(prog.clone()); }
    let mut consts = prog.consts.read().unwrap().clone();
    for g in &decl.generics {
        if !g.is_const { return Err(format!("`{}`: type parameter `{}` is not supported by fluxa-emu", decl.name, g.name)); }
        match args.iter().find(|(n, _)| *n == g.name) {
            Some((_, Arg::Val(v))) => { consts.insert(g.name.clone(), v.clone()); }
            _ => return Err(format!("`{}`: generic `{}` needs a value", decl.name, g.name)),
        }
    }
    Ok(Arc::new(Program { files: prog.files.clone(), consts_decl: prog.consts_decl.clone(), fns: prog.fns.clone(), tests: prog.tests.clone(), consts: std::sync::RwLock::new(consts) }))
}

/// a value's scalars, in memory order
fn flatten(v: &Value, out: &mut Vec<i128>) {
    match v {
        Value::Int(n) => out.push(*n),
        Value::Bool(b) => out.push(*b as i128),
        Value::Fixed(r, _) => out.push(*r),
        Value::Array(xs) | Value::Tuple(xs) => for x in xs { flatten(x, out) },
        _ => out.push(0),
    }
}

fn is_scalar(v: &Value) -> bool { matches!(v, Value::Int(_) | Value::Bool(_) | Value::Fixed(..)) }

/// compiles the leaves of networks, one code per task and set of scalar arguments
#[derive(Default)]
pub struct Builder {
    codes: Vec<ir::Code>,
    cache: HashMap<String, usize>,
}

impl Builder {
    /// the machine of network `net`; `ins` / `outs`: its channels to the outside, by name
    pub fn machine(&mut self, prog: &Arc<Program>, net: &Net, ins: Vec<(String, u32)>, outs: Vec<(String, u32)>) -> R<Machine> {
        let mut insts = vec![];
        // each channel's tokens: the type of the ports on it
        let mut chan_ty: Vec<Option<Ty>> = net.chans.iter().map(|c| Ty::of_rt(&c.elem).ok()).collect();
        for leaf in &net.leaves {
            let (prog2, task) = if leaf.task == "$broadcast" {
                let elem = leaf.args.iter().find_map(|(n, a)| if let (true, Arg::Chan(c)) = (n == "rx", a) { Some(*c) } else { None }).ok_or("broadcast: no input")?;
                let outs: Vec<&String> = leaf.args.iter().filter(|(n, _)| n.starts_with("outs")).map(|(n, _)| n).collect();
                let et = rt_text(&net.chans[elem].elem);
                let sends: String = outs.iter().map(|o| format!("{}.send(v); ", o)).collect();
                let ps: String = outs.iter().map(|o| format!(", {}: out chan<{}>", o, et)).collect();
                let src = format!("task Broadcast(rx: in chan<{}>{}) {{ for v in rx {{ {}}} }}", et, ps, sends);
                (fluxa::load_many(&[("broadcast.fx".into(), src)])?, "Broadcast".to_string())
            } else { (prog.clone(), leaf.task.clone()) };
            let decl = prog2.fns.get(&task).cloned().ok_or_else(|| format!("no task `{}`", task))?;
            let p = specialize(&prog2, &decl, &leaf.args)?;
            let mut it = Interp::new(Shared::new(p.clone(), false, true), 0, "emu".into());
            // the parameters: channels (ports), scalars (constants), arrays (data)
            let mut kinds = vec![];
            let mut chans = vec![];
            let mut datas = vec![];
            let mut key = task.clone();
            for prm in &decl.params {
                let a = leaf.args.iter().find(|(n, _)| *n == prm.name).map(|x| &x.1).ok_or_else(|| format!("{}: parameter `{}` has no value", task, prm.name))?;
                match (&prm.ty, a) {
                    (TypeExpr::Chan(t, _), Arg::Chan(c)) => {
                        let rt = block_on(it.eval_type(t)).map_err(|e| format!("{}: {:?}", task, e))?;
                        let ty = match Ty::of_rt(&rt) { Ok(t) => t, Err(_) => chan_ty[*c].clone().ok_or_else(|| format!("{}: channel `{}` needs a type", task, prm.name))? };
                        if chan_ty[*c].is_none() { chan_ty[*c] = Some(ty.clone()); }
                        let out = fluxa_synth::dir_of(&decl, &prm.name) == Some(true);
                        kinds.push((prm.name.clone(), ParamKind::Chan(ty, out)));
                        chans.push(*c as u32);
                    }
                    (_, Arg::Val(v)) if is_scalar(v) => {
                        key += &format!(",{}={}", prm.name, match v { Value::Int(n) => *n, Value::Bool(b) => *b as i128, Value::Fixed(r, _) => *r, _ => 0 });
                        kinds.push((prm.name.clone(), ParamKind::Const(v.clone())));
                    }
                    (t, Arg::Val(v)) => {
                        let rt = block_on(it.eval_type(t)).map_err(|e| format!("{}: {:?}", task, e))?;
                        let ty = Ty::of_rt(&rt).map_err(|m| format!("{}: parameter `{}`: {}", task, prm.name, m))?;
                        kinds.push((prm.name.clone(), ParamKind::Data(ty)));
                        let mut vals = vec![];
                        flatten(v, &mut vals);
                        datas.push((prm.name.clone(), vals));
                    }
                    (_, Arg::Chan(_)) => return Err(format!("{}: `{}` is not a channel parameter", task, prm.name)),
                }
            }
            for g in &decl.generics {
                if let Some((_, Arg::Val(v))) = leaf.args.iter().find(|(n, _)| *n == g.name) { key += &format!(",<{}>={:?}", g.name, v); }
            }
            let ci = match self.cache.get(&key) {
                Some(c) => *c,
                None => {
                    let consts = p.consts.read().unwrap().keys().cloned().collect();
                    let cfg = fluxa_synth::lower::lower(&decl, &consts).map_err(|e| format!("{}: {}", task, e))?;
                    let mut code = emit::compile_task(&p, &decl, &cfg, &kinds).map_err(|e| format!("{}: {}", task, e))?;
                    opt::optimize(&mut code);
                    self.codes.push(code);
                    self.cache.insert(key, self.codes.len() - 1);
                    self.codes.len() - 1
                }
            };
            let code = &self.codes[ci];
            let data = code.data.iter().map(|(n, ..)| datas.iter().find(|d| d.0 == *n).map(|d| d.1.clone()).unwrap_or_default()).collect();
            let name = if leaf.path.is_empty() { task.clone() } else { leaf.path.clone() };
            insts.push(Inst { name, code: ci, chans, data });
        }
        let mut chans = vec![];
        for (k, c) in net.chans.iter().enumerate() {
            let t = chan_ty[k].clone().unwrap_or(Ty::Int { lo: 0, hi: 0 });
            let ls = leaves(&t);
            chans.push(Chan { name: c.name.clone(), depth: c.depth as u32, leaves: ls.iter().map(leaf_kind).collect(), signed: ls.iter().map(|l| l.range().0 < 0).collect() });
        }
        Ok(Machine { codes: self.codes.clone(), insts, chans, ins, outs })
    }
}

/// a channel element type as Fluxa source (for the generated broadcast task)
fn rt_text(t: &RtType) -> String {
    match t {
        RtType::Int { lo, hi } => format!("int<{}..={}>", lo, hi),
        RtType::Bool => "bool".into(),
        RtType::Tuple(ts) => format!("({})", ts.iter().map(rt_text).collect::<Vec<_>>().join(", ")),
        RtType::Array(e, n) => format!("[{}; {}]", rt_text(e), n),
        RtType::Fixed { i, f, .. } => format!("fixed<{}, {}>", i, f),
        _ => "_".into(),
    }
}

/// task `top` of `prog` as a machine: its channel parameters are the outside's (inputs, outputs),
/// the others take `params`
pub fn compile(prog: &Arc<Program>, top: &str, params: &HashMap<String, Value>) -> R<Machine> {
    let decl = prog.fns.get(top).cloned().ok_or_else(|| format!("no task `{}`", top))?;
    let mut net = Net::default();
    let mut it = Interp::new(Shared::new(prog.clone(), false, true), 0, "emu".into());
    let (mut ins, mut outs, mut args) = (vec![], vec![], vec![]);
    for p in &decl.params {
        match &p.ty {
            TypeExpr::Chan(t, _) => {
                let rt = block_on(it.eval_type(t)).map_err(|e| format!("{}: {:?}", top, e))?;
                let c = net.chan(&p.name, rt, elab::param_depth(&mut it, &p.ty));
                if fluxa_synth::dir_of(&decl, &p.name) == Some(true) { outs.push((p.name.clone(), c as u32)); } else { ins.push((p.name.clone(), c as u32)); }
                args.push((p.name.clone(), Arg::Chan(c)));
            }
            _ => match params.get(&p.name) {
                Some(v) => args.push((p.name.clone(), Arg::Val(v.clone()))),
                None => return Err(format!("{}: parameter `{}` needs a value (--param {}=…)", top, p.name, p.name)),
            },
        }
    }
    for g in &decl.generics {
        match params.get(&g.name) { Some(v) => args.push((g.name.clone(), Arg::Val(v.clone()))), None => return Err(format!("{}: generic `{}` needs a value (--param {}=…)", top, g.name, g.name)) }
    }
    net.expand(prog, top, "", args, 0)?;
    Builder::default().machine(prog, &net, ins, outs)
}

// ---------------------------------------------------------------- in the simulator

/// tasks run compiled in the Fluxa simulator (`reda emu x.fx`): a spawn of one of them runs its
/// network on the executor instead of the interpreter
pub struct Provider {
    prog: Arc<Program>,
    /// only this task (else every task that compiles)
    only: Option<String>,
    /// what was compiled: task → instances, or the error
    pub report: Mutex<Vec<(String, Result<String, String>)>>,
}

impl Provider {
    pub fn new(prog: &Arc<Program>, only: Option<&str>) -> Provider { Provider { prog: prog.clone(), only: only.map(|s| s.to_string()), report: Mutex::new(vec![]) } }

    fn note(&self, task: &str, r: Result<String, String>) {
        let mut rep = self.report.lock().unwrap();
        if !rep.iter().any(|(t, _)| t == task) { rep.push((task.to_string(), r)); }
    }
}

struct Running { ex: exec::Exec, ins: Vec<u32>, outs: Vec<u32>, closed_sent: Vec<bool>, rec: Option<Record> }

/// FLUXA_EMU_RECORD=DIR: each machine's module and the tokens that went in and out
/// (`js/replay.mjs DIR` feeds the inputs to the WebAssembly module and compares the outputs)
struct Record { path: String, ins: Vec<Vec<String>>, outs: Vec<Vec<String>> }

impl Drop for Record {
    fn drop(&mut self) {
        let list = |v: &Vec<Vec<String>>| v.iter().map(|s| format!("[{}]", s.join(","))).collect::<Vec<_>>().join(",");
        let _ = std::fs::write(&self.path, format!("{{\"ins\":[{}],\"outs\":[{}]}}\n", list(&self.ins), list(&self.outs)));
    }
}

fn tok_text(vs: &[i128]) -> String { format!("[{}]", vs.iter().map(|v| format!("\"{}\"", v)).collect::<Vec<_>>().join(",")) }

impl HwTask for Running {
    fn cycle(&mut self, offer: &[Option<HwTok>], room: &[bool]) -> (Vec<bool>, Vec<Option<HwTok>>, bool) {
        let mut taken = vec![false; offer.len()];
        for (k, o) in offer.iter().enumerate() {
            let c = self.ins[k];
            match o {
                Some(HwTok::Close) => { if !self.ex.closed(c) { self.ex.close(c); taken[k] = true; if let Some(r) = &mut self.rec { r.ins[k].push("\"close\"".into()); } } }
                Some(HwTok::Data(v)) if self.ex.room(c) => { self.ex.push(c, &[*v]); taken[k] = true; if let Some(r) = &mut self.rec { r.ins[k].push(tok_text(&[*v])); } }
                Some(HwTok::Tuple(vs)) if self.ex.room(c) => { self.ex.push(c, vs); taken[k] = true; if let Some(r) = &mut self.rec { r.ins[k].push(tok_text(vs)); } }
                _ => {}
            }
        }
        let moved = self.ex.run(1 << 20);
        let mut sent = vec![None; room.len()];
        for (k, r) in room.iter().enumerate() {
            if !*r { continue; }
            let c = self.outs[k];
            match self.ex.pop(c) {
                Some(vs) => sent[k] = Some({ if let Some(r) = &mut self.rec { r.outs[k].push(tok_text(&vs)); }
                    if vs.len() == 1 && self.ex.m.chans[c as usize].leaves.len() == 1 { HwTok::Data(vs[0]) } else { HwTok::Tuple(vs) } }),
                None => if self.ex.closed(c) && !self.closed_sent[k] { sent[k] = Some(HwTok::Close); self.closed_sent[k] = true; },
            }
        }
        (taken, sent, moved)
    }
    fn error(&self) -> Option<String> { self.ex.error.clone() }
}

static RECORDED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl HwProvider for Provider {
    fn start(&self, task: &str, args: &[(String, Value)]) -> Option<Result<Box<dyn HwTask>, String>> {
        if self.only.as_deref().is_some_and(|o| o != task) { return None; }
        let decl = self.prog.fns.get(task)?.clone();
        let mut net = Net::default();
        let mut it = Interp::new(Shared::new(self.prog.clone(), false, true), 0, "emu".into());
        let (mut ins, mut outs, mut a) = (vec![], vec![], vec![]);
        for (n, v) in args {
            match v {
                Value::Rx(_) | Value::Tx(_) => {
                    let p = decl.params.iter().find(|p| p.name == *n);
                    let (rt, depth) = match p.map(|p| &p.ty) {
                        Some(t @ TypeExpr::Chan(e, _)) => (block_on(it.eval_type(e)).unwrap_or(RtType::Any), elab::param_depth(&mut it, t)),
                        _ => (RtType::Any, 2),
                    };
                    let c = net.chan(n, rt, depth);
                    if matches!(v, Value::Rx(_)) { ins.push((n.clone(), c as u32)); } else { outs.push((n.clone(), c as u32)); }
                    a.push((n.clone(), Arg::Chan(c)));
                }
                v => a.push((n.clone(), Arg::Val(v.clone()))),
            }
        }
        let r = net.expand(&self.prog, task, "", a, 0).and_then(|_| Builder::default().machine(&self.prog, &net, ins, outs));
        match r {
            Ok(m) => {
                if let Ok(d) = std::env::var("FLUXA_EMU_DUMP") { for c in &m.codes { let _ = std::fs::write(format!("{}/{}.txt", d, c.name), ir::dump(c)); } }
                self.note(task, Ok(m.stats()));
                let ins = m.ins.iter().map(|x| x.1).collect();
                let outs: Vec<u32> = m.outs.iter().map(|x| x.1).collect();
                let n = outs.len();
                let rec = std::env::var("FLUXA_EMU_RECORD").ok().map(|dir| {
                    let k = RECORDED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let w = wgen::compile(&m);
                    let _ = std::fs::write(format!("{}/m{}_{}.wasm", dir, std::process::id(), k), &w.wasm);
                    let _ = std::fs::write(format!("{}/m{}_{}.json", dir, std::process::id(), k), &w.json);
                    Record { path: format!("{}/m{}_{}.streams.json", dir, std::process::id(), k), ins: vec![vec![]; m.ins.len()], outs: vec![vec![]; n] }
                });
                Some(Ok(Box::new(Running { ex: exec::Exec::new(m), ins, outs, closed_sent: vec![false; n], rec })))
            }
            Err(e) => {
                self.note(task, Err(e.clone()));
                if self.only.is_some() { Some(Err(e)) } else { None }
            }
        }
    }
}
