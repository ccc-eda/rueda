//! The Veryl synthesis command line: `reda synth / fpga / asic / gatesim`.

use crate::elab;
use std::sync::OnceLock;

/// set by `reda` to its subcommand: argument errors point to `reda help <subcommand>`
pub static CMD: OnceLock<&'static str> = OnceLock::new();

use std::path::PathBuf;
use veryl_parser::Parser;

fn usage() -> ! {
    let cmd = CMD.get().copied().unwrap_or("synth");
    eprintln!("reda {}: wrong arguments (reda help {})", cmd, cmd);
    std::process::exit(2)
}

/// run Veryl synthesis with command-line arguments (without the program name); exits on errors
pub fn main(all: Vec<String>) {
    let (opts, rest) = eda::flow::Opts::parse(&all).unwrap_or_else(|e| { eprintln!("{}", e); usage() });
    let (mut top, mut list, mut gatesim) = (None::<String>, false, None::<String>);
    let mut files = vec![];
    let mut a = rest.into_iter();
    while let Some(x) = a.next() {
        match x.as_str() {
            "--top" | "-s" => top = a.next(),
            "--list" => list = true,
            "--gatesim" => gatesim = Some(a.next().unwrap_or_else(|| usage())),
            "-h" | "--help" => usage(),
            o if o.starts_with('-') && o.len() > 1 => { eprintln!("unknown option {}", o); usage() }
            _ => files.push(PathBuf::from(x)),
        }
    }
    if files.is_empty() { usage(); }
    opts.apply().unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(2) });
    let mut parsers = vec![];
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| { eprintln!("{}: {}", f.display(), e); std::process::exit(1) });
        match Parser::parse(&src, f) {
            Ok(p) => parsers.push(p),
            Err(e) => { eprintln!("{}: parse error: {:?}", f.display(), e); std::process::exit(1) }
        }
    }
    let mut lib = elab::Lib::new();
    for p in &parsers { lib.add(&p.veryl); }
    if list {
        for (n, t) in &lib.order { println!("{}{}", n, if *t { "  (#[test])" } else { "" }); }
        return;
    }
    let top = top.or_else(|| lib.order.iter().rev().find(|(_, t)| !t).map(|x| x.0.clone()))
        .unwrap_or_else(|| { eprintln!("no synthesizable (non-#[test]) module found"); std::process::exit(1) });

    if let Some(kind) = &gatesim {
        let srcs: Vec<(String, String)> = files.iter().map(|f| (f.display().to_string(), std::fs::read_to_string(f).unwrap())).collect();
        let prov = crate::gatesim::Provider::new(&lib, &top, kind);
        let f = |m: &str, ov: &std::collections::HashMap<String, veryl_sim::value::Val>| prov.get(m, ov);
        let r = veryl_sim::run_gates(&srcs, None, 1_000_000, false, true, Some(&f));
        for (ps, n) in prov.built.borrow().iter() {
            println!("gate-level simulation: {}{} as {} cells ({})", top, if ps.is_empty() { String::new() } else { format!(" #({})", ps) }, n, kind);
        }
        match r {
            Ok(r) => std::process::exit(if r.tops.iter().any(|t| t.fails > 0) { 1 } else { 0 }),
            Err(e) => { eprintln!("{}", e); std::process::exit(1) }
        }
    }
    let (nl, warnings) = match elab::synthesize(&lib, &top) {
        Ok(r) => r,
        Err(e) => { eprintln!("synthesis error in '{}': {}", top, e); std::process::exit(1) }
    };
    eda::flow::run(nl, warnings, &top, &opts);
}
