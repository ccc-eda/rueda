//! fluxa-emu against the simulator: every test of a program, with its tasks compiled, must pass
//! or fail as it does in the interpreter.

use std::sync::Arc;

fn files(names: &[&str]) -> Arc<fluxa::ast::Program> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let srcs: Vec<(String, String)> = names.iter().map(|n| (n.to_string(), std::fs::read_to_string(format!("{}/{}", dir, n)).unwrap())).collect();
    fluxa::load_many(&srcs).unwrap_or_else(|e| panic!("{}", e))
}

/// each test: (name, passed, message) with the tasks interpreted, then compiled
fn agree(prog: &Arc<fluxa::ast::Program>, top: Option<&str>) {
    let p = Arc::new(fluxa_emu::Provider::new(prog, top));
    let emu = fluxa::Options { coop: true, hw: Some(p.clone()), ..Default::default() };
    let mut n = 0;
    for t in &prog.tests {
        let want = fluxa::run_test(prog, t, &fluxa::Options { coop: true, ..Default::default() });
        let got = fluxa::run_test(prog, t, &emu);
        assert_eq!(want.passed, got.passed, "{}: simulator {} / emu {}: {} / {}", t.name, want.passed, got.passed, want.message, got.message);
        n += 1;
    }
    assert!(n > 0);
    // the tasks asked for were compiled
    if let Some(t) = top {
        let rep = p.report.lock().unwrap();
        assert!(rep.iter().any(|(k, r)| k == t && r.is_ok()), "{} was not compiled: {:?}", t, rep);
    }
}

#[test]
fn examples() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".fx")).collect();
    names.sort();
    for n in names { agree(&files(&[&format!("examples/{}", n)]), None); }
}

#[test]
fn rv32i() {
    for t in ["smoke", "tests_basic", "err_wb_depth", "err_bad_immediate"] {
        // the error examples fail before (or without) a Core to compile
        agree(&files(&["rv32i/rv32i.fx", &format!("rv32i/{}.fx", t)]), if t.starts_with("err") { None } else { Some("Core") });
    }
}

fn rv64(t: &str) { agree(&files(&["rv64/rv64.fx", "rv64/fpu.fx", &format!("rv64/{}.fx", t)]), Some("Core")); }

#[test] fn rv64_smoke() { rv64("smoke") }
#[test] fn rv64_basic() { rv64("tests_basic") }
#[test] fn rv64_mmu() { rv64("tests_mmu") }
#[test] fn rv64_amo() { rv64("tests_amo") }
#[test] fn rv64_fp() { rv64("tests_fp") }
#[test] fn rv64_irq() { rv64("tests_irq") }
#[test] fn rv64_system() { rv64("tests_system") }
#[test] fn rv64_linux_soc() { agree(&files(&["rv64/rv64.fx", "rv64/fpu.fx", "rv64/linux/soc.fx", "rv64/linux/tests_soc.fx"]), Some("Soc")); }
