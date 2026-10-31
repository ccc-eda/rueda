//! `reda synth / fpga / asic` for Fluxa (.fx) files: the front end (the program, the top task,
//! `--param`) here, the rest of the flow is `eda::flow`, as for Veryl.

use std::sync::OnceLock;

/// set by `reda` to its subcommand: argument errors point to `reda help <subcommand>`
pub static CMD: OnceLock<&'static str> = OnceLock::new();

fn usage() -> ! {
    let cmd = CMD.get().copied().unwrap_or("synth");
    eprintln!("reda {}: wrong arguments (reda help {})", cmd, cmd);
    std::process::exit(2)
}

/// run Fluxa synthesis with command-line arguments (without the program name); exits on errors
pub fn main(all: Vec<String>) {
    let (opts, rest) = eda::flow::Opts::parse(&all).unwrap_or_else(|e| { eprintln!("{}", e); usage() });
    let (mut top, mut list, mut gatesim) = (None::<String>, false, None::<String>);
    let mut params: Vec<(String, String)> = vec![];
    let mut files = vec![];
    let mut a = rest.into_iter();
    while let Some(x) = a.next() {
        match x.as_str() {
            "--top" | "-s" => top = a.next(),
            "--list" => list = true,
            "--param" | "-P" => {
                let kv = a.next().unwrap_or_else(|| usage());
                let (k, v) = kv.split_once('=').unwrap_or_else(|| { eprintln!("--param takes NAME=VALUE"); usage() });
                params.push((k.to_string(), v.to_string()));
            }
            "--gatesim" => gatesim = Some(a.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            o if o.starts_with('-') && o.len() > 1 => { eprintln!("unknown option {}", o); usage() }
            _ => files.push(x),
        }
    }
    if files.is_empty() { usage(); }
    let mut srcs = vec![];
    for f in &files {
        let s = std::fs::read_to_string(f).unwrap_or_else(|e| { eprintln!("{}: {}", f, e); std::process::exit(1) });
        srcs.push((f.clone(), s));
    }
    let prog = fluxa::load_many(&srcs).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1) });
    if list {
        for t in crate::tasks(&prog) { println!("{}", t); }
        return;
    }
    if let Some(kind) = gatesim {
        std::process::exit(gate_tests(&prog, top.as_deref(), &kind));
    }
    // a value: an integer, else a Fluxa expression evaluated in the program (an array: `prog=image(demo())`)
    let mut ps = crate::Params::new();
    for (k, v) in params {
        let x = match v.parse::<i128>() { Ok(n) => fluxa::value::Value::Int(n), Err(_) => fluxa::eval_str(&prog, &v).unwrap_or_else(|e| { eprintln!("--param {}: {}", k, e); std::process::exit(1) }) };
        ps.insert(k, x);
    }
    let params = ps;
    let top = top.or_else(|| crate::default_top(&prog)).unwrap_or_else(|| { eprintln!("no task to synthesize"); std::process::exit(1) });
    opts.apply().unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(2) });
    let s = crate::synthesize(&prog, &top, &params).unwrap_or_else(|e| { eprintln!("synthesis error in '{}': {}", top, e); std::process::exit(1) });
    eda::flow::run(s.netlist, s.warnings, &top, &opts);
}

/// `reda gatesim KIND x.fx`: the program's tests with `top` (else every task) replaced by its
/// `kind` netlist; the exit code (0 all passed, 1 a failure, 2 synthesis failed)
fn gate_tests(prog: &std::sync::Arc<fluxa::ast::Program>, top: Option<&str>, kind: &str) -> i32 {
    let p = match top {
        Some(t) => match crate::hw::Provider::new(prog, t, kind) { Ok(p) => p, Err(e) => { eprintln!("synthesis error in '{}': {}", t, e); return 2; } },
        None => crate::hw::Provider::all(prog, kind),
    };
    let p = std::sync::Arc::new(p);
    let opts = fluxa::Options { hw: Some(p.clone()), ..Default::default() };
    let (mut pass, mut fail) = (0, 0);
    let mut out = String::new();
    for t in &prog.tests {
        let r = fluxa::run_test(prog, t, &opts);
        out += &r.output;
        if r.passed {
            pass += 1;
            out += &format!("[PASS] {} ({:.3}s, {})\n", r.name, r.secs, kind);
        } else {
            fail += 1;
            out += &format!("[FAIL] {} ({:.3}s, {})\n", r.name, r.secs, kind);
            for l in r.message.lines() { out += &format!("       {}\n", l); }
        }
    }
    for (task, r) in p.report() {
        match r {
            Ok(n) => println!("gate-level simulation: {} as {} cells ({})", task, n, kind),
            Err(e) => println!("gate-level simulation: {} stays behavioral: {}", task, e),
        }
    }
    print!("{}", out);
    println!("{} passed, {} failed", pass, fail);
    if fail > 0 { 1 } else { 0 }
}
