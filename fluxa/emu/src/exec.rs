//! The executor: a compiled network run in Rust. Memory is one byte array (as the WebAssembly
//! module's): every instance's frame, every channel (a ring buffer: count, head, closed, then
//! `depth` tokens). The scheduler runs each instance until it waits on a channel, round after
//! round, until no instance moves.

use crate::ir::*;

const HDR: u32 = 16;

pub struct Exec {
    pub m: Machine,
    pub mem: Vec<u8>,
    frames: Vec<u32>,
    chans: Vec<u32>,
    tok: Vec<u32>,
    states: Vec<u32>,
    halted: Vec<bool>,
    regs: Vec<i128>,
    /// an instance that stopped the program: its message
    pub error: Option<String>,
    /// state transitions so far
    pub steps: u64,
    /// FLUXA_EMU_PROF: operations run per (instance, state)
    prof: Option<Vec<Vec<(u64, u64)>>>,
}

impl Drop for Exec {
    fn drop(&mut self) {
        let Some(p) = &self.prof else { return };
        let mut rows = vec![];
        for (i, st) in p.iter().enumerate() { for (s, (ops, n)) in st.iter().enumerate() { if *n > 0 { rows.push((*ops, *n, i, s)); } } }
        rows.sort_by(|a, b| b.cmp(a));
        let total: u64 = rows.iter().map(|r| r.0).sum();
        eprintln!("fluxa-emu profile: {} operations, {} transitions", total, self.steps);
        for (ops, n, i, s) in rows.iter().take(25) { eprintln!("  {:>12} ops {:>10} visits  {} state {} ({})", ops, n, self.m.insts[*i].name, s, self.m.codes[self.m.insts[*i].code].states[*s].at); }
    }
}

/// a token's scalars as i128 (an N leaf: its 64 bits, sign-extended)
fn rd(mem: &[u8], a: usize, k: K) -> i128 {
    if k == K::N { i64::from_le_bytes(mem[a..a + 8].try_into().unwrap()) as i128 } else { i128::from_le_bytes(mem[a..a + 16].try_into().unwrap()) }
}
fn wr(mem: &mut [u8], a: usize, k: K, v: i128) {
    if k == K::N { mem[a..a + 8].copy_from_slice(&(v as i64).to_le_bytes()); } else { mem[a..a + 16].copy_from_slice(&v.to_le_bytes()); }
}
fn u32at(mem: &[u8], a: usize) -> u32 { u32::from_le_bytes(mem[a..a + 4].try_into().unwrap()) }
fn set32(mem: &mut [u8], a: usize, v: u32) { mem[a..a + 4].copy_from_slice(&v.to_le_bytes()); }

pub fn tok_size(leaves: &[K]) -> u32 { leaves.iter().map(|k| k.size()).sum::<u32>().max(8) }

impl Exec {
    pub fn new(m: Machine) -> Exec {
        let mut at: u32 = 0;
        let mut frames = vec![];
        for i in &m.insts { frames.push(at); at += m.codes[i.code].frame.max(16).next_multiple_of(16); }
        let mut chans = vec![];
        let mut tok = vec![];
        for c in &m.chans { chans.push(at); let t = tok_size(&c.leaves); tok.push(t); at += (HDR + t * c.depth).next_multiple_of(16); }
        let mut mem = vec![0u8; at as usize];
        for (i, inst) in m.insts.iter().enumerate() {
            let code = &m.codes[inst.code];
            let f = frames[i] as usize;
            for (off, k, v) in &code.init { wr(&mut mem, f + *off as usize, *k, *v); }
            for ((_, off, ls, n), vals) in code.data.iter().zip(&inst.data) {
                let mut a = f + *off as usize;
                let mut j = 0;
                for _ in 0..*n { for k in ls { wr(&mut mem, a, *k, vals.get(j).copied().unwrap_or(0)); a += k.size() as usize; j += 1; } }
            }
        }
        let n = m.insts.len();
        let maxr = m.codes.iter().map(|c| c.regs.len()).max().unwrap_or(0);
        let prof = std::env::var("FLUXA_EMU_PROF").ok().map(|_| m.insts.iter().map(|i| vec![(0, 0); m.codes[i.code].states.len()]).collect());
        Exec { m, mem, frames, chans, tok, states: vec![0; n], halted: vec![false; n], regs: vec![0; maxr], error: None, steps: 0, prof }
    }

