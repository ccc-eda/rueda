//! A lowered task (fluxa-synth's `lower`: unique names, blocks cut at channel operations) as
//! emulator code. The expressions follow fluxa-synth's `gen` (the same ranges, conversions and
//! constant folding), but where `gen` makes a multiplexer the code branches: an `if` is an `if`,
//! an array indexed at run time is a load.
//!
//! The task's variables live in the instance's frame between states; in a state they are loaded
//! into registers first and the ones assigned are stored at its end. A function call is inlined,
//! its locals are registers. Like `gen`, a type pass runs first: untyped variables widen with
//! what they are given until nothing grows.

use crate::ir::{Addr, BOp, Code, Op, Port, Reg, State, Term, K};
use fluxa::ast::*;
use fluxa::interp::Interp;
use fluxa::runtime::{block_on, Shared};
use fluxa::value::{int_bits, Ovf, Round, RtType, Value};
use fluxa_synth::lower::{Cfg, Temp, Term as LTerm};
use fluxa_synth::ty::{widen, Ty, R};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// the kind of register a range needs
pub fn kind(lo: i128, hi: i128) -> K { if int_bits(lo, hi) <= 64 { K::N } else { K::W } }

/// a scalar type's kind
pub fn leaf_kind(t: &Ty) -> K { let (lo, hi) = t.range(); kind(lo, hi) }

/// the scalars of a type, in memory order
pub fn leaves(t: &Ty) -> Vec<Ty> {
    match t {
        Ty::Array(e, n) => { let l = leaves(e); (0..*n).flat_map(|_| l.clone()).collect() }
        Ty::Tuple(ts) => ts.iter().flat_map(leaves).collect(),
        t => vec![t.clone()],
    }
}

/// bytes of a value of type `t` in memory
pub fn size(t: &Ty) -> u32 {
    match t {
        Ty::Array(e, n) => size(e) * *n as u32,
        Ty::Tuple(ts) => ts.iter().map(size).sum(),
        t => leaf_kind(t).size(),
    }
}

/// a type as the simulator names it in messages
fn ty_name(t: &Ty) -> String {
    match t {
        Ty::Int { lo, hi } if *lo == 0 && (hi + 1).count_ones() == 1 => format!("u{}", (hi + 1).trailing_zeros()),
        Ty::Int { lo, hi } if *lo < 0 && -lo == hi + 1 && (-lo).count_ones() == 1 => format!("i{}", (-lo).trailing_zeros() + 1),
        Ty::Int { lo, hi } => format!("int<{}..={}>", lo, hi),
        Ty::Bool => "bool".into(),
        Ty::Fixed { i, f, .. } => format!("fixed<{}, {}>", i, f),
        t => format!("{:?}", t),
    }
}

fn has_array(t: &Ty) -> bool { match t { Ty::Array(..) => true, Ty::Tuple(ts) => ts.iter().any(has_array), _ => false } }

const LIM: i128 = 1i128 << 120;

