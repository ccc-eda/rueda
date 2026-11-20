//! `reda SUBCOMMAND --format json ...`: the result as one JSON document on stdout, for AI agents and
//! scripts (design: docs/note/ai-feedback.md). The schema is `rueda/1`: the same envelope for
//! every subcommand (diagnostics, tests, metrics). Without it nothing changes. (Not `--json`:
//! synth / fpga already take `--json OUT.json`, the yosys netlist.)
//!
//! Phase 1: Fluxa (.fx) `sim` / `test`, `list`, `synth`, `fpga` (iCE40 mapping; place & route
//! and timing come later). Other subcommands answer with a diagnostic and exit code 2.

use crate::json::J;
use std::sync::Arc;

pub const SCHEMA: &str = "rueda/1";

/// one report: built up by a subcommand, printed once at the end
struct Report {
    command: String,
    lang: Option<&'static str>,
    inputs: Vec<J>,
    diagnostics: Vec<J>,
    tests: Vec<J>,
    result: Option<J>,
    metrics: J,
    secs: f64,
}

impl Report {
    fn new(command: &str) -> Report {
        Report { command: command.to_string(), lang: None, inputs: vec![], diagnostics: vec![], tests: vec![], result: None, metrics: J::obj(), secs: 0.0 }
    }

    /// add a diagnostic; returns its index
    fn diag(&mut self, d: J) -> usize {
        self.diagnostics.push(d);
        self.diagnostics.len() - 1
    }

    fn error(&mut self, phase: &str, msg: &str) -> usize {
        self.diag(from_text(phase, "error", msg))
    }

    fn finish(self, exit: i32) -> i32 {
        let ok = exit == 0;
        let mut j = J::obj()
            .with("schema", SCHEMA)
            .with("command", self.command)
            .with_opt("lang", self.lang)
            .with("inputs", J::Arr(self.inputs))
            .with("ok", ok)
            .with("exit", exit)
            .with("diagnostics", J::Arr(self.diagnostics))
            .with("tests", J::Arr(self.tests));
        if let Some(r) = self.result { j.set("result", r); }
        j.set("metrics", self.metrics);
        // the only field that changes from run to run: leave it out when comparing two runs
        j.set("timing_info", J::obj().with("secs", (self.secs * 1000.0).round() / 1000.0));
        print!("{}", j.pretty());
        exit
    }
}

/// `--format F` / `--format=F` in `args`: the other arguments and F
pub fn take_format(args: &[String]) -> Option<(Vec<String>, String)> {
    let mut rest = vec![];
    let mut fmt = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--format" { fmt = Some(it.next().cloned().unwrap_or_default()); }
        else if let Some(f) = a.strip_prefix("--format=") { fmt = Some(f.to_string()); }
        else { rest.push(a.clone()); }
    }
    fmt.map(|f| (rest, f))
}

/// run `cmd` with `args` (--format already taken out); prints the report, returns the exit code
pub fn run(cmd: &str, args: &[String]) -> i32 {
    let mut r = Report::new(cmd);
    let fx = args.iter().any(|a| a.ends_with(".fx"));
    let code = match cmd {
        "sim" | "test" | "list" | "synth" | "fpga" if fx => {
            r.lang = Some("fluxa");
            fluxa(&mut r, cmd, args)
        }
        _ => {
            let what = if fx || !matches!(cmd, "sim" | "test" | "list" | "synth" | "fpga") { format!("reda {}", cmd) } else { format!("reda {} on Veryl files", cmd) };
            r.error("args", &format!("{} --format json is not supported yet (so far: Fluxa .fx files with sim, test, list, synth, fpga)", what));
            2
        }
    };
    r.finish(code)
}

// ------------------------------------------------------------------------------ Fluxa

struct FxArgs {
    files: Vec<String>,
    tests: Vec<String>,
    top: Option<String>,
    params: Vec<(String, String)>,
}

