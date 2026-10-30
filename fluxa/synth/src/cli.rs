//! `reda synth / fpga / asic` for Fluxa (.fx) files: the front end (the program, the top task,
//! `--param`) here, the rest of the flow is `eda::flow`, as for Veryl.

use std::collections::HashMap;
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
    let (mut top, mut list) = (None::<String>, false);
    let mut params: HashMap<String, i128> = HashMap::new();
    let mut files = vec![];
    let mut a = rest.into_iter();
    while let Some(x) = a.next() {
        match x.as_str() {
            "--top" | "-s" => top = a.next(),
            "--list" => list = true,
            "--param" | "-P" => {
                let kv = a.next().unwrap_or_else(|| usage());
                let (k, v) = kv.split_once('=').unwrap_or_else(|| { eprintln!("--param takes NAME=VALUE"); usage() });
                let v: i128 = v.parse().unwrap_or_else(|_| { eprintln!("--param {}: not an integer", kv); usage() });
                params.insert(k.to_string(), v);
            }
            "--gatesim" => { eprintln!("reda gatesim: Fluxa gate-level simulation comes in a later version"); std::process::exit(2) }
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
    let top = top.or_else(|| crate::default_top(&prog)).unwrap_or_else(|| { eprintln!("no task to synthesize"); std::process::exit(1) });
    opts.apply().unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(2) });
    let s = crate::synthesize(&prog, &top, &params).unwrap_or_else(|e| { eprintln!("synthesis error in '{}': {}", top, e); std::process::exit(1) });
    eda::flow::run(s.netlist, s.warnings, &top, &opts);
}
