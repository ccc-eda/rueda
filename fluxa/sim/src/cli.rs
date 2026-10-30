//! `reda sim` / `reda list` for Fluxa (.fx) files: run the tests of a Fluxa program.

use crate::{load_many, run_test, Options};
use std::process::exit;

fn usage() -> ! {
    eprintln!(
        "usage: reda sim [options] FILE.fx...    (multiple files are merged, like `iverilog a.v b.v`)\n\n\
         options:\n  \
           --test NAME   run only the test named NAME (repeatable)\n  \
           --list        list tests and exit\n  \
           --stats       print per-channel statistics (tokens, max FIFO occupancy)\n  \
           --trace       print every channel send/recv\n  \
           --coop        run the tasks on one thread, taking turns (the browser's scheduler)\n  \
           -h, --help    show this help"
    );
    exit(2)
}

/// run the simulator with command-line arguments (without the program name); exits on errors
pub fn main(args: &[String]) {
    let mut files: Vec<String> = Vec::new();
    let mut only: Vec<String> = Vec::new();
    let mut list = false;
    let mut opts = Options::default();
    let mut args = args.iter().cloned();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--test" => only.push(args.next().unwrap_or_else(|| usage())),
            "--list" => list = true,
            "--stats" => opts.stats = true,
            "--trace" => opts.trace = true,
            "--coop" => opts.coop = true,
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
        print!("{}", r.output);
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
