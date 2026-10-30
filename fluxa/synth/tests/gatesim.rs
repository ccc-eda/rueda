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

/// `src`'s task `top` synthesized and mapped to opt gates
fn gates(src: &str, top: &str) -> (eda::netlist::Netlist, fluxa_synth::hw::Iface) {
    let s = fluxa_synth::synthesize(&load(src), top, &Default::default()).unwrap_or_else(|e| panic!("{}", e));
    (eda::netsim::mapped(&s.netlist, "opt").unwrap(), s.iface)
}

#[test]
fn pipelined_fir_takes_a_sample_every_clock() {
    let prog = example("fir");
    let s = fluxa_synth::synthesize(&prog, "Fir", &Default::default()).unwrap();
    let mut hw = fluxa_synth::hw::HwSim::new(&s.netlist, &s.iface, &[]);
    let n = 100;
    let mut x: Vec<HwTok> = (0..n).map(|k| HwTok::Data(((k * 37) % 2000) as i128 - 1000)).collect();
    x.push(HwTok::Close);
    let out = hw.run_streams(&[x], 1000);
    assert_eq!(out[0].len(), n + 1);
    // II = 1: n samples and a few clocks to start and close
    assert!(hw.cycles <= n + 4, "{} cycles for {} samples", hw.cycles, n);
}

#[test]
fn pipelined_mac_and_its_unpipelined_twin() {
    let src = |attr: &str| format!("task Mac(a: in chan<i8>, b: in chan<i8>, out: out chan<i24>, len: u8) {{
        carry acc: i24 = 0;
        {} for _ in 0..len {{ acc += a.recv() * b.recv(); }}
        out.send(acc);
    }}", attr);
    let run = |src: &str| {
        let (nl, io) = gates(src, "Mac");
        let mut hw = fluxa_synth::hw::HwSim::new(&nl, &io, &[50]);
        let mut a: Vec<HwTok> = (0..50).map(|k| HwTok::Data(k - 20)).collect();
        let mut b: Vec<HwTok> = (0..50).map(|k| HwTok::Data(3 - k)).collect();
        a.push(HwTok::Close);
        b.push(HwTok::Close);
        let out = hw.run_streams(&[a, b], 1000);
        (out, hw.cycles)
    };
    let want: i128 = (0..50).map(|k| (k - 20) * (3 - k)).sum();
    let (p, pc) = run(&src("pipeline(ii = 1)"));
    let (f, fc) = run(&src(""));
    let (two, _) = run(&src("pipeline(ii = 2)"));
    for o in [&p, &f, &two] { assert_eq!(o[0], vec![HwTok::Data(want), HwTok::Close]); }
    assert!(pc <= 50 + 6, "pipelined: {} cycles", pc);
    assert!(fc >= 2 * 50, "states: {} cycles", fc);
}

#[test]
fn ii_one_that_cannot_be_met() {
    for (body, why) in [
        ("let v = a.recv(); let w = a.recv(); y.send(v + w);", "received twice"),
        ("let v = a.recv(); if v > 0 { y.send(v); }", "under a condition"),
        ("y.send(1); let v = a.recv(); acc += v;", "after a send"),
    ] {
        let src = format!("task T(a: in chan<i8>, y: out chan<i16>) {{ carry acc: i16 = 0; pipeline(ii = 1) for _ in 0..4 {{ {} }} y.send(acc); }}", body);
        let e = fluxa_synth::synthesize(&load(&src), "T", &Default::default()).err().unwrap_or_else(|| panic!("{} synthesized", body));
        assert!(e.contains("E0412") && e.contains(why), "{}", e);
    }
}

const REVERSE: &str = r#"
    task Rev(x: in chan<u8>, y: out chan<u8>) {
        let mut m: mem<u8, 16> = [0; 16];
        let mut n: u8 = 0;
        for v in x { m[n] = v; n += 1; }
        for k in 0..n { m[k] += 1; }
        for k in 0..n { y.send(m[n - 1 - k]); }
    }
    test rev {
        let (tx, rx) = chan::<u8>(depth = 16);
        let (ty, ry) = chan::<u8>(depth = 16);
        spawn Rev(rx, ty);
        for k in 0..12 { tx.send(k * 7); }
        tx.close();
        for k in 0..12 { assert ry.recv() == (11 - k) * 7 + 1; }
    }
