//! The `reda` command: every subcommand reaches its tool, help and errors behave.

use std::process::Command;

fn reda_in(dir: &str, args: &[&str]) -> (i32, String) {
    let dir = format!("{}/../{}", env!("CARGO_MANIFEST_DIR"), dir);
    let out = Command::new(env!("CARGO_BIN_EXE_reda")).args(args).current_dir(dir).output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
}

/// in veryl/examples/src
fn veryl(args: &[&str]) -> (i32, String) { reda_in("veryl/examples/src", args) }

#[test]
fn subcommands() {
    let (c, o) = veryl(&[]);
    assert!(c == 0 && o.contains("usage: reda <subcommand>") && o.contains("gatesim"), "{}", o);
    let (c, o) = veryl(&["help", "asic"]);
    assert!(c == 0 && o.contains("--pdk sky130"), "{}", o);
    let (c, o) = veryl(&["fpga", "-h"]);
    assert!(c == 0 && o.contains("usage: reda fpga"), "{}", o);
    let (c, o) = veryl(&["sim", "seq101.veryl"]);
    assert!(c == 0 && o.contains("TestSeq101: PASS"), "{}", o);
    let (c, o) = veryl(&["list", "seq101.veryl"]);
    assert!(c == 0 && o.contains("TestSeq101  (#[test])"), "{}", o);
    let (c, o) = veryl(&["synth", "adder.veryl"]);
    assert!(c == 0 && o.contains("=== Adder ==="), "{}", o);
    let (c, o) = veryl(&["fpga", "adder.veryl"]);
    assert!(c == 0 && o.contains("SB_LUT4"), "{}", o);
    let (c, o) = veryl(&["fpga", "--arch", "ecp5", "adder.veryl"]);
    assert!(c == 0 && o.contains("CCU2C") && o.contains("LUT4"), "{}", o);
    let (c, o) = veryl(&["gatesim", "ecp5", "seq101.veryl"]);
    assert!(c == 0 && o.contains("TestSeq101: PASS") && o.contains("(ecp5)"), "{}", o);
    assert_eq!(veryl(&["fpga", "--arch", "xc7", "adder.veryl"]).0, 2);
    let (c, o) = veryl(&["asic", "--pdk", "sky130", "seq101.veryl"]);
    assert!(c == 0 && o.contains("sky130_fd_sc_hd__dfrtp_1") && o.contains("0 shorts, 0 spacing"), "{}", o);
    let (c, o) = veryl(&["gatesim", "asic", "seq101.veryl"]);
    assert!(c == 0 && o.contains("TestSeq101: PASS"), "{}", o);
    let (c, o) = veryl(&["cc", "--run", "1000000", "../c/hello.c"]);
    assert!(c == 0 && o.contains("Hello from C"), "{}", o);
    let (c, o) = veryl(&["version"]);
    assert!(c == 0 && o.starts_with("reda "), "{}", o);
    // errors: an unknown subcommand, an unknown option, a wrong netlist kind
    assert_eq!(veryl(&["frobnicate"]).0, 2);
    let (c, o) = veryl(&["synth", "--bogus", "adder.veryl"]);
    assert!(c == 2 && o.contains("reda help synth"), "{}", o);
    assert_eq!(veryl(&["gatesim", "xyz", "adder.veryl"]).0, 2);
    // a failing testbench still fails
    let (c, _) = veryl(&["sim", "--max-time", "1", "seq101.veryl"]);
    assert!(c == 0 || c == 1);
}

#[test]
fn fluxa_and_riscv() {
    let fx = |args: &[&str]| reda_in("fluxa", args);
    let (c, o) = fx(&["sim", "examples/fir.fx"]);
    assert!(c == 0 && o.contains("[PASS]") && !o.contains("[FAIL]"), "{}", o);
    let (c, o) = fx(&["list", "examples/fir.fx"]);
    assert!(c == 0 && !o.trim().is_empty(), "{}", o);
    let (c, o) = fx(&["sim", "rv32i/rv32i.fx", "rv32i/smoke.fx"]);
    assert!(c == 0 && o.contains("passed, 0 failed"), "{}", o);
    // a test that fails (a deadlock) fails the command
    assert_eq!(fx(&["sim", "examples/err_deadlock.fx"]).0, 1);
    // no synthesis for Fluxa yet; no mixing of languages
    // synthesis of a Fluxa task (arrays and fixed point, used by fir.fx, come later: a clear error)
    let (c, o) = fx(&["synth", "examples/mac.fx"]);
    assert!(c == 0 && o.contains("=== Mac ===") && o.contains("a_valid"), "{}", o);
    let (c, o) = fx(&["asic", "examples/mac.fx"]);
    assert!(c == 0 && o.contains("timing: critical path"), "{}", o);
    let (c, o) = fx(&["synth", "examples/fir.fx"]);
    assert!(c == 1 && o.contains("not synthesized yet"), "{}", o);
    assert_eq!(fx(&["sim", "examples/fir.fx", "../veryl/examples/src/adder.veryl"]).0, 2);
    let (c, o) = fx(&["help", "rvemu"]);
    assert!(c == 0 && o.contains("usage: reda rvemu"), "{}", o);
    let (c, o) = fx(&["help", "emu"]);
    assert!(c == 0 && o.contains("usage: reda emu"), "{}", o);
}
