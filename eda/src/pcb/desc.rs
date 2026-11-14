//! The `.board` file: a board's parts and the nets between their pins, one statement a line.
//!
//! ```text
//! # a sensor module on the iCEBreaker's PMOD1A
//! board "PMOD sensor"
//! part J1 pmod
//! part U1 lm75 temp=25
//! part R1 res 4.7k
//! net SCL J1.1:od U1.SCL R1.1     # :od = the host drives this pin open-drain
//! net 3V3 J1.6 R1.2 U1.VCC
//! plug J1 icebreaker.PMOD1A       # where the connector goes in the simulation
//! ```
//!
//! `part ID KIND [VALUE] [KEY=VALUE]...`, `net NAME PART.PIN[:od]...`, `plug PART HOST.CONNECTOR`.
//! A pin is named by its name in the part library or its number.

type R<T> = Result<T, String>;

#[derive(Clone, Debug, Default)]
pub struct Pcb {
    pub name: String,
    pub parts: Vec<Part>,
    pub nets: Vec<Net>,
    pub plugs: Vec<Plug>,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub id: String,
    pub kind: String,
    /// the value (a resistor's 4.7k), if given
    pub value: Option<String>,
    pub props: Vec<(String, String)>,
    pub line: usize,
}

impl Part {
    pub fn prop(&self, key: &str) -> Option<&str> { self.props.iter().find(|p| p.0 == key).map(|p| p.1.as_str()) }
}

/// a pin on a net: `part.pin`; `od`: the host drives it open-drain (an output 1 lets go)
#[derive(Clone, Debug, PartialEq)]
pub struct PinRef { pub part: String, pub pin: String, pub od: bool }

impl std::fmt::Display for PinRef {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, "{}.{}", self.part, self.pin) }
}

#[derive(Clone, Debug)]
pub struct Net { pub name: String, pub pins: Vec<PinRef>, pub line: usize }

/// connector `part` plugged into `host` (e.g. "icebreaker.PMOD1A")
#[derive(Clone, Debug)]
pub struct Plug { pub part: String, pub host: String, pub line: usize }

/// the words of a line, a "quoted string" as one word, without the # comment
fn words(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            '"' => { quoted = !quoted; any = true; }
            '#' if !quoted => break,
            c if c.is_whitespace() && !quoted => { if any { out.push(std::mem::take(&mut cur)); any = false; } }
            c => { cur.push(c); any = true; }
        }
    }
    if any { out.push(cur); }
    out
}

pub fn parse(src: &str) -> R<Pcb> {
    let mut b = Pcb::default();
    for (i, text) in src.lines().enumerate() {
        let line = i + 1;
        let w = words(text);
        let Some(head) = w.first() else { continue };
        let err = |m: String| format!("line {}: {}", line, m);
        match head.as_str() {
            "board" => b.name = w[1..].join(" "),
            "part" => {
                let (Some(id), Some(kind)) = (w.get(1), w.get(2)) else { return Err(err("part ID KIND [VALUE] [KEY=VALUE]...".into())) };
                if b.parts.iter().any(|p| &p.id == id) { return Err(err(format!("part {} is declared twice", id))); }
                let mut value = None;
                let mut props = vec![];
                for x in &w[3..] {
                    match x.split_once('=') {
                        Some((k, v)) => props.push((k.to_string(), v.to_string())),
                        None if value.is_none() => value = Some(x.clone()),
                        None => return Err(err(format!("`{}`: a part has one value; other settings are KEY=VALUE", x))),
                    }
                }
                b.parts.push(Part { id: id.clone(), kind: kind.clone(), value, props, line });
            }
            "net" => {
                let Some(name) = w.get(1) else { return Err(err("net NAME PART.PIN...".into())) };
                if b.nets.iter().any(|n| &n.name == name) { return Err(err(format!("net {} is declared twice (one line lists all its pins)", name))); }
                let mut pins = vec![];
                for x in &w[2..] {
                    let (x, od) = match x.strip_suffix(":od") { Some(x) => (x, true), None => (x.as_str(), false) };
                    let Some((part, pin)) = x.split_once('.') else { return Err(err(format!("`{}` is not PART.PIN", x))) };
                    pins.push(PinRef { part: part.into(), pin: pin.into(), od });
                }
                b.nets.push(Net { name: name.clone(), pins, line });
            }
            "plug" => {
                let (Some(part), Some(host)) = (w.get(1), w.get(2)) else { return Err(err("plug PART HOST.CONNECTOR".into())) };
                b.plugs.push(Plug { part: part.clone(), host: host.clone(), line });
            }
            o => return Err(err(format!("unknown statement `{}` (board, part, net, plug)", o))),
        }
    }
    Ok(b)
}

/// a resistor's value in ohms: 4.7k, 10k, 1M, 330, 4k7
pub fn ohms(v: &str) -> Option<f64> {
    let v = v.trim_end_matches(['Ω', 'R', 'r']).trim();
    for (s, m) in [('k', 1e3), ('K', 1e3), ('M', 1e6), ('m', 1e-3)] {
        if let Some(i) = v.find(s) {
            let (a, b) = (&v[..i], &v[i + 1..]);
            let x: f64 = if b.is_empty() { a.parse().ok()? } else { format!("{}.{}", a, b).parse().ok()? };
            return Some(x * m);
        }
    }
    v.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_board() {
        let b = parse("board \"PMOD sensor\" # comment\npart J1 pmod\npart R1 res 4.7k\npart U1 lm75 temp=21.5\n\nnet SCL J1.1:od U1.SCL R1.1\nplug J1 icebreaker.PMOD1A\n").unwrap();
        assert_eq!(b.name, "PMOD sensor");
        assert_eq!(b.parts.len(), 3);
        assert_eq!(b.parts[1].value.as_deref(), Some("4.7k"));
        assert_eq!(b.parts[2].prop("temp"), Some("21.5"));
        assert_eq!(b.nets[0].pins[0], PinRef { part: "J1".into(), pin: "1".into(), od: true });
        assert_eq!(b.nets[0].line, 6);
        assert_eq!(b.plugs[0].host, "icebreaker.PMOD1A");
        assert!(parse("part R1 res\npart R1 res").unwrap_err().contains("line 2"));
        assert!(parse("wire a b").unwrap_err().contains("unknown statement"));
    }

    #[test]
    fn resistor_values() {
        assert_eq!(ohms("4.7k"), Some(4700.0));
        assert_eq!(ohms("4k7"), Some(4700.0));
        assert_eq!(ohms("330"), Some(330.0));
        assert_eq!(ohms("1M"), Some(1e6));
        assert_eq!(ohms("x"), None);
    }
}