/// the options --format json understands; anything else is an error (exit 2)
fn fx_args(r: &mut Report, cmd: &str, args: &[String]) -> Option<FxArgs> {
    let mut a = FxArgs { files: vec![], tests: vec![], top: None, params: vec![] };
    let mut it = args.iter();
    let sim = matches!(cmd, "sim" | "test");
    while let Some(x) = it.next() {
        let mut val = |r: &mut Report| -> Option<String> {
            let v = it.next().cloned();
            if v.is_none() { r.error("args", &format!("{} needs a value", x)); }
            v
        };
        match x.as_str() {
            "--test" if sim => a.tests.push(val(r)?),
            "--top" | "-s" if !sim => a.top = Some(val(r)?),
            "--param" | "-P" if !sim => {
                let kv = val(r)?;
                let Some((k, v)) = kv.split_once('=') else { r.error("args", "--param takes NAME=VALUE"); return None };
                a.params.push((k.to_string(), v.to_string()));
            }
            "--arch" if cmd == "fpga" => {
                let v = val(r)?;
                if v != "ice40" { r.error("args", &format!("--arch {} --format json is not supported yet (so far: ice40)", v)); return None; }
            }
            o if o.starts_with('-') && o.len() > 1 => {
                r.error("args", &format!("option {} is not supported with --format json yet", o));
                return None;
            }
            _ => a.files.push(x.clone()),
        }
    }
    if a.files.is_empty() { r.error("args", "no .fx files"); return None; }
    Some(a)
}

fn fluxa(r: &mut Report, cmd: &str, args: &[String]) -> i32 {
    let Some(a) = fx_args(r, cmd, args) else { return 2 };
    let mut srcs = vec![];
    for f in &a.files {
        match std::fs::read_to_string(f) {
            Ok(t) => {
                r.inputs.push(J::obj().with("path", f).with("bytes", t.len()));
                srcs.push((f.clone(), t));
            }
            Err(e) => { r.error("args", &format!("cannot read {}: {}", f, e)); return 2; }
        }
    }
    let prog = match fluxa::load_many(&srcs) {
        Ok(p) => p,
        Err(e) => { r.error("parse", &e); return 2; }
    };
    let start = std::time::Instant::now();
    let code = match cmd {
        "list" => {
            r.result = Some(J::obj()
                .with("tests", prog.tests.iter().map(|t| J::from(&t.name)).collect::<Vec<_>>())
                .with("tasks", fluxa_synth::tasks(&prog)));
            0
        }
        "sim" | "test" => fx_sim(r, &prog, &a),
        _ => fx_synth(r, &prog, &a, cmd == "fpga"),
    };
    r.secs = start.elapsed().as_secs_f64();
    code
}

fn fx_sim(r: &mut Report, prog: &Arc<fluxa::ast::Program>, a: &FxArgs) -> i32 {
    let tests: Vec<_> = prog.tests.iter().filter(|t| a.tests.is_empty() || a.tests.contains(&t.name)).collect();
    if tests.is_empty() { r.error("args", "no tests to run"); return 2; }
    // cooperative: the program's `print` output is collected (not printed into the JSON)
    let opts = fluxa::Options { coop: true, ..Default::default() };
    let (mut pass, mut fail) = (0usize, 0usize);
    for t in tests {
        let res = fluxa::run_test(prog, t, &opts);
        let mut tj = J::obj().with("name", &res.name).with("ok", res.passed);
        if res.passed {
            pass += 1;
        } else {
            fail += 1;
            let d = match &res.failure {
                Some(f) => failure(f, &res.name),
                None => from_text("sim", "error", &res.message),
            };
            tj.set("diagnostic", r.diag(d));
        }
        if !res.output.is_empty() { tj.set("output", res.output); }
        r.tests.push(tj);
    }
    r.metrics = J::obj().with("passed", pass).with("failed", fail);
    if fail > 0 { 1 } else { 0 }
}

