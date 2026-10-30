//! A task that builds a network (`chan`, `spawn`, `broadcast`) as one netlist: the instances'
//! netlists connected by FIFOs.
//!
//! The network is found by running the task's body in the simulator with a recording hardware
//! provider: every `spawn` (and `broadcast`) is recorded with its arguments instead of started,
//! every `chan()` is a channel of the simulator's world. So the graph is exactly what the
//! simulator builds, loops and comptime included. Then each instance is synthesized (its scalar
//! arguments as constants), copied into one netlist, and every channel between two instances
//! becomes a FIFO of the channel's depth; the task's own channel parameters are its ports.

use crate::hw::Iface;
use crate::ty::{Ty, R};
use eda::netlist::{self, bits_for, Bit, Builder, Dir, Netlist, Port};
use fluxa::ast::{FnDecl, Program, TypeExpr};
use fluxa::interp::Interp;
use fluxa::runtime::{block_on, HwProvider, HwTask, HwTok, RtErr, Shared};
use fluxa::value::{RtType, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// does the task build a network (spawn, broadcast)?
pub fn is_graph(decl: &FnDecl) -> bool {
    let mut spawns = false;
    fn stmts(b: &fluxa::ast::Block, f: &mut bool) {
        for s in &b.stmts {
            match &s.kind {
                fluxa::ast::StmtKind::Spawn(_) | fluxa::ast::StmtKind::Parallel(_) => *f = true,
                fluxa::ast::StmtKind::For { body, .. } => stmts(body, f),
                _ => {}
            }
        }
    }
    stmts(&decl.body, &mut spawns);
    crate::walk_block(&decl.body, &mut |e| if let fluxa::ast::ExprKind::Call(c, _) = &e.kind { if matches!(&c.kind, fluxa::ast::ExprKind::Path(p) if p.last().map(|x| x.as_str()) == Some("broadcast")) { spawns = true } });
    spawns
}

/// a recorded spawn: the task and its arguments (parameter name, value)
type Spawn = (String, Vec<(String, Value)>);

#[derive(Default)]
struct Recorder {
    spawns: Mutex<Vec<Spawn>>,
}

/// a recorded task does nothing
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

/// one end of a channel: an instance's port (instance, parameter name), or the task's own port
#[derive(Clone, Debug, PartialEq)]
enum End { Inst(usize, String), Top(String) }

struct Inst { name: String, s: crate::Synth }

pub fn build(prog: &Arc<Program>, decl: &Arc<FnDecl>, params: &HashMap<String, i128>) -> R<crate::Synth> {
    // ---- the network, as the simulator builds it
    let rec = Arc::new(Recorder::default());
    let sh = Shared::with_hw(prog.clone(), false, true, Some(rec.clone()));
    let mut it = Interp::new(sh.clone(), 0, decl.name.clone());
    let mut args = vec![];
    let mut top_chans: Vec<(String, usize, Option<bool>)> = vec![];
    for p in &decl.params {
        match &p.ty {
            TypeExpr::Chan(t, d) => {
                let elem = block_on(it.eval_type(t)).map_err(|e| format!("{}: {:?}", decl.name, e))?;
                let depth = match d { Some(d) => match block_on(it.eval(d)) { Ok(Value::Int(n)) => n.max(1) as usize, _ => 2 }, None => 2 };
                let id = sh.new_chan(0, p.name.clone(), elem, depth);
                args.push((None, if p.out == Some(true) { Value::Tx(id) } else { Value::Rx(id) }));
                top_chans.push((p.name.clone(), id, p.out));
            }
            _ => match params.get(&p.name) {
                Some(v) => args.push((None, Value::Int(*v))),
                None => return Err(format!("{}: the network's parameter `{}` must be constant (--param {}=…)", decl.name, p.name, p.name)),
            },
        }
    }
    let r = sh.run_coop(Box::pin(it.run_decl(decl, vec![], args)));
    match r {
        Ok(_) => {}
        Err(RtErr::Deadlock) => return Err(format!("{}: a task that spawns may only build the network (chan, spawn, broadcast), not use channels itself", decl.name)),
        Err(RtErr::Msg(l, m)) => return Err(format!("{}: {}", prog.loc(l), m)),
        Err(e) => return Err(format!("{}: {:?}", decl.name, e)),
    }
    let chans: Vec<(String, RtType, usize)> = sh.lock().chans.iter().map(|c| (c.name.clone(), c.elem.clone(), c.depth)).collect();
    let spawns = rec.spawns.lock().unwrap().clone();

    // ---- the instances, and the two ends of every channel
    let mut insts: Vec<Inst> = vec![];
    let mut producer: HashMap<usize, End> = HashMap::new();
    let mut consumer: HashMap<usize, End> = HashMap::new();
    for (task, a) in &spawns {
        let k = insts.len();
        let (s, ends) = if task == "$broadcast" {
            let (Some(Value::Rx(src)), Some(Value::Array(outs))) = (a.iter().find(|x| x.0 == "rx").map(|x| &x.1), a.iter().find(|x| x.0 == "outs").map(|x| &x.1)) else { return Err("broadcast: unexpected arguments".into()) };
            let outs: Vec<usize> = outs.iter().filter_map(|v| if let Value::Tx(c) = v { Some(*c) } else { None }).collect();
            let elem = &chans[*src].1;
            let sends: String = (0..outs.len()).map(|k| format!("o{}.send(v); ", k)).collect();
            let ps: String = (0..outs.len()).map(|k| format!(", o{}: out chan<{}>", k, elem)).collect();
            let src_text = format!("task Broadcast(x: in chan<{}>{}) {{ for v in x {{ {}}} }}", elem, ps, sends);
            let p2 = fluxa::load_many(&[("broadcast.fx".into(), src_text)])?;
            let s = crate::synthesize(&p2, "Broadcast", &HashMap::new())?;
            let mut ends = vec![("x".to_string(), *src, false)];
            for (k, c) in outs.iter().enumerate() { ends.push((format!("o{}", k), *c, true)); }
            (s, ends)
        } else {
            let sub = prog.fns.get(task).ok_or_else(|| format!("no task `{}`", task))?;
            let mut sp = HashMap::new();
            let mut ends = vec![];
            for (n, v) in a {
                match v {
                    Value::Int(x) => { sp.insert(n.clone(), *x); }
                    Value::Bool(b) => { sp.insert(n.clone(), *b as i128); }
                    Value::Rx(c) => ends.push((n.clone(), *c, false)),
                    Value::Tx(c) => ends.push((n.clone(), *c, true)),
                    other => return Err(format!("spawn {}: argument `{}` ({}) is not synthesized", task, n, fluxa::value::kind(other))),
                }
            }
            let _ = sub;
            let s = crate::synthesize(prog, task, &sp).map_err(|e| format!("{} (spawned by {}): {}", task, decl.name, e))?;
            // the direction is what the instance does with the channel
            let ends = ends.into_iter().map(|(n, c, _)| { let out = s.iface.outs.iter().any(|o| o.0 == n); (n, c, out) }).collect();
            (s, ends)
        };
        for (n, c, out) in ends {
            let e = End::Inst(k, n);
            let slot = if out { &mut producer } else { &mut consumer };
            if slot.insert(c, e).is_some() { return Err(format!("{}: channel `{}` has two {}s", decl.name, chans[c].0, if out { "sender" } else { "receiver" })); }
        }
        insts.push(Inst { name: task.clone(), s });
    }
    // the task's own channels: the other end is outside
    let mut iface = Iface::default();
    for (name, c, _) in &top_chans {
        let ty = Ty::of_rt(&chans[*c].1)?;
        match (producer.contains_key(c), consumer.contains_key(c)) {
            (false, true) => { producer.insert(*c, End::Top(name.clone())); iface.ins.push((name.clone(), ty)); }
            (true, false) => { consumer.insert(*c, End::Top(name.clone())); iface.outs.push((name.clone(), ty)); }
            _ => return Err(format!("{}: channel parameter `{}` must be used by exactly one spawned task", decl.name, name)),
        }
    }

    // ---- one netlist
    let mut b = Builder::new();
    let clk = b.wire();
    let rst = b.wire();
    let mut alias: HashMap<u32, Bit> = HashMap::new();
    let mut ports = vec![Port { name: "clk".into(), dir: Dir::In, bits: vec![clk] }, Port { name: "rst".into(), dir: Dir::In, bits: vec![rst] }];
    // every instance's cells copied with fresh wires; its ports by name
    let mut iports: Vec<HashMap<String, Vec<Bit>>> = vec![];
    for inst in &insts {
        let nl = &inst.s.netlist;
        let base = b.next_wire;
        let maxw = nl.cells.iter().flat_map(|c| c.conns.iter().map(|x| x.1)).chain(nl.ports.iter().flat_map(|p| p.bits.iter().copied()))
            .filter_map(|x| if let Bit::W(w) = x { Some(w) } else { None }).max().unwrap_or(0);
        let map = |x: Bit| if let Bit::W(w) = x { Bit::W(base + w) } else { x };
        for c in &nl.cells {
            b.cells.push(netlist::Cell { typ: c.typ.clone(), conns: c.conns.iter().map(|(p, x)| (*p, map(*x))).collect(), params: c.params.clone() });
        }
        b.next_wire = base + maxw + 1;
        let mut pm = HashMap::new();
        for p in &nl.ports { pm.insert(p.name.clone(), p.bits.iter().map(|x| map(*x)).collect::<Vec<Bit>>()); }
        // the instance's clock and reset are the network's
        for (n, to) in [("clk", clk), ("rst", rst)] { if let Some(bits) = pm.get(n) { if let Bit::W(w) = bits[0] { alias.insert(w, to); } } }
        iports.push(pm);
    }
    // the network's ports
    let mut top_in: HashMap<String, (Vec<Bit>, Bit, Bit, Bit)> = HashMap::new();
    for (n, t) in &iface.ins {
        let (d, c, v, r) = (b.wires(t.width() as u32), b.wire(), b.wire(), b.wire());
        ports.push(Port { name: format!("{}_data", n), dir: Dir::In, bits: d.clone() });
        ports.push(Port { name: format!("{}_close", n), dir: Dir::In, bits: vec![c] });
        ports.push(Port { name: format!("{}_valid", n), dir: Dir::In, bits: vec![v] });
        top_in.insert(n.clone(), (d, c, v, r));
    }
    let mut top_out_ready: HashMap<String, Bit> = HashMap::new();
    for (n, _) in &iface.outs {
        let r = b.wire();
        ports.push(Port { name: format!("{}_ready", n), dir: Dir::In, bits: vec![r] });
        top_out_ready.insert(n.clone(), r);
    }
    let get = |iports: &Vec<HashMap<String, Vec<Bit>>>, k: usize, n: &str| -> R<Vec<Bit>> { iports[k].get(n).cloned().ok_or_else(|| format!("instance {} has no port {}", insts[k].name, n)) };
    let set = |alias: &mut HashMap<u32, Bit>, to: &[Bit], from: &[Bit]| { for (t, f) in to.iter().zip(from) { if let Bit::W(w) = t { alias.insert(*w, *f); } } };
    let mut out_ports: Vec<Port> = vec![];
    for (c, (cname, elem, depth)) in chans.iter().enumerate() {
        let (Some(p), Some(q)) = (producer.get(&c), consumer.get(&c)) else {
            if producer.contains_key(&c) || consumer.contains_key(&c) { return Err(format!("{}: channel `{}` has only one end", decl.name, cname)); }
            continue;
        };
        let w = Ty::of_rt(elem)?.width();
        // the producer's side: data, close, valid out of it, ready into it
        let (pd, pc, pv, pr_target): (Vec<Bit>, Bit, Bit, Option<Vec<Bit>>) = match p {
            End::Inst(k, n) => (get(&iports, *k, &format!("{}_data", n))?, get(&iports, *k, &format!("{}_close", n))?[0], get(&iports, *k, &format!("{}_valid", n))?[0], Some(get(&iports, *k, &format!("{}_ready", n))?)),
            End::Top(n) => { let (d, c, v, _) = &top_in[n]; (d.clone(), *c, *v, None) }
        };
        // the consumer's side: its data, close, valid inputs (none for the network's own port), its ready
        type Inputs = Option<(Vec<Bit>, Vec<Bit>, Vec<Bit>)>;
        let (cd, cr): (Inputs, Bit) = match q {
            End::Inst(k, n) => (Some((get(&iports, *k, &format!("{}_data", n))?, get(&iports, *k, &format!("{}_close", n))?, get(&iports, *k, &format!("{}_valid", n))?)), get(&iports, *k, &format!("{}_ready", n))?[0]),
            End::Top(n) => (None, top_out_ready[n]),
        };
        let (cdata, cclose, cvalid, pready) = if matches!(p, End::Top(_)) || matches!(q, End::Top(_)) {
            // the network's own channel: straight through (the other end is the environment's)
            (pd.clone(), pc, pv, cr)
        } else {
            fifo(&mut b, rst, clk, *depth, w, &pd, pc, pv, cr)
        };
        match pr_target { Some(t) => set(&mut alias, &t, &[pready]), None => { if let End::Top(n) = p { out_ports.push(Port { name: format!("{}_ready", n), dir: Dir::Out, bits: vec![pready] }); } } }
        match (q, cd) {
            (End::Inst(..), Some((d, c, v))) => { let pd = netlist::pad(&cdata, d.len()); set(&mut alias, &d, &pd); set(&mut alias, &c, &[cclose]); set(&mut alias, &v, &[cvalid]); }
            (End::Top(n), _) => {
                out_ports.push(Port { name: format!("{}_data", n), dir: Dir::Out, bits: cdata.clone() });
                out_ports.push(Port { name: format!("{}_close", n), dir: Dir::Out, bits: vec![cclose] });
                out_ports.push(Port { name: format!("{}_valid", n), dir: Dir::Out, bits: vec![cvalid] });
            }
            _ => {}
        }
    }
    ports.extend(out_ports);
    // an instance input left unconnected (a scalar given no value) is an error
    let nl: Netlist = netlist::optimize(b, &alias, ports, vec![], &decl.name)?;
    let mut warnings = vec![];
    for inst in &insts { warnings.extend(inst.s.warnings.iter().map(|w| format!("{}: {}", inst.name, w))); }
    // instance k of task T is `t<k>` (its own instances below it)
    let mut tree = vec![(String::new(), decl.name.clone())];
    for (k, inst) in insts.iter().enumerate() {
        let me = format!("{}{}", inst.name.trim_start_matches('$').to_lowercase(), k);
        for (p, t) in &inst.s.insts { tree.push((if p.is_empty() { me.clone() } else { format!("{}.{}", me, p) }, t.clone())); }
    }
    Ok(crate::Synth { netlist: nl, iface, warnings, insts: tree })
}

/// a FIFO of `depth` tokens of `w` data bits (+ the close flag): push side (data, close, valid)
/// and the pop side's ready in; out: the pop side's data, close, valid and the push side's ready
#[allow(clippy::too_many_arguments)]
fn fifo(b: &mut Builder, rst: Bit, clk: Bit, depth: usize, w: usize, d: &[Bit], c: Bit, v: Bit, pop_ready: Bit) -> (Vec<Bit>, Bit, Bit, Bit) {
    let iw = bits_for(depth).max(1);
    let cw = bits_for(depth + 1).max(1);
    let entries: Vec<Vec<Bit>> = (0..depth).map(|_| b.wires(w as u32 + 1)).collect();
    let (wr, rd, cnt) = (b.wires(iw as u32), b.wires(iw as u32), b.wires(cw as u32));
    let k = |x: usize, n: usize| -> Vec<Bit> { (0..n).map(|i| Bit::of((x >> i) & 1 == 1)).collect() };
    let full = b.eq(&cnt, &k(depth, cw));
    let empty = b.eq(&cnt, &k(0, cw));
    let push_ready = b.not(full);
    let pop_valid = b.not(empty);
    let push = b.and(v, push_ready);
    let pop = b.and(pop_ready, pop_valid);
    let word = b.select(&entries, &rd);
    let mut din = netlist::pad(d, w);
    din.push(c);
    for (j, e) in entries.iter().enumerate() {
        let here = b.eq(&wr, &k(j, iw));
        let we = b.and(push, here);
        let nd = b.muxv(we, e, &din);
        for (q, dd) in e.iter().zip(nd) { b.ff("$_DFF_P_".into(), vec![("C", clk), ("D", dd), ("Q", *q)]); }
    }
    let step = |b: &mut Builder, p: &[Bit], go: Bit| -> Vec<Bit> {
        let last = b.eq(p, &k(depth - 1, iw));
        let inc = b.add(p, &k(1, iw));
        let nx = b.muxv(last, &inc, &k(0, iw));
        b.muxv(go, p, &nx)
    };
    let wr_d = step(b, &wr, push);
    let rd_d = step(b, &rd, pop);
    let up = b.add(&cnt, &k(1, cw));
    let down = b.sub(&cnt, &k(1, cw));
    let np = b.not(pop);
    let only_push = b.and(push, np);
    let npush = b.not(push);
    let only_pop = b.and(pop, npush);
    let c1 = b.muxv(only_push, &cnt, &up);
    let cnt_d = b.muxv(only_pop, &c1, &down);
    for (q, dd) in wr.iter().zip(wr_d).chain(rd.iter().zip(rd_d)).chain(cnt.iter().zip(cnt_d)) {
        b.ff("$_SDFF_PP0_".into(), vec![("C", clk), ("D", dd), ("R", rst), ("Q", *q)]);
    }
    (word[..w].to_vec(), word[w], pop_valid, push_ready)
}
