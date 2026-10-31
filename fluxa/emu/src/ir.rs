//! The emulator's code: a task as states (its lowered blocks), each a list of operations on
//! registers and the instance's frame, ending in a terminator (a jump, a branch, a channel
//! operation). Registers are 64-bit (`K::N`) or 128-bit (`K::W`); a value's range decides which
//! (Fluxa's integers are exact up to 120 bits, most fit 64). Nothing here knows Fluxa: the
//! executor (`exec.rs`) and the WebAssembly writer run the same code.

/// a register's width: N = 64 bits, W = 128 bits
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum K { N, W }

impl K {
    /// bytes in memory
    pub fn size(self) -> u32 { if self == K::N { 8 } else { 16 } }
}

pub type Reg = u32;

/// binary operations. On N registers they wrap at 64 bits (the emitter picks N only where the
/// result fits); comparisons give 0 / 1 in an N register. Shift amounts are N registers below the
/// width (the emitter makes sure). On W registers values are exact: `ShrU`, `DivU`, `RemU`,
/// `LtU`, `LeU` are not used there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BOp { Add, Sub, Mul, And, Or, Xor, Shl, ShrS, ShrU, DivS, DivU, RemS, RemU, Eq, Ne, LtS, LtU, LeS, LeU }

impl BOp {
    pub fn is_cmp(self) -> bool { matches!(self, BOp::Eq | BOp::Ne | BOp::LtS | BOp::LtU | BOp::LeS | BOp::LeU) }
}

/// a place in the instance's frame: `base` bytes in, plus the byte offset in register `off`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Addr { pub base: u32, pub off: Option<Reg> }

#[derive(Clone, Debug)]
pub enum Op {
    Const(Reg, i128),
    Mov(Reg, Reg),
    Load(Reg, Addr),
    Store(Addr, Reg),
    /// d = a OP b (a, b of the same kind; d of that kind, or N for a comparison)
    Bin(BOp, Reg, Reg, Reg),
    /// d (W) = s (N) extended: signed or unsigned
    Ext(Reg, Reg, bool),
    /// d (N) = the low 64 bits of s (W)
    Low(Reg, Reg),
    /// d = c ? a : b (c an N register, a, b and d of one kind)
    Sel(Reg, Reg, Reg, Reg),
    If(Reg, Vec<Op>, Vec<Op>),
    /// stop the program: message `traps[k]` of the code
    Trap(u32),
    /// copy `n` bytes inside the frame (whole arrays)
    Copy(Addr, Addr, u32),
}

/// how a state ends. Channel operations name a port of the task; a token's scalars are laid out
/// in the frame at the port's buffer (`Port::buf`), where `Recv` puts it and `Send` takes it.
#[derive(Clone, Debug)]
pub enum Term {
    Goto(u32),
    /// to the first state when register c is not 0, else to the second
    Branch(Reg, u32, u32),
    Halt,
    /// wait for a token: into the port's buffer, then `next`; the stream ended: `closed` (none: the
    /// program fails, a recv after the end)
    Recv { port: u32, closed: Option<u32>, next: u32 },
    /// wait for room, then send the port's buffer
    Send { port: u32, next: u32 },
    Close { port: u32, next: u32 },
    /// the end of the task: close the port unless the program did
    CloseOwned { port: u32, next: u32 },
}

#[derive(Clone, Debug)]
pub struct State {
    pub ops: Vec<Op>,
    pub term: Term,
    /// where it comes from in the program (file:line), for profiles
    pub at: String,
}

/// a channel parameter of the task
#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    pub out: bool,
    /// the token's scalars
    pub leaves: Vec<K>,
    /// where a token is in the frame
    pub buf: u32,
}

/// a task's code (one per task and set of scalar parameters)
#[derive(Clone, Debug)]
pub struct Code {
    pub name: String,
    pub regs: Vec<K>,
    /// bytes of frame an instance needs
    pub frame: u32,
    pub states: Vec<State>,
    pub ports: Vec<Port>,
    /// the frame's initial contents: (offset, kind, value)
    pub init: Vec<(u32, K, i128)>,
    /// array parameters whose words an instance gets at its start: (parameter, offset, the
    /// leaves of one element, elements)
    pub data: Vec<(String, u32, Vec<K>, u32)>,
    /// the messages of `Op::Trap`
    pub traps: Vec<String>,
}

/// a channel between two instances (or to the outside)
#[derive(Clone, Debug)]
pub struct Chan {
    pub name: String,
    pub depth: u32,
    pub leaves: Vec<K>,
    /// each leaf's sign (a token read from outside: an N leaf is u64 or i64)
    pub signed: Vec<bool>,
}

/// a task instance: its code, the channel behind each port, its array parameters' values
#[derive(Clone, Debug)]
pub struct Inst {
    pub name: String,
    pub code: usize,
    pub chans: Vec<u32>,
    /// for each `Code::data` entry: the values (one per leaf, element after element)
    pub data: Vec<Vec<i128>>,
}