/// a test's failure as a diagnostic
fn failure(f: &fluxa::Failure, test: &str) -> J {
    let mut d = J::obj().with_opt("code", f.code).with("severity", "error").with("phase", "sim").with("message", &f.msg)
        .with_opt("file", f.file.as_ref()).with_opt("line", f.line);
    let mut ctx = J::obj().with("test", test);
    if let Some(t) = &f.task { ctx.set("task", t); }
    d.set("context", ctx);
    if !f.blocked.is_empty() {
        let blocked: Vec<J> = f.blocked.iter().map(|(n, id, why)| {
            let mut b = J::obj().with("task", n).with("id", *id);
            // "recv on `CHAN` (FIFO empty)" / "send on `CHAN` (FIFO full)"
            if let Some((op, rest)) = why.split_once(" on `") {
                if let Some((ch, tail)) = rest.split_once('`') {
                    b.set("op", op);
                    b.set("chan", ch);
                    let st = tail.trim().trim_start_matches('(').trim_end_matches(')');
                    if !st.is_empty() { b.set("state", st); }
                    return b;
                }
            }
            b.with("why", why)
        }).collect();
        d.set("data", J::obj().with("blocked", J::Arr(blocked)));
        d.set("notes", vec!["every live task waits on a channel: look for a cycle of recv's, a missing send or close, or a FIFO too shallow"]);
    } else if let Some((data, note)) = overflow(&f.msg) {
        d.set("data", data);
        d.set("notes", vec![note]);
    }
    d
}

/// "… value V does not fit T (range LO..=HI)": the numbers, and the smallest integer type that holds V
fn overflow(msg: &str) -> Option<(J, String)> {
    let rest = &msg[msg.find("value ")? + 6..];
    let (v, rest) = rest.split_once(" does not fit ")?;
    let (ty, rest) = rest.split_once(" (range ")?;
    let (lo, hi) = rest.trim_end_matches(')').split_once("..=")?;
    let (vn, lon) = (v.parse::<i128>().ok()?, lo.parse::<i128>().ok()?);
    let data = J::obj().with("value", v).with("type", ty).with("range", vec![lo, hi]);
    let signed = lon < 0 || vn < 0;
    let need = (1..=127u32).find(|&b| if signed { vn >= -(1i128 << (b - 1)) && vn < (1i128 << (b - 1)) && lon >= -(1i128 << (b - 1)) } else { vn < (1i128 << b) })?;
    let t = format!("{}{}", if signed { "i" } else { "u" }, need);
    Some((data.with("min_type", t.as_str()), format!("the value needs at least {} (or saturate / wrap it explicitly: .sat::<T>(), .wrap::<T>())", t)))
}

fn fx_synth(r: &mut Report, prog: &Arc<fluxa::ast::Program>, a: &FxArgs, fpga: bool) -> i32 {
    let mut ps = fluxa_synth::Params::new();
    for (k, v) in &a.params {
        let x = match v.parse::<i128>() {
            Ok(n) => fluxa::value::Value::Int(n),
            Err(_) => match fluxa::eval_str(prog, v) {
                Ok(x) => x,
                Err(e) => { r.error("args", &format!("--param {}: {}", k, e)); return 2; }
            },
        };
        ps.insert(k.clone(), x);
    }
    let Some(top) = a.top.clone().or_else(|| fluxa_synth::default_top(prog)) else { r.error("synth", "no task to synthesize"); return 1 };
    let s = match fluxa_synth::synthesize(prog, &top, &ps) {
        Ok(s) => s,
        Err(e) => {
            let mut d = from_text("synth", "error", &e);
            d.set("context", J::obj().with("top", top.as_str()));
            r.diag(d);
            return 1;
        }
    };
    for w in &s.warnings { r.diag(from_text("synth", "warning", w)); }
    let mut nl = s.netlist;
    if fpga {
        nl = match eda::fpga::ice40::map(&nl) {
            Ok(n) => n,
            Err(e) => { r.error("map", &format!("iCE40 mapping: {}", e)); return 1; }
        };
    }
    let stats = nl.stats();
    let count = |f: &dyn Fn(&str) -> bool| -> usize { stats.iter().filter(|(t, _)| f(t)).map(|x| x.1).sum() };
    let ports: Vec<J> = nl.ports.iter().map(|p| J::obj()
        .with("name", &p.name)
        .with("dir", if p.dir == eda::netlist::Dir::In { "in" } else { "out" })
        .with("width", p.bits.len())).collect();
    let mut m = J::obj()
        .with("top", top.as_str())
        .with("ports", J::Arr(ports))
        .with("cells_total", count(&|_| true))
        .with("cells", J::Obj(stats.iter().map(|(t, n)| (t.clone(), J::from(*n))).collect()));
    if fpga {
        // iCE40UP5K: 5280 LUT4s (and as many flip-flops), 30 block RAMs, 8 DSPs
        let (lut, ff, bram, dsp) = (count(&|t| t == "SB_LUT4"), count(&|t| t.starts_with("SB_DFF")), count(&|t| t == "SB_RAM40_4K"), count(&|t| t == "SB_MAC16"));
        let pct = |n: usize, cap: usize| (n as f64 * 1000.0 / cap as f64).round() / 10.0;
        m.set("device", "ice40-up5k");
        m.set("lut", lut);
        m.set("ff", ff);
        m.set("carry", count(&|t| t == "SB_CARRY"));
        m.set("bram", bram);
        m.set("dsp", dsp);
        m.set("utilization_pct", J::obj().with("lut", pct(lut, 5280)).with("ff", pct(ff, 5280)).with("bram", pct(bram, 30)).with("dsp", pct(dsp, 8)));
        let over = lut > 5280 || ff > 5280 || bram > 30 || dsp > 8;
        if over { r.error("map", "the design does not fit the iCE40UP5K (see metrics.utilization_pct)"); }
        r.diag(J::obj().with("severity", "note").with("phase", "pnr").with("message", "place & route and timing are not in --format json yet (phase 1): the metrics are after LUT mapping"));
        r.metrics = m;
        return if over { 1 } else { 0 };
    }
    r.metrics = m;
    0
}

