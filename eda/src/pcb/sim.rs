//! A board in simulation: the level of every net and the parts that react to them.
//!
//! A net's level comes from its strong drivers: a supply, a push-pull output, anything that pulls
//! it low (open drain, an I²C target's ACK). Low wins over high (open drain), and a push-pull 1
//! against a low is a conflict (counted). A net no strong driver holds takes its level through a
//! resistor from a net that has one (a pull-up, an LED's series resistor to GND); otherwise it
//! floats (None). A pressed button joins its two nets. Parts with state (`Lm75`, `Hc595`) see the
//! levels after every change and may drive their pins in turn, until nothing changes.
//!
//! The host (the FPGA board) drives its connector pins and reads them back: `set_host`, `update`,
//! `host_level`; `fpga::board::Board` does that every clock cycle in which an output changed.

use super::erc::rail;
use super::parts::PinType;
use super::Board;

type R<T> = Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drive { Low, High }

/// a host pin on the board: the FPGA's package pin, its net, open drain or not
#[derive(Clone, Debug)]
pub struct HostPin { pub pin: u32, pub net: usize, pub od: bool }

struct Led { part: usize, a: Option<usize>, k: Option<usize>, lit: bool, since: u64, acc: u64 }

/// the LM75 as an I²C target
#[derive(Default)]
struct Lm75 {
    sda: Option<usize>, scl: Option<usize>, a: [Option<usize>; 3],
    /// the temperature in half degrees; the registers: pointer, configuration, hysteresis, overtemperature
    temp: i16, ptr: u8, conf: u8, thyst: u16, tos: u16,
    st: I2c, bits: u8, byte: u8,
    /// bytes being read, and the next one
    out: Vec<u8>, oi: usize,
    /// bytes written after the pointer
    written: u8,
    prev_scl: bool, prev_sda: bool,
    low: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum I2c {
    #[default]
    Idle,
    Addr,
    Write,
    /// this target's ACK (then: read or not)
    Ack(bool),
    Send,
    /// the controller's ACK after a byte this target sent
    CtrlAck,
}

#[derive(Default)]
struct Hc595 {
    ser: Option<usize>, srclk: Option<usize>, rclk: Option<usize>, mr: Option<usize>, oe: Option<usize>,
    q: [Option<usize>; 8], q7s: Option<usize>,
    shift: u8, latch: u8, prev_srclk: bool, prev_rclk: bool,
    /// OE_N as last seen (floating: enabled, the ERC warns)
    oe_level: Option<bool>,
    /// the levels were seen once (power-up is not an edge)
    started: bool,
}

enum Dev { Lm75(usize, Box<Lm75>), Hc595(usize, Hc595) }

/// what a part shows (for the IDE)
#[derive(Clone, Debug, PartialEq)]
pub enum Show {
    /// an LED: the fraction of the time it was lit
    Led(f64),
    /// a button: pressed
    Button(bool),
    /// an LM75: its temperature (°C)
    Temp(f64),
    /// a 74HC595: its output register
    Latch(u8),
}

pub struct Sim {
    nets: usize,
    rails: Vec<Option<bool>>,
    /// resistors: the two nets of each
    res: Vec<(usize, usize)>,
    /// buttons: (part, net 1, net 2, pressed)
    btns: Vec<(usize, Option<usize>, Option<usize>, bool)>,
    pub hosts: Vec<HostPin>,
    host_out: Vec<Option<bool>>,
    devs: Vec<Dev>,
    leds: Vec<Led>,
    level: Vec<Option<bool>>,
    group: Vec<usize>,
    /// cycles in which a push-pull 1 met a low
    pub conflicts: u64,
    t0: u64,
    ids: Vec<String>,
}

impl Sim {
    pub fn new(b: &Board) -> R<Sim> {
        let n = b.nets.len();
        let net = |part: usize, name: &str| b.kinds[part].pin(name).and_then(|p| b.net_of(part, p.0));
        let mut s = Sim {
            nets: n, rails: (0..n).map(|i| rail(b, i)).collect(), res: vec![], btns: vec![], hosts: vec![], host_out: vec![],
            devs: vec![], leds: vec![], level: vec![None; n], group: (0..n).collect(), conflicts: 0, t0: 0,
            ids: b.desc.parts.iter().map(|p| p.id.clone()).collect(),
        };
        for (i, (p, k)) in b.desc.parts.iter().zip(&b.kinds).enumerate() {
            match k.name {
                "res" => if let (Some(x), Some(y)) = (net(i, "1"), net(i, "2")) { s.res.push((x, y)) },
                "btn" => s.btns.push((i, net(i, "1"), net(i, "2"), false)),
                "led" => s.leds.push(Led { part: i, a: net(i, "A"), k: net(i, "K"), lit: false, since: 0, acc: 0 }),
                "lm75" => {
                    let temp: f64 = match p.prop("temp") { Some(t) => t.parse().map_err(|_| format!("line {}: {}: temp={} is not a number", p.line, p.id, t))?, None => 25.0 };
                    let mut d = Lm75 { sda: net(i, "SDA"), scl: net(i, "SCL"), a: [net(i, "A0"), net(i, "A1"), net(i, "A2")], prev_scl: true, prev_sda: true, ..Default::default() };
                    d.set_temp(temp);
                    d.thyst = 75 << 8;
                    d.tos = 80 << 8;
                    s.devs.push(Dev::Lm75(i, Box::new(d)));
                }
                "74hc595" => {
                    let mut d = Hc595 { ser: net(i, "SER"), srclk: net(i, "SRCLK"), rclk: net(i, "RCLK"), mr: net(i, "MR_N"), oe: net(i, "OE_N"), q7s: net(i, "Q7S"), ..Default::default() };
                    for k in 0..8 { d.q[k] = net(i, &format!("Q{}", k)); }
                    s.devs.push(Dev::Hc595(i, d));
                }
                _ => {}
            }
        }
        // the connectors' signal pins on the host's pins
        for pl in &b.desc.plugs {
            let (Some(i), Some(h)) = (b.desc.parts.iter().position(|x| x.id == pl.part), super::parts::host(&pl.host)) else {
                return Err(format!("line {}: plug {} {}: no such part or host connector", pl.line, pl.part, pl.host));
            };
            for (k, hp) in h.pins.iter().enumerate() {
                let Some(fpga) = hp else { continue };
                let num = k as u32 + 1;
                let Some(netn) = b.net_of(i, num) else { continue };
                let od = b.nets[netn].pins.iter().any(|p| p.part == i && p.num == num && p.od && p.ty == PinType::Signal);
                s.hosts.push(HostPin { pin: *fpga, net: netn, od });
            }
        }
        s.host_out = vec![None; s.hosts.len()];
        Ok(s)
    }