    // ------------------------------------------------------------ channels from outside

    /// can channel `c` take a token?
    pub fn room(&self, c: u32) -> bool { let a = self.chans[c as usize] as usize; u32at(&self.mem, a) < self.m.chans[c as usize].depth && u32at(&self.mem, a + 8) == 0 }
    pub fn closed(&self, c: u32) -> bool { u32at(&self.mem, self.chans[c as usize] as usize + 8) != 0 }
    pub fn count(&self, c: u32) -> u32 { u32at(&self.mem, self.chans[c as usize] as usize) }

    /// put a token (its scalars) into channel `c` (there must be room)
    pub fn push(&mut self, c: u32, vals: &[i128]) {
        let (a, ch) = (self.chans[c as usize] as usize, &self.m.chans[c as usize]);
        let (cnt, head) = (u32at(&self.mem, a), u32at(&self.mem, a + 4));
        let slot = (head + cnt) % ch.depth;
        let mut p = a + (HDR + slot * self.tok[c as usize]) as usize;
        let ls = ch.leaves.clone();
        for (k, v) in ls.iter().zip(vals) { wr(&mut self.mem, p, *k, *v); p += k.size() as usize; }
        set32(&mut self.mem, a, cnt + 1);
    }
    pub fn close(&mut self, c: u32) { set32(&mut self.mem, self.chans[c as usize] as usize + 8, 1); }

    /// take a token from channel `c`: its scalars as exact values (None: empty)
    pub fn pop(&mut self, c: u32) -> Option<Vec<i128>> {
        let (a, ch) = (self.chans[c as usize] as usize, &self.m.chans[c as usize]);
        let (cnt, head) = (u32at(&self.mem, a), u32at(&self.mem, a + 4));
        if cnt == 0 { return None; }
        let mut p = a + (HDR + head * self.tok[c as usize]) as usize;
        let mut out = vec![];
        for (k, s) in ch.leaves.iter().zip(&ch.signed) {
            let v = rd(&self.mem, p, *k);
            out.push(if *k == K::N && !*s { v as i64 as u64 as i128 } else { v });
            p += k.size() as usize;
        }
        set32(&mut self.mem, a + 4, (head + 1) % ch.depth);
        set32(&mut self.mem, a, cnt - 1);
        Some(out)
    }

    // ------------------------------------------------------------ running

    /// run every instance until none moves (or `budget` state transitions); true when something moved
    pub fn run(&mut self, budget: u64) -> bool {
        let end = self.steps + budget;
        let mut any = false;
        loop {
            let mut moved = false;
            for i in 0..self.m.insts.len() {
                if self.halted[i] || self.error.is_some() { continue; }
                if self.step(i, 4096) { moved = true; }
            }
            if !moved || self.error.is_some() { return any; }
            any = true;
            if self.steps >= end { return true; }
        }
    }

