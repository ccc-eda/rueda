//! The part library: each kind's pins (number, name, electrical type), a description, and (from
//! B2 on) its footprint. The behaviour of the active parts is in `sim`.

/// a pin's electrical type, for the ERC and the simulation
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PinType {
    /// a supply the board gets from its host (a connector's 3V3: true, GND: false)
    Supply(bool),
    /// a part's power input (VCC: true) or ground (GND: false)
    Power(bool),
    /// a logic input
    Input,
    /// a push-pull output
    Output,
    /// an open-drain output (pulls low or lets go)
    OpenDrain,
    /// open-drain both ways (I²C SDA on a target)
    OpenDrainIo,
    /// a resistor's, LED's or button's terminal
    Passive,
    /// a connector's signal pin (the host decides)
    Signal,
}

impl PinType {
    /// a short name: supply, power, in, out, od, odio, passive, signal
    pub fn name(self) -> &'static str {
        match self {
            PinType::Supply(_) => "supply", PinType::Power(_) => "power", PinType::Input => "in", PinType::Output => "out",
            PinType::OpenDrain => "od", PinType::OpenDrainIo => "odio", PinType::Passive => "passive", PinType::Signal => "signal",
        }
    }
}

pub struct Kind {
    pub name: &'static str,
    /// (number, name, type)
    pub pins: &'static [(u32, &'static str, PinType)],
    pub desc: (&'static str, &'static str),
}

impl Kind {
    /// a pin by name (any case) or number
    pub fn pin(&self, p: &str) -> Option<(u32, &'static str, PinType)> {
        self.pins.iter().copied().find(|x| x.1.eq_ignore_ascii_case(p) || p.parse::<u32>().ok() == Some(x.0))
    }
}

use PinType::*;

pub const KINDS: &[Kind] = &[
    Kind { name: "res", pins: &[(1, "1", Passive), (2, "2", Passive)], desc: ("電阻", "resistor") },
    Kind { name: "led", pins: &[(1, "A", Passive), (2, "K", Passive)], desc: ("LED（A 陽極、K 陰極）", "LED (A anode, K cathode)") },
    Kind { name: "btn", pins: &[(1, "1", Passive), (2, "2", Passive)], desc: ("按鈕（按下時 1、2 相通）", "push button (1 and 2 connect while pressed)") },
    Kind {
        name: "pmod",
        pins: &[(1, "1", Signal), (2, "2", Signal), (3, "3", Signal), (4, "4", Signal), (5, "5", Supply(false)), (6, "6", Supply(true)),
                (7, "7", Signal), (8, "8", Signal), (9, "9", Signal), (10, "10", Signal), (11, "11", Supply(false)), (12, "12", Supply(true))],
        desc: ("PMOD 連接器（12 腳：1-4、7-10 訊號，5 / 11 GND，6 / 12 3V3）", "PMOD connector (12 pins: 1-4, 7-10 signals, 5 / 11 GND, 6 / 12 3V3)"),
    },
    Kind {
        name: "lm75",
        pins: &[(1, "SDA", OpenDrainIo), (2, "SCL", Input), (3, "OS", OpenDrain), (4, "GND", Power(false)),
                (5, "A2", Input), (6, "A1", Input), (7, "A0", Input), (8, "VCC", Power(true))],
        desc: ("LM75 I²C 溫度感測器（位址 0x48 + A2A1A0，0.5 °C）", "LM75 I²C temperature sensor (address 0x48 + A2A1A0, 0.5 °C)"),
    },
    Kind {
        name: "74hc595",
        pins: &[(15, "Q0", Output), (1, "Q1", Output), (2, "Q2", Output), (3, "Q3", Output), (4, "Q4", Output), (5, "Q5", Output),
                (6, "Q6", Output), (7, "Q7", Output), (8, "GND", Power(false)), (9, "Q7S", Output), (10, "MR_N", Input),
                (11, "SRCLK", Input), (12, "RCLK", Input), (13, "OE_N", Input), (14, "SER", Input), (16, "VCC", Power(true))],
        desc: ("74HC595 8 位元移位暫存器（SRCLK 移入 SER，RCLK 鎖存到 Q0-Q7）", "74HC595 8-bit shift register (SRCLK shifts SER in, RCLK latches Q0-Q7)"),
    },
];

pub fn kind(name: &str) -> Option<&'static Kind> { KINDS.iter().find(|k| k.name == name) }

/// a host connector: per connector pin 1..=12 the host's pin (an FPGA package pin), None for power
pub struct Host { pub name: &'static str, pub pins: [Option<u32>; 12] }

/// the iCEBreaker's PMOD headers (pins 1-4, 7-10; 5 / 11 GND, 6 / 12 3V3)
pub const HOSTS: &[Host] = &[
    Host { name: "icebreaker.PMOD1A", pins: [Some(4), Some(2), Some(47), Some(45), None, None, Some(3), Some(48), Some(46), Some(44), None, None] },
    Host { name: "icebreaker.PMOD1B", pins: [Some(43), Some(38), Some(34), Some(31), None, None, Some(42), Some(36), Some(32), Some(28), None, None] },
];

pub fn host(name: &str) -> Option<&'static Host> { HOSTS.iter().find(|h| h.name.eq_ignore_ascii_case(name)) }
