//! A compiled network as a WebAssembly module: one function per code, `step(desc, budget)`,
//! which dispatches on the instance's state (`br_table`) and runs states until it waits on a
//! channel; `run(budget)` steps every instance in turn until none moves. Registers are i64
//! locals (a W register two: low and high halves); 128-bit multiplication, shifts and division
//! are helper functions. The memory is `ir::layout`'s, set up by data segments.
//!
//! Exports: `mem`, `run(budget: i32) -> i32` (the state transitions made; -1: an instance
//! stopped the program, its descriptor's address is at 0 and its trap + 1 at descriptor + 12).

use crate::ir::*;
use crate::wasm::*;

pub struct Output {
    pub wasm: Vec<u8>,
    pub json: String,
}

const MUL128: u32 = 0;
const SHL128: u32 = 1;
const SHR128: u32 = 2;
const DIV128: u32 = 3;
const REM128: u32 = 4;
const STEPS: u32 = 5;

fn t4_2() -> FType { FType(vec![I64; 4], vec![I64; 2]) }
fn t3_2() -> FType { FType(vec![I64; 3], vec![I64; 2]) }

/// (a0, a1) * (b0, b1) modulo 2^128
fn mul128() -> Func {
    let mut f = Func::new("mul128", t4_2());
    let (a0, a1, b0, b1) = (0, 1, 2, 3);
    let (al, ah, bl, bh, ll, lh, hl) = (f.local(I64), f.local(I64), f.local(I64), f.local(I64), f.local(I64), f.local(I64), f.local(I64));
    let m = 0xffff_ffffi64;
    f.get(a0).i64c(m).op(I64_AND).set(al);
    f.get(a0).i64c(32).op(I64_SHR_U).set(ah);
    f.get(b0).i64c(m).op(I64_AND).set(bl);
    f.get(b0).i64c(32).op(I64_SHR_U).set(bh);
    f.get(al).get(bl).op(I64_MUL).set(ll);
    f.get(al).get(bh).op(I64_MUL).set(lh);
    f.get(ah).get(bl).op(I64_MUL).set(hl);
    // lo
    f.get(a0).get(b0).op(I64_MUL);
    // hi = ah*bh + lh>>32 + hl>>32 + ((ll>>32) + (lh & m) + (hl & m)) >> 32 + a0*b1 + a1*b0
    f.get(ah).get(bh).op(I64_MUL);
    f.get(lh).i64c(32).op(I64_SHR_U).op(I64_ADD);
    f.get(hl).i64c(32).op(I64_SHR_U).op(I64_ADD);
    f.get(ll).i64c(32).op(I64_SHR_U).get(lh).i64c(m).op(I64_AND).op(I64_ADD).get(hl).i64c(m).op(I64_AND).op(I64_ADD).i64c(32).op(I64_SHR_U).op(I64_ADD);
    f.get(a0).get(b1).op(I64_MUL).op(I64_ADD);
    f.get(a1).get(b0).op(I64_MUL).op(I64_ADD);
    f
}

/// (lo, hi) << n, n < 128
fn shl128() -> Func {
    let mut f = Func::new("shl128", t3_2());
    let (lo, hi, n) = (0, 1, 2);
    let (rl, rh) = (f.local(I64), f.local(I64));
    f.get(n).op(I64_EQZ).if_().get(lo).get(hi).ret().end();
    f.get(n).i64c(64).op(I64_LT_U).if_();
    f.get(lo).get(n).op(I64_SHL).set(rl);
    f.get(hi).get(n).op(I64_SHL).get(lo).i64c(64).get(n).op(I64_SUB).op(I64_SHR_U).op(I64_OR).set(rh);
    f.else_();
    f.i64c(0).set(rl);
    f.get(lo).get(n).i64c(64).op(I64_SUB).op(I64_SHL).set(rh);
    f.end();
    f.get(rl).get(rh);
    f
}

