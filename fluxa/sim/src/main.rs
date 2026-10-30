use fluxa::{load_many, run_test, Options};
use std::process::exit;

fn usage() -> ! {
    eprintln!(
        "fluxa — behavioral simulator for the Fluxa HLS language\n\n\
         usage: fluxa <file.fx>... [options]    (multiple files are merged, like `iverilog a.v b.v`)\n\n\
         options:\n  \
           --test NAME   run only the test named NAME (repeatable)\n  \
           --list        list tests and exit\n  \
           --stats       print per-channel statistics (tokens, max FIFO occupancy)\n  \
           --trace       print every channel send/recv\n  \
           -h, --help    show this help"
    );
    exit(2)
}

fn main() {
    let mut files: Vec<String> = Vec::new();
    let mut only: Vec<String> = Vec::new();
    let mut list = false;
    let mut opts = Options { trace: false, stats: false };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--test" => only.push(args.next().unwrap_or_else(|| usage())),
            "--list" => list = true,
            "--stats" => opts.stats = true,
            "--trace" => opts.trace = true,
            "-h" | "--help" => usage(),
            s if s.starts_with('-') => {
                eprintln!("fluxa: unknown option `{}`", s);
                usage()
            }
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        usage()
    }
    let mut srcs = Vec::new();
    for f in &files {
        match std::fs::read_to_string(f) {
            Ok(t) => srcs.push((f.clone(), t)),
            Err(e) => {
                eprintln!("fluxa: cannot read {}: {}", f, e);
                exit(2)
            }
        }
    }
    let file = files.join(" ");
    let prog = match load_many(&srcs) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            exit(2)
        }
    };
    if list {
        for t in &prog.tests {
            println!("{}", t.name);
        }
        return;
    }
    let tests: Vec<_> = prog
        .tests
        .iter()
        .filter(|t| only.is_empty() || only.contains(&t.name))
        .collect();
    if tests.is_empty() {
        eprintln!("fluxa: no tests to run in {}", file);
        exit(2);
    }
    println!("fluxa: {} — {} task/function(s), {} test(s)", file, prog.fns.len(), tests.len());
    let (mut pass, mut fail) = (0, 0);
    for t in tests {
        let r = run_test(&prog, t, &opts);
        if r.passed {
            pass += 1;
            println!("[PASS] {} ({:.3}s)", r.name, r.secs);
        } else {
            fail += 1;
            println!("[FAIL] {} ({:.3}s)", r.name, r.secs);
            for l in r.message.lines() {
                println!("       {}", l);
            }
        }
        if !r.stats.is_empty() {
            print!("{}", r.stats);
        }
    }
    println!("{} passed, {} failed", pass, fail);
    if fail > 0 {
        exit(1);
    }
}