    /// run instance `i` until it waits (at most `max` transitions); true when it moved
    fn step(&mut self, i: usize, max: u32) -> bool {
        let code_ix = self.m.insts[i].code;
        let f = self.frames[i] as usize;
        let mut moved = false;
        for _ in 0..max {
            let s = self.states[i] as usize;
            let code = &self.m.codes[code_ix];
            let st = &code.states[s];
            if let Some(p) = &mut self.prof { let mut c = 0; count_ops(&st.ops, &code.regs, &self.regs, &mut c); p[i][s].0 += c; p[i][s].1 += 1; }
            if let Err(t) = run_ops(&st.ops, &code.regs, &mut self.regs, &mut self.mem, f) {
                self.error = Some(format!("{}: {}", self.m.insts[i].name, code.traps.get(t as usize).cloned().unwrap_or_default()));
                return moved;
            }
            let next = match &st.term {
                Term::Goto(t) => *t,
                Term::Branch(c, t, e) => if self.regs[*c as usize] != 0 { *t } else { *e },
                Term::Halt => { self.halted[i] = true; return moved; }
                Term::Recv { port, closed, next } => {
                    let c = self.m.insts[i].chans[*port as usize];
                    let a = self.chans[c as usize] as usize;
                    let (cnt, head) = (u32at(&self.mem, a), u32at(&self.mem, a + 4));
                    if cnt == 0 {
                        if u32at(&self.mem, a + 8) != 0 {
                            match closed { Some(x) => *x, None => { self.error = Some(format!("{}: recv on channel `{}` after the end of its stream", self.m.insts[i].name, self.m.chans[c as usize].name)); return moved; } }
                        } else { return moved; }
                    } else {
                        let t = self.tok[c as usize];
                        let from = a + (HDR + head * t) as usize;
                        let to = f + code.ports[*port as usize].buf as usize;
                        let n: u32 = code.ports[*port as usize].leaves.iter().map(|k| k.size()).sum();
                        self.mem.copy_within(from..from + n as usize, to);
                        set32(&mut self.mem, a + 4, (head + 1) % self.m.chans[c as usize].depth);
                        set32(&mut self.mem, a, cnt - 1);
                        *next
                    }
                }
                Term::Send { port, next } => {
                    let c = self.m.insts[i].chans[*port as usize];
                    let a = self.chans[c as usize] as usize;
                    let (cnt, head) = (u32at(&self.mem, a), u32at(&self.mem, a + 4));
                    let depth = self.m.chans[c as usize].depth;
                    if cnt >= depth { return moved; }
                    let t = self.tok[c as usize];
                    let to = a + (HDR + ((head + cnt) % depth) * t) as usize;
                    let from = f + code.ports[*port as usize].buf as usize;
                    let n: u32 = code.ports[*port as usize].leaves.iter().map(|k| k.size()).sum();
                    self.mem.copy_within(from..from + n as usize, to);
                    set32(&mut self.mem, a, cnt + 1);
                    *next
                }
                Term::Close { port, next } | Term::CloseOwned { port, next } => {
                    let c = self.m.insts[i].chans[*port as usize];
                    set32(&mut self.mem, self.chans[c as usize] as usize + 8, 1);
                    *next
                }
            };
            self.states[i] = next;
            self.steps += 1;
            moved = true;
        }
        moved
    }

    /// are all instances stopped?
    pub fn halted(&self) -> bool { self.halted.iter().all(|h| *h) }

    /// a frame's address (for tests and the memory loads of a machine)
    pub fn frame(&self, i: usize) -> u32 { self.frames[i] }
}

/// the operations `run_ops` will run (an `if` by the condition's current value: the
/// condition's register is set before the `if`, so this counts the arm of the last run)
fn count_ops(ops: &[Op], _k: &[K], r: &[i128], c: &mut u64) {
    for o in ops { *c += 1; if let Op::If(x, t, e) = o { count_ops(if r[*x as usize] != 0 { t } else { e }, _k, r, c); } }
}