    /// what the host drives on host pin `k` (Some: an output's value; None: it does not drive)
    pub fn set_host(&mut self, k: usize, v: Option<bool>) { self.host_out[k] = v; }

    /// the level the host reads on host pin `k` (a floating net reads 1, as with the FPGA's pull-up)
    pub fn host_level(&self, k: usize) -> bool { self.level[self.hosts[k].net].unwrap_or(true) }

    pub fn level(&self, net: usize) -> Option<bool> { self.level[net] }

    /// every net's level (None: floating), in the board's net order
    pub fn levels(&self) -> &[Option<bool>] { &self.level }

    fn part(&self, id: &str) -> R<usize> { self.ids.iter().position(|x| x == id).ok_or_else(|| format!("no part {}", id)) }

    /// press or release button `id`
    pub fn press(&mut self, id: &str, down: bool) -> R<()> {
        let p = self.part(id)?;
        let b = self.btns.iter_mut().find(|b| b.0 == p).ok_or_else(|| format!("{} is not a button", id))?;
        b.3 = down;
        self.regroup();
        Ok(())
    }

    /// set an LM75's temperature (°C)
    pub fn set_temp(&mut self, id: &str, t: f64) -> R<()> {
        let p = self.part(id)?;
        for d in self.devs.iter_mut() { if let Dev::Lm75(i, x) = d { if *i == p { x.set_temp(t); return Ok(()); } } }
        Err(format!("{} is not a temperature sensor", id))
    }

