//! The network a task builds, flattened: the leaf instances (tasks that do not spawn) with their
//! arguments, and the channels between them. As in fluxa-synth's `graph`, a task that spawns is
//! run in the simulator with a recording hardware provider: every `spawn` (and `broadcast`) is
//! recorded instead of started, so the network is exactly the one the simulator builds.

use fluxa::ast::{Program, TypeExpr};
use fluxa::interp::Interp;
use fluxa::runtime::{block_on, HwProvider, HwTask, HwTok, RtErr, Shared};
use fluxa::value::{RtType, Value};
use fluxa_synth::ty::R;
use std::sync::{Arc, Mutex};

/// an argument: a channel of the network, or a value
#[derive(Clone, Debug)]
pub enum Arg { Chan(usize), Val(Value) }

/// a task that does not spawn, with its arguments (parameters in order, then const generics)
#[derive(Clone, Debug)]
pub struct Leaf { pub task: String, pub path: String, pub args: Vec<(String, Arg)> }

#[derive(Clone, Debug)]
pub struct NetChan { pub name: String, pub elem: RtType, pub depth: usize }

#[derive(Clone, Debug, Default)]
pub struct Net {
    pub leaves: Vec<Leaf>,
    pub chans: Vec<NetChan>,
}

type Spawn = (String, Vec<(String, Value)>);

#[derive(Default)]
struct Recorder { spawns: Mutex<Vec<Spawn>> }

struct Idle;
impl HwTask for Idle {
    fn cycle(&mut self, offer: &[Option<HwTok>], room: &[bool]) -> (Vec<bool>, Vec<Option<HwTok>>, bool) { (vec![false; offer.len()], vec![None; room.len()], false) }
}

impl HwProvider for Recorder {
    fn start(&self, task: &str, args: &[(String, Value)]) -> Option<Result<Box<dyn HwTask>, String>> {
        self.spawns.lock().unwrap().push((task.to_string(), args.to_vec()));
        Some(Ok(Box::new(Idle)))
    }
}

impl Net {
    /// a new channel of the network
    pub fn chan(&mut self, name: &str, elem: RtType, depth: usize) -> usize {
        self.chans.push(NetChan { name: name.into(), elem, depth: depth.max(1) });
        self.chans.len() - 1
    }

    /// task `task` with `args` (channels of the network, values): a leaf, or the network it builds
    pub fn expand(&mut self, prog: &Arc<Program>, task: &str, path: &str, args: Vec<(String, Arg)>, depth: usize) -> R<()> {
        if task == "$broadcast" { self.leaves.push(Leaf { task: task.into(), path: path.into(), args }); return Ok(()); }
        let decl = prog.fns.get(task).cloned().ok_or_else(|| format!("no task `{}`", task))?;
        if !fluxa_synth::graph::is_graph(&decl) {
            self.leaves.push(Leaf { task: task.into(), path: path.into(), args });
            return Ok(());
        }
        if depth > 16 { return Err(format!("{}: networks nested too deep", task)); }
        // run the body: its channel parameters are channels of the simulator's world standing for ours
        let rec = Arc::new(Recorder::default());
        let sh = Shared::with_hw(prog.clone(), false, true, Some(rec.clone()));
        let mut it = Interp::new(sh.clone(), 0, decl.name.clone());
        let mut map: Vec<(usize, usize)> = vec![];
        let mut argv = vec![];
        for p in &decl.params {
            let a = args.iter().find(|(n, _)| *n == p.name).map(|x| x.1.clone()).ok_or_else(|| format!("{}: parameter `{}` has no value", task, p.name))?;
            match a {
                Arg::Chan(g) => {
                    let c = &self.chans[g];
                    let id = sh.new_chan(0, c.name.clone(), c.elem.clone(), c.depth);
                    map.push((id, g));
                    let out = matches!(&p.ty, TypeExpr::Chan(..)) && fluxa_synth::dir_of(&decl, &p.name) == Some(true);
                    argv.push((None, if out { Value::Tx(id) } else { Value::Rx(id) }));
                }
                Arg::Val(v) => argv.push((None, v)),
            }
        }
        let gvals: Vec<Value> = decl.generics.iter().map(|g| args.iter().find(|(n, _)| *n == g.name).and_then(|x| if let Arg::Val(v) = &x.1 { Some(v.clone()) } else { None }).unwrap_or(Value::Int(0))).collect();
        let r = sh.run_coop(Box::pin(it.run_decl(&decl, gvals, argv)));
        match r {
            Ok(_) => {}
            Err(RtErr::Deadlock) => return Err(format!("{}: a task that spawns may only build the network (chan, spawn, broadcast), not use channels itself", decl.name)),
            Err(RtErr::Msg(l, m)) => return Err(format!("{}: {}", prog.loc(l), m)),
            Err(e) => return Err(format!("{}: {:?}", decl.name, e)),
        }
        let world: Vec<(String, RtType, usize)> = sh.lock().chans.iter().map(|c| (c.name.clone(), c.elem.clone(), c.depth)).collect();
        let mut gid: Vec<usize> = vec![];
        for (k, (n, e, d)) in world.iter().enumerate() {
            match map.iter().find(|(l, _)| *l == k) {
                Some((_, g)) => gid.push(*g),
                None => { let g = self.chan(&format!("{}{}", if path.is_empty() { String::new() } else { format!("{}.", path) }, n), e.clone(), *d); gid.push(g); }
            }
        }
        let spawns = rec.spawns.lock().unwrap().clone();
        for (k, (t, a)) in spawns.into_iter().enumerate() {
            let conv = |v: &Value| -> Arg { match v { Value::Rx(c) | Value::Tx(c) => Arg::Chan(gid[*c]), v => Arg::Val(v.clone()) } };
            let args: Vec<(String, Arg)> = if t == "$broadcast" {
                let mut out = vec![];
                for (n, v) in &a {
                    match v {
                        Value::Array(xs) => for (j, x) in xs.iter().enumerate() { out.push((format!("{}{}", n, j), conv(x))); },
                        v => out.push((n.clone(), conv(v))),
                    }
                }
                out
            } else { a.iter().map(|(n, v)| (n.clone(), conv(v))).collect() };
            let me = format!("{}{}", t.trim_start_matches('$').to_lowercase(), k);
            let sub = if path.is_empty() { me } else { format!("{}.{}", path, me) };
            self.expand(prog, &t, &sub, args, depth + 1)?;
        }
        Ok(())
    }
}

/// a channel's depth from a parameter's type (`chan<T, depth = d>`), else 2
pub fn param_depth(it: &mut Interp, t: &TypeExpr) -> usize {
    match t {
        TypeExpr::Chan(_, Some(d)) => match block_on(it.eval(d)) { Ok(Value::Int(n)) => n.max(1) as usize, _ => 2 },
        _ => 2,
    }
}