/// (lo, hi) >> n (arithmetic), n < 128
fn shr128() -> Func {
    let mut f = Func::new("shr128", t3_2());
    let (lo, hi, n) = (0, 1, 2);
    let (rl, rh) = (f.local(I64), f.local(I64));
    f.get(n).op(I64_EQZ).if_().get(lo).get(hi).ret().end();
    f.get(n).i64c(64).op(I64_LT_U).if_();
    f.get(lo).get(n).op(I64_SHR_U).get(hi).i64c(64).get(n).op(I64_SUB).op(I64_SHL).op(I64_OR).set(rl);
    f.get(hi).get(n).op(I64_SHR_S).set(rh);
    f.else_();
    f.get(hi).get(n).i64c(64).op(I64_SUB).op(I64_SHR_S).set(rl);
    f.get(hi).i64c(63).op(I64_SHR_S).set(rh);
    f.end();
    f.get(rl).get(rh);
    f
}

/// signed 128-bit division (truncating) or remainder, by long division on the magnitudes
fn divrem128(rem: bool) -> Func {
    let mut f = Func::new(if rem { "rem128" } else { "div128" }, t4_2());
    let (a0, a1, b0, b1) = (0, 1, 2, 3);
    let (na, nb, q0, q1, r0, r1, i, t) = (f.local(I32), f.local(I32), f.local(I64), f.local(I64), f.local(I64), f.local(I64), f.local(I32), f.local(I64));
    // the signs, then the magnitudes
    f.get(a1).i64c(0).op(I64_LT_S).set(na);
    f.get(b1).i64c(0).op(I64_LT_S).set(nb);
    let neg = |f: &mut Func, x0: u32, x1: u32| {
        // (x0, x1) = -(x0, x1) = ~x + 1
        f.get(x0).i64c(-1).op(I64_XOR).i64c(1).op(I64_ADD).set(x0);
        f.get(x1).i64c(-1).op(I64_XOR).get(x0).op(I64_EQZ).op(I64_EXTEND_I32_U).op(I64_ADD).set(x1);
    };
    f.get(na).if_(); neg(&mut f, a0, a1); f.end();
    f.get(nb).if_(); neg(&mut f, b0, b1); f.end();
    f.i32c(128).set(i);
    f.block().loop_();
    f.get(i).op(I32_EQZ).br_if(1);
    // r = r << 1 | a's top bit; a <<= 1; q <<= 1
    f.get(r1).i64c(1).op(I64_SHL).get(r0).i64c(63).op(I64_SHR_U).op(I64_OR).set(r1);
    f.get(r0).i64c(1).op(I64_SHL).get(a1).i64c(63).op(I64_SHR_U).op(I64_OR).set(r0);
    f.get(a1).i64c(1).op(I64_SHL).get(a0).i64c(63).op(I64_SHR_U).op(I64_OR).set(a1);
    f.get(a0).i64c(1).op(I64_SHL).set(a0);
    f.get(q1).i64c(1).op(I64_SHL).get(q0).i64c(63).op(I64_SHR_U).op(I64_OR).set(q1);
    f.get(q0).i64c(1).op(I64_SHL).set(q0);
    // r >= b (unsigned)?
    f.get(r1).get(b1).op(I64_GT_U).get(r1).get(b1).op(I64_EQ).get(r0).get(b0).op(I64_GE_U).op(I32_AND).op(I32_OR);
    f.if_();
    f.get(r0).get(b0).op(I64_LT_U).op(I64_EXTEND_I32_U).set(t);
    f.get(r0).get(b0).op(I64_SUB).set(r0);
    f.get(r1).get(b1).op(I64_SUB).get(t).op(I64_SUB).set(r1);
    f.get(q0).i64c(1).op(I64_OR).set(q0);
    f.end();
    f.get(i).i32c(1).op(I32_SUB).set(i);
    f.br(0).end().end();
    if rem {
        f.get(na).if_(); neg(&mut f, r0, r1); f.end();
        f.get(r0).get(r1);
    } else {
        f.get(na).get(nb).op(0x73).if_(); neg(&mut f, q0, q1); f.end(); // i32.xor
        f.get(q0).get(q1);
    }
    f
}