    /// nets joined by pressed buttons
    fn regroup(&mut self) {
        let mut g: Vec<usize> = (0..self.nets).collect();
        fn find(g: &mut [usize], x: usize) -> usize { let mut r = x; while g[r] != r { r = g[r]; } g[x] = r; r }
        for (_, a, b, down) in &self.btns {
            if let (Some(a), Some(b), true) = (a, b, down) { let (x, y) = (find(&mut g, *a), find(&mut g, *b)); g[x] = y; }
        }
        for i in 0..self.nets { self.group[i] = find(&mut g, i); }
    }

    /// the levels from the drives (host and parts)
    fn resolve(&mut self, drives: &[(usize, Drive)]) {
        let n = self.nets;
        let (mut low, mut high, mut rl) = (vec![false; n], vec![false; n], vec![None; n]);
        for (net, d) in drives {
            let g = self.group[*net];
            if *d == Drive::Low { low[g] = true } else { high[g] = true }
        }
        for i in 0..n { if let Some(r) = self.rails[i] { rl[self.group[i]] = Some(r); } }
        let mut strong = vec![None; n];
        for g in 0..n {
            if self.group[g] != g { continue; }
            if (low[g] && high[g]) || rl[g].is_some_and(|r| (r && low[g]) || (!r && high[g])) { self.conflicts += 1; }
            strong[g] = rl[g].or(if low[g] { Some(false) } else if high[g] { Some(true) } else { None });
        }
        // through resistors: the level of the strong nets on the other side, when they agree
        let mut weak: Vec<Option<Option<bool>>> = vec![None; n];
        for (a, b) in &self.res {
            let (ga, gb) = (self.group[*a], self.group[*b]);
            for (x, y) in [(ga, gb), (gb, ga)] {
                if strong[x].is_none() {
                    if let Some(v) = strong[y] { weak[x] = Some(match weak[x] { None => Some(v), Some(w) if w == Some(v) => w, _ => None }); }
                }
            }
        }
        for i in 0..n {
            let g = self.group[i];
            self.level[i] = strong[g].or(weak[g].flatten());
        }
    }

    /// settle the board after the host's pins (or a button, a temperature) changed, at cycle `now`
    pub fn update(&mut self, now: u64) {
        for _ in 0..8 {
            let mut drives = vec![];
            for (h, v) in self.hosts.iter().zip(&self.host_out) {
                match (v, h.od) {
                    (Some(false), _) => drives.push((h.net, Drive::Low)),
                    (Some(true), false) => drives.push((h.net, Drive::High)),
                    _ => {}
                }
            }
            for d in &self.devs { d.drives(&mut drives); }
            self.resolve(&drives);
            let level = &self.level;
            let mut changed = false;
            for d in self.devs.iter_mut() { changed |= d.react(level); }
            if !changed { break; }
        }
        for l in self.leds.iter_mut() {
            let on = l.a.and_then(|a| self.level[a]) == Some(true) && l.k.and_then(|k| self.level[k]) == Some(false);
            if on != l.lit {
                if l.lit { l.acc += now.saturating_sub(l.since); }
                l.lit = on;
                l.since = now;
            }
        }
    }