/// x << k held to Fluxa's exact integers (as fluxa-synth's ty.rs)
fn shl_sat(x: i128, k: u32) -> i128 {
    if x == 0 { return 0; }
    match (k < 120).then(|| x.checked_mul(1i128 << k)).flatten() {
        Some(v) => v.clamp(-LIM, LIM - 1),
        None => if x > 0 { LIM - 1 } else { -LIM },
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum V { C(i128), R(Reg) }

/// a scalar: a constant or a register, the exact range of the integer, and (fixed point) its
/// fraction bits. A register holds the value (N: modulo 2^64, the range tells the sign).
#[derive(Clone, Debug, PartialEq)]
pub struct Sv { pub v: V, pub lo: i128, pub hi: i128, pub boolean: bool, pub frac: u32 }

impl Sv {
    pub fn konst(n: i128) -> Sv { Sv { v: V::C(n), lo: n, hi: n, boolean: false, frac: 0 } }
    fn bool_const(b: bool) -> Sv { Sv { v: V::C(b as i128), lo: 0, hi: 1, boolean: true, frac: 0 } }
    pub fn kind(&self) -> K { kind(self.lo, self.hi) }
    fn signed(&self) -> bool { self.lo < 0 }
    fn as_const(&self) -> Option<i128> { if let V::C(k) = self.v { Some(k) } else if self.lo == self.hi { Some(self.lo) } else { None } }
    fn ty(&self) -> Ty {
        if self.boolean { return Ty::Bool; }
        if self.frac > 0 {
            let w = int_bits(self.lo.min(-1), self.hi);
            return Ty::Fixed { i: w.saturating_sub(self.frac), f: self.frac, round: Round::Trunc, ovf: Ovf::Wrap };
        }
        Ty::Int { lo: self.lo, hi: self.hi }
    }
}

/// a place in the frame: `base` plus the run-time byte offset `off`, holding a value of `ty`
#[derive(Clone, Debug, PartialEq)]
pub struct Place { pub base: u32, pub off: Option<Reg>, pub ty: Ty }

/// a value: a scalar, a list (tuple, or an array held in registers), or an array in the frame
#[derive(Clone, Debug, PartialEq)]
pub enum EV { S(Sv), A(Vec<EV>), M(Place) }

impl EV {
    fn s(&self) -> R<&Sv> { match self { EV::S(s) => Ok(s), _ => Err("an array or tuple where a number is needed".into()) } }
}

/// a step into a variable: a constant index, or one known at run time
#[derive(Clone)]
enum Step { Const(usize), Var(Sv) }

pub struct Em<'p> {
    prog: &'p Arc<Program>,
    it: Interp,
    pub regs: Vec<K>,
    ops: Vec<Op>,
    /// the task's variables and their types
    pub vars: HashMap<String, Ty>,
    untyped: HashSet<String>,
    grew: bool,
    /// where the task's variables are in the frame
    slots: HashMap<String, u32>,
    pub frame: u32,
    pub init: Vec<(u32, K, i128)>,
    env: HashMap<String, EV>,
    /// task variables assigned in this state (stored at its end)
    dirty: HashSet<String>,
    /// inside an inlined function (its variables are locals)
    depth: usize,
    rename: usize,
    pub traps: Vec<String>,
    /// constant arrays put in the frame (indexed at run time), by constant name
    rom: HashMap<String, Place>,
    consts: HashSet<String>,
    /// the frame places of arrays that are locals of inlined functions, by name
    local_arrays: HashMap<String, Place>,
    /// each channel parameter's token buffer in the frame
    port_bufs: HashMap<String, u32>,
    /// array parameters: (parameter, offset, an element's leaves, elements)
    data: Vec<(String, u32, Vec<K>, u32)>,
    /// the type pass: a variable read before anything is known about it
    dry: bool,
    /// the line of the statement being compiled (for messages)
    line: u32,
}

fn err<T>(line: u32, prog: &Program, m: impl Into<String>) -> R<T> { Err(format!("{}: {}", prog.loc(line), m.into())) }

impl<'p> Em<'p> {
    pub fn new(prog: &'p Arc<Program>) -> Em<'p> {
        let sh = Shared::new(prog.clone(), false, true);
        let consts = prog.consts.read().unwrap().keys().cloned().collect();
        Em {
            prog, it: Interp::new(sh, 0, "emu".into()), regs: vec![], ops: vec![], vars: HashMap::new(), untyped: HashSet::new(), grew: false,
            slots: HashMap::new(), frame: 0, init: vec![], env: HashMap::new(), dirty: HashSet::new(), depth: 0, rename: 1_000_000, traps: vec![],
            rom: HashMap::new(), consts, local_arrays: HashMap::new(), port_bufs: HashMap::new(), data: vec![], dry: false, line: 0,
        }
    }

    fn here(&self, line: u32) -> String { self.prog.loc(line) }

    // ------------------------------------------------------------ registers and operations

    fn reg(&mut self, k: K) -> Reg { self.regs.push(k); (self.regs.len() - 1) as Reg }
    fn op(&mut self, o: Op) { self.ops.push(o); }

    /// the value in a register of kind `k` (a constant is loaded; N to W extends by the sign)
    fn in_kind(&mut self, s: &Sv, k: K) -> Reg {
        match s.v {
            V::C(c) => { let r = self.reg(k); let c = if k == K::N { c as i64 as i128 } else { c }; self.op(Op::Const(r, c)); r }
            V::R(r) => {
                let sk = self.regs[r as usize];
                if sk == k { return r; }
                let d = self.reg(k);
                if k == K::W { self.op(Op::Ext(d, r, s.signed())); } else { self.op(Op::Low(d, r)); }
                d
            }
        }
    }
    fn rg(&mut self, s: &Sv) -> Reg { let k = s.kind(); self.in_kind(s, k) }

    /// a result of `op` on registers of kind `k`, with range [lo, hi]
    #[allow(clippy::too_many_arguments)]
    fn bin(&mut self, op: BOp, k: K, a: Reg, b: Reg, lo: i128, hi: i128, frac: u32) -> Sv {
        let d = self.reg(if op.is_cmp() { K::N } else { k });
        self.op(Op::Bin(op, d, a, b));
        let d = if !op.is_cmp() && k == K::W && kind(lo, hi) == K::N { let n = self.reg(K::N); self.op(Op::Low(n, d)); n } else { d };
        Sv { v: V::R(d), lo, hi, boolean: false, frac }
    }

    fn kreg(&mut self, k: K, c: i128) -> Reg { self.in_kind(&Sv::konst(c), k) }

    fn trap(&mut self, line: u32, m: &str) -> u32 {
        let msg = format!("{}: {}", self.here(line), m);
        match self.traps.iter().position(|t| *t == msg) { Some(k) => k as u32, None => { self.traps.push(msg); (self.traps.len() - 1) as u32 } }
    }

    /// run-time check: stop with `m` when `bad` (an N register) is not 0
    fn check(&mut self, bad: Reg, line: u32, m: &str) {
        let t = self.trap(line, m);
        self.op(Op::If(bad, vec![Op::Trap(t)], vec![]));
    }

    // ------------------------------------------------------------ scalar arithmetic (ranges as fluxa-synth's ty.rs)

    fn upscale(&mut self, a: &Sv, f: u32) -> Sv { if a.frac >= f { a.clone() } else { Sv { frac: f, ..self.shl_const(a, f - a.frac) } } }
    fn align(&mut self, a: &Sv, b: &Sv) -> (Sv, Sv) { let f = a.frac.max(b.frac); (self.upscale(a, f), self.upscale(b, f)) }

    /// a + b, a - b, a * b (exact)
    fn arith(&mut self, op: BOp, a: &Sv, b: &Sv) -> Sv {
        let (a, b) = if op == BOp::Mul { (a.clone(), b.clone()) } else { self.align(a, b) };
        let (lo, hi, frac) = match op {
            BOp::Add => (a.lo + b.lo, a.hi + b.hi, a.frac),
            BOp::Sub => (a.lo - b.hi, a.hi - b.lo, a.frac),
            _ => {
                let p = [a.lo.saturating_mul(b.lo), a.lo.saturating_mul(b.hi), a.hi.saturating_mul(b.lo), a.hi.saturating_mul(b.hi)];
                (p.iter().copied().min().unwrap().max(-LIM), p.iter().copied().max().unwrap().min(LIM - 1), a.frac + b.frac)
            }
        };
        if let (V::C(x), V::C(y)) = (a.v, b.v) {
            let v = match op { BOp::Add => x + y, BOp::Sub => x - y, _ => x.wrapping_mul(y) };
            return Sv { v: V::C(v), lo: v, hi: v, boolean: false, frac };
        }
        let k = if kind(lo, hi) == K::W || a.kind() == K::W || b.kind() == K::W { K::W } else { K::N };
        let (ra, rb) = (self.in_kind(&a, k), self.in_kind(&b, k));
        self.bin(op, k, ra, rb, lo, hi, frac)
    }

    fn neg(&mut self, a: &Sv) -> Sv { self.arith(BOp::Sub, &Sv { frac: a.frac, ..Sv::konst(0) }, a) }

    /// a common way to compare or divide two values in N registers: signed, unsigned, or none (W)
    fn window(a: &Sv, b: &Sv) -> Option<bool> {
        let i63 = |s: &Sv| s.lo >= -(1i128 << 63) && s.hi < (1i128 << 63);
        let u64_ = |s: &Sv| s.lo >= 0 && s.hi < (1i128 << 64);
        if i63(a) && i63(b) { Some(true) } else if u64_(a) && u64_(b) { Some(false) } else { None }
    }

    fn divrem(&mut self, a: &Sv, b: &Sv, rem: bool, line: u32) -> R<Sv> {
        if a.frac > 0 || b.frac > 0 { return err(line, self.prog, "division of fixed-point values is not supported by fluxa-emu"); }
        let amax = a.lo.abs().max(a.hi.abs());
        let bmin = if b.lo <= 0 && b.hi >= 0 { 1 } else { b.lo.abs().min(b.hi.abs()).max(1) };
        let qmax = amax / bmin;
        let (qlo, qhi) = if a.signed() || b.signed() { (-qmax, qmax) } else { (a.lo / b.hi.max(1), qmax) };
        let rmax = (b.lo.abs().max(b.hi.abs()) - 1).max(0);
        let (rlo, rhi) = if a.signed() { (-rmax, rmax) } else { (0, rmax.min(a.hi)) };
        if let (V::C(x), V::C(y)) = (a.v, b.v) {
            if y == 0 { return err(line, self.prog, "division by zero"); }
            let v = if rem { x % y } else { x / y };
            return Ok(Sv::konst(v));
        }
        let (lo, hi) = if rem { (rlo, rhi) } else { (qlo, qhi) };
        let win = Self::window(a, b);
        let k = if win.is_some() && kind(lo, hi) == K::N { K::N } else { K::W };
        let (ra, rb) = (self.in_kind(a, k), self.in_kind(b, k));
        if b.lo <= 0 && b.hi >= 0 {
            let z = self.kreg(k, 0);
            let bad = self.reg(K::N);
            self.op(Op::Bin(BOp::Eq, bad, rb, z));
            self.check(bad, line, "division by zero");
        }
        let op = match (k, win, rem) {
            (K::N, Some(false), false) => BOp::DivU,
            (K::N, Some(false), true) => BOp::RemU,
            (_, _, false) => BOp::DivS,
            (_, _, true) => BOp::RemS,
        };
        Ok(self.bin(op, k, ra, rb, lo, hi, 0))
    }

    fn shl_const(&mut self, a: &Sv, k: u32) -> Sv {
        let (lo, hi) = (shl_sat(a.lo, k), shl_sat(a.hi, k));
        if let V::C(x) = a.v { let v = shl_sat(x, k); return Sv { v: V::C(v), lo: v, hi: v, boolean: false, frac: a.frac }; }
        if k == 0 { return Sv { lo, hi, boolean: false, ..a.clone() }; }
        let kk = if kind(lo, hi) == K::W || a.kind() == K::W { K::W } else { K::N };
        let ra = self.in_kind(a, kk);
        if k >= if kk == K::N { 64 } else { 128 } { return Sv { lo, hi, ..Sv::konst(0) }; }
        let rk = self.kreg(K::N, k as i128);
        self.bin(BOp::Shl, kk, ra, rk, lo, hi, a.frac)
    }

    /// arithmetic shift right (floor)
    fn shr_const(&mut self, a: &Sv, k: u32) -> Sv {
        let (lo, hi) = (a.lo >> k.min(127), a.hi >> k.min(127));
        if let V::C(x) = a.v { let v = x >> k.min(127); return Sv { v: V::C(v), lo: v, hi: v, boolean: false, frac: a.frac }; }
        if k == 0 { return Sv { boolean: false, ..a.clone() }; }
        let kk = a.kind();
        let width = if kk == K::N { 64 } else { 128 };
        let ra = self.rg(a);
        let (op, k) = if k >= width { if a.signed() { (BOp::ShrS, width - 1) } else { return Sv { lo, hi, ..Sv::konst(0) }; } } else if kk == K::N && !a.signed() { (BOp::ShrU, k) } else { (BOp::ShrS, k) };
        let rk = self.kreg(K::N, k as i128);
        self.bin(op, kk, ra, rk, lo, hi, a.frac)
    }

    /// a shift by a run-time amount (the range assumes the largest shift the amount allows)
    fn shift(&mut self, a: &Sv, amt: &Sv, left: bool, line: u32) -> R<Sv> {
        if amt.lo < 0 { return err(line, self.prog, "a shift amount must not be negative"); }
        if let V::C(k) = amt.v { return Ok(if left { self.shl_const(a, k.min(127) as u32) } else { self.shr_const(a, k.min(127) as u32) }); }
        let (lo, hi) = if left {
            let k = amt.hi.min(120) as u32;
            (a.lo.min(shl_sat(a.lo, k)), a.hi.max(shl_sat(a.hi, k)))
        } else { (a.lo.min(0), a.hi.max(0)) };
        let kk = if left && (kind(lo, hi) == K::W || a.kind() == K::W) { K::W } else { a.kind() };
        let width: i128 = if kk == K::N { 64 } else { 128 };
        let ra = self.in_kind(a, kk);
        // the amount as an N register below the width; a larger one gives 0 (left, or right of a
        // non-negative value) or the sign (right)
        let am = self.rg(amt);
        let am = if amt.kind() == K::W { let n = self.reg(K::N); self.op(Op::Low(n, am)); n } else { am };
        let big = amt.hi >= width || amt.kind() == K::W;
        let op = if left { BOp::Shl } else if kk == K::N && !a.signed() { BOp::ShrU } else { BOp::ShrS };
        if !big { return Ok(self.bin(op, kk, ra, am, lo, hi, a.frac)); }
        let wl = self.kreg(K::N, width);
        let inr = self.reg(K::N);
        // amounts are below 2^64 here unless the amount register is W (then its low bits: a shift
        // amount that large is a program past Fluxa's integers)
        self.op(Op::Bin(BOp::LtU, inr, am, wl));
        let top = self.kreg(K::N, width - 1);
        let cl = self.reg(K::N);
        self.op(Op::Sel(cl, inr, am, top));
        let s = self.bin(op, kk, ra, cl, lo, hi, a.frac);
        if op == BOp::ShrS { return Ok(s); }
        let z = self.kreg(s.kind(), 0);
        let sr = self.rg(&s);
        let d = self.reg(s.kind());
        self.op(Op::Sel(d, inr, sr, z));
        Ok(Sv { v: V::R(d), ..s })
    }

    /// &, |, ^: the result has the operands' common width
    fn bitop(&mut self, a: &Sv, b: &Sv, op: BOp) -> Sv {
        if a.boolean && b.boolean {
            if let (V::C(x), V::C(y)) = (a.v, b.v) { return Sv::bool_const(match op { BOp::And => x & y, BOp::Or => x | y, _ => x ^ y } != 0); }
            let (ra, rb) = (self.in_kind(a, K::N), self.in_kind(b, K::N));
            return Sv { boolean: true, ..self.bin(op, K::N, ra, rb, 0, 1, 0) };
        }
        let w = int_bits(a.lo.min(b.lo), a.hi.max(b.hi));
        let (lo, hi) = if a.signed() || b.signed() { (-(1i128 << (w - 1).min(126)), (1i128 << (w - 1).min(126)) - 1) } else { (0, (1i128 << w.min(126)) - 1) };
        if let (V::C(x), V::C(y)) = (a.v, b.v) { return Sv { lo, hi, ..Sv::konst(match op { BOp::And => x & y, BOp::Or => x | y, _ => x ^ y }) }; }
        let k = kind(lo, hi);
        let (ra, rb) = (self.in_kind(a, k), self.in_kind(b, k));
        self.bin(op, k, ra, rb, lo, hi, 0)
    }

    fn not_bits(&mut self, a: &Sv) -> Sv {
        if a.boolean { return self.not_bool(a); }
        let (lo, hi) = (-a.hi - 1, -a.lo - 1);
        if let V::C(x) = a.v { return Sv::konst(!x); }
        let k = if kind(lo, hi) == K::W || a.kind() == K::W { K::W } else { K::N };
        let ra = self.in_kind(a, k);
        let m = self.kreg(k, -1);
        self.bin(BOp::Xor, k, ra, m, lo, hi, 0)
    }

    fn not_bool(&mut self, b: &Sv) -> Sv {
        if let V::C(x) = b.v { return Sv::bool_const(x == 0); }
        let r = self.rg(b);
        let z = self.kreg(b.kind(), 0);
        Sv { boolean: true, ..self.bin(BOp::Eq, b.kind(), r, z, 0, 1, 0) }
    }

    /// a < b, a == b, ...
    fn compare(&mut self, a: &Sv, b: &Sv, op: BinOp) -> Sv {
        let (a, b) = self.align(a, b);
        if let (V::C(x), V::C(y)) = (a.v, b.v) {
            return Sv::bool_const(match op { BinOp::Eq => x == y, BinOp::Ne => x != y, BinOp::Lt => x < y, BinOp::Le => x <= y, BinOp::Gt => x > y, _ => x >= y });
        }
        // a comparison the ranges decide
        let known = match op {
            BinOp::Lt => if a.hi < b.lo { Some(true) } else if a.lo >= b.hi { Some(false) } else { None },
            BinOp::Le => if a.hi <= b.lo { Some(true) } else if a.lo > b.hi { Some(false) } else { None },
            BinOp::Gt => if a.lo > b.hi { Some(true) } else if a.hi <= b.lo { Some(false) } else { None },
            BinOp::Ge => if a.lo >= b.hi { Some(true) } else if a.hi < b.lo { Some(false) } else { None },
            BinOp::Eq => if a.hi < b.lo || b.hi < a.lo { Some(false) } else { None },
            _ => if a.hi < b.lo || b.hi < a.lo { Some(true) } else { None },
        };
        if let Some(t) = known { return Sv::bool_const(t); }
        let win = Self::window(&a, &b);
        let k = if win.is_some() { K::N } else { K::W };
        let signed = win.unwrap_or(true);
        let (ra, rb) = (self.in_kind(&a, k), self.in_kind(&b, k));
        let (o, x, y) = match op {
            BinOp::Eq => (BOp::Eq, ra, rb),
            BinOp::Ne => (BOp::Ne, ra, rb),
            BinOp::Lt => (if signed { BOp::LtS } else { BOp::LtU }, ra, rb),
            BinOp::Le => (if signed { BOp::LeS } else { BOp::LeU }, ra, rb),
            BinOp::Gt => (if signed { BOp::LtS } else { BOp::LtU }, rb, ra),
            _ => (if signed { BOp::LeS } else { BOp::LeU }, rb, ra),
        };
        Sv { boolean: true, ..self.bin(o, k, x, y, 0, 1, 0) }
    }

    /// c ? a : b
    fn select(&mut self, c: &Sv, a: &Sv, b: &Sv) -> Sv {
        if let V::C(x) = c.v { return if x != 0 { a.clone() } else { b.clone() }; }
        let (a, b) = self.align(a, b);
        if a.v == b.v { return Sv { lo: a.lo.min(b.lo), hi: a.hi.max(b.hi), ..a }; }
        let (lo, hi) = (a.lo.min(b.lo), a.hi.max(b.hi));
        let k = kind(lo, hi);
        let (ra, rb) = (self.in_kind(&a, k), self.in_kind(&b, k));
        let cr = self.truth(c);
        let d = self.reg(k);
        self.op(Op::Sel(d, cr, ra, rb));
        Sv { v: V::R(d), lo, hi, boolean: a.boolean && b.boolean, frac: a.frac }
    }

    /// the bool a value stands for, in an N register (0 / 1, or any non-zero for a boolean)
    fn truth(&mut self, v: &Sv) -> Reg {
        if v.boolean { return self.in_kind(v, K::N); }
        let r = self.rg(v);
        let z = self.kreg(v.kind(), 0);
        let d = self.reg(K::N);
        self.op(Op::Bin(BOp::Ne, d, r, z));
        d
    }
    fn truth_sv(&mut self, v: &Sv) -> Sv {
        if v.boolean { return v.clone(); }
        if let V::C(x) = v.v { return Sv::bool_const(x != 0); }
        let r = self.truth(v);
        Sv { v: V::R(r), lo: 0, hi: 1, boolean: true, frac: 0 }
    }

    /// the raw integer shifted right by `s` bits, rounded as the simulator's `round_shift` does
    fn round_shift(&mut self, a: &Sv, s: u32, r: Round) -> Sv {
        if s == 0 { return a.clone(); }
        let raw = Sv { frac: 0, ..a.clone() };
        let q = match r {
            Round::Trunc => self.shr_const(&raw, s),
            Round::Nearest => { let t = self.arith(BOp::Add, &raw, &Sv::konst(1i128 << (s - 1))); self.shr_const(&t, s) }
            Round::Even => {
                let q = self.shr_const(&raw, s);
                let mask = Sv { lo: 0, hi: (1i128 << s) - 1, ..Sv::konst((1i128 << s) - 1) };
                let rem = self.bitop(&raw, &mask, BOp::And);
                let rem = Sv { lo: 0, hi: (1i128 << s) - 1, ..rem };
                let half = Sv::konst(1i128 << (s - 1));
                let gt = self.compare(&rem, &half, BinOp::Gt);
                let eq = self.compare(&rem, &half, BinOp::Eq);
                let one = Sv { lo: 0, hi: 1, ..Sv::konst(1) };
                let odd = self.bitop(&q, &one, BOp::And);
                let odd = self.truth_sv(&Sv { lo: 0, hi: 1, ..odd });
                let e = self.bitop(&eq, &odd, BOp::And);
                let up = self.bitop(&gt, &e, BOp::Or);
                self.arith(BOp::Add, &q, &Sv { boolean: false, ..up })
            }
        };
        Sv { frac: a.frac, ..q }
    }

    /// keep the low bits of `v` as type range [lo, hi] (two's complement, the type's sign)
    fn wrap_to(&mut self, v: &Sv, lo: i128, hi: i128, frac: u32) -> Sv {
        if v.lo >= lo && v.hi <= hi {
            // fits: only the kind may change
            let k = kind(lo, hi);
            let r = match v.v { V::C(c) => return Sv { v: V::C(c), lo, hi, boolean: false, frac }, _ => self.in_kind(v, k) };
            return Sv { v: V::R(r), lo, hi, boolean: false, frac };
        }
        let w = int_bits(lo, hi);
        let signed = lo < 0;
        if let V::C(c) = v.v {
            let m = if w >= 127 { c } else { let u = c & ((1i128 << w) - 1); if signed && w > 0 && (u >> (w - 1)) & 1 == 1 { u - (1i128 << w) } else { u } };
            return Sv { v: V::C(m), lo, hi, boolean: false, frac };
        }
        let k = kind(lo, hi);
        let width = if k == K::N { 64 } else { 128 };
        let r = self.in_kind(v, k);
        if w >= width { return Sv { v: V::R(r), lo, hi, boolean: false, frac }; }
        if signed {
            let sh = self.kreg(K::N, (width - w) as i128);
            let t = self.reg(k);
            self.op(Op::Bin(BOp::Shl, t, r, sh));
            let d = self.reg(k);
            self.op(Op::Bin(BOp::ShrS, d, t, sh));
            Sv { v: V::R(d), lo, hi, boolean: false, frac }
        } else {
            let m = self.kreg(k, if w == 0 { 0 } else { (1i128 << w) - 1 });
            let d = self.reg(k);
            self.op(Op::Bin(BOp::And, d, r, m));
            Sv { v: V::R(d), lo, hi, boolean: false, frac }
        }
    }

    /// convert a scalar to scalar type `t` (fluxa-synth's `convert`)
    fn convert(&mut self, v: &Sv, t: &Ty, round: Round, ovf: Ovf) -> Sv {
        if *t == Ty::Bool { return self.truth_sv(v); }
        let tf = t.frac();
        let x = if v.frac > tf { let r = self.round_shift(v, v.frac - tf, round); Sv { frac: tf, ..r } } else { self.upscale(v, tf) };
        let (lo, hi) = t.range();
        let x = if ovf == Ovf::Sat && (x.lo < lo || x.hi > hi) {
            let mut y = x.clone();
            if x.hi > hi { let k = Sv { frac: tf, ..Sv::konst(hi) }; let over = self.compare(&y, &k, BinOp::Gt); y = self.select(&over, &k, &y); }
            if x.lo < lo { let k = Sv { frac: tf, ..Sv::konst(lo) }; let under = self.compare(&y, &k, BinOp::Lt); y = self.select(&under, &k, &y); }
            y
        } else { x };
        // check (the simulator's E0301): a value outside the type stops the program
        if ovf == Ovf::Check && (x.lo < lo || x.hi > hi) && x.as_const().is_none_or(|c| c < lo || c > hi) {
            let (kl, kh) = (Sv { frac: tf, ..Sv::konst(lo) }, Sv { frac: tf, ..Sv::konst(hi) });
            let under = self.compare(&x, &kl, BinOp::Lt);
            let over = self.compare(&x, &kh, BinOp::Gt);
            let bad = self.bitop(&under, &over, BOp::Or);
            let msg = format!("a value does not fit {}", ty_name(t));
            match bad.as_const() {
                Some(0) => {}
                Some(_) => { let k = self.trap(self.line, &msg); self.op(Op::Trap(k)); }
                None => { let r = self.rg(&bad); let line = self.line; self.check(r, line, &msg); }
            }
        }
        self.wrap_to(&x, lo, hi, tf)
    }

    /// the value as type `t` where it lands (`let x: T = e`, an argument, a variable): the type's
    /// own rounding (or trunc) and wrap
    fn fit(&mut self, v: &Sv, t: &Ty) -> Sv {
        let r = if let Ty::Fixed { round, .. } = t { *round } else { Round::Trunc };
        self.convert(v, t, r, Ovf::Check)
    }

    /// `fit`, keeping a narrower range than `t`'s when the value has one (arguments, results)
    fn fit_keep(&mut self, v: &Sv, t: &Ty) -> Sv {
        let (lo, hi) = t.range();
        if t.is_scalar() && *t != Ty::Bool && v.frac == t.frac() && !v.boolean && v.lo >= lo && v.hi <= hi { return v.clone(); }
        if *t == Ty::Bool && v.boolean { return v.clone(); }
        self.fit(v, t)
    }

    // ------------------------------------------------------------ values in the frame

    fn load_leaf(&mut self, base: u32, off: Option<Reg>, t: &Ty) -> Sv {
        let k = leaf_kind(t);
        let d = self.reg(k);
        self.op(Op::Load(d, Addr { base, off }));
        let (lo, hi) = t.range();
        Sv { v: V::R(d), lo, hi, boolean: *t == Ty::Bool, frac: t.frac() }
    }

    /// the value at place `p` (an array stays in the frame)
    fn read(&mut self, p: &Place) -> EV {
        match &p.ty {
            Ty::Array(..) => EV::M(p.clone()),
            Ty::Tuple(ts) => {
                let mut at = p.base;
                let mut out = vec![];
                for t in ts { out.push(self.read(&Place { base: at, off: p.off, ty: t.clone() })); at += size(t); }
                EV::A(out)
            }
            t => EV::S(self.load_leaf(p.base, p.off, t)),
        }
    }

    /// `v` into place `p` (converted to its type)
    fn write(&mut self, p: &Place, v: &EV, line: u32) -> R<()> {
        match (&p.ty, v) {
            (Ty::Array(..), EV::M(q)) => {
                if q == p { return Ok(()); }
                let n = size(&p.ty);
                if size(&q.ty) == n && leaves(&q.ty).iter().map(leaf_kind).eq(leaves(&p.ty).iter().map(leaf_kind)) && leaves(&q.ty) == leaves(&p.ty) {
                    self.op(Op::Copy(Addr { base: p.base, off: p.off }, Addr { base: q.base, off: q.off }, n));
                    return Ok(());
                }
                let xs = self.elems(v, line)?;
                self.write(p, &EV::A(xs), line)
            }
            (Ty::Array(e, n), EV::A(xs)) => {
                if xs.len() != *n { return err(line, self.prog, format!("an array of {} elements where {} are needed", xs.len(), n)); }
                let s = size(e);
                for (k, x) in xs.iter().enumerate() { self.write(&Place { base: p.base + k as u32 * s, off: p.off, ty: (**e).clone() }, x, line)?; }
                Ok(())
            }
            (Ty::Tuple(ts), EV::A(xs)) if ts.len() == xs.len() => {
                let mut at = p.base;
                for (t, x) in ts.iter().zip(xs) { self.write(&Place { base: at, off: p.off, ty: t.clone() }, x, line)?; at += size(t); }
                Ok(())
            }
            (t, EV::S(s)) if t.is_scalar() => {
                let f = self.fit(s, t);
                let r = self.in_kind(&f, leaf_kind(t));
                self.op(Op::Store(Addr { base: p.base, off: p.off }, r));
                Ok(())
            }
            _ => err(line, self.prog, format!("a value that does not fit {:?}", p.ty)),
        }
    }

    /// the elements of an array value
    fn elems(&mut self, v: &EV, line: u32) -> R<Vec<EV>> {
        match v {
            EV::A(xs) => Ok(xs.clone()),
            EV::M(p) => match &p.ty {
                Ty::Array(e, n) => { let s = size(e); Ok((0..*n).map(|k| self.read(&Place { base: p.base + k as u32 * s, off: p.off, ty: (**e).clone() })).collect()) }
                _ => err(line, self.prog, "not an array"),
            },
            EV::S(_) => err(line, self.prog, "a number where an array is needed"),
        }
    }

    /// frame bytes for a value of type `t`
    fn alloc(&mut self, t: &Ty) -> u32 {
        let a = self.frame;
        self.frame += size(t).max(8);
        a
    }

    /// the type a value has
    fn ev_ty(&self, v: &EV) -> Ty {
        match v {
            EV::S(s) => s.ty(),
            EV::M(p) => p.ty.clone(),
            EV::A(xs) => {
                if xs.is_empty() { return Ty::Array(Box::new(Ty::Int { lo: 0, hi: 0 }), 0); }
                let ts: Vec<Ty> = xs.iter().map(|x| self.ev_ty(x)).collect();
                if ts.iter().all(|t| t.is_scalar()) || ts.iter().all(|t| same_shape(t, &ts[0])) {
                    let e = ts.iter().skip(1).fold(ts[0].clone(), |a, b| widen(Some(a), b.clone()));
                    Ty::Array(Box::new(e), xs.len())
                } else { Ty::Tuple(ts) }
            }
        }
    }

    /// `v` held in registers or in the frame as type `t` (a variable's new value)
    fn fit_ev(&mut self, v: &EV, t: &Ty, line: u32) -> R<EV> {
        match (v, t) {
            (EV::S(s), t) if t.is_scalar() => Ok(EV::S(self.fit(s, t))),
            (EV::A(xs), Ty::Tuple(ts)) if xs.len() == ts.len() => Ok(EV::A(xs.iter().zip(ts).map(|(x, t)| self.fit_ev(x, t, line)).collect::<R<_>>()?)),
            (EV::A(xs), Ty::Array(e, n)) if xs.len() == *n && !has_array(t) => Ok(EV::A(xs.iter().map(|x| self.fit_ev(x, e, line)).collect::<R<_>>()?)),
            (EV::M(_), Ty::Array(..)) | (EV::A(_), Ty::Array(..)) => Ok(v.clone()),
            _ => err(line, self.prog, format!("a value of type {:?} does not fit {:?}", self.ev_ty(v), t)),
        }
    }

    // ------------------------------------------------------------ variables

    fn ty(&mut self, t: &TypeExpr, line: u32) -> R<Ty> {
        let rt = block_on(self.it.eval_type(t)).map_err(|e| format!("{}: type: {:?}", self.here(line), e))?;
        Ty::of_rt(&rt).map_err(|m| format!("{}: {}", self.here(line), m))
    }

    fn ty_opt(&mut self, t: &TypeExpr, line: u32) -> R<Option<Ty>> {
        let rt = block_on(self.it.eval_type(t)).map_err(|e| format!("{}: type: {:?}", self.here(line), e))?;
        if matches!(rt, RtType::Any) { return Ok(None); }
        Ty::of_rt(&rt).map(Some).map_err(|m| format!("{}: {}", self.here(line), m))
    }

    fn konst(&mut self, e: &Expr) -> Option<Value> {
        let mut local = false;
        fluxa_synth::walk_paths(e, &mut |p| if p.contains('.') || p.contains('~') { local = true });
        if local { return None; }
        block_on(self.it.eval(e)).ok()
    }

    fn value(&mut self, v: &Value, line: u32) -> R<EV> {
        Ok(match v {
            Value::Int(n) => EV::S(Sv::konst(*n)),
            Value::Bool(b) => EV::S(Sv::bool_const(*b)),
            Value::Fixed(r, f) => EV::S(Sv { frac: *f, ..Sv::konst(*r) }),
            Value::Array(xs) | Value::Tuple(xs) => EV::A(xs.iter().map(|x| self.value(x, line)).collect::<R<_>>()?),
            Value::Range(lo, hi) => EV::A((*lo..*hi).map(|k| EV::S(Sv::konst(k))).collect()),
            Value::Str(_) | Value::Unit => EV::S(Sv::konst(0)),
            Value::Float(x) => {
                let f = (0..=60).find(|f| { let y = x * 2f64.powi(*f); y.fract() == 0.0 && y.abs() < 2f64.powi(100) });
                match f {
                    Some(f) => EV::S(Sv { frac: f as u32, ..Sv::konst((x * 2f64.powi(f)) as i128) }),
                    None => return err(line, self.prog, format!("the float {} is not a binary fraction", x)),
                }
            }
            other => return err(line, self.prog, format!("the constant {} is not supported by fluxa-emu", fluxa::value::kind(other))),
        })
    }

    /// is `name` one of the task's variables (in the frame between states)?
    fn is_var(&self, name: &str) -> bool { self.slots.contains_key(name) || (self.depth == 0 && !self.local_arrays.contains_key(name)) }

    /// declare `name` of type `t` (None: the value's own type, growing) with value `v`
    fn declare(&mut self, name: &str, t: Option<Ty>, v: EV, line: u32) -> R<()> {
        if self.depth > 0 {
            // a local of an inlined function: registers (an array: a frame place of its own)
            let v = match &t { Some(t) => self.fit_ev(&v, t, line)?, None => v };
            let held = match &t { Some(t) => t.clone(), None => self.ev_ty(&v) };
            if has_array(&held) {
                let p = match self.local_arrays.get(name) { Some(p) => p.clone(), None => { let b = self.alloc(&held); let p = Place { base: b, off: None, ty: held.clone() }; self.local_arrays.insert(name.into(), p.clone()); p } };
                self.write(&p, &v, line)?;
                self.env.insert(name.into(), EV::M(p));
            } else {
                self.env.insert(name.into(), v);
            }
            return Ok(());
        }
        let t = match t {
            Some(t) => { self.untyped.remove(name); self.vars.entry(name.into()).or_insert(t).clone() }
            None => {
                if !self.vars.contains_key(name) { self.untyped.insert(name.into()); }
                let cur = self.vars.get(name).cloned();
                let t = widen(cur.clone(), self.ev_ty(&v));
                if cur.as_ref() != Some(&t) { self.grew = true; self.vars.insert(name.into(), t.clone()); }
                t
            }
        };
        self.set_var(name, &t, &v, line)
    }

    /// task variable `name` (of type `t`) gets `v`
    fn set_var(&mut self, name: &str, t: &Ty, v: &EV, line: u32) -> R<()> {
        match self.slots.get(name).copied() {
            Some(base) if has_array(t) => { self.write(&Place { base, off: None, ty: t.clone() }, v, line)?; self.env.insert(name.into(), EV::M(Place { base, off: None, ty: t.clone() })); }
            _ if has_array(t) => { self.env.insert(name.into(), v.clone()); }
            _ => {
                let f = self.fit_ev(v, t, line)?;
                // in the frame at once (the value stays known on this path)
                if let Some(base) = self.slots.get(name).copied() { self.write(&Place { base, off: None, ty: t.clone() }, &f, line)?; }
                else { self.dirty.insert(name.into()); }
                self.env.insert(name.into(), f);
            }
        }
        Ok(())
    }

    /// a task variable in the frame not known on this path: loaded where it is read (an array
    /// stays in the frame)
    fn load_var(&mut self, name: &str) -> Option<EV> {
        let (base, t) = (*self.slots.get(name)?, self.vars.get(name)?.clone());
        let v = self.read(&Place { base, off: None, ty: t });
        self.env.insert(name.into(), v.clone());
        Some(v)
    }

    /// is `name` a task variable that lives in the frame (written through, loaded when read)?
    fn in_frame(&self, name: &str) -> bool { self.slots.contains_key(name) && self.vars.get(name).is_some_and(|t| !has_array(t)) }

    /// `name` (whole, or the element / field at `path`) gets `v`
    fn store(&mut self, name: &str, path: &[Step], v: EV, line: u32) -> R<()> {
        if path.is_empty() {
            if self.depth > 0 || !self.is_var(name) {
                if let Some(EV::M(p)) = self.env.get(name).cloned() { return self.write(&p, &v, line); }
                self.env.insert(name.into(), v);
                return Ok(());
            }
            if !self.vars.contains_key(name) && self.untyped.contains(name) { return self.declare(name, None, v, line); }
            let t = match self.vars.get(name) { Some(t) => t.clone(), None => return err(line, self.prog, format!("assignment to `{}`, which is not a variable here", base(name))) };
            if self.untyped.contains(name) {
                let w = widen(Some(t.clone()), self.ev_ty(&v));
                if w != t { self.grew = true; self.vars.insert(name.into(), w); }
            }
            let t = self.vars[name].clone();
            return self.set_var(name, &t, &v, line);
        }
        if !self.env.contains_key(name) && self.slots.contains_key(name) { self.load_var(name); }
        let cur = match self.env.get(name).cloned() { Some(c) => c, None => return err(line, self.prog, format!("assignment to `{}`, which has no value here", base(name))) };
        match cur {
            EV::M(p) => {
                let target = self.place_at(&p, path, line)?;
                self.write(&target, &v, line)
            }
            cur => {
                let new = self.put(&cur, path, v, line)?;
                if self.depth == 0 && self.is_var(name) {
                    let t = self.vars.get(name).cloned().ok_or_else(|| format!("{}: `{}` is not a variable", self.here(line), base(name)))?;
                    self.set_var(name, &t, &new, line)
                } else { self.env.insert(name.into(), new); Ok(()) }
            }
        }
    }

    /// the place of the element / field at `path` inside place `p`
    fn place_at(&mut self, p: &Place, path: &[Step], line: u32) -> R<Place> {
        let mut p = p.clone();
        for s in path {
            p = match (&p.ty.clone(), s) {
                (Ty::Array(e, n), Step::Const(k)) => {
                    if *k >= *n { return err(line, self.prog, format!("index {} out of bounds (length {})", k, n)); }
                    Place { base: p.base + *k as u32 * size(e), off: p.off, ty: (**e).clone() }
                }
                (Ty::Array(e, n), Step::Var(i)) => {
                    let off = self.index_off(i, *n, size(e), line)?;
                    let off = match p.off { None => off, Some(o) => { let d = self.reg(K::N); self.op(Op::Bin(BOp::Add, d, o, off)); d } };
                    Place { base: p.base, off: Some(off), ty: (**e).clone() }
                }
                (Ty::Tuple(ts), Step::Const(k)) if *k < ts.len() => {
                    let at: u32 = ts[..*k].iter().map(size).sum();
                    Place { base: p.base + at, off: p.off, ty: ts[*k].clone() }
                }
                _ => return err(line, self.prog, "indexing something that is not an array"),
            };
        }
        Ok(p)
    }

    /// the byte offset of element `i` (checked against `n`) of `stride` bytes
    fn index_off(&mut self, i: &Sv, n: usize, stride: u32, line: u32) -> R<Reg> {
        if i.frac > 0 { return err(line, self.prog, "a fixed-point index"); }
        let ir = self.rg(i);
        if i.lo < 0 || i.hi >= n as i128 {
            let k = i.kind();
            let nn = self.kreg(k, n as i128);
            let bad = self.reg(K::N);
            // unsigned: a negative index is a huge one
            self.op(Op::Bin(if k == K::N { BOp::LeU } else { BOp::LeS }, bad, nn, ir));
            if k == K::W || i.lo < 0 {
                let z = self.kreg(k, 0);
                let neg = self.reg(K::N);
                self.op(Op::Bin(BOp::LtS, neg, ir, z));
                let b2 = self.reg(K::N);
                self.op(Op::Bin(BOp::Or, b2, bad, neg));
                self.check(b2, line, "index out of bounds");
            } else {
                self.check(bad, line, "index out of bounds");
            }
        }
        let ir = if i.kind() == K::W { let d = self.reg(K::N); self.op(Op::Low(d, ir)); d } else { ir };
        if stride == 1 { return Ok(ir); }
        let s = self.kreg(K::N, stride as i128);
        let d = self.reg(K::N);
        self.op(Op::Bin(BOp::Mul, d, ir, s));
        Ok(d)
    }

    /// `cur` (a value in registers) with the part at `path` replaced by `v`
    fn put(&mut self, cur: &EV, path: &[Step], v: EV, line: u32) -> R<EV> {
        let Some((step, rest)) = path.split_first() else { return Ok(v) };
        let EV::A(xs) = cur else {
            if let EV::M(p) = cur { let t = self.place_at(p, path, line)?; self.write(&t, &v, line)?; return Ok(cur.clone()); }
            return err(line, self.prog, "indexing something that is not an array");
        };
        let mut out = xs.clone();
        match step {
            Step::Const(k) => {
                if *k >= out.len() { return err(line, self.prog, format!("index {} out of bounds (length {})", k, out.len())); }
                out[*k] = self.put(&xs[*k], rest, v, line)?;
            }
            Step::Var(i) => {
                for k in 0..out.len() {
                    if i.hi < k as i128 || i.lo > k as i128 { continue; }
                    let hit = self.compare(i, &Sv::konst(k as i128), BinOp::Eq);
                    let nv = self.put(&xs[k], rest, v.clone(), line)?;
                    out[k] = self.select_ev(&hit, &nv, &xs[k], line)?;
                }
            }
        }
        Ok(EV::A(out))
    }

    fn select_ev(&mut self, c: &Sv, a: &EV, b: &EV, line: u32) -> R<EV> {
        match (a, b) {
            (EV::S(x), EV::S(y)) => Ok(EV::S(self.select(c, x, y))),
            (EV::A(xs), EV::A(ys)) if xs.len() == ys.len() => Ok(EV::A(xs.iter().zip(ys).map(|(x, y)| self.select_ev(c, x, y, line)).collect::<R<_>>()?)),
            (a, b) => { let (xs, ys) = (self.elems(a, line)?, self.elems(b, line)?); self.select_ev(c, &EV::A(xs), &EV::A(ys), line) }
        }
    }

    // ------------------------------------------------------------ control flow

    /// `then` when `c` holds, else `els`; the variables and the result merged after
    fn branch(&mut self, c: &Sv, cond: Option<&Expr>, then: &mut dyn FnMut(&mut Self) -> R<EV>, els: &mut dyn FnMut(&mut Self) -> R<EV>, line: u32) -> R<EV> {
        if let Some(k) = c.as_const() { return if k != 0 { then(self) } else { els(self) }; }
        let cr = self.truth(c);
        let before = self.env.clone();
        let outer = std::mem::take(&mut self.ops);
        let nt = match cond { Some(e) => self.narrow(e, true), None => vec![] };
        let tv = then(self)?;
        unnarrow(&mut self.env, &nt);
        let tops = std::mem::take(&mut self.ops);
        let tenv = std::mem::replace(&mut self.env, before);
        let nf = match cond { Some(e) => self.narrow(e, false), None => vec![] };
        let fv = els(self)?;
        unnarrow(&mut self.env, &nf);
        let fops = std::mem::take(&mut self.ops);
        let fenv = std::mem::take(&mut self.env);
        let (mut tops, mut fops) = (tops, fops);
        let mut env = HashMap::new();
        let mut names: Vec<&String> = tenv.keys().chain(fenv.keys()).collect();
        names.sort();
        names.dedup();
        for n in names {
            let (a, b) = (tenv.get(n), fenv.get(n));
            // a variable in the frame: known after the arms only when they agree (else loaded again)
            if self.in_frame(n) {
                if let (Some(a), Some(b)) = (a, b) { if a == b { env.insert(n.clone(), a.clone()); } }
                continue;
            }
            let v = match (a, b) {
                (Some(a), Some(b)) => self.merge(a, b, &mut tops, &mut fops, line)?,
                (Some(a), None) | (None, Some(a)) => a.clone(),
                _ => unreachable!(),
            };
            env.insert(n.clone(), v);
        }
        let v = self.merge(&tv, &fv, &mut tops, &mut fops, line).unwrap_or(tv);
        self.ops = outer;
        self.op(Op::If(cr, tops, fops));
        self.env = env;
        Ok(v)
    }

    /// one value from two arms: a register both arms set (their conversions in the arms)
    fn merge(&mut self, a: &EV, b: &EV, tops: &mut Vec<Op>, fops: &mut Vec<Op>, line: u32) -> R<EV> {
        if a == b { return Ok(a.clone()); }
        match (a, b) {
            (EV::S(x), EV::S(y)) => {
                let f = x.frac.max(y.frac);
                let (lo, hi) = (x.lo.min(y.lo) << (f - x.frac.min(y.frac)).min(64), x.hi.max(y.hi) << (f - x.frac.min(y.frac)).min(64));
                let k = kind(lo, hi);
                let d = self.reg(k);
                for (s, ops) in [(x, &mut *tops), (y, &mut *fops)] {
                    let saved = std::mem::replace(&mut self.ops, std::mem::take(ops));
                    let s2 = self.upscale(s, f);
                    let r = self.in_kind(&s2, k);
                    self.op(Op::Mov(d, r));
                    *ops = std::mem::replace(&mut self.ops, saved);
                }
                Ok(EV::S(Sv { v: V::R(d), lo, hi, boolean: x.boolean && y.boolean, frac: f }))
            }
            (EV::A(xs), EV::A(ys)) if xs.len() == ys.len() => Ok(EV::A(xs.iter().zip(ys).map(|(x, y)| self.merge(x, y, tops, fops, line)).collect::<R<_>>()?)),
            _ => {
                // arrays from different places: a copy each arm writes
                let t = self.ev_ty(a);
                let base = self.alloc(&t);
                let p = Place { base, off: None, ty: t };
                for (s, ops) in [(a, &mut *tops), (b, &mut *fops)] {
                    let saved = std::mem::replace(&mut self.ops, std::mem::take(ops));
                    self.write(&p, s, line)?;
                    *ops = std::mem::replace(&mut self.ops, saved);
                }
                Ok(EV::M(p))
            }
        }
    }

    /// the variables condition `c` bounds when it is `truth`, narrowed in the environment
    fn narrow(&mut self, c: &Expr, truth: bool) -> Vec<(String, EV, EV)> {
        let mut out = vec![];
        self.bounds(c, truth, &mut out);
        out
    }

    fn bounds(&mut self, c: &Expr, truth: bool, out: &mut Vec<(String, EV, EV)>) {
        match &c.kind {
            ExprKind::Unary('!', x) => self.bounds(x, !truth, out),
            ExprKind::Binary(BinOp::And, l, r) if truth => { self.bounds(l, true, out); self.bounds(r, true, out); }
            ExprKind::Binary(BinOp::Or, l, r) if !truth => { self.bounds(l, false, out); self.bounds(r, false, out); }
            ExprKind::Binary(op, l, r) if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne) => {
                let op = if truth { *op } else { match op { BinOp::Lt => BinOp::Ge, BinOp::Le => BinOp::Gt, BinOp::Gt => BinOp::Le, BinOp::Ge => BinOp::Lt, BinOp::Eq => BinOp::Ne, _ => BinOp::Eq } };
                let flip = |o: BinOp| match o { BinOp::Lt => BinOp::Gt, BinOp::Le => BinOp::Ge, BinOp::Gt => BinOp::Lt, BinOp::Ge => BinOp::Le, o => o };
                self.bound(l, op, r, out);
                self.bound(r, flip(op), l, out);
            }
            _ => {}
        }
    }

    fn bound(&mut self, x: &Expr, op: BinOp, y: &Expr, out: &mut Vec<(String, EV, EV)>) {
        let ExprKind::Path(p) = &x.kind else { return };
        if p.len() != 1 { return; }
        let Some(EV::S(v)) = self.env.get(&p[0]).cloned() else { return };
        if v.boolean || v.frac > 0 { return; }
        let (ylo, yhi) = match &y.kind {
            ExprKind::Path(q) if q.len() == 1 && self.env.contains_key(&q[0]) => match self.env.get(&q[0]) { Some(EV::S(w)) if !w.boolean && w.frac == 0 => (w.lo, w.hi), _ => return },
            _ => match self.konst(y) { Some(Value::Int(k)) => (k, k), _ => return },
        };
        let (mut lo, mut hi) = (v.lo, v.hi);
        match op {
            BinOp::Lt => hi = hi.min(yhi.saturating_sub(1)),
            BinOp::Le => hi = hi.min(yhi),
            BinOp::Gt => lo = lo.max(ylo.saturating_add(1)),
            BinOp::Ge => lo = lo.max(ylo),
            BinOp::Eq => { lo = lo.max(ylo); hi = hi.min(yhi); }
            _ => if ylo == yhi { if lo == ylo { lo += 1 } else if hi == ylo { hi -= 1 } },
        }
        if lo > hi || (lo == v.lo && hi == v.hi) { return; }
        // a narrower range may need a narrower register
        let nv = match v.v {
            V::R(r) if self.regs[r as usize] != kind(lo, hi) => { let d = self.reg(kind(lo, hi)); self.op(Op::Low(d, r)); Sv { v: V::R(d), lo, hi, ..v.clone() } }
            _ => Sv { lo, hi, ..v.clone() },
        };
        let n = EV::S(nv);
        self.env.insert(p[0].clone(), n.clone());
        out.push((p[0].clone(), n, EV::S(v)));
    }

    // ------------------------------------------------------------ statements

    pub fn stmts(&mut self, ss: &[Stmt]) -> R<()> {
        for s in ss { self.stmt(s)?; }
        Ok(())
    }

    fn bind(&mut self, p: &Pattern, v: EV, t: Option<Ty>, line: u32) -> R<()> {
        match p {
            Pattern::Ident(n) => self.declare(n, t, v, line),
            Pattern::Wild => Ok(()),
            Pattern::Tuple(ps) => {
                let xs = match v { EV::A(xs) => xs, v @ EV::M(_) => self.elems(&v, line)?, _ => return err(line, self.prog, format!("cannot destructure this value into a {}-tuple", ps.len())) };
                if xs.len() != ps.len() { return err(line, self.prog, format!("cannot destructure this value into a {}-tuple", ps.len())); }
                for (p, x) in ps.iter().zip(xs) { self.bind(p, x, None, line)?; }
                Ok(())
            }
        }
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        let line = s.line;
        self.line = line;
        match &s.kind {
            StmtKind::Let { pat, ty, init, .. } => {
                let t = match ty { Some(t) => self.ty_opt(t, line)?, None => None };
                let v = match init {
                    Some(e) => self.eval(e)?,
                    None => match &t { Some(t) => self.zero(t), None => return err(line, self.prog, "`let` needs an initializer or a type") },
                };
                self.bind(pat, v, t, line)
            }
            StmtKind::Carry { name, ty, init } => {
                let t = self.ty(ty, line)?;
                let v = self.eval(init)?;
                self.declare(name, Some(t), v, line)
            }
            StmtKind::Assign { target, op, value } => {
                let (name, path) = self.target(target)?;
                let rhs = self.eval(value)?;
                let v = match op {
                    None => rhs,
                    Some(op) => {
                        let cur = self.eval(target)?;
                        let (a, b) = (cur.s()?.clone(), rhs.s()?.clone());
                        let wrap_ty = if matches!(op, BinOp::AddW | BinOp::SubW | BinOp::MulW) { Some(self.part_ty(&name, &path, line)?) } else { None };
                        EV::S(self.binop(*op, &a, &b, wrap_ty, line)?)
                    }
                };
                self.store(&name, &path, v, line)
            }
            StmtKind::Expr(e) => { self.eval(e)?; Ok(()) }
            StmtKind::Assert { cond, text, .. } => {
                let c = self.scalar(cond)?;
                if let Some(k) = c.as_const() {
                    if k == 0 { let t = self.trap(line, &format!("assertion failed: {}", text)); self.op(Op::Trap(t)); }
                    return Ok(());
                }
                let tc = self.truth_sv(&c);
                let b = self.not_bool(&tc);
                let r = self.rg(&b);
                self.check(r, line, &format!("assertion failed: {}", text));
                Ok(())
            }
            StmtKind::For { attr, pat, iter, body } => {
                let items = self.iterate(iter, line)?;
                if items.len() > 4096 { return err(line, self.prog, format!("{} iterations are too many to unroll", items.len())); }
                let commit = !matches!(attr, LoopAttr::Unroll(_));
                for it in items {
                    self.bind(pat, it, None, line)?;
                    self.stmts(&body.stmts)?;
                    if let Some(t) = &body.tail { self.eval(t)?; }
                    if commit { self.commit_regs(); }
                }
                Ok(())
            }
            StmtKind::Return(_) => err(line, self.prog, "`return` here is not supported by fluxa-emu yet"),
            StmtKind::Spawn(_) | StmtKind::Parallel(_) => err(line, self.prog, "spawn is not compiled here"),
        }
    }

    fn zero(&mut self, t: &Ty) -> EV {
        match t {
            Ty::Array(e, n) => EV::A((0..*n).map(|_| self.zero(e)).collect()),
            Ty::Tuple(ts) => EV::A(ts.iter().map(|t| self.zero(t)).collect()),
            t => { let (lo, hi) = t.range(); EV::S(Sv { v: V::C(0), lo: lo.min(0), hi: hi.max(0), boolean: *t == Ty::Bool, frac: t.frac() }) }
        }
    }

    fn iterate(&mut self, iter: &Expr, line: u32) -> R<Vec<EV>> {
        if let ExprKind::Range(lo, hi, incl) = &iter.kind {
            let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else {
                return err(line, self.prog, "a loop without channel operations needs constant bounds (fluxa-emu unrolls it)");
            };
            let h = if *incl { h + 1 } else { h };
            return Ok((l..h).map(|k| EV::S(Sv::konst(k))).collect());
        }
        let v = self.eval(iter)?;
        match v {
            EV::S(_) => err(line, self.prog, "a loop over something that is neither a range nor an array"),
            v => self.elems(&v, line),
        }
    }

    /// `@reg` variables written in a loop body: their pending values become current
    fn commit_regs(&mut self) {
        let pend: Vec<String> = self.env.keys().filter(|k| k.ends_with("~p")).cloned().collect();
        for p in pend {
            let v = self.env[&p].clone();
            let n = p.trim_end_matches("~p").to_string();
            match self.vars.get(&n).cloned() {
                Some(t) if self.depth == 0 && self.is_var(&n) => { let _ = self.set_var(&n, &t, &v, 0); }
                _ => { self.env.insert(n, v); }
            }
        }
    }

    fn target(&mut self, e: &Expr) -> R<(String, Vec<Step>)> {
        match &e.kind {
            ExprKind::Path(p) if p.len() == 1 => Ok((p[0].clone(), vec![])),
            ExprKind::Index(b, i) => {
                let (n, mut path) = self.target(b)?;
                let iv = self.scalar(i)?;
                path.push(match iv.as_const() { Some(k) if k >= 0 => Step::Const(k as usize), _ => Step::Var(iv) });
                Ok((n, path))
            }
            ExprKind::Field(b, f) => {
                let (n, mut path) = self.target(b)?;
                let k: usize = f.parse().map_err(|_| format!("{}: unknown field `{}`", self.here(e.line), f))?;
                path.push(Step::Const(k));
                Ok((n, path))
            }
            _ => err(e.line, self.prog, "invalid assignment target"),
        }
    }

    fn part_ty(&self, name: &str, path: &[Step], line: u32) -> R<Ty> {
        let mut t = match self.vars.get(name).cloned() {
            Some(t) => t,
            None => match self.env.get(name) { Some(v) => self.ev_ty(v), None => return Err(format!("{}: wrapping operators need a typed variable on the left", self.here(line))) },
        };
        for s in path {
            t = match (t, s) {
                (Ty::Array(e, _), _) => *e,
                (Ty::Tuple(ts), Step::Const(k)) => ts[*k].clone(),
                _ => return err(line, self.prog, "bad index"),
            };
        }
        Ok(t)
    }

    // ------------------------------------------------------------ expressions

    pub fn scalar(&mut self, e: &Expr) -> R<Sv> {
        let v = self.eval(e)?;
        v.s().cloned().map_err(|m| format!("{}: {}", self.here(e.line), m))
    }

    pub fn eval(&mut self, e: &Expr) -> R<EV> {
        let line = e.line;
        match &e.kind {
            ExprKind::Int(n) => Ok(EV::S(Sv::konst(*n))),
            ExprKind::Bool(b) => Ok(EV::S(Sv::bool_const(*b))),
            ExprKind::Float(_) => match self.konst(e) { Some(v) => self.value(&v, line), None => err(line, self.prog, "floating point is only for compile-time constants") },
            ExprKind::Path(p) => {
                let n = p.last().unwrap();
                if p.len() == 1 {
                    if let Some(v) = self.env.get(n) { return Ok(v.clone()); }
                    if self.slots.contains_key(n) { if let Some(v) = self.load_var(n) { return Ok(v); } }
                }
                if let Some(pl) = self.rom.get(n) { return Ok(EV::M(pl.clone())); }
                let c = self.prog.consts.read().unwrap().get(n).cloned();
                match c {
                    // a long constant array: in the frame, so a run-time index is a load
                    Some(Value::Array(xs)) if xs.len() > 16 && xs.iter().all(|x| matches!(x, Value::Int(_) | Value::Bool(_))) => {
                        let v = self.value(&Value::Array(xs.clone()), line)?;
                        let t = self.ev_ty(&v);
                        let base = self.alloc(&t);
                        if let (Ty::Array(et, _), EV::A(items)) = (&t, &v) {
                            let k = leaf_kind(et);
                            for (j, x) in items.iter().enumerate() { if let EV::S(s) = x { if let V::C(c) = s.v { self.init.push((base + j as u32 * k.size(), k, c)); } } }
                        }
                        let pl = Place { base, off: None, ty: t };
                        self.rom.insert(n.clone(), pl.clone());
                        Ok(EV::M(pl))
                    }
                    Some(v) => self.value(&v, line),
                    // the type pass: a variable whose type is not known yet (the next round knows it)
                    None if self.dry && p.len() == 1 && (n.contains('.') || n.contains('~')) => { self.grew = true; Ok(EV::S(Sv::konst(0))) }
                    None => err(line, self.prog, format!("`{}` has no value here (not a variable or constant)", base(n))),
                }
            }
            ExprKind::Unary(c, x) => {
                let v = self.scalar(x)?;
                Ok(EV::S(match c {
                    '-' => self.neg(&v),
                    '!' => { let t = self.truth_sv(&v); self.not_bool(&t) }
                    _ => self.not_bits(&v),
                }))
            }
            ExprKind::Binary(op, l, r) => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    let a = self.scalar(l)?;
                    let a = self.truth_sv(&a);
                    let and = *op == BinOp::And;
                    // the right side only when it matters (it may index what the left side checks)
                    let r = (**r).clone();
                    let mut right = |em: &mut Self| -> R<EV> { let b = em.scalar(&r)?; Ok(EV::S(em.truth_sv(&b))) };
                    let mut left = |_: &mut Self| -> R<EV> { Ok(EV::S(Sv::bool_const(!and))) };
                    let v = if and { self.branch(&a, Some(l), &mut right, &mut left, line)? } else { self.branch(&a, Some(l), &mut left, &mut right, line)? };
                    return Ok(match v { EV::S(s) => EV::S(Sv { boolean: true, ..s }), v => v });
                }
                let (a, b) = (self.scalar(l)?, self.scalar(r)?);
                let wrap_ty = if matches!(op, BinOp::AddW | BinOp::SubW | BinOp::MulW) {
                    let (n, path) = self.target(l).map_err(|_| format!("{}: wrapping operators need a typed variable on the left", self.here(line)))?;
                    Some(self.part_ty(&n, &path, line)?)
                } else { None };
                Ok(EV::S(self.binop(*op, &a, &b, wrap_ty, line)?))
            }
            ExprKind::Cast(x, t) => {
                let v = self.eval(x)?;
                let t = self.ty(t, line)?;
                self.convert_ev(&v, &t, Round::Trunc, Ovf::Wrap, line)
            }
            ExprKind::Call(c, a) => self.call(c, a, line),
            ExprKind::Method(r, n, t, a) => self.method(r, n, t, a, line),
            ExprKind::Field(x, f) => {
                let v = self.eval(x)?;
                let k: usize = f.parse().map_err(|_| format!("{}: unknown field `{}`", self.here(line), f))?;
                match v {
                    EV::A(xs) if k < xs.len() => Ok(xs[k].clone()),
                    EV::M(p) => { let q = self.place_at(&p, &[Step::Const(k)], line)?; Ok(self.read(&q)) }
                    _ => err(line, self.prog, format!("no field `{}`", f)),
                }
            }
            ExprKind::Index(x, i) => {
                let v = self.eval(x)?;
                if let ExprKind::Range(lo, hi, incl) = &i.kind {
                    let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else { return err(line, self.prog, "a slice needs constant bounds") };
                    let h = if *incl { h + 1 } else { h };
                    let xs = self.elems(&v, line)?;
                    if l < 0 || h < l || h as usize > xs.len() { return err(line, self.prog, format!("slice {}..{} out of bounds", l, h)); }
                    return Ok(EV::A(xs[l as usize..h as usize].to_vec()));
                }
                let k = self.scalar(i)?;
                match v {
                    EV::A(xs) => self.index(&xs, &k, line),
                    EV::M(p) => {
                        let step = match k.as_const() { Some(c) if c >= 0 => Step::Const(c as usize), _ => Step::Var(k) };
                        let q = self.place_at(&p, &[step], line)?;
                        Ok(self.read(&q))
                    }
                    EV::S(s) => {
                        // bit k
                        let sh = self.shift(&s, &k, false, line)?;
                        let one = Sv { lo: 0, hi: 1, ..Sv::konst(1) };
                        let b = self.bitop(&sh, &one, BOp::And);
                        Ok(EV::S(Sv { lo: 0, hi: 1, ..b }))
                    }
                }
            }
            ExprKind::Slice(x, h, l) => {
                let v = self.scalar(x)?;
                let (Some(h), Some(l)) = (self.scalar(h)?.as_const(), self.scalar(l)?.as_const()) else { return err(line, self.prog, "a bit slice needs constant bounds") };
                if l < 0 || h < l { return err(line, self.prog, format!("invalid bit slice [{}:{}]", h, l)); }
                let w = (h - l + 1) as u32;
                let s = self.shr_const(&Sv { frac: 0, ..v }, l as u32);
                Ok(EV::S(self.wrap_to(&s, 0, (1i128 << w) - 1, 0)))
            }
            ExprKind::Range(lo, hi, incl) => {
                let (Some(l), Some(h)) = (self.scalar(lo)?.as_const(), self.scalar(hi)?.as_const()) else { return err(line, self.prog, "a range used as a value needs constant bounds") };
                let h = if *incl { h + 1 } else { h };
                Ok(EV::A((l..h).map(|k| EV::S(Sv::konst(k))).collect()))
            }
            ExprKind::Array(xs) | ExprKind::Tuple(xs) => Ok(EV::A(xs.iter().map(|x| self.eval(x)).collect::<R<_>>()?)),
            ExprKind::Repeat(x, n) => {
                let v = self.eval(x)?;
                let Some(n) = self.scalar(n)?.as_const() else { return err(line, self.prog, "[x; n] needs a constant n") };
                Ok(EV::A(vec![v; n.max(0) as usize]))
            }
            ExprKind::Block(b) => {
                self.stmts(&b.stmts)?;
                match &b.tail { Some(t) => self.eval(t), None => Ok(EV::S(Sv::konst(0))) }
            }
            ExprKind::Comptime(_) => match self.konst(e) {
                Some(v) => self.value(&v, line),
                None => err(line, self.prog, "comptime block that reads run-time values"),
            },
            ExprKind::If(c, t, el) => {
                let cv = self.scalar(c)?;
                let t = t.clone();
                let el = el.clone();
                let mut then = |em: &mut Self| -> R<EV> { em.stmts(&t.stmts)?; match &t.tail { Some(x) => em.eval(x), None => Ok(EV::S(Sv::konst(0))) } };
                let mut els = |em: &mut Self| -> R<EV> { match &el { Some(x) => em.eval(x), None => Ok(EV::S(Sv::konst(0))) } };
                self.branch(&cv, Some(c), &mut then, &mut els, line)
            }
            ExprKind::Match(s, arms) => {
                let sv = self.scalar(s)?;
                self.matching(&sv, arms, line)
            }
            ExprKind::Str(_) => Ok(EV::S(Sv::konst(0))),
            _ => err(line, self.prog, "this expression is not supported by fluxa-emu yet"),
        }
    }

    /// the arms in order: the first that matches
    fn matching(&mut self, sv: &Sv, arms: &[Arm], line: u32) -> R<EV> {
        let Some((a, rest)) = arms.split_first() else { return Ok(EV::S(Sv::konst(0))) };
        let c = match &a.pat {
            MatchPat::Wild => Sv::bool_const(true),
            MatchPat::Bind(n) => {
                let c = self.prog.consts.read().unwrap().get(n).cloned();
                match c {
                    Some(v) => { let k = self.value(&v, line)?; self.compare(sv, k.s()?, BinOp::Eq) }
                    None => { self.declare(n, None, EV::S(sv.clone()), line)?; Sv::bool_const(true) }
                }
            }
            MatchPat::Lit(e) => { let v = self.scalar(e)?; self.compare(sv, &v, BinOp::Eq) }
            MatchPat::Range(lo, hi, incl) => {
                let (l, h) = (self.scalar(lo)?, self.scalar(hi)?);
                let a1 = self.compare(sv, &l, BinOp::Ge);
                let a2 = self.compare(sv, &h, if *incl { BinOp::Le } else { BinOp::Lt });
                self.bitop(&a1, &a2, BOp::And)
            }
            MatchPat::Tuple(_) => return err(line, self.prog, "tuple patterns are not supported by fluxa-emu yet"),
        };
        let a = a.clone();
        let rest = rest.to_vec();
        let sv2 = sv.clone();
        // a guard: only looked at when the pattern matches
        let c = match &a.guard {
            Some(g) if c.as_const() != Some(0) => {
                let g = g.clone();
                let mut yes = |em: &mut Self| -> R<EV> { let v = em.scalar(&g)?; Ok(EV::S(em.truth_sv(&v))) };
                let mut no = |_: &mut Self| -> R<EV> { Ok(EV::S(Sv::bool_const(false))) };
                match self.branch(&c, None, &mut yes, &mut no, line)? { EV::S(s) => s, _ => unreachable!() }
            }
            _ => c,
        };
        if rest.is_empty() && c.as_const().is_none() {
            // the last arm: taken when nothing else matched (a match is exhaustive)
            return self.eval(&a.body);
        }
        let mut then = |em: &mut Self| -> R<EV> { em.eval(&a.body) };
        let mut els = |em: &mut Self| -> R<EV> { em.matching(&sv2, &rest, line) };
        self.branch(&c, None, &mut then, &mut els, line)
    }

    /// element `k` of a list (a run-time index: a load from a copy in the frame, or a choice)
    fn index(&mut self, xs: &[EV], k: &Sv, line: u32) -> R<EV> {
        if let Some(c) = k.as_const() {
            return xs.get(c as usize).cloned().ok_or_else(|| format!("{}: index {} out of bounds (length {})", self.here(line), c, xs.len()));
        }
        if xs.is_empty() { return err(line, self.prog, "indexing an empty array"); }
        if xs.len() <= 8 {
            let t = xs.iter().skip(1).fold(self.ev_ty(&xs[0]), |a, b| widen(Some(a), self.ev_ty(b)));
            let mut out = self.fit_ev(&xs[0], &t, line)?;
            for (j, x) in xs.iter().enumerate().skip(1) {
                if k.hi < j as i128 || k.lo > j as i128 { continue; }
                let hit = self.compare(k, &Sv::konst(j as i128), BinOp::Eq);
                let xv = self.fit_ev(x, &t, line)?;
                out = self.select_ev(&hit, &xv, &out, line)?;
            }
            if k.lo < 0 || k.hi >= xs.len() as i128 { let _ = self.index_off(k, xs.len(), 1, line)?; }
            return Ok(out);
        }
        let t = self.ev_ty(&EV::A(xs.to_vec()));
        let base = self.alloc(&t);
        let p = Place { base, off: None, ty: t };
        self.write(&p, &EV::A(xs.to_vec()), line)?;
        let q = self.place_at(&p, &[Step::Var(k.clone())], line)?;
        Ok(self.read(&q))
    }

    fn binop(&mut self, op: BinOp, a: &Sv, b: &Sv, wrap_ty: Option<Ty>, line: u32) -> R<Sv> {
        Ok(match op {
            BinOp::Add => self.arith(BOp::Add, a, b),
            BinOp::Sub => self.arith(BOp::Sub, a, b),
            BinOp::Mul => self.arith(BOp::Mul, a, b),
            BinOp::Div => self.divrem(a, b, false, line)?,
            BinOp::Rem => self.divrem(a, b, true, line)?,
            BinOp::AddW | BinOp::SubW | BinOp::MulW => {
                let t = wrap_ty.ok_or_else(|| format!("{}: wrapping operators need a typed variable on the left", self.here(line)))?;
                let v = match op { BinOp::AddW => self.arith(BOp::Add, a, b), BinOp::SubW => self.arith(BOp::Sub, a, b), _ => self.arith(BOp::Mul, a, b) };
                self.convert(&v, &t, Round::Trunc, Ovf::Wrap)
            }
            BinOp::Shl | BinOp::Shr => match b.as_const() {
                Some(k) if (0..=120).contains(&k) => if op == BinOp::Shl { self.shl_const(a, k as u32) } else { self.shr_const(a, k as u32) },
                _ => self.shift(a, b, op == BinOp::Shl, line)?,
            },
            BinOp::BitAnd => self.bitop(a, b, BOp::And),
            BinOp::BitOr => self.bitop(a, b, BOp::Or),
            BinOp::BitXor => self.bitop(a, b, BOp::Xor),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => self.compare(a, b, op),
            BinOp::And | BinOp::Or => unreachable!(),
        })
    }

    fn convert_ev(&mut self, v: &EV, t: &Ty, r: Round, o: Ovf, line: u32) -> R<EV> {
        match (v, t) {
            (EV::S(s), t) if t.is_scalar() => Ok(EV::S(self.convert(s, t, r, o))),
            (EV::A(_) | EV::M(_), Ty::Array(e, _)) => { let xs = self.elems(v, line)?; Ok(EV::A(xs.iter().map(|x| self.convert_ev(x, e, r, o, line)).collect::<R<_>>()?)) }
            (EV::A(_) | EV::M(_), t) if t.is_scalar() => { let xs = self.elems(v, line)?; Ok(EV::A(xs.iter().map(|x| self.convert_ev(x, t, r, o, line)).collect::<R<_>>()?)) }
            _ => err(line, self.prog, "cannot convert this value to that type"),
        }
    }

    fn targ_ty(&mut self, t: &[TArg], line: u32) -> R<Ty> {
        match t.first() { Some(TArg::Ty(t)) => self.ty(t, line), _ => err(line, self.prog, "expected a type argument, e.g. `.sat::<u8>()`") }
    }

    fn modes(&self, t: &Ty, a: &[Arg], ovf_default: Option<Ovf>) -> (Round, Ovf) {
        let (mut r, mut o) = match t { Ty::Fixed { round, ovf, .. } => (*round, *ovf), _ => (Round::Trunc, Ovf::Check) };
        if let Some(d) = ovf_default { o = d; }
        for x in a {
            let name = match &x.value.kind { ExprKind::Path(p) => p.last().cloned().unwrap_or_default(), _ => continue };
            match (x.name.as_deref(), name.as_str()) {
                (Some("round"), "trunc") => r = Round::Trunc,
                (Some("round"), "nearest") => r = Round::Nearest,
                (Some("round"), "even") => r = Round::Even,
                (Some("ovf"), "sat") => o = Ovf::Sat,
                (Some("ovf"), "wrap") => o = Ovf::Wrap,
                (Some("ovf"), "check") => o = Ovf::Check,
                _ => {}
            }
        }
        (r, o)
    }

    fn method(&mut self, r: &Expr, n: &str, t: &[TArg], a: &[Arg], line: u32) -> R<EV> {
        let v = self.eval(r)?;
        let arr = !matches!(v, EV::S(_));
        match n {
            "to" | "quantize" => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, None); self.convert_ev(&v, &ty, rr, o, line) }
            "sat" => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, Some(Ovf::Sat)); self.convert_ev(&v, &ty, rr, o, line) }
            "wrap" => { let ty = self.targ_ty(t, line)?; let (rr, o) = self.modes(&ty, a, Some(Ovf::Wrap)); self.convert_ev(&v, &ty, rr, o, line) }
            "len" if arr => { let n = match &v { EV::M(p) => match &p.ty { Ty::Array(_, n) => *n, _ => 0 }, EV::A(xs) => xs.len(), _ => 0 }; Ok(EV::S(Sv::konst(n as i128))) }
            "rev" if arr => { let mut xs = self.elems(&v, line)?; xs.reverse(); Ok(EV::A(xs)) }
            "unrolled" | "iter" | "collect" if arr => Ok(v),
            "enumerate" if arr => { let xs = self.elems(&v, line)?; Ok(EV::A(xs.into_iter().enumerate().map(|(i, x)| EV::A(vec![EV::S(Sv::konst(i as i128)), x])).collect())) }
            "map" if arr => { let xs = self.elems(&v, line)?; let f = a.first().ok_or("map() takes a lambda")?.value.clone(); Ok(EV::A(self.lambda(&f, xs, line)?)) }
            "sum" | "tree_sum" if arr => {
                let xs = self.elems(&v, line)?;
                let mut items: Vec<Sv> = xs.iter().map(|x| x.s().cloned()).collect::<R<_>>()?;
                if items.is_empty() { return err(line, self.prog, "sum of an empty array"); }
                if n == "sum" {
                    let mut acc = items.remove(0);
                    for x in items { acc = self.arith(BOp::Add, &acc, &x); }
                    return Ok(EV::S(acc));
                }
                while items.len() > 1 {
                    let mut next = vec![];
                    for ch in items.chunks(2) { next.push(if ch.len() == 2 { self.arith(BOp::Add, &ch[0], &ch[1]) } else { ch[0].clone() }); }
                    items = next;
                }
                Ok(EV::S(items.remove(0)))
            }
            "abs" if !arr => {
                let s = v.s()?.clone();
                let ng = self.neg(&s);
                let lt = self.compare(&s, &Sv { frac: s.frac, ..Sv::konst(0) }, BinOp::Lt);
                let x = self.select(&lt, &ng, &s);
                let hi = s.lo.abs().max(s.hi.abs());
                let lo = if s.lo >= 0 { s.lo } else if s.hi <= 0 { -s.hi } else { 0 };
                Ok(EV::S(self.wrap_to(&x, lo, hi, s.frac)))
            }
            "xor_reduce" | "popcount" if !arr => {
                let s = v.s()?.clone();
                let w = int_bits(s.lo, s.hi);
                let mut acc = Sv::konst(0);
                let raw = Sv { frac: 0, ..s };
                for i in 0..w {
                    let b = self.shr_const(&raw, i);
                    let b = self.wrap_to(&b, 0, 1, 0);
                    acc = if n == "popcount" { self.arith(BOp::Add, &acc, &b) } else { let x = self.bitop(&acc, &b, BOp::Xor); Sv { lo: 0, hi: 1, ..x } };
                }
                Ok(EV::S(acc))
            }
            other => err(line, self.prog, format!("method `{}` is not supported by fluxa-emu here", other)),
        }
    }

    fn lambda(&mut self, lam: &Expr, items: Vec<EV>, line: u32) -> R<Vec<EV>> {
        let (ps, body) = match &lam.kind { ExprKind::Lambda(p, b) => (p.clone(), b.clone()), _ => return err(line, self.prog, "expected a lambda such as `|x| x * 2`") };
        let mut out = vec![];
        self.depth += 1;
        for it in items {
            let r = (|| -> R<()> {
                if ps.len() == 1 { self.declare(&ps[0], None, it, line)?; } else if let EV::A(xs) = it { for (p, x) in ps.iter().zip(xs) { self.declare(p, None, x, line)?; } }
                Ok(())
            })();
            if let Err(e) = r { self.depth -= 1; return Err(e); }
            match self.eval(&body) { Ok(v) => out.push(v), Err(e) => { self.depth -= 1; return Err(e); } }
        }
        self.depth -= 1;
        Ok(out)
    }

    fn call(&mut self, c: &Expr, a: &[Arg], line: u32) -> R<EV> {
        let name = match &c.kind { ExprKind::Path(p) | ExprKind::PathT(p, _) => p.last().unwrap().clone(), _ => return err(line, self.prog, "callee must be a function name") };
        match name.as_str() {
            "print" => return Ok(EV::S(Sv::konst(0))),
            "min" | "max" if a.len() == 2 => {
                let (x, y) = (self.scalar(&a[0].value)?, self.scalar(&a[1].value)?);
                let lt = self.compare(&x, &y, BinOp::Lt);
                return Ok(EV::S(if name == "min" { self.select(&lt, &x, &y) } else { self.select(&lt, &y, &x) }));
            }
            "abs" if a.len() == 1 => {
                let r = Expr { kind: ExprKind::Method(Box::new(a[0].value.clone()), "abs".into(), vec![], vec![]), line };
                return self.eval(&r);
            }
            "concat" => {
                let mut out = vec![];
                for x in a { let v = self.eval(&x.value)?; out.extend(self.elems(&v, line)?); }
                return Ok(EV::A(out));
            }
            "len" if a.len() == 1 => { let v = self.eval(&a[0].value)?; let xs = self.elems(&v, line)?; return Ok(EV::S(Sv::konst(xs.len() as i128))); }
            "zip" if a.len() == 2 => {
                let (x, y) = (self.eval(&a[0].value)?, self.eval(&a[1].value)?);
                let (xs, ys) = (self.elems(&x, line)?, self.elems(&y, line)?);
                return Ok(EV::A(xs.into_iter().zip(ys).map(|(p, q)| EV::A(vec![p, q])).collect()));
            }
            "array" if a.len() == 2 => {
                let Some(n) = self.scalar(&a[0].value)?.as_const() else { return err(line, self.prog, "array(n, ..) needs a constant n") };
                let items = (0..n).map(|k| EV::S(Sv::konst(k))).collect();
                return Ok(EV::A(self.lambda(&a[1].value, items, line)?));
            }
            _ => {}
        }
        let decl = match self.prog.fns.get(&name).cloned() {
            Some(d) => d,
            None => return match self.konst(&Expr { kind: ExprKind::Call(Box::new(c.clone()), a.to_vec()), line }) {
                Some(v) => self.value(&v, line),
                None => err(line, self.prog, format!("`{}` is not supported by fluxa-emu", name)),
            },
        };
        if decl.kind == FnKind::Task { return err(line, self.prog, format!("`{}` is a task: spawn it", name)); }
        if self.depth > 32 { return err(line, self.prog, "recursion is not supported by fluxa-emu"); }
        // a call with only constant arguments: the simulator's value (comptime folding)
        let mut vals = vec![];
        for x in a { vals.push((x.name.clone(), self.eval(&x.value)?)); }
        self.rename += 1;
        let (params, body) = fluxa_synth::lower::rename_fn(&decl, self.rename, &self.consts);
        let mut positional = vals.iter().filter(|(n, _)| n.is_none()).map(|(_, v)| v.clone());
        let mut binds = vec![];
        for (p, u) in decl.params.iter().zip(&params) {
            let v = match vals.iter().find(|(n, _)| n.as_deref() == Some(p.name.as_str())) { Some((_, v)) => v.clone(), None => match positional.next() { Some(v) => v, None => return err(line, self.prog, format!("missing argument `{}`", p.name)) } };
            let t = self.ty_opt(&p.ty, line)?;
            binds.push((u.clone(), v, t));
        }
        self.depth += 1;
        let r = (|| -> R<EV> {
            for (u, v, t) in binds {
                let v = match (&t, &v) {
                    (Some(t), EV::S(s)) if t.is_scalar() => EV::S(self.fit_keep(s, t)),
                    // an array argument: the caller's place (read only)
                    (_, EV::M(_)) => v,
                    (Some(t), v) => self.fit_ev(v, t, line)?,
                    (None, v) => v.clone(),
                };
                self.env.insert(u, v);
            }
            let mut stmts = body.stmts.clone();
            let mut tail = body.tail.clone();
            if tail.is_none() {
                if let Some(Stmt { kind: StmtKind::Return(Some(e)), .. }) = stmts.last() { tail = Some(Box::new(e.clone())); stmts.pop(); }
            }
            self.stmts(&stmts)?;
            match &tail { Some(t) => self.eval(t), None => Ok(EV::S(Sv::konst(0))) }
        })();
        self.depth -= 1;
        let v = r?;
        match &decl.ret {
            Some(rt) => match self.ty_opt(rt, line)? {
                Some(t) if t.is_scalar() => { let s = v.s().map_err(|m| format!("{}: {}", self.here(line), m))?.clone(); Ok(EV::S(self.fit_keep(&s, &t))) }
                Some(t) => self.fit_ev(&v, &t, line),
                None => Ok(v),
            },
            None => Ok(v),
        }
    }
}