type Regs = Vec<(u32, Option<u32>)>;
fn lo(r: &Regs, x: Reg) -> u32 { r[x as usize].0 }
fn hi(r: &Regs, x: Reg) -> u32 { r[x as usize].1.expect("a W register") }
fn isw(r: &Regs, x: Reg) -> bool { r[x as usize].1.is_some() }

struct Gen<'a> {
    f: Func,
    /// each register's locals: low (or the only) and high
    regs: Regs,
    code: &'a Code,
    d: u32,
    budget: u32,
    fr: u32,
    s: u32,
    n: u32,
    ch: u32,
    cnt: u32,
    head: u32,
    addr: u32,
    t64: u32,
}

impl Gen<'_> {

    /// the address of `a` (i32) on the stack
    fn addr(&mut self, a: &Addr) {
        self.f.get(self.fr);
        if let Some(o) = a.off { self.f.get(lo(&self.regs, o)).op(I32_WRAP_I64).op(I32_ADD); }
    }

    fn ops(&mut self, ops: &[Op]) {
        for o in ops { self.op(o); }
    }

    fn op(&mut self, o: &Op) {
        match o {
            Op::Const(d, v) => {
                if isw(&self.regs, *d) { self.f.i64c(*v as i64).set(lo(&self.regs, *d)); self.f.i64c((*v >> 64) as i64).set(hi(&self.regs, *d)); } else { self.f.i64c(*v as i64).set(lo(&self.regs, *d)); }
            }
            Op::Mov(d, s) => {
                self.f.get(lo(&self.regs, *s)).set(lo(&self.regs, *d));
                if isw(&self.regs, *d) { if isw(&self.regs, *s) { self.f.get(hi(&self.regs, *s)).set(hi(&self.regs, *d)); } else { self.f.get(lo(&self.regs, *s)).i64c(63).op(I64_SHR_S).set(hi(&self.regs, *d)); } }
            }
            Op::Load(d, a) => {
                self.addr(a);
                if isw(&self.regs, *d) {
                    self.f.tee(self.addr).load64(a.base).set(lo(&self.regs, *d));
                    self.f.get(self.addr).load64(a.base + 8).set(hi(&self.regs, *d));
                } else { self.f.load64(a.base).set(lo(&self.regs, *d)); }
            }
            Op::Store(a, s) => {
                self.addr(a);
                if isw(&self.regs, *s) {
                    self.f.tee(self.addr).get(lo(&self.regs, *s)).store64(a.base);
                    self.f.get(self.addr).get(hi(&self.regs, *s)).store64(a.base + 8);
                } else { self.f.get(lo(&self.regs, *s)).store64(a.base); }
            }
            Op::Bin(op, d, a, b) => if isw(&self.regs, *a) { self.binw(*op, *d, *a, *b) } else { self.binn(*op, *d, *a, *b) },
            Op::Ext(d, s, signed) => {
                self.f.get(lo(&self.regs, *s)).set(lo(&self.regs, *d));
                if *signed { self.f.get(lo(&self.regs, *s)).i64c(63).op(I64_SHR_S).set(hi(&self.regs, *d)); } else { self.f.i64c(0).set(hi(&self.regs, *d)); }
            }
            Op::Low(d, s) => { self.f.get(lo(&self.regs, *s)).set(lo(&self.regs, *d)); }
            Op::Sel(d, c, a, b) => {
                let halves: Vec<(u32, u32, u32)> = if isw(&self.regs, *d) { vec![(lo(&self.regs, *d), lo(&self.regs, *a), lo(&self.regs, *b)), (hi(&self.regs, *d), hi(&self.regs, *a), hi(&self.regs, *b))] } else { vec![(lo(&self.regs, *d), lo(&self.regs, *a), lo(&self.regs, *b))] };
                for (dd, aa, bb) in halves { self.f.get(aa).get(bb).get(lo(&self.regs, *c)).op(I64_EQZ).op(I32_EQZ).op(SELECT).set(dd); }
            }
            Op::If(c, t, e) => {
                self.f.get(lo(&self.regs, *c)).op(I64_EQZ).op(I32_EQZ).if_();
                self.ops(t);
                if !e.is_empty() { self.f.else_(); self.ops(e); }
                self.f.end();
            }
            Op::Trap(k) => {
                self.f.get(self.d).i32c(*k as i32 + 1).store32(12);
                self.f.get(self.d).get(self.s).store32(0);
                self.f.i32c(-1).ret();
            }
            Op::Copy(d, s, n) => {
                self.addr(d);
                if d.base > 0 { self.f.i32c(d.base as i32).op(I32_ADD); }
                self.addr(s);
                if s.base > 0 { self.f.i32c(s.base as i32).op(I32_ADD); }
                self.f.i32c(*n as i32).memcopy();
            }
        }
    }

    fn binn(&mut self, op: BOp, d: Reg, a: Reg, b: Reg) {
        self.f.get(lo(&self.regs, a)).get(lo(&self.regs, b));
        let (code, cmp) = match op {
            BOp::Add => (I64_ADD, false), BOp::Sub => (I64_SUB, false), BOp::Mul => (I64_MUL, false),
            BOp::And => (I64_AND, false), BOp::Or => (I64_OR, false), BOp::Xor => (I64_XOR, false),
            BOp::Shl => (I64_SHL, false), BOp::ShrS => (I64_SHR_S, false), BOp::ShrU => (I64_SHR_U, false),
            BOp::DivS => (I64_DIV_S, false), BOp::DivU => (I64_DIV_U, false), BOp::RemS => (I64_REM_S, false), BOp::RemU => (I64_REM_U, false),
            BOp::Eq => (I64_EQ, true), BOp::Ne => (I64_NE, true), BOp::LtS => (I64_LT_S, true), BOp::LtU => (I64_LT_U, true),
            BOp::LeS => (I64_LE_S, true), BOp::LeU => (I64_LE_U, true),
        };
        self.f.op(code);
        if cmp { self.f.op(I64_EXTEND_I32_U); }
        self.f.set(lo(&self.regs, d));
    }

    fn binw(&mut self, op: BOp, d: Reg, a: Reg, b: Reg) {
        let (al, ah) = (lo(&self.regs, a), hi(&self.regs, a));
        let bw = isw(&self.regs, b);
        let (bl, bh) = (lo(&self.regs, b), if bw { hi(&self.regs, b) } else { 0 });
        let t = self.t64;
        match op {
            BOp::Add => {
                self.f.get(al).get(bl).op(I64_ADD).set(t);
                self.f.get(ah).get(bh).op(I64_ADD).get(t).get(al).op(I64_LT_U).op(I64_EXTEND_I32_U).op(I64_ADD).set(hi(&self.regs, d));
                self.f.get(t).set(lo(&self.regs, d));
            }
            BOp::Sub => {
                self.f.get(al).get(bl).op(I64_SUB).set(t);
                self.f.get(ah).get(bh).op(I64_SUB).get(al).get(bl).op(I64_LT_U).op(I64_EXTEND_I32_U).op(I64_SUB).set(hi(&self.regs, d));
                self.f.get(t).set(lo(&self.regs, d));
            }
            BOp::And | BOp::Or | BOp::Xor => {
                let c = match op { BOp::And => I64_AND, BOp::Or => I64_OR, _ => I64_XOR };
                self.f.get(al).get(bl).op(c).set(t);
                self.f.get(ah).get(bh).op(c).set(hi(&self.regs, d));
                self.f.get(t).set(lo(&self.regs, d));
            }
            BOp::Mul | BOp::DivS | BOp::DivU | BOp::RemS | BOp::RemU => {
                let fun = match op { BOp::Mul => MUL128, BOp::DivS | BOp::DivU => DIV128, _ => REM128 };
                self.f.get(al).get(ah).get(bl).get(bh).call(fun).set(hi(&self.regs, d)).set(lo(&self.regs, d));
            }
            // the amount is an N register
            BOp::Shl => { self.f.get(al).get(ah).get(bl).call(SHL128).set(hi(&self.regs, d)).set(lo(&self.regs, d)); }
            BOp::ShrS | BOp::ShrU => { self.f.get(al).get(ah).get(bl).call(SHR128).set(hi(&self.regs, d)).set(lo(&self.regs, d)); }
            BOp::Eq => { self.f.get(al).get(bl).op(I64_EQ).get(ah).get(bh).op(I64_EQ).op(I32_AND).op(I64_EXTEND_I32_U).set(lo(&self.regs, d)); }
            BOp::Ne => { self.f.get(al).get(bl).op(I64_NE).get(ah).get(bh).op(I64_NE).op(I32_OR).op(I64_EXTEND_I32_U).set(lo(&self.regs, d)); }
            BOp::LtS | BOp::LtU | BOp::LeS | BOp::LeU => {
                let low = if matches!(op, BOp::LtS | BOp::LtU) { I64_LT_U } else { I64_LE_U };
                self.f.get(ah).get(bh).op(I64_LT_S).get(ah).get(bh).op(I64_EQ).get(al).get(bl).op(low).op(I32_AND).op(I32_OR).op(I64_EXTEND_I32_U).set(lo(&self.regs, d));
            }
        }
    }

    /// go to state `to` (on the stack when None), `extra` blocks deeper than the state's code
    fn goto(&mut self, to: Option<u32>, me: usize, extra: u32) {
        if let Some(t) = to { self.f.i32c(t as i32); }
        self.f.set(self.s);
        self.f.get(self.n).i32c(1).op(I32_ADD).tee(self.n).get(self.budget).op(I32_GE_U).if_();
        self.f.get(self.d).get(self.s).store32(0).get(self.n).ret();
        self.f.end();
        let k = self.code.states.len();
        self.f.br((k - 1 - me) as u32 + extra);
    }

    /// stop here: the state saved, the transitions made returned
    fn wait(&mut self, me: usize) {
        self.f.get(self.d).i32c(me as i32).store32(0).get(self.n).ret();
    }

    fn port_chan(&mut self, port: u32) {
        self.f.get(self.d).load32(16 + 4 * port).set(self.ch);
    }

    fn state(&mut self, me: usize) {
        let st = &self.code.states[me];
        self.ops(&st.ops);
        match &st.term {
            Term::Goto(t) => self.goto(Some(*t), me, 0),
            Term::Branch(c, t, e) => {
                self.f.i32c(*t as i32).i32c(*e as i32).get(lo(&self.regs, *c)).op(I64_EQZ).op(I32_EQZ).op(SELECT);
                self.goto(None, me, 0);
            }
            Term::Halt => { self.f.get(self.d).i32c(1).store32(4); self.wait(me); }
            Term::Recv { port, closed, next } => {
                let p = &self.code.ports[*port as usize];
                let (buf, nbytes, tok) = (p.buf, p.leaves.iter().map(|k| k.size()).sum::<u32>(), tok_size(&p.leaves));
                self.port_chan(*port);
                self.f.get(self.ch).load32(0).tee(self.cnt).op(I32_EQZ).if_();
                self.f.get(self.ch).load32(8).if_();
                match closed {
                    Some(c) => self.goto(Some(*c), me, 2),
                    None => { self.f.get(self.d).i32c(0x7fff_ffff).store32(12).get(self.d).i32c(me as i32).store32(0).i32c(-1).ret(); }
                }
                self.f.end();
                self.wait(me);
                self.f.end();
                // the token at the head into the port's buffer
                self.f.get(self.fr).i32c(buf as i32).op(I32_ADD);
                self.f.get(self.ch).i32c(CHAN_HDR as i32).op(I32_ADD).get(self.ch).load32(4).tee(self.head).i32c(tok as i32).op(I32_MUL).op(I32_ADD);
                self.f.i32c(nbytes as i32).memcopy();
                self.f.get(self.ch).get(self.head).i32c(1).op(I32_ADD).get(self.ch).load32(12).op(I32_REM_U).store32(4);
                self.f.get(self.ch).get(self.cnt).i32c(1).op(I32_SUB).store32(0);
                self.goto(Some(*next), me, 0);
            }
            Term::Send { port, next } => {
                let p = &self.code.ports[*port as usize];
                let (buf, nbytes, tok) = (p.buf, p.leaves.iter().map(|k| k.size()).sum::<u32>(), tok_size(&p.leaves));
                self.port_chan(*port);
                self.f.get(self.ch).load32(0).tee(self.cnt).get(self.ch).load32(12).op(I32_GE_U).if_();
                self.wait(me);
                self.f.end();
                // the slot after the last token
                self.f.get(self.ch).i32c(CHAN_HDR as i32).op(I32_ADD);
                self.f.get(self.ch).load32(4).get(self.cnt).op(I32_ADD).get(self.ch).load32(12).op(I32_REM_U).i32c(tok as i32).op(I32_MUL).op(I32_ADD);
                self.f.get(self.fr).i32c(buf as i32).op(I32_ADD);
                self.f.i32c(nbytes as i32).memcopy();
                self.f.get(self.ch).get(self.cnt).i32c(1).op(I32_ADD).store32(0);
                self.goto(Some(*next), me, 0);
            }
            Term::Close { port, next } | Term::CloseOwned { port, next } => {
                self.port_chan(*port);
                self.f.get(self.ch).i32c(1).store32(8);
                self.goto(Some(*next), me, 0);
            }
        }
    }
}