    /// what each part shows since the last call, at cycle `now`: (part id, state)
    pub fn take_show(&mut self, now: u64) -> Vec<(String, Show)> {
        let w = now.saturating_sub(self.t0).max(1) as f64;
        let mut out = vec![];
        for l in self.leds.iter_mut() {
            let t = l.acc + if l.lit { now.saturating_sub(l.since.max(self.t0)) } else { 0 };
            let duty = if now == self.t0 { l.lit as u8 as f64 } else { (t as f64 / w).min(1.0) };
            out.push((self.ids[l.part].clone(), Show::Led(duty)));
            l.acc = 0;
            l.since = now;
        }
        for b in &self.btns { out.push((self.ids[b.0].clone(), Show::Button(b.3))); }
        for d in &self.devs {
            match d {
                Dev::Lm75(i, x) => out.push((self.ids[*i].clone(), Show::Temp(x.temp as f64 / 2.0))),
                Dev::Hc595(i, x) => out.push((self.ids[*i].clone(), Show::Latch(x.latch))),
            }
        }
        self.t0 = now;
        out
    }
}

impl Dev {
    fn drives(&self, out: &mut Vec<(usize, Drive)>) {
        match self {
            Dev::Lm75(_, d) => if let (true, Some(sda)) = (d.low, d.sda) { out.push((sda, Drive::Low)) },
            Dev::Hc595(_, d) => d.drives(out),
        }
    }

    /// see the levels; true if what the part drives changed
    fn react(&mut self, level: &[Option<bool>]) -> bool {
        match self {
            Dev::Lm75(_, d) => d.react(level),
            Dev::Hc595(_, d) => d.react(level),
        }
    }
}

/// a net's level as a logic input (floating: 1)
fn lv(level: &[Option<bool>], n: Option<usize>) -> bool { n.and_then(|n| level[n]).unwrap_or(true) }

impl Lm75 {
    fn set_temp(&mut self, t: f64) { self.temp = (t * 2.0).round().clamp(-110.0, 250.0) as i16; }

    /// the bytes of the register the pointer selects
    fn reg(&self) -> Vec<u8> {
        let w = |v: u16| vec![(v >> 8) as u8, v as u8];
        match self.ptr & 3 { 0 => w((self.temp as u16) << 7), 1 => vec![self.conf], 2 => w(self.thyst), _ => w(self.tos) }
    }

    fn write(&mut self, b: u8) {
        if self.written == 0 { self.ptr = b & 3; } else {
            let k = self.written - 1;
            match self.ptr & 3 {
                1 => self.conf = b,
                2 => self.thyst = if k == 0 { (b as u16) << 8 | (self.thyst & 0xff) } else { (self.thyst & 0xff00) | (b as u16 & 0x80) },
                3 => self.tos = if k == 0 { (b as u16) << 8 | (self.tos & 0xff) } else { (self.tos & 0xff00) | (b as u16 & 0x80) },
                _ => {}
            }
        }
        self.written = self.written.saturating_add(1);
    }

    /// send bit `bits` (msb first) of the current byte
    fn put(&mut self) { self.low = (self.out[self.oi % self.out.len()] >> (7 - self.bits)) & 1 == 0; }

