//! Co-simulation: the Fluxa tests of a program, with the synthesized task replaced by its
//! netlist (every kind of netlist, both schedulers), must give the simulator's results.

use fluxa::runtime::{HwProvider, HwTask, HwTok};
use fluxa::value::Value;
use fluxa_synth::hw::Provider;
use std::sync::Arc;

fn load(src: &str) -> Arc<fluxa::ast::Program> {
    fluxa::load_many(&[("t.fx".to_string(), src.to_string())]).unwrap_or_else(|e| panic!("{}", e))
}

/// every test of `prog` with `top` (empty: every task) as `kind` gates: (name, passed, message)
fn gate_run(prog: &Arc<fluxa::ast::Program>, top: &str, kind: &str, coop: bool) -> Vec<(String, bool, String)> {
    let p = if top.is_empty() { Provider::all(prog, kind) } else { Provider::new(prog, top, kind).unwrap_or_else(|e| panic!("{}", e)) };
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
                // a deadlock is a deadlock (a network in hardware is one task, so the counts differ)
                let both_deadlock = w.2.starts_with("deadlock") && g.2.starts_with("deadlock");
                if !both_deadlock { assert_eq!(w.2.lines().next(), g.2.lines().next(), "{} ({}): messages differ", w.0, kind); }
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

fn example(name: &str) -> Arc<fluxa::ast::Program> {
    let src = std::fs::read_to_string(format!("{}/../examples/{}.fx", env!("CARGO_MANIFEST_DIR"), name)).unwrap();
    fluxa::load_many(&[(format!("{}.fx", name), src)]).unwrap()
}

#[test]
fn fir_fixed_point_and_reg_delay_line() { agree(&example("fir"), "Fir", &["opt", "ice40", "asic"]); }

#[test]
fn fanout_every_task_in_gates() { agree(&example("fanout"), "", &["opt", "ice40", "ecp5"]); }

#[test]
fn gemm_systolic_array_in_gates() { agree(&example("gemm"), "", &["opt", "asic"]); }

#[test]
fn reg_reads_see_the_old_value() {
    // inside the loop, `h` read after its write still gives the previous iteration's value
    let prog = load(r#"
        task Delay(x: in chan<u8>, y: out chan<u16>) {
            let mut h: u8 @reg = 7;
            let mut a: [u8; 3] @reg = [1, 2, 3];
            for v in x {
                h = v;
                a[0] = v;
                y.send(h * 256 + a[0]);
            }
            y.send(h);
        }
        test delay {
            let (tx, rx) = chan::<u8>(depth = 4);
            let (ty, ry) = chan::<u16>(depth = 8);
            spawn Delay(rx, ty);
            for v in [10, 20, 30] { tx.send(v); }
            tx.close();
            assert ry.recv() == 7 * 256 + 1;
            assert ry.recv() == 10 * 256 + 10;
            assert ry.recv() == 20 * 256 + 20;
            assert ry.recv() == 30;
        }
    "#);
    agree(&prog, "Delay", &["opt", "ice40"]);
}

#[test]
fn rounding_and_overflow_modes() {
    let prog = load(r#"
        task Conv(x: in chan<i16>, y: out chan<i16>) {
            for v in x {
                let f: fixed<8, 4> = v.to::<fixed<8, 4>>(ovf = wrap) * 0.375;
                y.send(f.to::<i16>(round = trunc));
                y.send(f.to::<i16>(round = nearest));
                y.send(f.to::<i16>(round = even));
                y.send(f.to::<i8>(round = nearest, ovf = sat));
                let g: fixed<4, 2> = f.to::<fixed<4, 2>>(round = even, ovf = sat);
                y.send(g.to::<i16>(round = even));
                y.send((v >> 2).wrap::<i8>());
            }
        }
        test conv {
            let (tx, rx) = chan::<i16>(depth = 4);
            let (ty, ry) = chan::<i16>(depth = 64);
            spawn Conv(rx, ty);
            for v in [-7, -6, -5, -3, -2, 0, 2, 3, 5, 6, 7, 100, -100, 500] { tx.send(v); }
            tx.close();
            let mut n = 0;
            for _ in ry { n += 1; }
            assert n == 14 * 6;
        }
        test values {
            let (tx, rx) = chan::<i16>(depth = 4);
            let (ty, ry) = chan::<i16>(depth = 8);
            spawn Conv(rx, ty);
            tx.send(-7);
            tx.close();
            // -7 * 0.375 = -2.625
            assert ry.recv() == -3;
            assert ry.recv() == -3;
            assert ry.recv() == -3;
            assert ry.recv() == -3;
            assert ry.recv() == -2;
            assert ry.recv() == -2;
        }
    "#);
    // the gates must print the same token sequence: compare the outputs, not only the asserts
    let want = behavioral(&prog);
    assert!(want.iter().all(|r| r.1), "{:?}", want);
    agree(&prog, "Conv", &["opt", "ice40"]);
    // and the token sequence itself, value by value, against the simulator's
    let s = fluxa_synth::synthesize(&prog, "Conv", &Default::default()).unwrap();
    let mut h = fluxa_synth::hw::HwSim::new(&s.netlist, &s.iface, &[]);
    let ins = [-7i128, -6, -5, -3, -2, 0, 2, 3, 5, 6, 7, 100, -100, 500];
    let out = h.run_streams(&[ins.iter().map(|v| HwTok::Data(*v)).chain([HwTok::Close]).collect()], 2000);
    let sim = conv_reference(&ins);
    assert_eq!(out[0], sim);
}

/// the simulator's outputs for `Conv`, token by token (a test task collecting them)
fn conv_reference(ins: &[i128]) -> Vec<HwTok> {
    let src = format!(r#"
        task Conv(x: in chan<i16>, y: out chan<i16>) {{
            for v in x {{
                let f: fixed<8, 4> = v.to::<fixed<8, 4>>(ovf = wrap) * 0.375;
                y.send(f.to::<i16>(round = trunc));
                y.send(f.to::<i16>(round = nearest));
                y.send(f.to::<i16>(round = even));
                y.send(f.to::<i8>(round = nearest, ovf = sat));
                let g: fixed<4, 2> = f.to::<fixed<4, 2>>(round = even, ovf = sat);
                y.send(g.to::<i16>(round = even));
                y.send((v >> 2).wrap::<i8>());
            }}
        }}
        test dump {{
            let (tx, rx) = chan::<i16>(depth = 64);
            let (ty, ry) = chan::<i16>(depth = 512);
            spawn Conv(rx, ty);
            for v in {:?} {{ tx.send(v); }}
            tx.close();
            for v in ry {{ print(v); }}
        }}
    "#, ins);
    let prog = load(&src);
    let r = fluxa::run_test(&prog, &prog.tests[0], &fluxa::Options { coop: true, ..Default::default() });
    assert!(r.passed, "{}", r.message);
    r.output.lines().map(|l| HwTok::Data(l.trim().parse().unwrap())).chain([HwTok::Close]).collect()
}

#[test]
fn array_with_run_time_index() {
    let prog = load(r#"
        task Ram(cmd: in chan<u8>, y: out chan<u8>) {
            let mut mem: [u8; 8] = [0; 8];
            for c in cmd {
                let a = c % 8;
                if c >= 128 { mem[a] = c - 128; } else { y.send(mem[a]); }
            }
        }
        test ram {
            let (tx, rx) = chan::<u8>(depth = 4);
            let (ty, ry) = chan::<u8>(depth = 8);
            spawn Ram(rx, ty);
            for c in [128 + 3, 128 + 8 + 5, 3, 5, 6, 128 + 16 + 6, 6] { tx.send(c); }
            tx.close();
            assert ry.recv() == 3;
            assert ry.recv() == 13;
            assert ry.recv() == 0;
            assert ry.recv() == 22;
        }
    "#);
    agree(&prog, "Ram", &["opt", "ice40", "asic"]);
}

#[test]
fn network_task_is_one_netlist() { agree(&example("pipeline"), "Pipeline", &["opt", "ice40", "ecp5", "asic"]); }

/// the FIFOs have the channels' depths: a network that needs depth 2 deadlocks at depth 1, in
/// the simulator and in the gates alike
#[test]
fn network_fifo_depth_is_the_channels() {
    for (depth, ok) in [(1, false), (2, true)] {
        let prog = load(&format!(r#"
            task Two(a: out chan<u8>, b: out chan<u8>) {{ a.send(1); a.send(2); b.send(3); }}
            task Swap(a: in chan<u8>, b: in chan<u8>, y: out chan<u8>) {{ let z = b.recv(); y.send(z); y.send(a.recv()); y.send(a.recv()); }}
            task Net(y: out chan<u8>) {{
                let (ta, ra) = chan::<u8>(depth = {});
                let (tb, rb) = chan::<u8>(depth = 1);
                spawn Two(ta, tb);
                spawn Swap(ra, rb, y);
            }}
            test order {{
                let (ty, ry) = chan::<u8>(depth = 4);
                spawn Net(ty);
                assert ry.recv() == 3;
                assert ry.recv() == 1;
                assert ry.recv() == 2;
            }}
        "#, depth));
        let want = behavioral(&prog);
        assert_eq!(want[0].1, ok, "depth {}: {}", depth, want[0].2);
        agree(&prog, "Net", &["opt"]);
    }
}
