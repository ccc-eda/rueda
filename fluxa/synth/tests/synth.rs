//! Fluxa tasks synthesized to gates and run in eda's gate simulator through their stream ports.

use fluxa_synth::hw::{HwSim, Tok};
use std::collections::HashMap;

fn synth(src: &str, top: &str) -> fluxa_synth::Synth {
    let prog = fluxa::load_many(&[("t.fx".to_string(), src.to_string())]).unwrap_or_else(|e| panic!("{}", e));
    fluxa_synth::synthesize(&prog, top, &HashMap::new()).unwrap_or_else(|e| panic!("{}", e))
}

fn data(v: &[i128]) -> Vec<Tok> { v.iter().map(|x| Tok::Data(*x)).chain([Tok::Close]).collect() }

#[test]
fn double_stream() {
    let s = synth("task Double(x: in chan<i16>, y: out chan<i17>) { for v in x { y.send(v * 2); } }", "Double");
    let mut h = HwSim::new(&s.netlist, &s.iface, &[]);
    let out = h.run_streams(&[data(&[1, -3, 100, 32767, -32768])], 200);
    assert_eq!(out[0], data(&[2, -6, 200, 65534, -65536]));
}

#[test]
fn mac_example() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/mac.fx")).unwrap();
    let s = synth(&src, "Mac");
    assert_eq!(s.iface.scalars.len(), 1, "len is an input port");
    let xs = [3i128, -4, 5, 32767, -32768, 7, 0, 100];
    let ys = [2i128, 9, -1, 32767, -32768, 7, 5, -100];
    let want: i128 = xs.iter().zip(&ys).map(|(a, b)| a * b).sum();
    let mut h = HwSim::new(&s.netlist, &s.iface, &[8]);
    let out = h.run_streams(&[data(&xs), data(&ys)], 500);
    assert_eq!(out[0], vec![Tok::Data(want), Tok::Close]);
}

#[test]
fn control_and_functions() {
    let src = r#"
        fn clamp_add(a: u8, b: u8) -> u8 { let s = a + b; s.sat::<u8>() }
        task Classify(x: in chan<u8>, y: out chan<u8>) {
            let mut n: u16 = 0;
            for v in x {
                n += 1;
                let c = match v { 0 => 0, 1..10 => 1, 10..=49 => 2, _ => 3 };
                if v % 2 == 0 {
                    y.send(clamp_add(v, 200));
                } else {
                    y.send(c);
                }
            }
            y.send(n.wrap::<u8>());
        }
    "#;
    let s = synth(src, "Classify");
    let mut h = HwSim::new(&s.netlist, &s.iface, &[]);
    let out = h.run_streams(&[data(&[0, 3, 10, 51, 99, 7])], 300);
    // even: clamp(v + 200); odd: the class
    assert_eq!(out[0], data(&[200, 1, 210, 3, 3, 1, 6]));
}