fn same_shape(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Array(x, n), Ty::Array(y, m)) => n == m && same_shape(x, y),
        (Ty::Tuple(x), Ty::Tuple(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same_shape(a, b)),
        (a, b) => a.is_scalar() && b.is_scalar(),
    }
}

fn unnarrow(env: &mut HashMap<String, EV>, narrowed: &[(String, EV, EV)]) {
    for (n, nv, old) in narrowed.iter().rev() {
        if env.get(n) == Some(nv) { env.insert(n.clone(), old.clone()); }
    }
}

pub fn base(n: &str) -> &str { n.split(['.', '~']).next().unwrap_or(n) }

// ---------------------------------------------------------------- a task

/// a parameter of the task as compiled
#[derive(Clone, Debug)]
pub enum ParamKind {
    Chan(Ty, bool),
    /// a scalar given as a constant
    Const(Value),
    /// an array: words given when an instance starts (`Code::data`)
    Data(Ty),
}

struct Task<'a> {
    decl: &'a FnDecl,
    cfg: &'a Cfg,
    params: Vec<(String, String, ParamKind)>,
    /// channel (unique name) → port index
    ports: HashMap<String, u32>,
    port_ty: Vec<Ty>,
    mems: HashMap<String, Ty>,
}

/// compile task `decl` (lowered to `cfg`) with its parameters `params` (scalars: constants,
/// arrays: data at the start, channels: ports)
pub fn compile_task(prog: &Arc<Program>, decl: &FnDecl, cfg: &Cfg, params: &[(String, ParamKind)]) -> R<Code> {
    let mut ports = HashMap::new();
    let mut port_ty = vec![];
    let mut ps = vec![];
    for ((p, u), (_, k)) in decl.params.iter().zip(&cfg.params).zip(params) {
        if let ParamKind::Chan(t, _) = k { ports.insert(u.clone(), port_ty.len() as u32); port_ty.push(t.clone()); }
        ps.push((p.name.clone(), u.clone(), k.clone()));
    }
    let mut task = Task { decl, cfg, params: ps, ports, port_ty, mems: HashMap::new() };
    {
        let mut em = Em::new(prog);
        for (m, t, n, _) in &cfg.mems {
            let mt = em.ty(&TypeExpr::Mem(Box::new(t.clone()), Box::new(n.clone())), decl.line)?;
            task.mems.insert(m.clone(), mt);
        }
    }
    // the type pass: untyped variables widen until nothing grows
    let mut vars: HashMap<String, Ty> = HashMap::new();
    let mut untyped: HashSet<String> = HashSet::new();
    let mut last_err = None;
    for round in 0..16 {
        let mut em = Em::new(prog);
        em.vars = vars.clone();
        em.untyped = untyped.clone();
        em.dry = true;
        let r = emit_all(&mut em, &task, true);
        if let Err(e) = &r { last_err = Some(e.clone()); }
        if !em.grew {
            r?;
            vars = em.vars;
            for m in task.mems.keys() { vars.remove(m); }
            for (_, u, k) in &task.params { if let ParamKind::Data(_) = k { vars.remove(u); } }
            break;
        }
        vars = em.vars;
        untyped = em.untyped;
        for m in task.mems.keys() { vars.remove(m); }
        for (_, u, k) in &task.params { if let ParamKind::Data(_) = k { vars.remove(u); } }
        if round == 12 {
            // still growing: as wide as Fluxa's integers
            for n in &untyped { if let Some(Ty::Int { .. }) = vars.get(n) { vars.insert(n.clone(), Ty::Int { lo: -LIM, hi: LIM - 1 }); } }
        }
        if round == 15 { return Err(last_err.unwrap_or_else(|| format!("{}: the variables' types do not settle", decl.name))); }
    }
    let mut em = Em::new(prog);
    em.vars = vars;
    let states = emit_all(&mut em, &task, false)?;
    let mut code_ports = vec![];
    for (name, _, k) in &task.params {
        if let ParamKind::Chan(t, out) = k {
            let buf = em.port_bufs.get(name).copied().unwrap_or(0);
            code_ports.push(Port { name: name.clone(), out: *out, leaves: leaves(t).iter().map(leaf_kind).collect(), buf });
        }
    }
    let mut mems = vec![];
    for (m, ..) in &cfg.mems {
        if let (Some(off), Some(Ty::Array(e, n))) = (em.slots.get(m), task.mems.get(m)) { mems.push((base(m).to_string(), *off, leaf_kind(e), *n as u32)); }
    }
    Ok(Code { name: decl.name.clone(), regs: em.regs, frame: em.frame.next_multiple_of(16), states, ports: code_ports, init: em.init, data: em.data.clone(), traps: em.traps, mems })
}