/// the step function of `code`
fn step(code: &Code, ci: usize) -> Func {
    let f = Func::new(&format!("step_{}_{}", ci, code.name), FType(vec![I32, I32], vec![I32]));
    let mut g = Gen { f, regs: vec![], code, d: 0, budget: 1, fr: 0, s: 0, n: 0, ch: 0, cnt: 0, head: 0, addr: 0, t64: 0 };
    g.fr = g.f.local(I32);
    g.s = g.f.local(I32);
    g.n = g.f.local(I32);
    g.ch = g.f.local(I32);
    g.cnt = g.f.local(I32);
    g.head = g.f.local(I32);
    g.addr = g.f.local(I32);
    g.t64 = g.f.local(I64);
    for k in &code.regs {
        let lo = g.f.local(I64);
        let hi = if *k == K::W { Some(g.f.local(I64)) } else { None };
        g.regs.push((lo, hi));
    }
    g.f.get(g.d).load32(8).set(g.fr);
    g.f.get(g.d).load32(0).set(g.s);
    let k = code.states.len();
    g.f.loop_();
    for _ in 0..k { g.f.block(); }
    g.f.get(g.s);
    g.f.op(0x0e).u(k as u64);
    for i in 0..k { g.f.u(i as u64); }
    g.f.u((k - 1) as u64);
    for i in 0..k {
        g.f.end();
        g.state(i);
    }
    g.f.end();
    g.f.i32c(0);
    g.f
}