/// a compiled network: its codes, instances and channels; `ins` / `outs` are the channels of
/// the top task's own channel parameters (the outside's ends), by parameter name
#[derive(Clone, Debug, Default)]
pub struct Machine {
    pub codes: Vec<Code>,
    pub insts: Vec<Inst>,
    pub chans: Vec<Chan>,
    pub ins: Vec<(String, u32)>,
    pub outs: Vec<(String, u32)>,
}

impl Machine {
    /// a short summary: codes, states, operations, instances, channels
    pub fn stats(&self) -> String {
        fn count(ops: &[Op]) -> usize { ops.iter().map(|o| 1 + match o { Op::If(_, t, f) => count(t) + count(f), _ => 0 }).sum() }
        let states: usize = self.codes.iter().map(|c| c.states.len()).sum();
        let ops: usize = self.codes.iter().flat_map(|c| &c.states).map(|s| count(&s.ops)).sum();
        let frame: u64 = self.insts.iter().map(|i| self.codes[i.code].frame as u64).sum();
        format!("{} task(s) compiled, {} instance(s), {} channel(s), {} states, {} operations, {} KB of frames",
            self.codes.len(), self.insts.len(), self.chans.len(), states, ops, frame / 1024)
    }
}

/// where everything is in memory (the executor's and the WebAssembly module's alike): a control
/// word (the instance that stopped the program), then each instance's descriptor (state, halted,
/// frame, the trap it stopped at + 1, then the address of the channel behind each port), the
/// frames, the channels (count, head, closed, depth, then `depth` tokens of `tok` bytes)
#[derive(Clone, Debug)]
pub struct Layout {
    pub desc: Vec<u32>,
    pub frames: Vec<u32>,
    pub chans: Vec<u32>,
    pub tok: Vec<u32>,
    pub size: u32,
}

/// a channel's header bytes
pub const CHAN_HDR: u32 = 16;

pub fn tok_size(leaves: &[K]) -> u32 { leaves.iter().map(|k| k.size()).sum::<u32>().max(8) }

pub fn layout(m: &Machine) -> Layout {
    let mut at: u32 = 16;
    let mut desc = vec![];
    for i in &m.insts { desc.push(at); at += (16 + 4 * i.chans.len() as u32).next_multiple_of(16); }
    let mut frames = vec![];
    for i in &m.insts { frames.push(at); at += m.codes[i.code].frame.max(16).next_multiple_of(16); }
    let (mut chans, mut tok) = (vec![], vec![]);
    for c in &m.chans { chans.push(at); let t = tok_size(&c.leaves); tok.push(t); at += (CHAN_HDR + t * c.depth).next_multiple_of(16); }
    Layout { desc, frames, chans, tok, size: at }
}

/// the code as text, one operation a line (FLUXA_EMU_DUMP)
pub fn dump(c: &Code) -> String {
    fn ops(out: &mut String, os: &[Op], ind: usize, regs: &[K]) {
        for o in os {
            let r = |x: &Reg| format!("{}{}", if regs[*x as usize] == K::W { "w" } else { "r" }, x);
            let a = |x: &Addr| match x.off { Some(o) => format!("[{}+{}]", x.base, r(&o)), None => format!("[{}]", x.base) };
            let line = match o {
                Op::Const(d, v) => format!("{} = {}", r(d), v),
                Op::Mov(d, s) => format!("{} = {}", r(d), r(s)),
                Op::Load(d, x) => format!("{} = load {}", r(d), a(x)),
                Op::Store(x, s) => format!("store {} = {}", a(x), r(s)),
                Op::Bin(op, d, x, y) => format!("{} = {:?} {} {}", r(d), op, r(x), r(y)),
                Op::Ext(d, s, sg) => format!("{} = ext{} {}", r(d), if *sg { "s" } else { "u" }, r(s)),
                Op::Low(d, s) => format!("{} = low {}", r(d), r(s)),
                Op::Sel(d, c, x, y) => format!("{} = {} ? {} : {}", r(d), r(c), r(x), r(y)),
                Op::Trap(k) => format!("trap {}", k),
                Op::Copy(d, s, n) => format!("copy {} = {} ({} bytes)", a(d), a(s), n),
                Op::If(c, t, f) => {
                    out.push_str(&format!("{}if {} {{\n", "  ".repeat(ind), r(c)));
                    ops(out, t, ind + 1, regs);
                    out.push_str(&format!("{}}} else {{\n", "  ".repeat(ind)));
                    ops(out, f, ind + 1, regs);
                    out.push_str(&format!("{}}}\n", "  ".repeat(ind)));
                    continue;
                }
            };
            out.push_str(&format!("{}{}\n", "  ".repeat(ind), line));
        }
    }
    let mut out = String::new();
    for (k, s) in c.states.iter().enumerate() {
        out.push_str(&format!("state {} ({}):\n", k, s.at));
        ops(&mut out, &s.ops, 1, &c.regs);
        out.push_str(&format!("  -> {:?}\n", s.term));
    }
    out
}