impl Em<'_> {
    /// store the task variables the state assigned
    fn epilogue(&mut self) -> R<()> {
        let mut names: Vec<String> = self.dirty.drain().collect();
        names.sort();
        for n in names {
            let (Some(base), Some(t), Some(v)) = (self.slots.get(&n).copied(), self.vars.get(&n).cloned(), self.env.get(&n).cloned()) else { continue };
            if has_array(&t) { continue; }
            self.write(&Place { base, off: None, ty: t }, &v, 0)?;
        }
        Ok(())
    }
}

/// every state's code; `dry`: the type pass (code thrown away)
fn emit_all(em: &mut Em, task: &Task, dry: bool) -> R<Vec<State>> {
    let cfg = task.cfg;
    // the frame: the variables, the memories, the ports' token buffers, the array parameters
    let mut names: Vec<String> = em.vars.keys().cloned().collect();
    names.sort();
    for n in &names { let t = em.vars[n].clone(); let b = em.alloc(&t); em.slots.insert(n.clone(), b); }
    let mut mnames: Vec<&String> = task.mems.keys().collect();
    mnames.sort();
    for m in mnames { let t = task.mems[m].clone(); let b = em.alloc(&t); em.slots.insert(m.clone(), b); em.vars.insert(m.clone(), t); }
    for (name, u, k) in &task.params {
        match k {
            ParamKind::Chan(t, _) => { let b = em.alloc(t); em.port_bufs.insert(name.clone(), b); let _ = u; }
            ParamKind::Data(t) => {
                // a memory starting with this parameter's words: the memory's place
                let from = cfg.mems.iter().find(|m| m.3.as_deref() == Some(u.as_str())).map(|m| m.0.clone());
                let b = match &from { Some(m) => em.slots[m], None => em.alloc(t) };
                let ls: Vec<K> = match t { Ty::Array(e, _) => leaves(e).iter().map(leaf_kind).collect(), t => leaves(t).iter().map(leaf_kind).collect() };
                let n = match t { Ty::Array(_, n) => *n as u32, _ => 1 };
                em.data.push((name.clone(), b, ls, n));
                em.slots.insert(u.clone(), b);
                em.vars.insert(u.clone(), t.clone());
            }
            ParamKind::Const(_) => {}
        }
    }
    // the lowering's temporaries get their types
    for (n, t) in &cfg.temps {
        if em.vars.contains_key(n) && !em.untyped.contains(n) { continue; }
        let ty = match t {
            Temp::ChanElem(ch) => task.ports.get(ch).map(|p| task.port_ty[*p as usize].clone()).ok_or_else(|| format!("`{}` is not a channel parameter of the task", base(ch)))?,
            Temp::MemElem(m) => match &task.mems[m] { Ty::Array(e, _) => (**e).clone(), t => t.clone() },
            Temp::Like(e) => {
                let saved = std::mem::take(&mut em.ops);
                em.env.clear();
                em.bind_params(task)?;
                let r = match &e.kind {
                    ExprKind::Range(lo, hi, incl) => em.scalar(lo).and_then(|l| em.scalar(hi).map(|h| (l, h, *incl))).map(|(l, h, incl)| Ty::Int { lo: l.lo.min(h.lo), hi: (h.hi + incl as i128).max(l.hi) }),
                    _ => em.eval(e).map(|v| em.ev_ty(&v)),
                };
                em.ops = saved;
                match r { Ok(t) => t, Err(_) => { em.untyped.insert(n.clone()); em.grew = true; continue; } }
            }
        };
        if em.vars.get(n) != Some(&ty) {
            if !em.slots.contains_key(n) { let b = em.alloc(&ty); em.slots.insert(n.clone(), b); }
            em.vars.insert(n.clone(), ty);
            em.untyped.remove(n);
        }
    }
    let mut states: Vec<State> = (0..cfg.blocks.len()).map(|_| State { ops: vec![], term: Term::Halt, at: String::new() }).collect();
    let mut extra: Vec<State> = vec![];
    let n0 = cfg.blocks.len() as u32;
    let new_state = |extra: &mut Vec<State>, s: State| -> u32 { extra.push(s); n0 + extra.len() as u32 - 1 };
    for (si, b) in cfg.blocks.iter().enumerate() {
        em.env.clear();
        em.dirty.clear();
        em.ops.clear();
        em.bind_params(task)?;
        let line = b.stmts.first().map(|s| s.line).unwrap_or(task.decl.line);
        // the head of a pipelined loop: its tokens and condition come first (states of their own)
        if let LTerm::Pipe { head, cond, recvs, sends, exit } = &b.term {
            // body state: the statements, then the values to send into the ports' buffers
            em.stmts(&b.stmts)?;
            for (ch, v) in sends {
                let p = task.ports.get(ch).copied().ok_or_else(|| format!("send on `{}`, which is not a channel parameter", base(ch)))?;
                let val = em.env.get(v).cloned().ok_or_else(|| format!("the value sent on `{}` is not known", base(ch)))?;
                em.port_put(task, p, &val, line)?;
            }
            em.epilogue()?;
            let body_ops = std::mem::take(&mut em.ops);
            // sends, then back to the head
            let head_state = si as u32;
            let mut after = head_state;
            for (ch, _) in sends.iter().rev() {
                let p = task.ports[ch];
                after = new_state(&mut extra, State { ops: vec![], term: Term::Send { port: p, next: after }, at: String::new() });
            }
            let body = new_state(&mut extra, State { ops: body_ops, term: Term::Goto(after), at: String::new() });
            // the other inputs
            let mut next = body;
            for (dst, ch) in recvs.iter().rev() {
                let p = task.ports[ch];
                let copy = em.recv_copy(task, p, dst, next, line)?;
                let c = new_state(&mut extra, copy);
                next = new_state(&mut extra, State { ops: vec![], term: Term::Recv { port: p, closed: None, next: c }, at: String::new() });
            }
            // the head: a token (the end of the stream leaves) or the condition
            if let Some((dst, ch)) = head {
                let p = task.ports[ch];
                let copy = em.recv_copy(task, p, dst, next, line)?;
                let c = new_state(&mut extra, copy);
                states[si] = State { ops: vec![], term: Term::Recv { port: p, closed: Some(*exit as u32), next: c }, at: em.here(line) };
            } else if let Some(c) = cond {
                em.env.clear();
                em.ops.clear();
                em.bind_params(task)?;
                let cv = em.scalar(c)?;
                let cr = em.truth(&cv);
                states[si] = State { ops: std::mem::take(&mut em.ops), term: Term::Branch(cr, next, *exit as u32), at: em.here(line) };
            } else {
                states[si] = State { ops: vec![], term: Term::Goto(next), at: em.here(line) };
            }
            continue;
        }
        em.stmts(&b.stmts)?;
        let term = match &b.term {
            LTerm::Goto(t) => { em.epilogue()?; Term::Goto(*t as u32) }
            LTerm::Halt => { em.epilogue()?; Term::Halt }
            LTerm::Branch(c, t, f) => {
                let cv = em.scalar(c)?;
                let cr = em.truth(&cv);
                em.epilogue()?;
                Term::Branch(cr, *t as u32, *f as u32)
            }
            LTerm::Recv { dst, ch, closed, next } => {
                em.epilogue()?;
                let p = task.ports.get(ch).copied().ok_or_else(|| format!("recv on `{}`, which is not a channel parameter", base(ch)))?;
                let copy = em.recv_copy(task, p, dst, *next as u32, line)?;
                let c = new_state(&mut extra, copy);
                let w = new_state(&mut extra, State { ops: vec![], term: Term::Recv { port: p, closed: closed.map(|x| x as u32), next: c }, at: String::new() });
                Term::Goto(w)
            }
            LTerm::Send { ch, val, next } => {
                let p = task.ports.get(ch).copied().ok_or_else(|| format!("send on `{}`, which is not a channel parameter", base(ch)))?;
                if !matches!(task.params.iter().find(|x| &x.1 == ch).map(|x| &x.2), Some(ParamKind::Chan(_, true))) { return Err(format!("send on `{}`, which is not an output channel of the task", base(ch))); }
                let v = em.eval(val)?;
                em.port_put(task, p, &v, line)?;
                em.epilogue()?;
                let w = new_state(&mut extra, State { ops: vec![], term: Term::Send { port: p, next: *next as u32 }, at: String::new() });
                Term::Goto(w)
            }
            LTerm::Close { ch, next } => {
                em.epilogue()?;
                let p = task.ports[ch];
                let w = new_state(&mut extra, State { ops: vec![], term: Term::Close { port: p, next: *next as u32 }, at: String::new() });
                Term::Goto(w)
            }
            LTerm::CloseOwned { ch, next } => {
                em.epilogue()?;
                let p = task.ports[ch];
                let w = new_state(&mut extra, State { ops: vec![], term: Term::CloseOwned { port: p, next: *next as u32 }, at: String::new() });
                Term::Goto(w)
            }
            LTerm::MemRead { dst, mem, addr, next } => {
                let a = em.scalar(addr)?;
                let mt = task.mems[mem].clone();
                let mp = Place { base: em.slots[mem], off: None, ty: mt };
                let q = em.place_at(&mp, &[Step::Var(a)], line)?;
                let v = em.read(&q);
                em.store(dst, &[], v, line)?;
                em.epilogue()?;
                Term::Goto(*next as u32)
            }
            LTerm::MemWrite { mem, addr, val, next } => {
                let a = em.scalar(addr)?;
                let v = em.eval(val)?;
                let mt = task.mems[mem].clone();
                let mp = Place { base: em.slots[mem], off: None, ty: mt };
                let q = em.place_at(&mp, &[Step::Var(a)], line)?;
                em.write(&q, &v, line)?;
                em.epilogue()?;
                Term::Goto(*next as u32)
            }
            LTerm::Pipe { .. } => unreachable!(),
        };
        states[si] = State { ops: std::mem::take(&mut em.ops), term, at: em.here(line) };
        if dry { states[si].ops.clear(); }
    }
    states.extend(extra);
    if dry { for s in &mut states { s.ops.clear(); } }
    Ok(states)
}