// ------------------------------------------------------------------------------ text

/// a diagnostic from a tool's message: "FILE:LINE: …", "line N: …", "error[E0412]: …"
fn from_text(phase: &str, severity: &str, msg: &str) -> J {
    let (file, line, rest) = split_loc(msg);
    let (code, rest) = split_code(rest);
    let code = code.or_else(|| fluxa::diag::code(rest));
    let mut d = J::obj().with_opt("code", code).with("severity", severity).with("phase", phase).with("message", rest);
    if let Some(f) = file { d.set("file", f); }
    if let Some(l) = line { d.set("line", l); }
    d
}

/// "FILE:LINE: rest" or "line N: rest" → (file, line, rest)
fn split_loc(msg: &str) -> (Option<&str>, Option<u32>, &str) {
    if let Some(t) = msg.strip_prefix("line ") {
        if let Some((n, rest)) = t.split_once(": ") {
            if let Ok(n) = n.parse() { return (None, Some(n), rest); }
        }
    }
    // FILE:LINE: (a file name has no ": ")
    if let Some((head, rest)) = msg.split_once(": ") {
        if let Some((f, n)) = head.rsplit_once(':') {
            if let Ok(n) = n.parse() { if !f.is_empty() { return (Some(f), Some(n), rest); } }
        }
    }
    (None, None, msg)
}

/// "error[E0412]: rest" → (E0412, rest)
fn split_code(msg: &str) -> (Option<&str>, &str) {
    for p in ["error[", "warning["] {
        if let Some(i) = msg.find(p) {
            let t = &msg[i + p.len()..];
            if let Some((c, rest)) = t.split_once("]: ") {
                if c.len() == 5 && c.starts_with(['E', 'W']) { return (Some(c), rest); }
            }
        }
    }
    (None, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations() {
        assert_eq!(split_loc("a/b.fx:12: syntax error: x"), (Some("a/b.fx"), Some(12), "syntax error: x"));
        assert_eq!(split_loc("line 7: error[E0412]: no"), (None, Some(7), "error[E0412]: no"));
        assert_eq!(split_loc("deadlock: all"), (None, None, "deadlock: all"));
        assert_eq!(split_code("error[E0412]: pipeline"), (Some("E0412"), "pipeline"));
    }

    #[test]
    fn overflow_type() {
        let (d, _) = overflow("assignment to `acc`: value 8589410312 does not fit i24 (range -8388608..=8388607)").unwrap();
        assert_eq!(d, J::obj().with("value", "8589410312").with("type", "i24").with("range", vec!["-8388608", "8388607"]).with("min_type", "i34"));
        let (d, _) = overflow("`let d`: value 300 does not fit u8 (range 0..=255)").unwrap();
        assert!(matches!(d, J::Obj(ref kv) if kv.last().unwrap().1 == J::from("u9")));
    }
}