    fn react(&mut self, level: &[Option<bool>]) -> bool {
        let (scl, sda) = (lv(level, self.scl), lv(level, self.sda));
        let was = self.low;
        if scl && self.prev_scl && sda != self.prev_sda {
            // START (SDA falls while SCL is high) or STOP (rises)
            self.st = if sda { I2c::Idle } else { I2c::Addr };
            self.bits = 0;
            self.byte = 0;
            self.low = false;
        } else if scl && !self.prev_scl {
            match self.st {
                I2c::Addr | I2c::Write => { self.byte = self.byte << 1 | sda as u8; self.bits += 1; }
                // the controller's ACK: 1 (NACK) ends the read
                I2c::CtrlAck if sda => self.st = I2c::Idle,
                _ => {}
            }
        } else if !scl && self.prev_scl {
            match self.st {
                I2c::Addr if self.bits == 8 => {
                    let a = 0x48 | self.a.iter().enumerate().map(|(k, n)| (n.and_then(|n| level[n]).unwrap_or(false) as u8) << k).sum::<u8>();
                    if self.byte >> 1 == a {
                        self.low = true;
                        let read = self.byte & 1 == 1;
                        self.st = I2c::Ack(read);
                        if !read { self.written = 0; }
                    } else { self.st = I2c::Idle; }
                }
                I2c::Write if self.bits == 8 => { let b = self.byte; self.write(b); self.low = true; self.st = I2c::Ack(false); }
                I2c::Ack(read) => {
                    self.low = false;
                    self.bits = 0;
                    self.byte = 0;
                    if read { self.out = self.reg(); self.oi = 0; self.st = I2c::Send; self.put(); } else { self.st = I2c::Write; }
                }
                I2c::Send => {
                    self.bits += 1;
                    if self.bits == 8 { self.low = false; self.st = I2c::CtrlAck; } else { self.put(); }
                }
                I2c::CtrlAck => { self.oi += 1; self.bits = 0; self.st = I2c::Send; self.put(); }
                _ => {}
            }
        }
        self.prev_scl = scl;
        self.prev_sda = sda;
        self.low != was
    }
}

impl Hc595 {
    fn drives(&self, out: &mut Vec<(usize, Drive)>) {
        let d = |b: bool| if b { Drive::High } else { Drive::Low };
        if let Some(n) = self.q7s { out.push((n, d(self.shift >> 7 & 1 == 1))); }
        // OE_N high: Q0-Q7 let go
        if self.oe_level != Some(true) {
            for (k, q) in self.q.iter().enumerate() { if let Some(n) = q { out.push((*n, d(self.latch >> k & 1 == 1))); } }
        }
    }