impl Em<'_> {
    /// the task's parameters in the environment: constants, arrays in the frame
    fn bind_params(&mut self, task: &Task) -> R<()> {
        for (_, u, k) in &task.params {
            match k {
                ParamKind::Const(v) => { let v = self.value(v, task.decl.line)?; self.env.insert(u.clone(), v); }
                ParamKind::Data(t) => { let b = self.slots[u]; self.env.insert(u.clone(), EV::M(Place { base: b, off: None, ty: t.clone() })); }
                ParamKind::Chan(..) => {}
            }
        }
        Ok(())
    }

    /// a value into port `p`'s token buffer (converted to the channel's type)
    fn port_put(&mut self, task: &Task, p: u32, v: &EV, line: u32) -> R<()> {
        let name = task.params.iter().find(|x| task.ports.get(&x.1) == Some(&p)).map(|x| x.0.clone()).unwrap();
        let base = self.port_bufs[&name];
        self.write(&Place { base, off: None, ty: task.port_ty[p as usize].clone() }, v, line)
    }

    /// a state that copies port `p`'s token into variable `dst`, then goes to `next`
    fn recv_copy(&mut self, task: &Task, p: u32, dst: &str, next: u32, line: u32) -> R<State> {
        let saved = (std::mem::take(&mut self.ops), std::mem::take(&mut self.env), std::mem::take(&mut self.dirty));
        let name = task.params.iter().find(|x| task.ports.get(&x.1) == Some(&p)).map(|x| x.0.clone()).unwrap();
        let base = self.port_bufs[&name];
        self.bind_params(task)?;
        let v = self.read(&Place { base, off: None, ty: task.port_ty[p as usize].clone() });
        let r = self.store(dst, &[], v, line).and_then(|_| self.epilogue());
        let ops = std::mem::take(&mut self.ops);
        (self.ops, self.env, self.dirty) = saved;
        r?;
        Ok(State { ops, term: Term::Goto(next), at: format!("{} (recv)", self.here(line)) })
    }
}
