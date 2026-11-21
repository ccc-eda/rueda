//! `reda … --format json`: one JSON document on stdout (schema rueda/1), the error codes and
//! counterexamples of fluxa/examples/err_*.fx, the same bytes on every run.

use std::process::Command;

/// run reda in the repository root: (exit code, stdout)
fn reda(args: &[&str]) -> (i32, String) {
    let root = format!("{}/..", env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(env!("CARGO_BIN_EXE_reda")).args(args).current_dir(root).output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string())
}

/// the report without its one varying line (timing_info.secs)
fn stable(s: &str) -> String {
    s.lines().filter(|l| !l.trim_start().starts_with("\"secs\"")).collect::<Vec<_>>().join("\n")
}

fn has(o: &str, s: &str) -> bool { o.contains(s) }

#[test]
fn sim_errors() {
    let (c, o) = reda(&["sim", "--format", "json", "fluxa/examples/err_overflow.fx"]);
    assert!(c == 1 && o.starts_with("{\n  \"schema\": \"rueda/1\""), "{}", o);
    assert!(has(&o, "\"code\": \"E0301\"") && has(&o, "\"line\": 4") && has(&o, "\"task\": \"Mac\"") && has(&o, "\"min_type\": \"i31\""), "{}", o);
    assert!(has(&o, "\"ok\": false") && has(&o, "\"diagnostic\": 0"), "{}", o);

    let (c, o) = reda(&["sim", "--format", "json", "fluxa/examples/err_narrow.fx"]);
    assert!(c == 1 && has(&o, "\"code\": \"E0301\"") && has(&o, "\"min_type\": \"u9\""), "{}", o);

    let (c, o) = reda(&["sim", "--format", "json", "fluxa/examples/err_linear.fx"]);
    assert!(c == 1 && has(&o, "\"code\": \"E0702\"") && has(&o, "\"line\": 6"), "{}", o);

    let (c, o) = reda(&["sim", "--format=json", "fluxa/examples/err_deadlock.fx"]);
    assert!(c == 1 && has(&o, "\"code\": \"E0701\"") && has(&o, "\"op\": \"recv\"") && has(&o, "\"state\": \"FIFO empty\""), "{}", o);
    assert_eq!(o.matches("\"op\": \"recv\"").count(), 3, "{}", o);
}

#[test]
fn sim_pass_and_list() {
    let (c, o) = reda(&["test", "--format", "json", "fluxa/examples/fir.fx"]);
    assert!(c == 0 && has(&o, "\"ok\": true") && has(&o, "\"passed\": 2") && has(&o, "\"diagnostics\": []"), "{}", o);
    let (c, o) = reda(&["sim", "--format", "json", "--test", "fir_impulse", "fluxa/examples/fir.fx"]);
    assert!(c == 0 && has(&o, "\"passed\": 1"), "{}", o);
    let (c, o) = reda(&["list", "--format", "json", "fluxa/examples/fir4.fx"]);
    assert!(c == 0 && has(&o, "\"result\"") && has(&o, "\"Fir4\"") && has(&o, "\"fir4_step\""), "{}", o);
}

#[test]
fn synth_and_fpga() {
    let (c, o) = reda(&["fpga", "--format", "json", "fluxa/examples/fir4.fx"]);
    assert!(c == 0 && has(&o, "\"device\": \"ice40-up5k\"") && has(&o, "\"lut\": ") && has(&o, "\"SB_LUT4\": "), "{}", o);
    assert!(has(&o, "\"utilization_pct\"") && has(&o, "\"severity\": \"note\""), "{}", o);
    let (c, o) = reda(&["synth", "--format", "json", "fluxa/examples/mac.fx"]);
    assert!(c == 0 && has(&o, "\"cells_total\": ") && has(&o, "\"dir\": \"in\""), "{}", o);
}

#[test]
fn deterministic() {
    for args in [["sim", "fluxa/examples/err_deadlock.fx"], ["fpga", "fluxa/examples/fir4.fx"]] {
        let (_, a) = reda(&[args[0], "--format", "json", args[1]]);
        let (_, b) = reda(&[args[0], "--format", "json", args[1]]);
        assert_eq!(stable(&a), stable(&b));
    }
}