"#;

#[test]
fn mem_is_a_block_ram() {
    let prog = load(REVERSE);
    agree(&prog, "Rev", &["opt", "ice40", "ecp5", "asic"]);
    let s = fluxa_synth::synthesize(&prog, "Rev", &Default::default()).unwrap();
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    let nl = eda::netsim::mapped(&s.netlist, "ice40").unwrap();
    assert!(nl.cells.iter().any(|c| c.typ == "SB_RAM40_4K"), "no block RAM");
    // not as m[i]: the memory stays registers, with a warning, and still works
    let other = REVERSE.replace("for k in 0..n { m[k] += 1; }", "for k in 0..16 { m[k] += 1; } let z = m;");
    let s = fluxa_synth::synthesize(&load(&other), "Rev", &Default::default()).unwrap();
    assert!(s.warnings.iter().any(|w| w.contains("stays registers")), "{:?}", s.warnings);
}

#[test]
fn tuple_channels_match_with_sends_and_constant_patterns() {
    let prog = load(r#"
        const K_INC: u8 = 0;
        const K_FWD: u8 = 1;
        task E(x: in chan<(u8, u8)>, y: out chan<(u2, u8)>, r: out chan<u8>) {
            for (c, a) in x {
                let (k, v) = match c {
                    K_INC => (1, a + 1),
                    K_FWD => {
                        r.send(a);
                        (2, a)
                    }
                    _ => (0, 0),
                };
                if k == 0 { return; }
                y.send((k, v.wrap::<u8>()));
            }
        }
        test t {
            let (tx, rx) = chan::<(u8, u8)>(depth = 8);
            let (ty, ry) = chan::<(u2, u8)>(depth = 8);
            let (tr, rr) = chan::<u8>(depth = 8);
            spawn E(rx, ty, tr);
            tx.send((0, 255)); tx.send((1, 7)); tx.send((2, 9)); tx.send((0, 1));
            tx.close();
            let a = ry.recv();
            assert a.0 == 1 && a.1 == 0;
            let b = ry.recv();
            assert b.0 == 2 && b.1 == 7;
            assert rr.recv() == 7;
            for _ in ry { assert false, "E returns at the third token"; }
        }
    "#);
    agree(&prog, "E", &["opt", "ice40"]);
}

/// the RV32I processor (four pipeline tasks) with every stage as gates: two programs (all 24
/// tests take minutes; `reda gatesim opt fluxa/rv32i/rv32i.fx fluxa/rv32i/tests_basic.fx`)
#[test]
fn rv32i_pipeline_in_gates() {
    let rd = |f: &str| std::fs::read_to_string(format!("{}/../rv32i/{}", env!("CARGO_MANIFEST_DIR"), f)).unwrap();
    let prog = fluxa::load_many(&[("rv32i.fx".into(), rd("rv32i.fx")), ("tests_basic.fx".into(), rd("tests_basic.fx"))]).unwrap();
    let p = Arc::new(Provider::all(&prog, "opt"));
    let opts = fluxa::Options { coop: true, hw: Some(p.clone()), ..Default::default() };
    for name in ["sum_1_to_10", "all_six_branches"] {
        let t = prog.tests.iter().find(|t| t.name == name).unwrap();
        let r = fluxa::run_test(&prog, t, &opts);
        assert!(r.passed, "{}: {}", name, r.message);
    }
    // the four stages ran as gates (a task that does not synthesize would stay in the simulator)
    let built = p.report();
    for stage in ["Fetch", "Decode", "Execute", "Mem"] {
        let r = built.iter().find(|(n, _)| n == stage).map(|(_, r)| r.clone());
        assert!(matches!(r, Some(Ok(c)) if c > 1000), "{}: {:?}", stage, r);
    }
}