    fn react(&mut self, level: &[Option<bool>]) -> bool {
        let before = (self.shift, self.latch, self.oe_level);
        let srclk = lv(level, self.srclk);
        let rclk = lv(level, self.rclk);
        if !self.started { self.prev_srclk = srclk; self.prev_rclk = rclk; self.started = true; }
        if !lv(level, self.mr) { self.shift = 0; } else if srclk && !self.prev_srclk { self.shift = self.shift << 1 | lv(level, self.ser) as u8; }
        if rclk && !self.prev_rclk { self.latch = self.shift; }
        self.prev_srclk = srclk;
        self.prev_rclk = rclk;
        self.oe_level = self.oe.and_then(|n| level[n]);
        before != (self.shift, self.latch, self.oe_level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the sensor module of the plan: LM75 on I²C (SCL on PMOD pin 1, SDA driven on 2 and read on 8),
    /// a 74HC595 (SER 3, SRCLK 4, RCLK 7) with LEDs on Q0 and Q7, a button on pin 9 to GND
    const MODULE: &str = "part J1 pmod\npart U1 lm75 temp=23.5\npart R1 res 4.7k\npart R2 res 4.7k\npart U2 74hc595\n\
        part D1 led\npart D2 led\npart R3 res 330\npart R4 res 330\npart S1 btn\npart R5 res 10k\n\
        net SCL J1.1:od U1.SCL R1.1\nnet SDA J1.2:od J1.8 U1.SDA R2.1\nnet 3V3 J1.6 R1.2 R2.2 R5.2 U1.VCC U2.VCC U2.MR_N\n\
        net GND J1.5 U1.GND U1.A0 U1.A1 U1.A2 U2.GND U2.OE_N R3.2 R4.2 S1.2\n\
        net SER J1.3 U2.SER\nnet SRCLK J1.4 U2.SRCLK\nnet RCLK J1.7 U2.RCLK\n\
        net Q0 U2.Q0 D1.A\nnet Q7 U2.Q7 D2.A\nnet K1 D1.K R3.1\nnet K2 D2.K R4.1\nnet BTN J1.9 S1.1 R5.1\n\
        plug J1 icebreaker.PMOD1A\n";

    struct Host { s: Sim, t: u64 }
    impl Host {
        fn new() -> Host {
            let b = Board::parse(MODULE).unwrap();
            assert!(super::super::erc::check(&b).is_empty(), "{:?}", super::super::erc::check(&b));
            let mut s = Sim::new(&b).unwrap();
            // FPGA pins: SCL 4 and SDA 2 open drain (let go), SER 47, SRCLK 45, RCLK 3 outputs (low); 48 and 46 inputs
            for k in 0..s.hosts.len() {
                let p = s.hosts[k].pin;
                s.set_host(k, if [4, 2].contains(&p) { Some(true) } else if [47, 45, 3].contains(&p) { Some(false) } else { None });
            }
            s.update(0);
            Host { s, t: 0 }
        }
        fn k(&self, pin: u32) -> usize { self.s.hosts.iter().position(|h| h.pin == pin).unwrap() }
        fn set(&mut self, pin: u32, v: bool) { let k = self.k(pin); self.s.set_host(k, Some(v)); self.t += 1; self.s.update(self.t); }
        fn get(&self, pin: u32) -> bool { self.s.host_level(self.k(pin)) }
        // I²C on SCL = 4, SDA = 2 (read back on 48)
        fn start(&mut self) { self.set(2, true); self.set(4, true); self.set(2, false); self.set(4, false); }
        fn stop(&mut self) { self.set(2, false); self.set(4, true); self.set(2, true); }
        fn bit(&mut self, b: bool) -> bool { self.set(2, b); self.set(4, true); let r = self.get(48); self.set(4, false); r }
        /// a byte out; true if acknowledged
        fn put(&mut self, b: u8) -> bool { for k in (0..8).rev() { self.bit(b >> k & 1 == 1); } !self.bit(true) }
        fn take(&mut self, ack: bool) -> u8 { let mut v = 0; for _ in 0..8 { v = v << 1 | self.bit(true) as u8; } self.bit(!ack); v }
        fn temp(&mut self) -> (u8, u8) {
            self.start(); assert!(self.put(0x90)); assert!(self.put(0)); self.stop();
            self.start(); assert!(self.put(0x91));
            let r = (self.take(true), self.take(false));
            self.stop();
            r
        }
    }

    #[test]
    fn lm75_reads_its_temperature_over_i2c() {
        let mut h = Host::new();
        assert!(h.get(48), "SDA idles high through its pull-up");
        assert_eq!(h.temp(), (23, 0x80));
        h.s.set_temp("U1", -0.5).unwrap();
        assert_eq!(h.temp(), (0xFF, 0x80));
        // another address: no ACK
        h.start();
        assert!(!h.put(0x92));
        h.stop();
        // the configuration register: written, read back
        h.start(); assert!(h.put(0x90)); assert!(h.put(1)); assert!(h.put(0x02)); h.stop();
        h.start(); assert!(h.put(0x91)); assert_eq!(h.take(false), 0x02); h.stop();
        assert_eq!(h.s.conflicts, 0);
    }

    #[test]
    fn hc595_shifts_and_latches_onto_the_leds() {
        let mut h = Host::new();
        for k in (0..8).rev() { h.set(47, 0x81u8 >> k & 1 == 1); h.set(45, true); h.set(45, false); }
        let led = |h: &mut Host, id: &str| h.s.take_show(h.t).into_iter().find(|x| x.0 == id).map(|x| x.1).unwrap();
        assert_eq!(led(&mut h, "U2"), Show::Latch(0));
        h.set(3, true);
        h.set(3, false);
        h.t += 100;
        h.s.update(h.t);
        assert_eq!(led(&mut h, "U2"), Show::Latch(0x81));
        assert_eq!(led(&mut h, "D1"), Show::Led(1.0));
        assert_eq!(led(&mut h, "D2"), Show::Led(1.0));
    }

    #[test]
    fn a_button_pulls_its_pin_low() {
        let mut h = Host::new();
        assert!(h.get(46), "pulled up");
        h.s.press("S1", true).unwrap();
        h.s.update(1);
        assert!(!h.get(46));
        h.s.press("S1", false).unwrap();
        h.s.update(2);
        assert!(h.get(46));
    }
}