#[test]
fn errors_and_unsupported() {
    let (c, o) = reda(&["sim", "--format", "json", "veryl/examples/src/seq101.veryl"]);
    assert!(c == 2 && has(&o, "not supported yet") && has(&o, "\"ok\": false"), "{}", o);
    let (c, o) = reda(&["fpga", "--format", "json", "--bin", "x.bin", "fluxa/examples/fir4.fx"]);
    assert!(c == 2 && has(&o, "option --bin is not supported"), "{}", o);
    let (c, o) = reda(&["sim", "--format", "json", "no_such_file.fx"]);
    assert!(c == 2 && has(&o, "cannot read no_such_file.fx"), "{}", o);
    let (c, _) = reda(&["sim", "--format", "xml", "fluxa/examples/fir.fx"]);
    assert_eq!(c, 2);
}

#[test]
fn static_check() {
    // T-CARRY before simulation: the bound of `len: u16` makes the need i47 (the simulator only saw i31)
    let (c, o) = reda(&["check", "--format", "json", "fluxa/examples/err_overflow.fx"]);
    assert!(c == 1 && has(&o, "\"phase\": \"check\"") && has(&o, "\"code\": \"E0301\"") && has(&o, "\"min_type\": \"i47\"") && has(&o, "\"line\": 3"), "{}", o);
    let (c, o) = reda(&["check", "--format", "json", "fluxa/examples/err_narrow.fx"]);
    assert!(c == 1 && has(&o, "\"range\": \"300..=300\"") && has(&o, "\"min_type\": \"u9\""), "{}", o);
    let (c, o) = reda(&["check", "--format", "json", "fluxa/examples/gemm.fx"]);
    assert!(c == 1 && o.matches("\"code\": \"E0703\"").count() == 2, "{}", o);
    let (c, o) = reda(&["check", "--format", "json", "fluxa/examples/fir4.fx"]);
    assert!(c == 0 && has(&o, "\"findings\": 0"), "{}", o);
    let (c, o) = reda(&["check", "fluxa/examples/err_overflow.fx"]);
    assert!(c == 1 && has(&o, "err_overflow.fx:3: warning[E0301] in `Mac`") && has(&o, "fix: declare it as i47"), "{}", o);
}

#[test]
fn static_check_channels() {
    let (c, o) = reda(&["check", "--format", "json", "fluxa/examples/err_deadlock.fx"]);
    assert!(c == 1 && has(&o, "\"code\": \"E0501\"") && has(&o, "\"sent\": \"0\"") && has(&o, "\"received\": \"1\""), "{}", o);
    let (c, o) = reda(&["check", "--format", "json", "fluxa/examples/err_linear.fx"]);
    assert!(c == 1 && has(&o, "\"code\": \"E0702\"") && has(&o, "\"spawned\": \"5\""), "{}", o);
    let (c, o) = reda(&["check", "--format", "json", "fluxa/examples/fir.fx"]);
    assert!(c == 1 && has(&o, "\"code\": \"W0503\"") && has(&o, "\"sent\": \"17\""), "{}", o);
    for ok in ["fluxa/examples/fir4.fx", "fluxa/examples/mac.fx", "fluxa/examples/basics.fx"] {
        let (c, o) = reda(&["check", "--format", "json", ok]);
        assert!(c == 0, "{}: {}", ok, o);
    }
}

#[test]
fn synthesis_agrees_with_the_simulator() {
    // a channel of another type than the port, and +% on an inferred variable: refused by both
    let (c, o) = reda(&["synth", "--format", "json", "--top", "Net", "fluxa/examples/err_chan_type.fx"]);
    assert!(c == 1 && has(&o, "carries i8, the parameter is chan<i16>"), "{}", o);
    let (c, o) = reda(&["synth", "--format", "json", "--top", "Inc", "fluxa/examples/err_wrap_untyped.fx"]);
    assert!(c == 1 && has(&o, "wrapping operators need a typed variable"), "{}", o);
}
