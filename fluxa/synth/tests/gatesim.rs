//! Co-simulation: the Fluxa tests of a program, with the synthesized task replaced by its
//! netlist (every kind of netlist, both schedulers), must give the simulator's results.

use fluxa::runtime::{HwProvider, HwTask, HwTok};
use fluxa::value::Value;
use fluxa_synth::hw::Provider;
use std::sync::Arc;

fn load(src: &str) -> Arc<fluxa::ast::Program> {
    fluxa::load_many(&[("t.fx".to_string(), src.to_string())]).unwrap_or_else(|e| panic!("{}", e))
}

/// every test of `prog` with `top` as `kind` gates: (name, passed, message)
fn gate_run(prog: &Arc<fluxa::ast::Program>, top: &str, kind: &str, coop: bool) -> Vec<(String, bool, String)> {
    let p = Provider::new(prog, top, kind).unwrap_or_else(|e| panic!("{}", e));
    let opts = fluxa::Options { coop, hw: Some(Arc::new(p)), ..Default::default() };
    prog.tests.iter().map(|t| { let r = fluxa::run_test(prog, t, &opts); (r.name, r.passed, r.message) }).collect()
}

fn behavioral(prog: &Arc<fluxa::ast::Program>) -> Vec<(String, bool, String)> {
    prog.tests.iter().map(|t| { let r = fluxa::run_test(prog, t, &fluxa::Options::default()); (r.name, r.passed, r.message) }).collect()
}

/// the gate-level runs agree with the behavioral one, test by test
fn agree(prog: &Arc<fluxa::ast::Program>, top: &str, kinds: &[&str]) {
    let want = behavioral(prog);
    assert!(!want.is_empty());
    for kind in kinds {
        for coop in [false, true] {
            let got = gate_run(prog, top, kind, coop);
            for (w, g) in want.iter().zip(&got) {
                assert_eq!(w.1, g.1, "{} ({}, coop {}): behavioral {} / gates {}: {} / {}", w.0, kind, coop, w.1, g.1, w.2, g.2);
                assert_eq!(w.2.lines().next(), g.2.lines().next(), "{} ({}): messages differ", w.0, kind);
            }
        }
    }
}

const ALL: &[&str] = &["opt", "ice40", "ecp5", "asic", "sky130"];

#[test]
fn mac_on_every_netlist() {
    let prog = fluxa::load_many(&[("mac.fx".into(), std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/mac.fx")).unwrap())]).unwrap();
    agree(&prog, "Mac", ALL);
}

#[test]
fn streams_control_and_end_of_stream() {
    let prog = load(r#"
        fn clamp_add(a: u8, b: u8) -> u8 { let s = a + b; s.sat::<u8>() }
        task Classify(x: in chan<u8>, y: out chan<u8>) {
            let mut n: u16 = 0;
            for v in x {
                n += 1;
                let c = match v { 0 => 0, 1..10 => 1, 10..=49 => 2, _ => 3 };
                if v % 2 == 0 { y.send(clamp_add(v, 200)); } else { y.send(c); }
            }
            y.send(n.wrap::<u8>());
        }
        test classify_and_count {
            let (tx, rx) = chan::<u8>(depth = 2);
            let (ty, ry) = chan::<u8>(depth = 2);
            spawn Classify(rx, ty);
            for v in [0, 3, 10, 51, 99, 7] { tx.send(v); }
            tx.close();
            let want = [200, 1, 210, 3, 3, 1, 6];
            for k in 0..7 { assert ry.recv() == want[k]; }
            // the task ended: its output is closed
            let mut extra = 0;
            for _ in ry { extra += 1; }
            assert extra == 0;
        }
        test wrong_expectation_fails {
            let (tx, rx) = chan::<u8>(depth = 2);
            let (ty, ry) = chan::<u8>(depth = 2);
            spawn Classify(rx, ty);
            tx.send(4);
            tx.close();
            assert ry.recv() == 205, "clamp_add(4, 200) is 204";
        }
    "#);
    agree(&prog, "Classify", &["opt", "ice40", "asic"]);
}

#[test]
fn deadlock_with_hardware_is_reported() {
    // the test waits for an output the task only sends after a second input that never comes
    let prog = load(r#"
        task Pair(x: in chan<i8>, y: out chan<i8>) {
            for v in x { let w = x.recv(); y.send(v + w); }
        }
        test waits_forever {
            let (tx, rx) = chan::<i8>(depth = 1);
            let (ty, ry) = chan::<i8>(depth = 1);
            spawn Pair(rx, ty);
            tx.send(1);
            assert ry.recv() == 1;
        }
    "#);
    agree(&prog, "Pair", &["opt"]);
    assert!(gate_run(&prog, "Pair", "opt", true)[0].2.contains("deadlock"));
}

/// hardware that answers wrongly: the test must fail, so the hook really runs the hardware
struct Liar;
struct LiarTask;
impl HwTask for LiarTask {
    fn cycle(&mut self, offer: &[Option<HwTok>], room: &[bool]) -> (Vec<bool>, Vec<Option<HwTok>>, bool) {
        let taken: Vec<bool> = offer.iter().map(|o| o.is_some()).collect();
        let sent = room.iter().zip(offer).map(|(r, o)| match (r, o) { (true, Some(HwTok::Data(v))) => Some(HwTok::Data(v + 1)), (true, Some(HwTok::Close)) => Some(HwTok::Close), _ => None }).collect();
        (taken, sent, false)
    }
}
impl HwProvider for Liar {
    fn start(&self, task: &str, _: &[(String, Value)]) -> Option<Result<Box<dyn HwTask>, String>> { (task == "Copy").then(|| Ok(Box::new(LiarTask) as Box<dyn HwTask>)) }
}

#[test]
fn the_hardware_is_what_runs() {
    let prog = load(r#"
        task Copy(x: in chan<i8>, y: out chan<i8>) { for v in x { y.send(v); } }
        test copies {
            let (tx, rx) = chan::<i8>(depth = 1);
            let (ty, ry) = chan::<i8>(depth = 1);
            spawn Copy(rx, ty);
            tx.send(5);
            tx.close();
            assert ry.recv() == 5;
        }
    "#);
    assert!(behavioral(&prog)[0].1);
    let r = fluxa::run_test(&prog, &prog.tests[0], &fluxa::Options { hw: Some(Arc::new(Liar)), ..Default::default() });
    assert!(!r.passed && r.message.contains("assertion failed"), "{}", r.message);
}
