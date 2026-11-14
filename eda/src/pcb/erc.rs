//! Electrical rule check: what would not work on the real board, found from the nets and the
//! pins' types (`parts::PinType`).

use super::parts::{self, PinType};
use super::Board;

#[derive(Clone, Debug, PartialEq)]
pub struct Diag { pub error: bool, pub msg: String }

impl std::fmt::Display for Diag {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}: {}", if self.error { "error" } else { "warning" }, self.msg) }
}

/// the level a net is held at by a supply: Some(true) 3V3, Some(false) GND
pub fn rail(b: &Board, n: usize) -> Option<bool> {
    b.nets[n].pins.iter().find_map(|p| match p.ty { PinType::Supply(v) => Some(v), _ => None })
}

/// the nets a resistor connects net `n` to
pub fn through_resistors(b: &Board, n: usize) -> Vec<usize> {
    let mut out = vec![];
    for p in &b.nets[n].pins {
        if b.kinds[p.part].name != "res" { continue; }
        let other = if p.num == 1 { 2 } else { 1 };
        if let Some(m) = b.net_of(p.part, other) { out.push(m); }
    }
    out
}

pub fn check(b: &Board) -> Vec<Diag> {
    let mut d = vec![];
    let err = |d: &mut Vec<Diag>, m: String| d.push(Diag { error: true, msg: m });
    let warn = |d: &mut Vec<Diag>, m: String| d.push(Diag { error: false, msg: m });
    let pin_name = |p: &super::Pin| format!("{}.{}", b.part_id(p.part), p.name);

    for (i, n) in b.nets.iter().enumerate() {
        let has = |f: &dyn Fn(PinType) -> bool| n.pins.iter().filter(|p| f(p.ty)).collect::<Vec<_>>();
        let supplies = has(&|t| matches!(t, PinType::Supply(_)));
        if supplies.iter().any(|p| p.ty == PinType::Supply(true)) && supplies.iter().any(|p| p.ty == PinType::Supply(false)) {
            err(&mut d, format!("net {}: 3V3 and GND shorted ({})", n.name, supplies.iter().map(|p| pin_name(p)).collect::<Vec<_>>().join(", ")));
        }
        let outs = has(&|t| t == PinType::Output);
        if outs.len() > 1 { err(&mut d, format!("net {}: push-pull outputs drive each other ({})", n.name, outs.iter().map(|p| pin_name(p)).collect::<Vec<_>>().join(", "))); }
        if !outs.is_empty() && !supplies.is_empty() { err(&mut d, format!("net {}: output {} is shorted to a supply", n.name, pin_name(outs[0]))); }
        // power pins on the right rail
        for p in &n.pins {
            if let PinType::Power(v) = p.ty {
                if rail(b, i) != Some(v) { warn(&mut d, format!("{} is on net {}, not on {}", pin_name(p), n.name, if v { "a 3V3 supply" } else { "GND" })); }
            }
        }
        // open drain needs a pull-up
        let od = n.pins.iter().any(|p| p.od || matches!(p.ty, PinType::OpenDrain | PinType::OpenDrainIo));
        if od && rail(b, i).is_none() && !through_resistors(b, i).iter().any(|m| rail(b, *m) == Some(true)) {
            warn(&mut d, format!("net {}: open-drain without a pull-up resistor to 3V3 (it never goes high)", n.name));
        }
        // an input with nothing that drives it
        let driven = n.pins.iter().any(|p| matches!(p.ty, PinType::Output | PinType::OpenDrain | PinType::OpenDrainIo | PinType::Signal | PinType::Supply(_)))
            || !through_resistors(b, i).is_empty();
        if !driven {
            for p in n.pins.iter().filter(|p| p.ty == PinType::Input) { warn(&mut d, format!("{} (net {}) is not driven: it floats", pin_name(p), n.name)); }
        }
        if n.pins.len() == 1 { warn(&mut d, format!("net {} has one pin ({})", n.name, pin_name(&n.pins[0]))); }
    }
    // pins on no net
    for (pi, (part, k)) in b.desc.parts.iter().zip(&b.kinds).enumerate() {
        for (num, name, ty) in k.pins {
            if b.net_of(pi, *num).is_some() { continue; }
            match ty {
                PinType::Input => warn(&mut d, format!("{}.{} is not connected: it floats", part.id, name)),
                PinType::Power(_) => warn(&mut d, format!("{}.{} (power) is not connected", part.id, name)),
                PinType::Passive => warn(&mut d, format!("{}.{} is not connected", part.id, name)),
                _ => {}
            }
        }
        // an LED without a series resistor
        if k.name == "led" {
            let nets: Vec<usize> = [1, 2].iter().filter_map(|n| b.net_of(pi, *n)).collect();
            if !nets.is_empty() && nets.iter().all(|n| through_resistors(b, *n).is_empty()) {
                warn(&mut d, format!("{}: an LED without a series resistor", part.id));
            }
        }
    }
    // connectors and where they plug in
    for p in &b.desc.plugs {
        match b.desc.parts.iter().position(|x| x.id == p.part) {
            None => err(&mut d, format!("line {}: plug: no part {}", p.line, p.part)),
            Some(i) if b.kinds[i].name != "pmod" => err(&mut d, format!("line {}: plug: {} is a {}, not a connector", p.line, p.part, b.kinds[i].name)),
            Some(_) if parts::host(&p.host).is_none() => err(&mut d, format!("line {}: plug: no host connector {} (there are {})", p.line, p.host,
                parts::HOSTS.iter().map(|h| h.name).collect::<Vec<_>>().join(", "))),
            _ => {}
        }
    }
    for (i, k) in b.kinds.iter().enumerate() {
        if k.name == "pmod" && !b.desc.plugs.iter().any(|p| p.part == b.desc.parts[i].id) {
            warn(&mut d, format!("connector {} is plugged into nothing (plug {} icebreaker.PMOD1A)", b.desc.parts[i].id, b.desc.parts[i].id));
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags(src: &str) -> Vec<String> { check(&Board::parse(src).unwrap()).iter().map(|d| d.to_string()).collect() }

    #[test]
    fn a_good_board_is_clean() {
        let d = diags("part J1 pmod\npart U1 lm75\npart R1 res 4.7k\npart R2 res 4.7k\npart R3 res 10k\n\
            net SCL J1.1:od U1.SCL R1.1\nnet SDA J1.2:od J1.8 U1.SDA R2.1\nnet 3V3 J1.6 R1.2 R2.2 R3.2 U1.VCC\nnet GND J1.5 U1.GND U1.A0 U1.A1 U1.A2\n\
            net OS U1.OS J1.3 R3.1\nplug J1 icebreaker.PMOD1A\n");
        assert!(d.is_empty(), "{:?}", d);
    }

    #[test]
    fn finds_the_mistakes() {
        let d = diags("part J1 pmod\npart U1 lm75\npart U2 74hc595\npart U3 74hc595\npart D1 led\n\
            net SDA J1.2:od U1.SDA\nnet SCL J1.1 U1.SCL\nnet Q U2.Q0 U3.Q0 D1.A\nnet BAD J1.6 J1.5\nnet VCC U1.VCC J1.3\n");
        let has = |s: &str| assert!(d.iter().any(|x| x.contains(s)), "no `{}` in {:?}", s, d);
        has("net SDA: open-drain without a pull-up");
        has("error: net Q: push-pull outputs drive each other (U2.Q0, U3.Q0)");
        has("error: net BAD: 3V3 and GND shorted");
        has("U1.VCC is on net VCC, not on a 3V3 supply");
        has("U2.SER is not connected: it floats");
        has("D1: an LED without a series resistor");
        has("connector J1 is plugged into nothing");
    }

    #[test]
    fn resolution_errors() {
        assert!(Board::parse("part U1 lm76").err().unwrap().contains("no kind `lm76`"));
        assert!(Board::parse("part U1 lm75\nnet A U1.SDB").err().unwrap().contains("has no pin `SDB`"));
        assert!(Board::parse("part U1 lm75\nnet A U1.1\nnet B U1.SDA").err().unwrap().contains("U1.SDA is on net A and on net B"));
        // pins by number
        let b = Board::parse("part U1 74hc595\nnet A U1.14").unwrap();
        assert_eq!(b.nets[0].pins[0].name, "SER");
    }
}
