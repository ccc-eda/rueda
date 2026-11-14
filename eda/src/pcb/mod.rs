//! Boards: the parts on a board and the nets between their pins (a `.board` file), checked by
//! electrical rules and simulated with the FPGA board they plug into. The plan:
//! docs/note/pcb-plan.md (B1: description, ERC, simulation; later KiCad, Gerber, routing).
//!
//! - `desc`: the `.board` file
//! - `parts`: the part library (pins and their electrical types) and the hosts' connectors
//! - `erc`: electrical rule check
//! - `sim`: net levels and part models, run by the virtual iCEBreaker (`fpga::board`)

pub mod desc;
pub mod erc;
pub mod parts;
pub mod sim;

use parts::{Kind, PinType};

type R<T> = Result<T, String>;

/// a pin of a net, resolved against the part library
#[derive(Clone, Debug)]
pub struct Pin {
    pub part: usize,
    /// its number in the part's package, its name, its type
    pub num: u32,
    pub name: &'static str,
    pub ty: PinType,
    pub od: bool,
}

#[derive(Clone, Debug)]
pub struct Net { pub name: String, pub pins: Vec<Pin> }

/// a board with every pin resolved: the parts' kinds, the nets' pins
pub struct Board {
    pub desc: desc::Pcb,
    pub kinds: Vec<&'static Kind>,
    pub nets: Vec<Net>,
}

impl Board {
    pub fn parse(src: &str) -> R<Board> { Board::new(desc::parse(src)?) }

    /// resolve parts and pins: unknown kinds or pins, or a pin on two nets, are errors
    pub fn new(desc: desc::Pcb) -> R<Board> {
        let mut kinds = vec![];
        for p in &desc.parts {
            let k = parts::kind(&p.kind).ok_or_else(|| format!("line {}: part {}: no kind `{}` (the library has {})", p.line, p.id, p.kind,
                parts::KINDS.iter().map(|k| k.name).collect::<Vec<_>>().join(", ")))?;
            kinds.push(k);
        }
        let mut nets = vec![];
        let mut seen: Vec<(usize, u32, String)> = vec![];
        for n in &desc.nets {
            let mut pins = vec![];
            for r in &n.pins {
                let part = desc.parts.iter().position(|p| p.id == r.part).ok_or_else(|| format!("line {}: net {}: no part {}", n.line, n.name, r.part))?;
                let k = kinds[part];
                let (num, name, ty) = k.pin(&r.pin).ok_or_else(|| format!("line {}: net {}: {} ({}) has no pin `{}` (its pins: {})", n.line, n.name, r.part, k.name, r.pin,
                    k.pins.iter().map(|p| p.1).collect::<Vec<_>>().join(", ")))?;
                if let Some(s) = seen.iter().find(|s| s.0 == part && s.1 == num) {
                    return Err(format!("line {}: {} is on net {} and on net {} (a pin is on one net)", n.line, r, s.2, n.name));
                }
                seen.push((part, num, n.name.clone()));
                if r.od && ty != PinType::Signal { return Err(format!("line {}: {}: `:od` is for a connector pin the host drives", n.line, r)); }
                pins.push(Pin { part, num, name, ty, od: r.od });
            }
            nets.push(Net { name: n.name.clone(), pins });
        }
        Ok(Board { desc, kinds, nets })
    }

    pub fn part_id(&self, i: usize) -> &str { &self.desc.parts[i].id }

    /// the net a part's pin is on
    pub fn net_of(&self, part: usize, num: u32) -> Option<usize> {
        self.nets.iter().position(|n| n.pins.iter().any(|p| p.part == part && p.num == num))
    }

    /// a summary: parts and nets, one a line
    pub fn report(&self) -> String {
        let mut s = format!("board {}: {} parts, {} nets\n", if self.desc.name.is_empty() { "(no name)" } else { &self.desc.name }, self.desc.parts.len(), self.nets.len());
        for (p, k) in self.desc.parts.iter().zip(&self.kinds) {
            s += &format!("  {:4} {:8} {:8} {}\n", p.id, p.kind, p.value.as_deref().unwrap_or(""), k.desc.1);
        }
        for n in &self.nets {
            s += &format!("  net {:8} {}\n", n.name, n.pins.iter().map(|p| format!("{}.{}", self.part_id(p.part), p.name)).collect::<Vec<_>>().join(" "));
        }
        for p in &self.desc.plugs { s += &format!("  plug {} into {}\n", p.part, p.host); }
        s
    }
}