/// the module of machine `m`
pub fn compile(m: &Machine) -> Output {
    let l = layout(m);
    let mut funcs = vec![mul128(), shl128(), shr128(), divrem128(false), divrem128(true)];
    for (ci, c) in m.codes.iter().enumerate() { funcs.push(step(c, ci)); }
    // run(budget): every instance in turn until none moves
    let mut r = Func::new("run", FType(vec![I32], vec![I32]));
    let (total, moved, x) = (r.local(I32), r.local(I32), r.local(I32));
    r.loop_();
    r.i32c(0).set(moved);
    for (i, inst) in m.insts.iter().enumerate() {
        let d = l.desc[i] as i32;
        r.i32c(d).load32(4).op(I32_EQZ).if_();
        r.i32c(d).i32c(4096).call(STEPS + inst.code as u32).tee(x).i32c(0).op(I32_LT_S).if_();
        r.i32c(0).i32c(d).store32(0).i32c(-1).ret();
        r.end();
        r.get(x).if_().i32c(1).set(moved).get(total).get(x).op(I32_ADD).set(total).end();
        r.end();
    }
    r.get(moved).op(I32_EQZ).get(total).get(0).op(I32_GE_U).op(I32_OR).if_().get(total).ret().end();
    r.br(0).end();
    r.get(total);
    let run = funcs.len() as u32;
    funcs.push(r);
    // the memory's first contents: descriptors, channel depths, frames
    let mut data: Vec<(u32, Vec<u8>)> = vec![];
    for (i, inst) in m.insts.iter().enumerate() {
        let mut b = vec![0u8; 16 + 4 * inst.chans.len()];
        b[8..12].copy_from_slice(&l.frames[i].to_le_bytes());
        for (p, c) in inst.chans.iter().enumerate() { b[16 + 4 * p..20 + 4 * p].copy_from_slice(&l.chans[*c as usize].to_le_bytes()); }
        data.push((l.desc[i], b));
        let code = &m.codes[inst.code];
        let f = l.frames[i];
        for (off, k, v) in &code.init {
            let bytes = if *k == K::N { (*v as i64).to_le_bytes().to_vec() } else { v.to_le_bytes().to_vec() };
            data.push((f + off, bytes));
        }
        for ((_, off, ls, n), vals) in code.data.iter().zip(&inst.data) {
            let mut b = vec![];
            let mut j = 0;
            for _ in 0..*n { for k in ls { let v = vals.get(j).copied().unwrap_or(0); if *k == K::N { b.extend((v as i64).to_le_bytes()); } else { b.extend(v.to_le_bytes()); } j += 1; } }
            if b.iter().any(|x| *x != 0) { data.push((f + off, b)); }
        }
    }
    for (k, c) in m.chans.iter().enumerate() { data.push((l.chans[k] + 12, c.depth.to_le_bytes().to_vec())); }
    let pages = l.size.div_ceil(65536) + 1;
    let wasm = module(pages, &funcs, &[("run".to_string(), run)], &data);
    Output { wasm, json: meta(m, &l) }
}