/// run straight-line code; Err(k): trap k
fn run_ops(ops: &[Op], kinds: &[K], r: &mut [i128], mem: &mut [u8], f: usize) -> Result<(), u32> {
    let is_w = |reg: Reg, _: &[i128]| kinds[reg as usize] == K::W;
    for o in ops {
        match o {
            Op::Const(d, v) => r[*d as usize] = *v,
            Op::Mov(d, s) => r[*d as usize] = r[*s as usize],
            Op::Load(d, a) => {
                let p = f + a.base as usize + a.off.map(|x| r[x as usize] as u64 as usize).unwrap_or(0);
                // the kind: the register's (8 or 16 bytes)
                r[*d as usize] = if is_w(*d, r) { i128::from_le_bytes(mem[p..p + 16].try_into().unwrap()) } else { i64::from_le_bytes(mem[p..p + 8].try_into().unwrap()) as i128 };
            }
            Op::Store(a, s) => {
                let p = f + a.base as usize + a.off.map(|x| r[x as usize] as u64 as usize).unwrap_or(0);
                if is_w(*s, r) { mem[p..p + 16].copy_from_slice(&r[*s as usize].to_le_bytes()); } else { mem[p..p + 8].copy_from_slice(&(r[*s as usize] as i64).to_le_bytes()); }
            }
            Op::Bin(op, d, a, b) => {
                let (x, y) = (r[*a as usize], r[*b as usize]);
                r[*d as usize] = if is_w(*a, r) { binw(*op, x, y) } else { binn(*op, x as i64, y as i64) };
            }
            Op::Ext(d, s, signed) => r[*d as usize] = if *signed { r[*s as usize] } else { r[*s as usize] as i64 as u64 as i128 },
            Op::Low(d, s) => r[*d as usize] = r[*s as usize] as i64 as i128,
            Op::Sel(d, c, a, b) => r[*d as usize] = if r[*c as usize] != 0 { r[*a as usize] } else { r[*b as usize] },
            Op::If(c, t, e) => run_ops(if r[*c as usize] != 0 { t } else { e }, kinds, r, mem, f)?,
            Op::Trap(k) => return Err(*k),
            Op::Copy(d, s, n) => {
                let pd = f + d.base as usize + d.off.map(|x| r[x as usize] as u64 as usize).unwrap_or(0);
                let ps = f + s.base as usize + s.off.map(|x| r[x as usize] as u64 as usize).unwrap_or(0);
                mem.copy_within(ps..ps + *n as usize, pd);
            }
        }
    }
    Ok(())
}


fn binn(op: BOp, x: i64, y: i64) -> i128 {
    (match op {
        BOp::Add => x.wrapping_add(y),
        BOp::Sub => x.wrapping_sub(y),
        BOp::Mul => x.wrapping_mul(y),
        BOp::And => x & y,
        BOp::Or => x | y,
        BOp::Xor => x ^ y,
        BOp::Shl => x.wrapping_shl(y as u32),
        BOp::ShrS => x.wrapping_shr(y as u32),
        BOp::ShrU => ((x as u64).wrapping_shr(y as u32)) as i64,
        BOp::DivS => x.wrapping_div(y),
        BOp::DivU => ((x as u64) / (y as u64)) as i64,
        BOp::RemS => x.wrapping_rem(y),
        BOp::RemU => ((x as u64) % (y as u64)) as i64,
        BOp::Eq => (x == y) as i64,
        BOp::Ne => (x != y) as i64,
        BOp::LtS => (x < y) as i64,
        BOp::LtU => ((x as u64) < (y as u64)) as i64,
        BOp::LeS => (x <= y) as i64,
        BOp::LeU => ((x as u64) <= (y as u64)) as i64,
    }) as i128
}

fn binw(op: BOp, x: i128, y: i128) -> i128 {
    match op {
        BOp::Add => x.wrapping_add(y),
        BOp::Sub => x.wrapping_sub(y),
        BOp::Mul => x.wrapping_mul(y),
        BOp::And => x & y,
        BOp::Or => x | y,
        BOp::Xor => x ^ y,
        BOp::Shl => x.wrapping_shl(y as u32),
        BOp::ShrS | BOp::ShrU => x.wrapping_shr(y as u32),
        BOp::DivS | BOp::DivU => x.wrapping_div(y),
        BOp::RemS | BOp::RemU => x.wrapping_rem(y),
        BOp::Eq => (x == y) as i128,
        BOp::Ne => (x != y) as i128,
        BOp::LtS | BOp::LtU => (x < y) as i128,
        BOp::LeS | BOp::LeU => (x <= y) as i128,
    }
}