fn jstr(s: &str) -> String { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")) }

/// what the harness needs: the instances (descriptor, code), the channels (address, depth, token
/// layout), the outside's channels, the codes' trap messages
fn meta(m: &Machine, l: &Layout) -> String {
    let insts: Vec<String> = m.insts.iter().enumerate().map(|(i, x)| format!("{{\"name\":{},\"desc\":{},\"code\":{}}}", jstr(&x.name), l.desc[i], x.code)).collect();
    let chans: Vec<String> = m.chans.iter().enumerate().map(|(k, c)| format!(
        "{{\"name\":{},\"addr\":{},\"depth\":{},\"tok\":{},\"leaves\":[{}],\"signed\":[{}]}}",
        jstr(&c.name), l.chans[k], c.depth, l.tok[k],
        c.leaves.iter().map(|k| if *k == K::N { "8" } else { "16" }).collect::<Vec<_>>().join(","),
        c.signed.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(","))).collect();
    let ends = |v: &[(String, u32)]| v.iter().map(|(n, c)| format!("[{},{}]", jstr(n), c)).collect::<Vec<_>>().join(",");
    let traps: Vec<String> = m.codes.iter().map(|c| format!("[{}]", c.traps.iter().map(|t| jstr(t)).collect::<Vec<_>>().join(","))).collect();
    format!("{{\"size\":{},\"insts\":[{}],\"chans\":[{}],\"ins\":[{}],\"outs\":[{}],\"traps\":[{}]}}\n",
        l.size, insts.join(","), chans.join(","), ends(&m.ins), ends(&m.outs), traps.join(","))
}
